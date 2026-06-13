//! ACP 客户端：通过 stdio JSON-RPC 2.0 驱动 agent 适配器二进制
//! （codex-acp / claude-agent-acp，与 nextop 的 codex_adapter.go /
//! standard_acp_adapter.go 同构）。
//!
//! 生命周期：spawn -> initialize -> session/new -> (session/prompt -> session/update*)* 。
//! 支持 nextop 同款会话能力：session/set_mode（权限模式）、
//! session/set_config_option（模型 / 推理力度）、新会话、stopReason。
//! 读线程把事件翻译成 [`AcpEvent`] 经 mpsc 发给 UI。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// 一个可接入的 agent 后端（参考 nextop 的 provider 配置）
pub struct AgentBackend {
    pub command: &'static str,
    pub args: &'static [&'static str],
    /// nextop 的 provider id："codex" / "claude-code"
    pub provider: &'static str,
}

pub const CODEX_BACKEND: AgentBackend = AgentBackend {
    command: "codex-acp",
    args: &[
        "--config",
        "model=gpt-5.5",
        "--config",
        "model_reasoning_effort=high",
    ],
    provider: "codex",
};

pub const CLAUDE_BACKEND: AgentBackend = AgentBackend {
    command: "claude-agent-acp",
    args: &[],
    provider: "claude-code",
};

/// Claude Code 静态模型目录（nextop claude_model_catalog.go）
pub const CLAUDE_STATIC_MODELS: &[&str] =
    &["default", "sonnet", "opus", "haiku", "sonnet[1m]", "opusplan"];

/// 推理力度档位（nextop composer_options.go：codex / claude-code 共用 low..xhigh）
pub const REASONING_EFFORTS: &[(&str, &str)] =
    &[("low", "低"), ("medium", "中"), ("high", "高"), ("xhigh", "超高")];

/// 推理力度配置项 id（nextop reasoningConfigOptionID）
pub fn effort_config_id(provider: &str) -> &'static str {
    if provider == "codex" {
        "reasoning_effort"
    } else {
        "effort"
    }
}

/// 各 provider 的权限模式目录与默认值
/// （nextop composer_options.go permissionModeConfigForProvider + zh-CN locale）
pub fn static_modes(provider: &str) -> Vec<ModeOption> {
    let raw: &[(&str, &str, &str)] = match provider {
        "codex" => &[
            ("read-only", "请求批准", "编辑外部文件或使用互联网前始终询问你"),
            ("auto", "代我批准", "仅在检测到可能不安全的操作时询问你"),
            ("full-access", "完全访问", "可不受限制地访问互联网和你电脑上的任何文件"),
        ],
        "claude-code" => &[
            ("default", "默认", "默认较保守；需要执行修改或高风险操作时会先询问你"),
            ("acceptEdits", "接受编辑", "允许直接修改文件；遇到更高风险操作时仍会先询问你"),
            ("dontAsk", "不再询问", "不会弹出确认；未预先允许的操作会被直接拒绝"),
            ("bypassPermissions", "绕过权限", "尽量不做权限拦截，适合你完全信任的任务"),
        ],
        _ => &[],
    };
    raw.iter()
        .map(|(id, name, desc)| ModeOption {
            id: (*id).to_owned(),
            name: (*name).to_owned(),
            description: (*desc).to_owned(),
        })
        .collect()
}

/// 默认权限模式（nextop defaultPermissionModeIDForProvider）
pub fn default_mode(provider: &str) -> &'static str {
    match provider {
        "claude-code" => "default",
        _ => "auto",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanEntry {
    pub content: String,
    pub status: String, // pending | in_progress | completed
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionOption {
    pub id: String,
    pub name: String,
    /// allow_once / allow_always / reject_once / reject_always
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDiff {
    pub path: String,
    pub old_text: String,
    pub new_text: String,
}

/// 权限模式选项（nextop PermissionModeOption）
#[derive(Debug, Clone)]
pub struct ModeOption {
    pub id: String,
    pub name: String,
    pub description: String,
}

/// 模型选项（nextop AgentModelOption）
#[derive(Debug, Clone)]
pub struct ModelOption {
    pub id: String,
    pub name: String,
}

/// configOptions 描述符解析结果（nextop acpLiveState 的 configOptions 快照）。
/// codex-acp 下发 model / reasoning_effort（set_mode 后还有 mode 审批预设）；
/// claude-agent-acp 下发 mode / model / effort。
#[derive(Debug, Default)]
pub struct ConfigSnapshot {
    pub models: Vec<ModelOption>,
    pub current_model: Option<String>,
    pub current_effort: Option<String>,
    pub effort_supported: bool,
    pub modes: Vec<ModeOption>,
    pub current_mode: Option<String>,
}

#[derive(Debug)]
pub enum AcpEvent {
    SessionReady {
        model: String,
        models: Vec<ModelOption>,
        mode: String,
        modes: Vec<ModeOption>,
        effort: String,
        effort_supported: bool,
    },
    AgentChunk(String),
    ThoughtChunk(String),
    Plan(Vec<PlanEntry>),
    ToolCall {
        id: String,
        title: String,
        kind: String,
        text: Option<String>,
        diff: Option<ToolDiff>,
        locations: Vec<String>,
    },
    ToolUpdate {
        id: String,
        status: Option<String>,
        title: Option<String>,
        text: Option<String>,
        diff: Option<ToolDiff>,
        locations: Vec<String>,
    },
    /// 上下文用量（claude-agent-acp 的 usage_update）
    Usage {
        used: u64,
        size: u64,
    },
    /// 可用斜杠命令（available_commands_update）
    Commands(Vec<(String, String)>),
    /// 权限模式切换通知（current_mode_update，例如 claude 的 plan 循环）
    CurrentMode(String),
    /// 配置项快照（session/set_config_option 结果或 config_option_update 通知）
    ConfigOptions(ConfigSnapshot),
    /// 权限请求（转发给 UI，等待用户选择——nextop 的 interactive prompt）
    PermissionRequest {
        request_id: i64,
        title: String,
        options: Vec<PermissionOption>,
    },
    /// 回合结束；stop_reason 来自 session/prompt 结果
    /// （end_turn / max_tokens / refusal / cancelled / error）
    TurnDone {
        stop_reason: String,
    },
    Error(String),
}

const INIT_ID: i64 = 1;

/// 我们发出的请求类型，用于把响应路由到正确的处理（nextop 按 call 区分）
#[derive(Debug, Clone, Copy, PartialEq)]
enum ReqKind {
    NewSession,
    Prompt,
    SetMode,
    SetConfig,
}

type PendingMap = Arc<Mutex<HashMap<i64, ReqKind>>>;

pub struct AcpAgent {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    pub rx: Receiver<AcpEvent>,
    session_id: Arc<Mutex<Option<String>>>,
    pending: PendingMap,
    next_id: Arc<AtomicI64>,
}

impl AcpAgent {
    /// 启动适配器并完成握手（异步进行，结果以事件形式到达）
    pub fn spawn(backend: &AgentBackend, cwd: &str) -> Result<Self, String> {
        // .app bundle 经 LaunchServices 启动时不继承登录 shell 的 PATH，
        // 而适配器常装在 nvm / homebrew 的 bin 下。先补全 PATH。
        let path = enriched_path();
        dbglog(&format!("spawn {}, PATH={path}", backend.command));
        let stderr_to = if std::env::var("MIRAGE_CODEX_DEBUG").is_ok() {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/mirage-codex-debug.log")
                .map(Stdio::from)
                .unwrap_or_else(|_| Stdio::null())
        } else {
            Stdio::null()
        };
        let mut child = Command::new(backend.command)
            .args(backend.args)
            .env("PATH", &path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr_to)
            .spawn()
            .map_err(|e| format!("无法启动 {}：{e}", backend.command))?;

        let stdin = Arc::new(Mutex::new(child.stdin.take().unwrap()));
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = channel::<AcpEvent>();
        let session_id = Arc::new(Mutex::new(None::<String>));
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let next_id = Arc::new(AtomicI64::new(INIT_ID + 1));

        send_json(
            &stdin,
            &json!({
                "jsonrpc": "2.0", "id": INIT_ID, "method": "initialize",
                "params": {
                    "protocolVersion": 1,
                    "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false } }
                }
            }),
        );

        // 看门狗：若 15s 内仍未建立会话，给 UI 一个可读错误
        let watch_session = session_id.clone();
        let watch_tx = tx.clone();
        let cmd_name = backend.command;
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(15));
            if watch_session.lock().unwrap().is_none() {
                let _ = watch_tx.send(AcpEvent::Error(format!(
                    "会话创建超时：{cmd_name} 未响应 session/new。\n请确认已登录，且网络/代理可达模型服务。"
                )));
            }
        });

        // 自检/截图回归时自动批准权限，无人值守跑通；正常运行交给权限模式 + 审批卡
        // （对齐 nextop：权限请求始终以审批卡形式交给用户决定）
        let auto = std::env::var("MIRAGE_SHOT").is_ok() || std::env::var("MIRAGE_APPSHOT").is_ok();
        let auto_approve = Arc::new(AtomicBool::new(auto));
        let reader_stdin = stdin.clone();
        let reader_session = session_id.clone();
        let reader_auto = auto_approve;
        let reader_pending = pending.clone();
        let reader_next = next_id.clone();
        let reader_cwd = cwd.to_owned();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else {
                    dbglog("stdout read error");
                    break;
                };
                dbglog(&format!("<< {}", &line.chars().take(120).collect::<String>()));
                let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                handle_message(
                    &msg,
                    &tx,
                    &reader_stdin,
                    &reader_session,
                    &reader_cwd,
                    &reader_auto,
                    &reader_pending,
                    &reader_next,
                );
            }
            dbglog("reader loop ended");
            let _ = tx.send(AcpEvent::Error("agent 适配器连接已断开".into()));
        });

        Ok(Self {
            child,
            stdin,
            rx,
            session_id,
            pending,
            next_id,
        })
    }

    /// 把用户的权限选择回传给适配器
    pub fn respond_permission(&self, request_id: i64, option_id: &str) {
        send_json(
            &self.stdin,
            &json!({
                "jsonrpc": "2.0", "id": request_id,
                "result": { "outcome": { "outcome": "selected", "optionId": option_id } }
            }),
        );
    }

    pub fn ready(&self) -> bool {
        self.session_id.lock().unwrap().is_some()
    }

    /// 发送请求并登记请求类型，响应在读线程按类型路由
    fn request(&self, kind: ReqKind, method: &str, params: Value) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.pending.lock().unwrap().insert(id, kind);
        send_json(
            &self.stdin,
            &json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        );
    }

    pub fn send_prompt(&self, text: &str) {
        let Some(sid) = self.session_id.lock().unwrap().clone() else {
            return;
        };
        self.request(
            ReqKind::Prompt,
            "session/prompt",
            json!({ "sessionId": sid, "prompt": [{ "type": "text", "text": text }] }),
        );
    }

    pub fn cancel(&self) {
        let Some(sid) = self.session_id.lock().unwrap().clone() else {
            return;
        };
        send_json(
            &self.stdin,
            &json!({
                "jsonrpc": "2.0", "method": "session/cancel",
                "params": { "sessionId": sid }
            }),
        );
    }

    /// 权限模式切换（nextop applyPermissionMode：session/set_mode）
    pub fn set_mode(&self, mode_id: &str) {
        let Some(sid) = self.session_id.lock().unwrap().clone() else {
            return;
        };
        self.request(
            ReqKind::SetMode,
            "session/set_mode",
            json!({ "sessionId": sid, "modeId": mode_id }),
        );
    }

    /// 配置项热更新（nextop setSessionConfigOption：session/set_config_option，
    /// configId = "model" / "reasoning_effort" / "effort"）
    pub fn set_config(&self, config_id: &str, value: &str) {
        let Some(sid) = self.session_id.lock().unwrap().clone() else {
            return;
        };
        self.request(
            ReqKind::SetConfig,
            "session/set_config_option",
            json!({ "sessionId": sid, "configId": config_id, "value": value }),
        );
    }

}

impl Drop for AcpAgent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn dbglog(msg: &str) {
    if std::env::var("MIRAGE_CODEX_DEBUG").is_err() {
        return;
    }
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/mirage-codex-debug.log")
    {
        let _ = writeln!(f, "[acp] {msg}");
    }
}

/// 把 nvm / homebrew / 常见安装目录补进 PATH。
fn enriched_path() -> String {
    let mut dirs: Vec<String> = Vec::new();
    if let Ok(p) = std::env::var("PATH") {
        dirs.extend(p.split(':').map(str::to_owned));
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let nvm = format!("{home}/.nvm/versions/node");
    if let Ok(entries) = std::fs::read_dir(&nvm) {
        let mut versions: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        versions.sort();
        if let Some(latest) = versions.last() {
            dirs.push(format!("{}/bin", latest.display()));
        }
    }
    for extra in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        &format!("{home}/.cargo/bin"),
        &format!("{home}/.local/bin"),
    ] {
        dirs.push(extra.to_string());
    }
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| !d.is_empty() && seen.insert(d.clone()));
    dirs.join(":")
}

fn send_json(stdin: &Arc<Mutex<ChildStdin>>, v: &Value) {
    let method = v.get("method").and_then(Value::as_str).unwrap_or("response");
    if let Ok(mut s) = stdin.lock() {
        match writeln!(s, "{v}").and_then(|_| s.flush()) {
            Ok(_) => dbglog(&format!(">> sent {method}")),
            Err(e) => dbglog(&format!(">> SEND FAILED {method}: {e}")),
        }
    } else {
        dbglog(">> stdin lock failed");
    }
}

fn send_new_session(
    stdin: &Arc<Mutex<ChildStdin>>,
    cwd: &str,
    pending: &PendingMap,
    next_id: &Arc<AtomicI64>,
) {
    let id = next_id.fetch_add(1, Ordering::SeqCst);
    pending.lock().unwrap().insert(id, ReqKind::NewSession);
    send_json(
        stdin,
        &json!({
            "jsonrpc": "2.0", "id": id, "method": "session/new",
            "params": { "cwd": cwd, "mcpServers": [] }
        }),
    );
}

/// 从 tool_call / tool_call_update 里解析 content 数组、rawOutput、locations
fn parse_tool_fields(update: &Value) -> (Option<String>, Option<ToolDiff>, Vec<String>) {
    let mut text_parts: Vec<String> = Vec::new();
    let mut diff = None;

    if let Some(items) = update.get("content").and_then(Value::as_array) {
        for item in items {
            match item.get("type").and_then(Value::as_str).unwrap_or("") {
                "content" => {
                    if let Some(t) = item.pointer("/content/text").and_then(Value::as_str) {
                        if !t.is_empty() {
                            text_parts.push(t.to_owned());
                        }
                    }
                }
                "diff" => {
                    diff = Some(ToolDiff {
                        path: str_at(item, "path"),
                        old_text: item
                            .get("oldText")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        new_text: item
                            .get("newText")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    });
                }
                _ => {}
            }
        }
    }
    // rawOutput 的 stdout/stderr 也并入文本
    if let Some(out) = update.pointer("/rawOutput/stdout").and_then(Value::as_str) {
        if !out.is_empty() {
            text_parts.push(out.to_owned());
        }
    }
    if let Some(err) = update.pointer("/rawOutput/stderr").and_then(Value::as_str) {
        if !err.is_empty() {
            text_parts.push(err.to_owned());
        }
    }

    let locations = update
        .get("locations")
        .and_then(Value::as_array)
        .map(|ls| {
            ls.iter()
                .filter_map(|l| l.get("path").and_then(Value::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();

    let text = if text_parts.is_empty() {
        None
    } else {
        Some(text_parts.join("\n"))
    };
    (text, diff, locations)
}

/// 解析 session/new 结果里的 models（ACP SessionModelState）
fn parse_models(result: &Value) -> (String, Vec<ModelOption>) {
    let current = result
        .pointer("/models/currentModelId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let models = result
        .pointer("/models/availableModels")
        .and_then(Value::as_array)
        .map(|ms| {
            ms.iter()
                .filter_map(|m| {
                    let id = m
                        .get("modelId")
                        .or_else(|| m.get("id"))
                        .and_then(Value::as_str)?
                        .to_owned();
                    let name = m
                        .get("name")
                        .or_else(|| m.get("displayName"))
                        .and_then(Value::as_str)
                        .unwrap_or(&id)
                        .to_owned();
                    Some(ModelOption { id, name })
                })
                .collect()
        })
        .unwrap_or_default();
    (current, models)
}

/// 解析 session/new 结果里的 modes（ACP SessionModeState）
fn parse_modes(result: &Value) -> (String, Vec<ModeOption>) {
    let current = result
        .pointer("/modes/currentModeId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let modes = result
        .pointer("/modes/availableModes")
        .and_then(Value::as_array)
        .map(|ms| {
            ms.iter()
                .filter_map(|m| {
                    let id = m
                        .get("id")
                        .or_else(|| m.get("modeId"))
                        .and_then(Value::as_str)?
                        .to_owned();
                    let name = m
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(&id)
                        .to_owned();
                    Some(ModeOption {
                        id,
                        name,
                        description: str_at(m, "description"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    (current, modes)
}

/// 解析 configOptions 描述符（nextop applyACPConfigOptionDescriptors）
fn parse_config_options(descriptors: &Value) -> ConfigSnapshot {
    let mut snap = ConfigSnapshot::default();
    let Some(arr) = descriptors.as_array() else {
        return snap;
    };
    let current_value = |opt: &Value| -> Option<String> {
        for key in ["currentValue", "current_value", "value"] {
            if let Some(s) = opt.get(key).and_then(Value::as_str) {
                return Some(s.to_owned());
            }
        }
        None
    };
    // 选项可能是 {value,name,description} 对象或纯字符串
    let option_entries = |opt: &Value| -> Vec<(String, String, String)> {
        opt.get("options")
            .and_then(Value::as_array)
            .map(|os| {
                os.iter()
                    .filter_map(|o| {
                        if let Some(s) = o.as_str() {
                            return Some((s.to_owned(), s.to_owned(), String::new()));
                        }
                        let id = o
                            .get("value")
                            .or_else(|| o.get("id"))
                            .and_then(Value::as_str)?
                            .to_owned();
                        let name = o
                            .get("name")
                            .or_else(|| o.get("label"))
                            .and_then(Value::as_str)
                            .unwrap_or(&id)
                            .to_owned();
                        Some((id, name, str_at(o, "description")))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    for opt in arr {
        match opt.get("id").and_then(Value::as_str).unwrap_or("") {
            "model" => {
                snap.models = option_entries(opt)
                    .into_iter()
                    .map(|(id, name, _)| ModelOption { id, name })
                    .collect();
                snap.current_model = current_value(opt);
            }
            "effort" | "reasoning_effort" | "model_reasoning_effort" => {
                snap.effort_supported = true;
                snap.current_effort = current_value(opt);
            }
            "mode" => {
                snap.modes = option_entries(opt)
                    .into_iter()
                    .map(|(id, name, description)| ModeOption {
                        id,
                        name,
                        description,
                    })
                    .collect();
                snap.current_mode = current_value(opt);
            }
            _ => {}
        }
    }
    snap
}

#[allow(clippy::too_many_arguments)]
fn handle_message(
    msg: &Value,
    tx: &Sender<AcpEvent>,
    stdin: &Arc<Mutex<ChildStdin>>,
    session: &Arc<Mutex<Option<String>>>,
    cwd: &str,
    auto_approve: &Arc<AtomicBool>,
    pending: &PendingMap,
    next_id: &Arc<AtomicI64>,
) {
    let id = msg.get("id").and_then(Value::as_i64);
    let method = msg.get("method").and_then(Value::as_str);

    match (id, method) {
        // ---- 我们发出的请求的响应 ----
        (Some(rid), None) => {
            let kind = if rid == INIT_ID {
                None
            } else {
                pending.lock().unwrap().remove(&rid)
            };
            if let Some(err) = msg.get("error") {
                let text = err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("未知错误");
                let _ = tx.send(AcpEvent::Error(text.to_owned()));
                // prompt 出错也要结束回合，避免 UI 卡在工作中
                if kind == Some(ReqKind::Prompt) {
                    let _ = tx.send(AcpEvent::TurnDone {
                        stop_reason: "error".into(),
                    });
                }
                return;
            }
            let result = msg.get("result").cloned().unwrap_or(Value::Null);
            if rid == INIT_ID {
                // 给适配器一点时间走完初始化收尾
                std::thread::sleep(std::time::Duration::from_millis(400));
                send_new_session(stdin, cwd, pending, next_id);
                return;
            }
            match kind {
                Some(ReqKind::NewSession) => {
                    if let Some(sid) = result.get("sessionId").and_then(Value::as_str) {
                        *session.lock().unwrap() = Some(sid.to_owned());
                        let (acp_model, acp_models) = parse_models(&result);
                        let (acp_mode, acp_modes) = parse_modes(&result);
                        let snap = parse_config_options(
                            result.get("configOptions").unwrap_or(&Value::Null),
                        );
                        // configOptions 的 model 是可直接 set 的纯模型值，
                        // models.availableModels 可能是模型/力度组合 id（codex-acp）——前者优先
                        let mut model = snap.current_model.clone().unwrap_or(acp_model);
                        if model.is_empty() {
                            model = "agent".into();
                        }
                        let models = if snap.models.is_empty() {
                            acp_models
                        } else {
                            snap.models
                        };
                        let mode = if acp_mode.is_empty() {
                            snap.current_mode.unwrap_or_default()
                        } else {
                            acp_mode
                        };
                        let modes = if acp_modes.is_empty() { snap.modes } else { acp_modes };
                        let _ = tx.send(AcpEvent::SessionReady {
                            model,
                            models,
                            mode,
                            modes,
                            effort: snap.current_effort.unwrap_or_default(),
                            effort_supported: snap.effort_supported,
                        });
                    } else {
                        let _ = tx.send(AcpEvent::Error(
                            "session/new 未返回 sessionId（可能需要先登录）".into(),
                        ));
                    }
                }
                Some(ReqKind::Prompt) => {
                    let stop = result
                        .get("stopReason")
                        .and_then(Value::as_str)
                        .unwrap_or("end_turn")
                        .to_owned();
                    let _ = tx.send(AcpEvent::TurnDone { stop_reason: stop });
                }
                Some(ReqKind::SetConfig) => {
                    let snap =
                        parse_config_options(result.get("configOptions").unwrap_or(&Value::Null));
                    let _ = tx.send(AcpEvent::ConfigOptions(snap));
                }
                // set_mode 成功无须额外处理（乐观更新 + current_mode_update 兜底）
                Some(ReqKind::SetMode) | None => {}
            }
        }

        // ---- 服务端通知 ----
        (None, Some("session/update")) => {
            // 新会话后旧会话可能仍有残余事件：按 sessionId 过滤（nextop 按会话路由）
            let cur = session.lock().unwrap().clone();
            if let (Some(m), Some(c)) = (
                msg.pointer("/params/sessionId").and_then(Value::as_str),
                cur.as_deref(),
            ) {
                if !m.is_empty() && m != c {
                    return;
                }
            }
            let Some(update) = msg.pointer("/params/update") else {
                return;
            };
            let kind = update
                .get("sessionUpdate")
                .and_then(Value::as_str)
                .unwrap_or("");
            match kind {
                "agent_message_chunk" | "agent_thought_chunk" => {
                    let text = update
                        .pointer("/content/text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    if !text.is_empty() {
                        let ev = if kind == "agent_message_chunk" {
                            AcpEvent::AgentChunk(text)
                        } else {
                            AcpEvent::ThoughtChunk(text)
                        };
                        let _ = tx.send(ev);
                    }
                }
                "available_commands_update" => {
                    let cmds = update
                        .get("availableCommands")
                        .and_then(Value::as_array)
                        .map(|cs| {
                            cs.iter()
                                .map(|c| (str_at(c, "name"), str_at(c, "description")))
                                .collect()
                        })
                        .unwrap_or_default();
                    let _ = tx.send(AcpEvent::Commands(cmds));
                }
                "usage_update" => {
                    let _ = tx.send(AcpEvent::Usage {
                        used: update.get("used").and_then(Value::as_u64).unwrap_or(0),
                        size: update.get("size").and_then(Value::as_u64).unwrap_or(0),
                    });
                }
                // 权限模式变化（nextop acpModeValue 的字段优先序）
                "current_mode_update" => {
                    let mode = ["currentModeId", "mode", "modeId", "mode_id", "name", "value"]
                        .iter()
                        .find_map(|k| update.get(*k).and_then(Value::as_str))
                        .unwrap_or("")
                        .to_owned();
                    if !mode.is_empty() {
                        let _ = tx.send(AcpEvent::CurrentMode(mode));
                    }
                }
                // 配置项变化（nextop applyACPUpdateToLiveState 的 config_option_update）
                "config_option_update" => {
                    let descriptors = update
                        .get("configOptions")
                        .or_else(|| update.get("config_options"))
                        .cloned()
                        .unwrap_or(Value::Null);
                    let snap = parse_config_options(&descriptors);
                    if !snap.models.is_empty()
                        || snap.current_model.is_some()
                        || snap.current_effort.is_some()
                        || snap.effort_supported
                        || !snap.modes.is_empty()
                        || snap.current_mode.is_some()
                    {
                        let _ = tx.send(AcpEvent::ConfigOptions(snap));
                    }
                }
                "plan" => {
                    let entries = update
                        .get("entries")
                        .and_then(Value::as_array)
                        .map(|es| {
                            es.iter()
                                .map(|e| PlanEntry {
                                    content: str_at(e, "content"),
                                    status: str_at(e, "status"),
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let _ = tx.send(AcpEvent::Plan(entries));
                }
                "tool_call" => {
                    let (text, diff, locations) = parse_tool_fields(update);
                    let _ = tx.send(AcpEvent::ToolCall {
                        id: str_at(update, "toolCallId"),
                        title: str_at(update, "title"),
                        kind: str_at(update, "kind"),
                        text,
                        diff,
                        locations,
                    });
                }
                "tool_call_update" => {
                    let (text, diff, locations) = parse_tool_fields(update);
                    let status = update
                        .get("status")
                        .and_then(Value::as_str)
                        .or_else(|| update.pointer("/rawOutput/status").and_then(Value::as_str))
                        .map(str::to_owned);
                    let title = update
                        .get("title")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let _ = tx.send(AcpEvent::ToolUpdate {
                        id: str_at(update, "toolCallId"),
                        status,
                        title,
                        text,
                        diff,
                        locations,
                    });
                }
                _ => {}
            }
        }

        // ---- 服务端请求：权限。自检模式直接批准；正常模式转发 UI 等用户选择 ----
        (Some(rid), Some("session/request_permission")) => {
            let options = msg
                .pointer("/params/options")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if auto_approve.load(Ordering::Relaxed) {
                let pick = options
                    .iter()
                    .find(|o| {
                        o.get("kind")
                            .and_then(Value::as_str)
                            .is_some_and(|k| k.starts_with("allow"))
                    })
                    .or(options.first())
                    .and_then(|o| o.get("optionId"))
                    .cloned()
                    .unwrap_or(Value::String("allow".into()));
                send_json(
                    stdin,
                    &json!({
                        "jsonrpc": "2.0", "id": rid,
                        "result": { "outcome": { "outcome": "selected", "optionId": pick } }
                    }),
                );
            } else {
                let title = msg
                    .pointer("/params/toolCall/title")
                    .and_then(Value::as_str)
                    .unwrap_or("执行操作")
                    .to_owned();
                let opts = options
                    .iter()
                    .map(|o| PermissionOption {
                        id: str_at(o, "optionId"),
                        name: str_at(o, "name"),
                        kind: str_at(o, "kind"),
                    })
                    .collect();
                let _ = tx.send(AcpEvent::PermissionRequest {
                    request_id: rid,
                    title,
                    options: opts,
                });
            }
        }

        // 其他服务端请求：返回空结果避免阻塞
        (Some(rid), Some(_)) => {
            send_json(stdin, &json!({ "jsonrpc": "2.0", "id": rid, "result": null }));
        }
        _ => {}
    }
}

fn str_at(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}
