//! Agent 聊天界面（Codex / Claude Code 共用）：逐帧参考 nextop 的 Agent GUI——
//! 左侧会话列表（新建/切换/置顶/删除，状态点+标题——nextop 的 session list）+
//! 消息流（用户气泡 / 助手 Markdown / 思考折叠 / 工具卡片含 diff 与文件位置 /
//! Plan 计划清单）+ 底部 composer + 处理中省略号 + 发送/停止按钮。
//! 每个会话独立一个适配器进程（同 nextop：一个 session 一个 sidecar）。

use std::sync::mpsc::Receiver;

use egui::{
    vec2, Align, Color32, CornerRadius, FontId, Key, Layout, RichText, ScrollArea, Stroke,
    StrokeKind, TextEdit, Ui,
};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use serde::{Deserialize, Serialize};

use crate::codex::{
    default_mode, effort_config_id, static_modes, AcpAgent, AcpEvent, AgentBackend,
    ConfigSnapshot, ModeOption, ModelOption, PermissionOption, PlanEntry, ToolDiff,
    CLAUDE_STATIC_MODELS, REASONING_EFFORTS,
};

/// 把 agent 下发的模式列表本地化成 nextop 的中文标签（zh-CN locale 目录）；
/// 目录之外的模式（claude 的 auto / plan）补上对应中文
fn localize_modes(provider: &str, modes: Vec<ModeOption>) -> Vec<ModeOption> {
    let zh = static_modes(provider);
    modes
        .into_iter()
        .map(|mut m| {
            if let Some(s) = zh.iter().find(|s| s.id == m.id) {
                m.name = s.name.clone();
                m.description = s.description.clone();
            } else {
                match m.id.as_str() {
                    "auto" => {
                        m.name = "自动判定".to_owned();
                        m.description = "由模型自动批准或拒绝权限请求".to_owned();
                    }
                    "plan" => {
                        m.name = "计划模式".to_owned();
                        m.description = "只做规划，不实际执行工具".to_owned();
                    }
                    _ => {}
                }
            }
            m
        })
        .collect()
}

#[derive(Serialize, Deserialize)]
pub enum ChatItem {
    User(String),
    Assistant(String),
    Thought { text: String, expanded: bool },
    Tool {
        id: String,
        title: String,
        kind: String,
        status: String,
        text: String,
        diff: Option<ToolDiff>,
        locations: Vec<String>,
        expanded: bool,
    },
    /// 权限审批卡片（询问模式，nextop 的 AgentApprovalCallCard）
    Approval {
        request_id: i64,
        title: String,
        options: Vec<PermissionOption>,
        decided: Option<String>,
    },
    /// 回合级系统提示（取消/上限/拒绝等 stopReason，nextop 的 turn 提示行）
    Notice(String),
    /// 回合文件变更汇总（nextop 的 AgentTurnSummaryRow）
    TurnSummary {
        /// (路径, +新增行, -删除行)
        files: Vec<(String, usize, usize)>,
        expanded: bool,
    },
}

/// draw_item 返回的用户动作
enum ItemAction {
    None,
    Permission { request_id: i64, option_id: String, name: String },
}

/// 会话列表行上的用户动作
enum SideAction {
    None,
    New,
    Switch(usize),
    Pin(usize),
    Close(usize),
}

const NEW_SESSION_TITLE: &str = "新会话";

// ---- 持久化（nextop 的会话/消息存储，这里按 provider 一个 JSON 文件）----

#[derive(Serialize, Deserialize, Default)]
struct StoreFile {
    #[serde(default)]
    recent_workspaces: Vec<String>,
    #[serde(default)]
    sessions: Vec<SessionRecord>,
}

#[derive(Serialize, Deserialize)]
struct SessionRecord {
    title: String,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    items: Vec<ChatItem>,
    #[serde(default)]
    plan: Vec<PlanEntry>,
}

/// 写盘用借用版，避免每次保存克隆全部历史
#[derive(Serialize)]
struct StoreFileRef<'a> {
    recent_workspaces: &'a [String],
    sessions: Vec<SessionRecordRef<'a>>,
}

#[derive(Serialize)]
struct SessionRecordRef<'a> {
    title: &'a str,
    pinned: bool,
    workspace: &'a Option<String>,
    items: &'a [ChatItem],
    plan: &'a [PlanEntry],
}

fn store_path(provider: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    format!("{home}/MirageWorkspace/agent-sessions-{provider}.json")
}

fn default_workspace_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    format!("{home}/MirageWorkspace")
}

/// 工作区显示名：目录名（None = 默认工作区）
fn ws_label(ws: &Option<String>) -> String {
    match ws {
        None => "默认工作区".to_owned(),
        Some(p) => std::path::Path::new(p.trim_end_matches('/'))
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.clone()),
    }
}

fn push_recent(recents: &mut Vec<String>, path: String) {
    recents.retain(|p| *p != path);
    recents.insert(0, path);
    recents.truncate(8);
}

/// 系统文件夹选择（osascript choose folder），后台线程避免卡住 UI
fn pick_folder_async() -> Receiver<Option<String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let out = std::process::Command::new("osascript")
            .args([
                "-e",
                "POSIX path of (choose folder with prompt \"选择工作区文件夹\")",
            ])
            .output();
        let path = out
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .trim_end_matches('/')
                    .to_owned()
            })
            .filter(|s| !s.is_empty());
        let _ = tx.send(path);
    });
    rx
}

/// 单个 agent 会话（nextop 的 Session：标题/置顶/工作区/状态 + 独立 sidecar 进程）
struct SessionPane {
    title: String,
    pinned: bool,
    /// 工作区目录（None = 默认 ~/MirageWorkspace；接入后不可改，同 nextop cwd）
    workspace: Option<String>,
    /// 内容有变更待写盘
    dirty: bool,
    runtime: Option<AcpAgent>,
    items: Vec<ChatItem>,
    plan: Vec<PlanEntry>,
    draft: String,
    running: bool,
    model: String,
    /// 模型目录（claude 静态目录 / codex 由 configOptions 下发——对齐 nextop）
    models: Vec<ModelOption>,
    /// 当前权限模式与该 provider 的模式目录（nextop 的 permission modes）
    current_mode: String,
    modes: Vec<ModeOption>,
    /// 推理力度（nextop 的 reasoning effort：low/medium/high/xhigh）
    current_effort: String,
    effort_supported: bool,
    usage: Option<(u64, u64)>,
    /// 可用斜杠命令（来自 available_commands_update）
    commands: Vec<(String, String)>,
    /// 运行中追加的排队 prompts（nextop 的 queued prompts）
    queued: Vec<String>,
    /// 已展开的工具组（按组起始下标）
    groups_expanded: std::collections::HashSet<usize>,
    error: Option<String>,
    started: bool,
}

impl SessionPane {
    fn new() -> Self {
        Self {
            title: NEW_SESSION_TITLE.to_owned(),
            pinned: false,
            workspace: None,
            dirty: false,
            runtime: None,
            items: Vec::new(),
            plan: Vec::new(),
            draft: String::new(),
            running: false,
            model: String::new(),
            models: Vec::new(),
            current_mode: String::new(),
            modes: Vec::new(),
            current_effort: String::new(),
            effort_supported: false,
            usage: None,
            commands: Vec::new(),
            queued: Vec::new(),
            groups_expanded: std::collections::HashSet::new(),
            error: None,
            started: false,
        }
    }

    /// 从持久化记录还原（runtime 不恢复：旧 sidecar 已死，下次发消息重新接入）
    fn from_record(rec: SessionRecord) -> Self {
        let mut s = Self::new();
        s.title = rec.title;
        s.pinned = rec.pinned;
        s.workspace = rec.workspace;
        s.items = rec.items;
        s.plan = rec.plan;
        // 重启后未决审批已无法回应：标记过期
        for it in &mut s.items {
            if let ChatItem::Approval { decided, .. } = it {
                if decided.is_none() {
                    *decided = Some("已过期（会话重启）".to_owned());
                }
            }
        }
        s
    }

    /// 发送首条消息时启动适配器（每会话独立进程，cwd = 所选工作区）
    fn ensure_started(&mut self, backend: &'static AgentBackend) {
        if self.started {
            return;
        }
        self.started = true;
        // 默认用专用空工作目录，避免适配器在 HOME 这种超大目录上扫描卡死
        let cwd = self.workspace.clone().unwrap_or_else(default_workspace_dir);
        let _ = std::fs::create_dir_all(&cwd);
        match AcpAgent::spawn(backend, &cwd) {
            Ok(rt) => self.runtime = Some(rt),
            Err(e) => self.error = Some(e),
        }
    }

    fn ready(&self) -> bool {
        self.runtime.as_ref().is_some_and(|r| r.ready())
    }

    /// 有未决审批请求（nextop 的 waiting 状态）
    fn waiting(&self) -> bool {
        self.items
            .iter()
            .any(|i| matches!(i, ChatItem::Approval { decided: None, .. }))
    }

    /// 推入用户消息；首条用户消息生成会话标题（nextop 从首条 prompt 派生 Title）
    fn push_user(&mut self, text: String) {
        if self.title == NEW_SESSION_TITLE {
            self.title = text.chars().take(16).collect();
        }
        self.items.push(ChatItem::User(text));
        self.dirty = true;
    }

    fn submit(&mut self, backend: &'static AgentBackend) {
        let text = self.draft.trim().to_owned();
        if text.is_empty() {
            return;
        }
        // 运行中 -> 排队（nextop 的 queued prompts）
        if self.running {
            self.queued.push(text);
            self.draft.clear();
            return;
        }
        // 首条消息触发接入（此前可选工作区——nextop 创建会话时定 cwd）
        if !self.started {
            self.ensure_started(backend);
        }
        if !self.ready() {
            // 连接中：先排队，pump 在就绪后自动发出
            self.queued.push(text);
            self.draft.clear();
            return;
        }
        self.push_user(text.clone());
        if let Some(rt) = &self.runtime {
            rt.send_prompt(&text);
        }
        self.running = true;
        self.draft.clear();
    }

    /// 把 ACP 事件灌进消息列表（后台会话也持续消化）
    fn pump(&mut self, backend: &'static AgentBackend) {
        {
            let Some(rt) = &self.runtime else { return };
            while let Ok(ev) = rt.rx.try_recv() {
                match ev {
                    AcpEvent::SessionReady {
                        model,
                        models,
                        mode,
                        modes,
                        effort,
                        effort_supported,
                    } => {
                        let provider = backend.provider;
                        self.model = model;
                        // 模型目录：agent 未下发时回退到静态目录（nextop 的 claude-static）
                        let mut ms = models;
                        if ms.is_empty() && provider == "claude-code" {
                            ms = CLAUDE_STATIC_MODELS
                                .iter()
                                .map(|m| ModelOption {
                                    id: (*m).to_owned(),
                                    name: (*m).to_owned(),
                                })
                                .collect();
                        }
                        // 当前模型不在目录里则补一项（nextop containsModelOption 的兜底）
                        if !self.model.is_empty()
                            && self.model != "agent"
                            && !ms.iter().any(|m| m.id == self.model)
                        {
                            ms.push(ModelOption {
                                id: self.model.clone(),
                                name: self.model.clone(),
                            });
                        }
                        self.models = ms;
                        self.modes = if modes.is_empty() {
                            static_modes(provider)
                        } else {
                            localize_modes(provider, modes)
                        };
                        self.current_mode = if mode.is_empty() {
                            default_mode(provider).to_owned()
                        } else {
                            mode
                        };
                        // codex 始终支持 reasoning_effort（nextop 对 codex 无条件应用）
                        self.effort_supported = effort_supported || provider == "codex";
                        self.current_effort = if effort.is_empty() {
                            if provider == "codex" {
                                "high".to_owned() // 与启动参数 model_reasoning_effort=high 一致
                            } else {
                                String::new()
                            }
                        } else {
                            effort
                        };
                    }
                    AcpEvent::CurrentMode(m) => self.current_mode = m,
                    AcpEvent::ConfigOptions(snap) => {
                        let ConfigSnapshot {
                            models,
                            current_model,
                            current_effort,
                            effort_supported,
                            modes,
                            current_mode,
                        } = snap;
                        if !models.is_empty() {
                            self.models = models;
                        }
                        if let Some(m) = current_model {
                            self.model = m;
                        }
                        if let Some(e) = current_effort {
                            self.current_effort = e;
                        }
                        if effort_supported {
                            self.effort_supported = true;
                        }
                        if !modes.is_empty() {
                            self.modes = localize_modes(backend.provider, modes);
                        }
                        if let Some(m) = current_mode {
                            self.current_mode = m;
                        }
                    }
                    AcpEvent::AgentChunk(t) => {
                        if let Some(ChatItem::Assistant(body)) = self.items.last_mut() {
                            body.push_str(&t);
                        } else {
                            self.items.push(ChatItem::Assistant(t));
                        }
                    }
                    AcpEvent::ThoughtChunk(t) => {
                        if let Some(ChatItem::Thought { text, .. }) = self.items.last_mut() {
                            text.push_str(&t);
                        } else {
                            self.items.push(ChatItem::Thought {
                                text: t,
                                expanded: false,
                            });
                        }
                    }
                    AcpEvent::Plan(entries) => self.plan = entries,
                    AcpEvent::Usage { used, size } => self.usage = Some((used, size)),
                    AcpEvent::ToolCall {
                        id,
                        title,
                        kind,
                        text,
                        diff,
                        locations,
                    } => {
                        self.items.push(ChatItem::Tool {
                            id,
                            title,
                            kind,
                            status: "running".into(),
                            text: text.unwrap_or_default(),
                            diff,
                            locations,
                            expanded: false,
                        });
                    }
                    AcpEvent::ToolUpdate {
                        id,
                        status,
                        title,
                        text,
                        diff,
                        locations,
                    } => {
                        if let Some(ChatItem::Tool {
                            status: s,
                            title: t,
                            text: txt,
                            diff: d,
                            locations: locs,
                            ..
                        }) = self.items.iter_mut().rev().find(
                            |it| matches!(it, ChatItem::Tool { id: tid, .. } if *tid == id),
                        ) {
                            if let Some(ns) = status {
                                *s = ns;
                            }
                            if let Some(nt) = title {
                                *t = nt;
                            }
                            if let Some(ntext) = text {
                                *txt = ntext;
                            }
                            if diff.is_some() {
                                *d = diff;
                            }
                            if !locations.is_empty() {
                                *locs = locations;
                            }
                        }
                    }
                    AcpEvent::Commands(cmds) => self.commands = cmds,
                    AcpEvent::PermissionRequest {
                        request_id,
                        title,
                        options,
                    } => {
                        self.items.push(ChatItem::Approval {
                            request_id,
                            title,
                            options,
                            decided: None,
                        });
                    }
                    AcpEvent::TurnDone { stop_reason } => {
                        self.running = false;
                        // Turn Summary：聚合本回合的文件变更（nextop AgentTurnSummaryRow）
                        let mut files: Vec<(String, usize, usize)> = Vec::new();
                        for it in self.items.iter().rev() {
                            match it {
                                ChatItem::User(_) => break,
                                ChatItem::Tool { diff: Some(d), .. } if !d.path.is_empty()
                                    // 同一文件取最后一次 diff（rev 遍历先见即最后）
                                    && !files.iter().any(|(p, ..)| *p == d.path) => {
                                        files.push((
                                            d.path.clone(),
                                            d.new_text.lines().count(),
                                            d.old_text.lines().count(),
                                        ));
                                    }
                                _ => {}
                            }
                        }
                        if !files.is_empty() {
                            files.reverse();
                            self.items.push(ChatItem::TurnSummary {
                                files,
                                expanded: false,
                            });
                        }
                        // 非正常结束的回合给一行提示（nextop 的 turn canceled/failed 提示）
                        let note = match stop_reason.as_str() {
                            "cancelled" | "canceled" => Some("已停止本回合"),
                            "max_tokens" => Some("回合达到 token 上限"),
                            "max_turn_requests" => Some("回合达到请求次数上限"),
                            "refusal" => Some("模型拒绝继续本次请求"),
                            _ => None,
                        };
                        if let Some(n) = note {
                            self.items.push(ChatItem::Notice(n.to_owned()));
                        }
                        self.dirty = true; // 回合结束：消息记录落盘
                    }
                    AcpEvent::Error(e) => {
                        self.error = Some(e);
                        self.running = false;
                        self.dirty = true;
                    }
                }
            }
        }

        // 回合结束且有排队 prompt：自动发出下一条（nextop 的 queued prompts）
        if !self.running && !self.queued.is_empty() {
            let ready = self.runtime.as_ref().is_some_and(|r| r.ready());
            if ready {
                let next = self.queued.remove(0);
                self.push_user(next.clone());
                if let Some(rt) = &self.runtime {
                    rt.send_prompt(&next);
                }
                self.running = true;
            }
        }
    }

    /// 会话列表行的状态点颜色（nextop session status：连接/就绪/工作/等待/出错）
    fn status_color(&self) -> Color32 {
        if self.error.is_some() {
            Color32::from_rgb(0xE5, 0x5B, 0x5B)
        } else if !self.started {
            Color32::from_gray(80)
        } else if !self.ready() {
            Color32::from_gray(110)
        } else if self.waiting() || self.running {
            Color32::from_rgb(0xE8, 0xB4, 0x3A)
        } else {
            Color32::from_rgb(0x4C, 0xC3, 0x66)
        }
    }
}

pub struct AgentApp {
    backend: &'static AgentBackend,
    display_name: &'static str,
    /// 会话列表（nextop 的 workspace agent sessions）
    sessions: Vec<SessionPane>,
    active: usize,
    /// 打开中的头部下拉菜单："mode" | "model" | "effort" | "ws"
    open_menu: Option<&'static str>,
    /// 最近使用过的工作区目录（持久化）
    recent_workspaces: Vec<String>,
    /// 进行中的文件夹选择对话框：(会话下标, 结果通道)
    folder_pick: Option<(usize, Receiver<Option<String>>)>,
    /// 自检/截图模式不读写持久化文件，保证回归确定性
    persist: bool,
    /// 结构性变更（建删/置顶/工作区）待写盘
    dirty: bool,
    md_cache: CommonMarkCache,
}

impl AgentApp {
    pub fn new(backend: &'static AgentBackend, display_name: &'static str) -> Self {
        // 自检/截图模式默认不读写存档（保证回归确定性）；MIRAGE_PERSIST=1 强制开（测持久化用）
        let persist = std::env::var("MIRAGE_PERSIST").is_ok()
            || (std::env::var("MIRAGE_SHOT").is_err() && std::env::var("MIRAGE_APPSHOT").is_err());
        let mut sessions: Vec<SessionPane> = Vec::new();
        let mut recent_workspaces = Vec::new();
        if persist {
            if let Ok(bytes) = std::fs::read(store_path(backend.provider)) {
                if let Ok(store) = serde_json::from_slice::<StoreFile>(&bytes) {
                    recent_workspaces = store.recent_workspaces;
                    sessions = store
                        .sessions
                        .into_iter()
                        .map(SessionPane::from_record)
                        .collect();
                }
            }
        }
        if sessions.is_empty() {
            sessions.push(SessionPane::new());
        }
        let active = sessions.len() - 1;
        Self {
            backend,
            display_name,
            sessions,
            active,
            open_menu: None,
            recent_workspaces,
            folder_pick: None,
            persist,
            dirty: false,
            md_cache: CommonMarkCache::default(),
        }
    }

    /// 写盘：全部会话的消息记录 + 最近工作区（nextop 的会话持久化）
    fn save(&self) {
        if !self.persist {
            return;
        }
        let path = store_path(self.backend.provider);
        if let Some(dir) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let store = StoreFileRef {
            recent_workspaces: &self.recent_workspaces,
            sessions: self
                .sessions
                .iter()
                .map(|s| SessionRecordRef {
                    title: &s.title,
                    pinned: s.pinned,
                    workspace: &s.workspace,
                    items: &s.items,
                    plan: &s.plan,
                })
                .collect(),
        };
        if let Ok(json) = serde_json::to_vec(&store) {
            let _ = std::fs::write(path, json);
        }
    }

    /// 有变更就写盘（pump 每帧都会被 main 调用，窗口关着也能保存）
    fn save_if_dirty(&mut self) {
        if self.dirty || self.sessions.iter().any(|s| s.dirty) {
            self.save();
            self.dirty = false;
            for s in &mut self.sessions {
                s.dirty = false;
            }
        }
    }

    fn active_session(&mut self) -> &mut SessionPane {
        if self.sessions.is_empty() {
            self.sessions.push(SessionPane::new());
            self.active = 0;
        }
        let n = self.sessions.len();
        if self.active >= n {
            self.active = n - 1;
        }
        &mut self.sessions[self.active]
    }

    /// 所有会话持续消化事件（后台会话的回合也会推进——nextop 会话并行）
    pub fn pump(&mut self) {
        let backend = self.backend;
        for s in &mut self.sessions {
            s.pump(backend);
        }
        self.save_if_dirty();
    }

    pub fn animating(&self) -> bool {
        self.sessions.iter().any(|s| s.running)
    }

    /// 当前激活会话是否在跑回合（自检用）
    pub fn running(&self) -> bool {
        self.sessions.get(self.active).is_some_and(|s| s.running)
    }

    /// 自检模式：会话就绪后程序化发送一轮 prompt，返回是否已发送
    pub fn auto_prompt(&mut self, text: &str) -> bool {
        let backend = self.backend;
        self.active_session().ensure_started(backend);
        self.pump();
        let s = self.active_session();
        if !s.ready() || s.running {
            return false;
        }
        s.push_user(text.to_owned());
        if let Some(rt) = &s.runtime {
            rt.send_prompt(text);
        }
        s.running = true;
        true
    }

    pub fn has_assistant_reply(&self) -> bool {
        self.sessions
            .get(self.active)
            .is_some_and(|s| s.items.iter().any(|i| matches!(i, ChatItem::Assistant(_))))
    }

    /// 自检/演示用：展开所有思考与工具折叠块
    pub fn expand_all(&mut self) {
        let s = self.active_session();
        for it in &mut s.items {
            match it {
                ChatItem::Thought { expanded, .. } | ChatItem::Tool { expanded, .. } => {
                    *expanded = true
                }
                _ => {}
            }
        }
    }

    pub fn show(&mut self, ui: &mut Ui, now: f64) {
        let backend = self.backend;
        let display_name = self.display_name;
        // 会话不再激活即接入：首条消息才 spawn（之前可选工作区）
        self.pump();

        // 文件夹选择对话框的结果（osascript 后台线程）
        if let Some((idx, rx)) = &self.folder_pick {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(300));
            if let Ok(res) = rx.try_recv() {
                let idx = *idx;
                if let Some(path) = res {
                    if let Some(s) = self.sessions.get_mut(idx) {
                        if !s.started {
                            s.workspace = Some(path.clone());
                            s.dirty = true;
                        }
                    }
                    push_recent(&mut self.recent_workspaces, path);
                    self.dirty = true;
                }
                self.folder_pick = None;
            }
        }

        let bg = Color32::from_rgb(0x1B, 0x1B, 0x1F);
        ui.painter().rect_filled(ui.max_rect(), 0, bg);
        let full = ui.max_rect();

        // ---- 左侧会话列表（nextop 的 session list：置顶在前 + 状态点 + 标题）----
        let side_w = 150.0_f32.min(full.width() * 0.34);
        let side = egui::Rect::from_min_max(
            full.min,
            egui::pos2(full.left() + side_w, full.bottom()),
        );
        ui.painter()
            .rect_filled(side, 0, Color32::from_rgb(0x1E, 0x1E, 0x23));
        ui.painter().line_segment(
            [side.right_top(), side.right_bottom()],
            Stroke::new(1.0, Color32::from_gray(45)),
        );

        // 列表头：标题 + 新建按钮
        let side_head =
            egui::Rect::from_min_size(side.min, vec2(side.width(), 30.0));
        ui.painter().text(
            side_head.left_center() + vec2(10.0, 0.0),
            egui::Align2::LEFT_CENTER,
            "会话",
            FontId::proportional(11.5),
            Color32::from_gray(140),
        );
        let mut side_action = SideAction::None;
        let add_rect = egui::Rect::from_center_size(
            egui::pos2(side_head.right() - 16.0, side_head.center().y),
            vec2(20.0, 20.0),
        );
        let add_resp = ui.interact(add_rect, ui.id().with("sess-new"), egui::Sense::click());
        if add_resp.hovered() {
            ui.painter()
                .rect_filled(add_rect, CornerRadius::same(5), Color32::from_rgb(0x2B, 0x2B, 0x32));
        }
        ui.painter().text(
            add_rect.center(),
            egui::Align2::CENTER_CENTER,
            "+",
            FontId::proportional(15.0),
            if add_resp.hovered() {
                Color32::from_gray(230)
            } else {
                Color32::from_gray(150)
            },
        );
        if add_resp.clicked() {
            side_action = SideAction::New;
        }

        // 分组：按工作区区分（默认组在前），组内置顶在前、其余按创建顺序
        let mut groups: Vec<(Option<String>, Vec<usize>)> = Vec::new();
        for (i, s) in self.sessions.iter().enumerate() {
            if let Some(g) = groups.iter_mut().find(|(k, _)| *k == s.workspace) {
                g.1.push(i);
            } else {
                groups.push((s.workspace.clone(), vec![i]));
            }
        }
        groups.sort_by_key(|(k, _)| k.is_some());
        for (_, idxs) in &mut groups {
            idxs.sort_by_key(|&i| (!self.sessions[i].pinned, i));
        }
        // 只有出现具体工作区时才显示组头（全默认时保持清爽）
        let show_group_heads =
            groups.len() > 1 || groups.first().is_some_and(|(k, _)| k.is_some());
        let rows_rect = egui::Rect::from_min_max(
            egui::pos2(side.left(), side_head.bottom()),
            side.max,
        );
        let mut rows_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rows_rect)
                .layout(Layout::top_down(Align::Min)),
        );
        rows_ui.set_clip_rect(rows_rect.intersect(ui.clip_rect()));
        ScrollArea::vertical()
            .id_salt("session-list")
            .auto_shrink([false, false])
            .max_height(rows_rect.height())
            .show(&mut rows_ui, |ui| {
                for (ws, idxs) in &groups {
                    // 工作区组头（nextop 列表按工作区区分）
                    if show_group_heads {
                        let (gr, _) = ui.allocate_exact_size(
                            vec2(ui.available_width(), 20.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().text(
                            egui::pos2(gr.left() + 10.0, gr.center().y + 2.0),
                            egui::Align2::LEFT_CENTER,
                            ws_label(ws),
                            FontId::proportional(10.0),
                            Color32::from_gray(115),
                        );
                    }
                    for &idx in idxs {
                    let (row, row_resp) = ui.allocate_exact_size(
                        vec2(ui.available_width(), 30.0),
                        egui::Sense::click(),
                    );
                    let s = &self.sessions[idx];
                    let is_active = idx == self.active;
                    let hovered = row_resp.hovered();
                    if is_active {
                        ui.painter().rect_filled(
                            row.shrink2(vec2(4.0, 1.0)),
                            CornerRadius::same(6),
                            Color32::from_rgb(0x33, 0x33, 0x3B),
                        );
                    } else if hovered {
                        ui.painter().rect_filled(
                            row.shrink2(vec2(4.0, 1.0)),
                            CornerRadius::same(6),
                            Color32::from_rgb(0x26, 0x26, 0x2C),
                        );
                    }
                    // 状态点
                    ui.painter().circle_filled(
                        egui::pos2(row.left() + 14.0, row.center().y),
                        3.0,
                        s.status_color(),
                    );
                    // 悬停时右侧显示 置顶/关闭；置顶常驻 ★
                    let close_rect = egui::Rect::from_center_size(
                        egui::pos2(row.right() - 14.0, row.center().y),
                        vec2(16.0, 16.0),
                    );
                    let pin_rect = egui::Rect::from_center_size(
                        egui::pos2(row.right() - 32.0, row.center().y),
                        vec2(16.0, 16.0),
                    );
                    let show_tools = hovered
                        || ui.rect_contains_pointer(close_rect)
                        || ui.rect_contains_pointer(pin_rect);
                    let title_right = if show_tools {
                        pin_rect.left() - 2.0
                    } else if s.pinned {
                        row.right() - 24.0
                    } else {
                        row.right() - 6.0
                    };
                    let title_color = if is_active {
                        Color32::from_gray(235)
                    } else {
                        Color32::from_gray(175)
                    };
                    ui.painter()
                        .with_clip_rect(egui::Rect::from_min_max(
                            egui::pos2(row.left() + 24.0, row.top()),
                            egui::pos2(title_right, row.bottom()),
                        ))
                        .text(
                            egui::pos2(row.left() + 24.0, row.center().y),
                            egui::Align2::LEFT_CENTER,
                            &s.title,
                            FontId::proportional(11.5),
                            title_color,
                        );
                    if show_tools {
                        let pin_resp = ui.interact(
                            pin_rect,
                            ui.id().with(("sess-pin", idx)),
                            egui::Sense::click(),
                        );
                        ui.painter().text(
                            pin_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            if s.pinned { "★" } else { "☆" },
                            FontId::proportional(11.0),
                            if pin_resp.hovered() {
                                Color32::from_rgb(0xE8, 0xB4, 0x3A)
                            } else {
                                Color32::from_gray(130)
                            },
                        );
                        let close_resp = ui.interact(
                            close_rect,
                            ui.id().with(("sess-close", idx)),
                            egui::Sense::click(),
                        );
                        ui.painter().text(
                            close_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "✕",
                            FontId::proportional(11.0),
                            if close_resp.hovered() {
                                Color32::from_rgb(0xE5, 0x5B, 0x5B)
                            } else {
                                Color32::from_gray(130)
                            },
                        );
                        if close_resp.clicked() {
                            side_action = SideAction::Close(idx);
                        } else if pin_resp.clicked() {
                            side_action = SideAction::Pin(idx);
                        } else if row_resp.clicked() {
                            side_action = SideAction::Switch(idx);
                        }
                    } else if s.pinned {
                        ui.painter().text(
                            egui::pos2(row.right() - 14.0, row.center().y),
                            egui::Align2::CENTER_CENTER,
                            "★",
                            FontId::proportional(10.0),
                            Color32::from_gray(110),
                        );
                        if row_resp.clicked() {
                            side_action = SideAction::Switch(idx);
                        }
                    } else if row_resp.clicked() {
                        side_action = SideAction::Switch(idx);
                    }
                    }
                }
            });

        // 应用会话列表动作（nextop 的 Create/Delete/UpdatePin/切换）
        match side_action {
            SideAction::None => {}
            SideAction::New => {
                self.sessions.push(SessionPane::new());
                self.active = self.sessions.len() - 1;
                self.open_menu = None;
                self.dirty = true;
            }
            SideAction::Switch(i) => {
                if i != self.active {
                    self.active = i;
                    self.open_menu = None;
                }
            }
            SideAction::Pin(i) => {
                if let Some(s) = self.sessions.get_mut(i) {
                    s.pinned = !s.pinned;
                    s.dirty = true;
                }
            }
            SideAction::Close(i) => {
                if i < self.sessions.len() {
                    self.sessions.remove(i); // drop 时杀掉 sidecar 进程
                    if self.sessions.is_empty() {
                        self.sessions.push(SessionPane::new());
                        self.active = 0;
                    } else if i < self.active || self.active >= self.sessions.len() {
                        self.active = self.active.saturating_sub(1);
                    }
                    self.open_menu = None;
                    self.dirty = true;
                }
            }
        }
        // 校正 active 越界（不再激活即接入）
        let _ = self.active_session();

        // ---- 右侧聊天区（原有布局整体右移）----
        let main_rect = egui::Rect::from_min_max(
            egui::pos2(side.right() + 1.0, full.top()),
            full.max,
        );
        let composer_h = 78.0;
        let header_h = 30.0;
        let mut want_new = false;
        {
            let Self {
                sessions,
                active,
                open_menu,
                md_cache,
                recent_workspaces,
                folder_pick,
                ..
            } = self;
            let s = &mut sessions[*active];

            // ---- 头部：后端名 + 状态 + 会话选项（模式/模型/力度/新会话——nextop composer options）----
            let header =
                egui::Rect::from_min_size(main_rect.min, vec2(main_rect.width(), header_h));
            ui.painter()
                .rect_filled(header, 0, Color32::from_rgb(0x22, 0x22, 0x27));

            let session_ready = s.ready();
            let waiting = s.waiting();
            let mut status = if s.error.is_some() {
                "出错".to_owned()
            } else if !s.started {
                "未接入 · 发送后启动".to_owned()
            } else if !session_ready {
                format!("连接 {}…", backend.command)
            } else if waiting {
                format!("{} · 等待批准", s.model)
            } else if s.running {
                format!("{} · 工作中", s.model)
            } else {
                format!("{} · 就绪", s.model)
            };
            if s.started && s.workspace.is_some() {
                status.push_str(&format!(" · {}", ws_label(&s.workspace)));
            }
            if let Some((used, size)) = s.usage {
                if size > 0 {
                    status.push_str(&format!(
                        " · 上下文 {:.0}k/{:.0}k",
                        used as f64 / 1000.0,
                        size as f64 / 1000.0
                    ));
                }
            }

            // 右侧按钮组（从右往左排）。未接入：新会话｜工作区（可不选）；
            // 已接入：新会话｜模式｜模型｜力度（cwd 已定，同 nextop 不可改）
            let mut btn_x = header.right() - 8.0;
            let mut place = |w: f32| {
                let r = egui::Rect::from_min_max(
                    egui::pos2(btn_x - w, header.top() + 4.0),
                    egui::pos2(btn_x, header.bottom() - 4.0),
                );
                btn_x -= w + 6.0;
                r
            };
            let new_rect = place(56.0);
            let (ws_rect, mode_rect, model_rect, effort_rect) = if !s.started {
                (Some(place(150.0)), None, None, None)
            } else {
                (
                    None,
                    Some(place(86.0)),
                    Some(place(108.0)),
                    if s.effort_supported {
                        Some(place(64.0))
                    } else {
                        None
                    },
                )
            };
            let btns_left = btn_x;

            // 左侧标题与状态（裁剪到按钮区之前，避免窄窗口重叠）
            ui.painter()
                .with_clip_rect(egui::Rect::from_min_max(
                    header.min,
                    egui::pos2(btns_left - 4.0, header.bottom()),
                ))
                .text(
                    header.left_center() + vec2(12.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    format!("{}    {status}", display_name),
                    FontId::proportional(12.0),
                    Color32::from_gray(170),
                );

            let mode_label = s
                .modes
                .iter()
                .find(|m| m.id == s.current_mode)
                .map(|m| m.name.clone())
                .unwrap_or_else(|| {
                    if s.current_mode.is_empty() {
                        "模式".to_owned()
                    } else {
                        s.current_mode.clone()
                    }
                });
            let model_label = if s.model.is_empty() {
                "模型".to_owned()
            } else {
                s.model.chars().take(14).collect::<String>()
            };
            let effort_label = REASONING_EFFORTS
                .iter()
                .find(|(v, _)| *v == s.current_effort)
                .map(|(_, l)| format!("力度 {l}"))
                .unwrap_or_else(|| "力度".to_owned());

            if header_btn(ui, new_rect, "新会话", "new-session", false).clicked() {
                want_new = true;
            }
            if let Some(wr) = ws_rect {
                let label = format!("工作区 {}", ws_label(&s.workspace));
                if header_btn(ui, wr, &label, "ws-btn", *open_menu == Some("ws")).clicked() {
                    *open_menu = if *open_menu == Some("ws") { None } else { Some("ws") };
                }
            }
            if let Some(mr) = mode_rect {
                if header_btn(ui, mr, &mode_label, "mode-btn", *open_menu == Some("mode"))
                    .clicked()
                {
                    *open_menu = if *open_menu == Some("mode") { None } else { Some("mode") };
                }
            }
            if let Some(mr) = model_rect {
                if header_btn(ui, mr, &model_label, "model-btn", *open_menu == Some("model"))
                    .clicked()
                {
                    *open_menu = if *open_menu == Some("model") { None } else { Some("model") };
                }
            }
            if let Some(er) = effort_rect {
                if header_btn(ui, er, &effort_label, "effort-btn", *open_menu == Some("effort"))
                    .clicked()
                {
                    *open_menu = if *open_menu == Some("effort") { None } else { Some("effort") };
                }
            }
            let menu_anchor = match *open_menu {
                Some("mode") => mode_rect,
                Some("model") => model_rect,
                Some("effort") => effort_rect,
                Some("ws") => ws_rect,
                _ => None,
            };

            // ---- 消息列表 ----
            let list_rect = egui::Rect::from_min_max(
                header.left_bottom(),
                egui::pos2(main_rect.right(), main_rect.bottom() - composer_h),
            );
            let list_inner = list_rect.shrink2(vec2(10.0, 4.0));
            let mut list_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(list_inner)
                    .layout(Layout::top_down(Align::Min)),
            );
            list_ui.set_clip_rect(list_inner.intersect(ui.clip_rect()));
            ScrollArea::vertical()
                .stick_to_bottom(true)
                .auto_shrink([false, false])
                .max_height(list_inner.height())
                .show(&mut list_ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add_space(6.0);
                    if let Some(err) = &s.error {
                        ui.colored_label(Color32::from_rgb(0xFF, 0x45, 0x3A), err);
                    }
                    // 未接入空会话：提示可选工作区（nextop 创建会话面板的提示）
                    if !s.started && s.items.is_empty() && s.error.is_none() {
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new("可先在右上角选择工作区（可不选，默认 MirageWorkspace），\n输入消息即接入会话。")
                                .color(Color32::from_gray(120))
                                .size(12.0),
                        );
                    }
                    let cache = &mut *md_cache;
                    // 工具分组：连续 ≥3 个已完成工具折叠成一行（nextop 的 AgentToolGroupRow）
                    let mut action = ItemAction::None;
                    let len = s.items.len();
                    let mut i = 0;
                    while i < len {
                        let group_end = {
                            let is_done_tool = |it: &ChatItem| {
                                matches!(it, ChatItem::Tool { status, .. }
                                    if matches!(status.as_str(), "completed" | "failed" | "canceled" | "cancelled"))
                            };
                            let mut j = i;
                            while j < len && is_done_tool(&s.items[j]) {
                                j += 1;
                            }
                            j
                        };
                        if group_end - i >= 3 {
                            let n = group_end - i;
                            let expanded = s.groups_expanded.contains(&i);
                            let arrow = if expanded { "▾" } else { "▸" };
                            let head = ui.add(
                                egui::Label::new(
                                    RichText::new(format!("{arrow} 🔧 {n} 个工具调用已完成"))
                                        .color(Color32::from_gray(150))
                                        .size(12.0),
                                )
                                .sense(egui::Sense::click()),
                            );
                            if head.clicked() {
                                if expanded {
                                    s.groups_expanded.remove(&i);
                                } else {
                                    s.groups_expanded.insert(i);
                                }
                            }
                            if expanded {
                                for k in i..group_end {
                                    if let ItemAction::Permission { .. } =
                                        draw_item(ui, &mut s.items[k], cache, now)
                                    {
                                        // 已完成工具组里不会出现审批卡
                                    }
                                    ui.add_space(6.0);
                                }
                            }
                            ui.add_space(8.0);
                            i = group_end;
                        } else {
                            let act = draw_item(ui, &mut s.items[i], cache, now);
                            if !matches!(act, ItemAction::None) {
                                action = act;
                            }
                            ui.add_space(8.0);
                            i += 1;
                        }
                    }
                    // 用户点了审批按钮：回传选择并标记结果
                    if let ItemAction::Permission {
                        request_id,
                        option_id,
                        name,
                    } = action
                    {
                        if let Some(rt) = &s.runtime {
                            rt.respond_permission(request_id, &option_id);
                        }
                        for it in &mut s.items {
                            if let ChatItem::Approval {
                                request_id: rid,
                                decided,
                                ..
                            } = it
                            {
                                if *rid == request_id {
                                    *decided = Some(name.clone());
                                }
                            }
                        }
                    }
                    // Plan 计划清单（常驻卡片，随更新刷新——nextop 的 plan 渲染）
                    if !s.plan.is_empty() {
                        draw_plan(ui, &s.plan);
                        ui.add_space(8.0);
                    }
                    // 处理中：动画省略号
                    if s.running {
                        let dots = match ((now * 2.5) as usize) % 4 {
                            0 => "",
                            1 => ".",
                            2 => "..",
                            _ => "...",
                        };
                        ui.label(
                            RichText::new(format!("正在处理{dots}"))
                                .color(Color32::from_gray(140))
                                .italics(),
                        );
                    }
                    ui.add_space(4.0);
                });

            // ---- Composer ----
            let comp_rect = egui::Rect::from_min_max(
                egui::pos2(main_rect.left(), main_rect.bottom() - composer_h),
                main_rect.max,
            );
            let p = ui.painter();
            p.line_segment(
                [comp_rect.left_top(), comp_rect.right_top()],
                Stroke::new(1.0, Color32::from_gray(50)),
            );

            // 排队中的 prompts（composer 上方浮层，可删除）
            if !s.queued.is_empty() {
                let mut remove_q: Option<usize> = None;
                let qh = 22.0 * s.queued.len() as f32 + 8.0;
                let q_rect = egui::Rect::from_min_max(
                    egui::pos2(main_rect.left() + 10.0, comp_rect.top() - qh - 4.0),
                    egui::pos2(main_rect.right() - 10.0, comp_rect.top() - 4.0),
                );
                ui.painter().rect_filled(
                    q_rect,
                    CornerRadius::same(8),
                    Color32::from_rgb(0x26, 0x29, 0x33),
                );
                for (i, q) in s.queued.iter().enumerate() {
                    let y = q_rect.top() + 4.0 + i as f32 * 22.0 + 11.0;
                    ui.painter().text(
                        egui::pos2(q_rect.left() + 10.0, y),
                        egui::Align2::LEFT_CENTER,
                        format!("⏳ {}", q.chars().take(60).collect::<String>()),
                        FontId::proportional(12.0),
                        Color32::from_gray(170),
                    );
                    let del = egui::Rect::from_center_size(
                        egui::pos2(q_rect.right() - 16.0, y),
                        egui::vec2(18.0, 18.0),
                    );
                    let dresp =
                        ui.interact(del, ui.id().with(("delq", i)), egui::Sense::click());
                    ui.painter().text(
                        del.center(),
                        egui::Align2::CENTER_CENTER,
                        "✕",
                        FontId::proportional(11.0),
                        if dresp.hovered() {
                            Color32::from_rgb(0xE5, 0x5B, 0x5B)
                        } else {
                            Color32::from_gray(110)
                        },
                    );
                    if dresp.clicked() {
                        remove_q = Some(i);
                    }
                }
                if let Some(i) = remove_q {
                    s.queued.remove(i);
                }
            }

            // 斜杠命令面板（draft 以 "/" 开头时，nextop 的 SlashCommandPalette）
            if s.draft.starts_with('/') && !s.commands.is_empty() {
                let needle = s.draft.trim_start_matches('/').to_lowercase();
                let matches: Vec<&(String, String)> = s
                    .commands
                    .iter()
                    .filter(|(n, _)| n.to_lowercase().starts_with(&needle))
                    .take(5)
                    .collect();
                if !matches.is_empty() {
                    let row_h = 26.0;
                    let ph = row_h * matches.len() as f32 + 8.0;
                    let pal = egui::Rect::from_min_max(
                        egui::pos2(main_rect.left() + 10.0, comp_rect.top() - ph - 4.0),
                        egui::pos2(main_rect.right() - 80.0, comp_rect.top() - 4.0),
                    );
                    ui.painter().rect_filled(
                        pal,
                        CornerRadius::same(8),
                        Color32::from_rgb(0x2C, 0x2C, 0x34),
                    );
                    ui.painter().rect_stroke(
                        pal,
                        CornerRadius::same(8),
                        Stroke::new(1.0, Color32::from_gray(70)),
                        StrokeKind::Inside,
                    );
                    let mut pick: Option<String> = None;
                    for (i, (name, desc)) in matches.iter().enumerate() {
                        let row = egui::Rect::from_min_size(
                            egui::pos2(pal.left() + 4.0, pal.top() + 4.0 + i as f32 * row_h),
                            egui::vec2(pal.width() - 8.0, row_h),
                        );
                        let rresp =
                            ui.interact(row, ui.id().with(("slash", i)), egui::Sense::click());
                        if rresp.hovered() {
                            ui.painter().rect_filled(
                                row,
                                CornerRadius::same(5),
                                Color32::from_rgb(0x2C, 0x62, 0xD6),
                            );
                        }
                        ui.painter().text(
                            egui::pos2(row.left() + 8.0, row.center().y),
                            egui::Align2::LEFT_CENTER,
                            format!("/{name}"),
                            FontId::monospace(12.0),
                            Color32::from_gray(225),
                        );
                        ui.painter().text(
                            egui::pos2(row.left() + 140.0, row.center().y),
                            egui::Align2::LEFT_CENTER,
                            desc.chars().take(50).collect::<String>(),
                            FontId::proportional(11.0),
                            Color32::from_gray(130),
                        );
                        if rresp.clicked() {
                            pick = Some(format!("/{name} "));
                        }
                    }
                    if let Some(p) = pick {
                        s.draft = p;
                    }
                }
            }
            let field = comp_rect.shrink2(vec2(10.0, 10.0));
            let box_rect = egui::Rect::from_min_max(
                field.min,
                egui::pos2(field.right() - 64.0, field.bottom()),
            );
            p.rect_filled(
                box_rect,
                CornerRadius::same(10),
                Color32::from_rgb(0x26, 0x26, 0x2C),
            );
            p.rect_stroke(
                box_rect,
                CornerRadius::same(10),
                Stroke::new(1.0, Color32::from_gray(62)),
                StrokeKind::Inside,
            );

            let hint = format!("向 {display_name} 提问…");
            let te = TextEdit::multiline(&mut s.draft)
                .frame(egui::Frame::NONE)
                .hint_text(RichText::new(hint).color(Color32::from_gray(110)))
                .text_color(Color32::from_gray(230))
                .font(FontId::proportional(13.5))
                .desired_rows(2)
                .desired_width(box_rect.width() - 16.0);
            let resp = ui.put(box_rect.shrink2(vec2(8.0, 6.0)), te);

            if resp.has_focus()
                && ui.input(|inp| inp.key_pressed(Key::Enter) && !inp.modifiers.shift)
            {
                s.draft = s.draft.trim_end_matches('\n').to_owned();
                s.submit(backend);
            }

            let btn_rect = egui::Rect::from_center_size(
                egui::pos2(field.right() - 26.0, field.center().y),
                vec2(40.0, 30.0),
            );
            let label = if s.running { "停止" } else { "发送" };
            let btn = ui.put(
                btn_rect,
                egui::Button::new(RichText::new(label).size(12.5))
                    .corner_radius(CornerRadius::same(8)),
            );
            if btn.clicked() {
                if s.running {
                    if let Some(rt) = &s.runtime {
                        rt.cancel();
                    }
                } else {
                    s.submit(backend);
                }
            }

            // ---- 头部下拉菜单（最后绘制盖在列表上；风格同斜杠命令面板）----
            if let (Some(menu), Some(anchor)) = (*open_menu, menu_anchor) {
                // 行数据：(id, 标题, 描述)
                let rows: Vec<(String, String, String)> = match menu {
                    "mode" => s
                        .modes
                        .iter()
                        .map(|m| (m.id.clone(), m.name.clone(), m.description.clone()))
                        .collect(),
                    "model" => s
                        .models
                        .iter()
                        .map(|m| (m.id.clone(), m.name.clone(), String::new()))
                        .collect(),
                    // 工作区：默认 + 最近目录 + 浏览（nextop 创建会话时选 cwd）
                    "ws" => {
                        let mut rows = vec![(
                            String::new(),
                            "默认工作区".to_owned(),
                            "~/MirageWorkspace".to_owned(),
                        )];
                        for p in recent_workspaces.iter() {
                            // 路径取尾部更有辨识度（共享 /Users/... 前缀）
                            let n = p.chars().count();
                            let tail = if n > 24 {
                                format!("…{}", p.chars().skip(n - 23).collect::<String>())
                            } else {
                                p.clone()
                            };
                            rows.push((p.clone(), ws_label(&Some(p.clone())), tail));
                        }
                        rows.push(("::browse".to_owned(), "浏览文件夹…".to_owned(), String::new()));
                        rows
                    }
                    _ => REASONING_EFFORTS
                        .iter()
                        .map(|(v, l)| ((*v).to_owned(), (*l).to_owned(), String::new()))
                        .collect(),
                };
                let current = match menu {
                    "mode" => s.current_mode.clone(),
                    "model" => s.model.clone(),
                    "ws" => s.workspace.clone().unwrap_or_default(),
                    _ => s.current_effort.clone(),
                };
                if rows.is_empty() {
                    *open_menu = None;
                } else {
                    let two_line = menu == "mode" || menu == "ws";
                    let row_h = if two_line { 38.0 } else { 26.0 };
                    let w: f32 = if two_line { 252.0 } else { 190.0 };
                    let h = row_h * rows.len() as f32 + 8.0;
                    let left = (anchor.right() - w).max(main_rect.left() + 6.0);
                    let pal = egui::Rect::from_min_size(
                        egui::pos2(left, header.bottom() + 4.0),
                        vec2(w, h),
                    );
                    ui.painter().rect_filled(
                        pal,
                        CornerRadius::same(8),
                        Color32::from_rgb(0x2C, 0x2C, 0x34),
                    );
                    ui.painter().rect_stroke(
                        pal,
                        CornerRadius::same(8),
                        Stroke::new(1.0, Color32::from_gray(70)),
                        StrokeKind::Inside,
                    );
                    let mut pick: Option<String> = None;
                    for (i, (id, name, desc)) in rows.iter().enumerate() {
                        let row = egui::Rect::from_min_size(
                            egui::pos2(pal.left() + 4.0, pal.top() + 4.0 + i as f32 * row_h),
                            vec2(pal.width() - 8.0, row_h),
                        );
                        let rresp = ui.interact(
                            row,
                            ui.id().with(("opt-menu", menu, i)),
                            egui::Sense::click(),
                        );
                        if rresp.hovered() {
                            ui.painter().rect_filled(
                                row,
                                CornerRadius::same(5),
                                Color32::from_rgb(0x2C, 0x62, 0xD6),
                            );
                        }
                        let is_cur = *id == current;
                        let mark = if is_cur { "✓ " } else { "    " };
                        let title_color = if rresp.hovered() {
                            Color32::WHITE
                        } else if is_cur {
                            Color32::from_gray(245)
                        } else {
                            Color32::from_gray(200)
                        };
                        if desc.is_empty() {
                            ui.painter().text(
                                egui::pos2(row.left() + 8.0, row.center().y),
                                egui::Align2::LEFT_CENTER,
                                format!("{mark}{name}"),
                                FontId::proportional(12.0),
                                title_color,
                            );
                        } else {
                            ui.painter().text(
                                egui::pos2(row.left() + 8.0, row.top() + 11.0),
                                egui::Align2::LEFT_CENTER,
                                format!("{mark}{name}"),
                                FontId::proportional(12.0),
                                title_color,
                            );
                            ui.painter().text(
                                egui::pos2(row.left() + 26.0, row.top() + 27.0),
                                egui::Align2::LEFT_CENTER,
                                desc.chars().take(24).collect::<String>(),
                                FontId::proportional(10.0),
                                if rresp.hovered() {
                                    Color32::from_gray(235)
                                } else {
                                    Color32::from_gray(130)
                                },
                            );
                        }
                        if rresp.clicked() {
                            pick = Some(id.clone());
                        }
                    }
                    if let Some(id) = pick {
                        if menu == "ws" {
                            // 工作区选择：仅未接入时有效（接入后 cwd 不可改）
                            if !s.started {
                                if id == "::browse" {
                                    *folder_pick = Some((*active, pick_folder_async()));
                                } else if id.is_empty() {
                                    s.workspace = None;
                                    s.dirty = true;
                                } else {
                                    s.workspace = Some(id.clone());
                                    push_recent(recent_workspaces, id);
                                    s.dirty = true;
                                }
                            }
                        } else if let Some(rt) = &s.runtime {
                            match menu {
                                // 乐观更新；current_mode_update / configOptions 结果会兜底纠正
                                "mode" => {
                                    rt.set_mode(&id);
                                    s.current_mode = id;
                                }
                                "model" => {
                                    rt.set_config("model", &id);
                                    s.model = id;
                                }
                                _ => {
                                    rt.set_config(effort_config_id(backend.provider), &id);
                                    s.current_effort = id;
                                }
                            }
                        }
                        *open_menu = None;
                    } else {
                        // 点击菜单与按钮之外：关闭
                        let pressed = ui.input(|inp| inp.pointer.any_pressed());
                        let pos = ui.input(|inp| inp.pointer.interact_pos());
                        if pressed && !pos.is_some_and(|p| pal.contains(p) || anchor.contains(p)) {
                            *open_menu = None;
                        }
                    }
                }
            }
        }

        // 头部"新会话"按钮：与列表 "+" 一致，新建并切换（nextop Create session）
        if want_new {
            self.sessions.push(SessionPane::new());
            self.active = self.sessions.len() - 1;
            self.open_menu = None;
            self.dirty = true;
        }
        self.save_if_dirty();
    }
}

/// 头部小按钮（沿用原权限切换按钮的样式）
fn header_btn(
    ui: &mut Ui,
    rect: egui::Rect,
    label: &str,
    id: &str,
    active: bool,
) -> egui::Response {
    let resp = ui.interact(rect, ui.id().with(id), egui::Sense::click());
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(6),
        if active || resp.hovered() {
            Color32::from_rgb(0x33, 0x33, 0x3B)
        } else {
            Color32::from_rgb(0x2B, 0x2B, 0x32)
        },
    );
    ui.painter().with_clip_rect(rect).text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        if active {
            Color32::from_gray(230)
        } else {
            Color32::from_gray(160)
        },
    );
    resp
}

fn draw_item(
    ui: &mut Ui,
    item: &mut ChatItem,
    cache: &mut CommonMarkCache,
    now: f64,
) -> ItemAction {
    let mut action = ItemAction::None;
    match item {
        // 用户消息：右对齐气泡（macOS 信息 App 发送气泡：systemBlue + 白字 + 统一大圆角）
        ChatItem::User(text) => {
            ui.with_layout(Layout::top_down(Align::Max), |ui| {
                let max_w = ui.available_width() * 0.75;
                egui::Frame::new()
                    .fill(Color32::from_rgb(0x0A, 0x84, 0xFF))
                    .corner_radius(CornerRadius::same(14))
                    .inner_margin(vec2(11.0, 7.0))
                    .show(ui, |ui| {
                        ui.set_max_width(max_w);
                        ui.label(
                            RichText::new(text.as_str()).color(Color32::WHITE).size(13.0),
                        );
                    });
            });
        }
        // 助手消息：Markdown 渲染（egui_commonmark / pulldown-cmark）。
        // 窗口是手绘深色而 egui 默认跟随系统主题（浅色时行内代码会变白底），
        // 这里强制深色 visuals，再把正文提到 macOS labelColor、链接用 linkColor
        ChatItem::Assistant(text) => {
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                ui.set_max_width(ui.available_width() * 0.95);
                let v = ui.visuals_mut();
                *v = egui::Visuals::dark();
                v.widgets.noninteractive.fg_stroke.color = Color32::from_gray(217);
                v.hyperlink_color = Color32::from_rgb(0x41, 0x9C, 0xFF);
                v.code_bg_color = Color32::from_rgb(0x2D, 0x2D, 0x32);
                CommonMarkViewer::new().show(ui, cache, text);
            });
        }
        // 思考：折叠披露
        ChatItem::Thought { text, expanded } => {
            let arrow = if *expanded { "▾" } else { "▸" };
            let head = ui.add(
                egui::Label::new(
                    RichText::new(format!("{arrow} 思考过程"))
                        .color(Color32::from_gray(130))
                        .size(12.0),
                )
                .sense(egui::Sense::click()),
            );
            if head.clicked() {
                *expanded = !*expanded;
            }
            if *expanded {
                ui.indent("thought", |ui| {
                    // 思考内容同样走 Markdown 渲染，仅整体调暗、字号缩小以示弱化
                    ui.set_max_width(ui.available_width() * 0.95);
                    let v = ui.visuals_mut();
                    *v = egui::Visuals::dark();
                    v.widgets.noninteractive.fg_stroke.color = Color32::from_gray(140);
                    v.hyperlink_color = Color32::from_rgb(0x41, 0x9C, 0xFF);
                    v.code_bg_color = Color32::from_rgb(0x2D, 0x2D, 0x32);
                    for style in ui.style_mut().text_styles.values_mut() {
                        style.size *= 12.0 / 14.0;
                    }
                    CommonMarkViewer::new().show(ui, cache, text);
                });
            }
        }
        // 工具调用卡片：状态点 + 标题 + diff 统计 + 展开（输出/diff/文件位置）
        ChatItem::Tool {
            title,
            kind,
            status,
            text,
            diff,
            locations,
            expanded,
            ..
        } => {
            // 状态全集（nextop statusKind；配色用 macOS 深色系统色 green/red/yellow）
            let (dot, dot_color, working) = match status.as_str() {
                "completed" => ("●", Color32::from_rgb(0x30, 0xD1, 0x58), false),
                "failed" => ("●", Color32::from_rgb(0xFF, 0x45, 0x3A), false),
                "canceled" | "cancelled" => ("◌", Color32::from_gray(110), false),
                "waiting" | "pending" => ("○", Color32::from_rgb(0xFF, 0xD6, 0x0A), false),
                _ => ("●", Color32::from_rgb(0xFF, 0xD6, 0x0A), true),
            };
            egui::Frame::new()
                .fill(Color32::from_rgb(0x23, 0x23, 0x29))
                .corner_radius(CornerRadius::same(8))
                .inner_margin(vec2(9.0, 6.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width() * 0.95);
                    let arrow = if *expanded { "▾" } else { "▸" };
                    // diff 统计 +N/-N
                    let stats = diff.as_ref().map(|d| {
                        let plus = d.new_text.lines().count();
                        let minus = d.old_text.lines().count();
                        (plus, minus)
                    });
                    let head = ui
                        .scope(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(dot).color(dot_color).size(12.5));
                                ui.label(
                                    RichText::new(format!("{arrow} {title}"))
                                        .color(Color32::from_gray(200))
                                        .size(12.5),
                                );
                                if !kind.is_empty() {
                                    ui.label(
                                        RichText::new(kind.as_str())
                                            .color(Color32::from_gray(110))
                                            .size(11.0),
                                    );
                                }
                                // 运行中：三个动画点（nextop 的 working 动画）
                                if working {
                                    let dots = match ((now * 3.0) as usize) % 4 {
                                        0 => "·",
                                        1 => "··",
                                        2 => "···",
                                        _ => "",
                                    };
                                    ui.label(
                                        RichText::new(dots)
                                            .color(Color32::from_rgb(0xFF, 0xD6, 0x0A))
                                            .size(13.0),
                                    );
                                }
                                if let Some((plus, minus)) = stats {
                                    ui.label(
                                        RichText::new(format!("+{plus}"))
                                            .color(Color32::from_rgb(0x30, 0xD1, 0x58))
                                            .size(11.0),
                                    );
                                    ui.label(
                                        RichText::new(format!("-{minus}"))
                                            .color(Color32::from_rgb(0xFF, 0x45, 0x3A))
                                            .size(11.0),
                                    );
                                }
                            });
                        })
                        .response
                        .interact(egui::Sense::click());
                    if head.clicked() {
                        *expanded = !*expanded;
                    }

                    if *expanded {
                        // 文件位置
                        for loc in locations.iter() {
                            ui.label(
                                RichText::new(format!("📄 {loc}"))
                                    .color(Color32::from_gray(150))
                                    .size(11.5),
                            );
                        }
                        // diff 红绿块
                        if let Some(d) = diff {
                            draw_diff(ui, d);
                        }
                        // 文本输出
                        if !text.is_empty() {
                            ui.add_space(4.0);
                            egui::Frame::new()
                                .fill(Color32::from_rgb(0x15, 0x15, 0x18))
                                .corner_radius(CornerRadius::same(6))
                                .inner_margin(6.0)
                                .show(ui, |ui| {
                                    let shown: String = text.chars().take(2000).collect();
                                    ui.label(
                                        RichText::new(shown)
                                            .color(Color32::from_gray(180))
                                            .font(FontId::monospace(11.5)),
                                    );
                                });
                        }
                    }
                });
        }
        // 权限审批卡片：允许/拒绝按钮（nextop 的 AgentApprovalCallCard）
        ChatItem::Approval {
            request_id,
            title,
            options,
            decided,
        } => {
            egui::Frame::new()
                .fill(Color32::from_rgb(0x2C, 0x26, 0x1C))
                .corner_radius(CornerRadius::same(8))
                .stroke(Stroke::new(1.0, Color32::from_rgb(0x8A, 0x6A, 0x2A)))
                .inner_margin(vec2(10.0, 8.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width() * 0.95);
                    ui.label(
                        RichText::new(format!("⚠ 请求权限：{title}"))
                            .color(Color32::from_rgb(0xFF, 0xD6, 0x0A))
                            .size(12.5),
                    );
                    match decided {
                        Some(name) => {
                            ui.label(
                                RichText::new(format!("已选择：{name}"))
                                    .color(Color32::from_gray(150))
                                    .size(12.0),
                            );
                        }
                        None => {
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                for opt in options.iter() {
                                    let allow = opt.kind.starts_with("allow");
                                    let btn = egui::Button::new(
                                        RichText::new(&opt.name).size(12.0).color(
                                            if allow {
                                                Color32::WHITE
                                            } else {
                                                Color32::from_gray(200)
                                            },
                                        ),
                                    )
                                    // macOS 深色推按钮：主操作 systemBlue，次操作中性灰，圆角 6
                                    .fill(if allow {
                                        Color32::from_rgb(0x0A, 0x84, 0xFF)
                                    } else {
                                        Color32::from_rgb(0x3A, 0x3A, 0x3C)
                                    })
                                    .corner_radius(CornerRadius::same(6));
                                    if ui.add(btn).clicked() {
                                        action = ItemAction::Permission {
                                            request_id: *request_id,
                                            option_id: opt.id.clone(),
                                            name: opt.name.clone(),
                                        };
                                    }
                                }
                            });
                        }
                    }
                });
        }
        // 回合级提示行（取消/上限/拒绝）：仿 macOS 信息 App 的居中系统小字
        ChatItem::Notice(text) => {
            ui.with_layout(Layout::top_down(Align::Center), |ui| {
                ui.label(
                    RichText::new(text.as_str())
                        .color(Color32::from_gray(120))
                        .size(11.0),
                );
            });
        }
        // 回合文件变更汇总卡（nextop AgentTurnSummaryRow：前 3 个 + 折叠展开）
        ChatItem::TurnSummary { files, expanded } => {
            let total_add: usize = files.iter().map(|(_, a, _)| a).sum();
            let total_del: usize = files.iter().map(|(_, _, d)| d).sum();
            egui::Frame::new()
                .fill(Color32::from_rgb(0x20, 0x26, 0x21))
                .corner_radius(CornerRadius::same(8))
                .stroke(Stroke::new(1.0, Color32::from_rgb(0x35, 0x4A, 0x38)))
                .inner_margin(vec2(10.0, 8.0))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width() * 0.95);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("📝 {} 个文件已更改", files.len()))
                                .color(Color32::from_gray(215))
                                .size(12.5)
                                .strong(),
                        );
                        ui.label(
                            RichText::new(format!("+{total_add}"))
                                .color(Color32::from_rgb(0x30, 0xD1, 0x58))
                                .size(11.5),
                        );
                        ui.label(
                            RichText::new(format!("-{total_del}"))
                                .color(Color32::from_rgb(0xFF, 0x45, 0x3A))
                                .size(11.5),
                        );
                    });
                    let show_n = if *expanded { files.len() } else { files.len().min(3) };
                    for (path, add, del) in files.iter().take(show_n) {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(path.as_str())
                                    .color(Color32::from_gray(175))
                                    .font(FontId::monospace(11.5)),
                            );
                            ui.label(
                                RichText::new(format!("+{add}"))
                                    .color(Color32::from_rgb(0x30, 0xD1, 0x58))
                                    .size(11.0),
                            );
                            ui.label(
                                RichText::new(format!("-{del}"))
                                    .color(Color32::from_rgb(0xFF, 0x45, 0x3A))
                                    .size(11.0),
                            );
                        });
                    }
                    if files.len() > 3 {
                        let label = if *expanded {
                            "收起".to_owned()
                        } else {
                            format!("展开其余 {} 个…", files.len() - 3)
                        };
                        // macOS linkColor（深色 #419CFF）
                        let resp = ui.add(
                            egui::Label::new(
                                RichText::new(label)
                                    .color(Color32::from_rgb(0x41, 0x9C, 0xFF))
                                    .size(11.5),
                            )
                            .sense(egui::Sense::click()),
                        );
                        if resp.clicked() {
                            *expanded = !*expanded;
                        }
                    }
                });
        }
    }
    action
}

/// Plan 计划清单（nextop 的 plan entries：pending/in_progress/completed）
fn draw_plan(ui: &mut Ui, plan: &[PlanEntry]) {
    egui::Frame::new()
        .fill(Color32::from_rgb(0x21, 0x24, 0x2C))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(vec2(10.0, 8.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width() * 0.95);
            ui.label(
                RichText::new("计划")
                    .color(Color32::from_rgb(0x41, 0x9C, 0xFF))
                    .size(12.0)
                    .strong(),
            );
            ui.add_space(2.0);
            for e in plan {
                let (icon, color) = match e.status.as_str() {
                    "completed" => ("✓", Color32::from_rgb(0x30, 0xD1, 0x58)),
                    "in_progress" => ("◐", Color32::from_rgb(0xFF, 0xD6, 0x0A)),
                    _ => ("○", Color32::from_gray(110)),
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon).color(color).size(12.5));
                    let text_color = if e.status == "completed" {
                        Color32::from_gray(120)
                    } else {
                        Color32::from_gray(200)
                    };
                    let mut rt = RichText::new(&e.content).color(text_color).size(12.5);
                    if e.status == "completed" {
                        rt = rt.strikethrough();
                    }
                    ui.label(rt);
                });
            }
        });
}

/// diff 红绿块（简化 unified：删除行红底、新增行绿底，各截断显示）
fn draw_diff(ui: &mut Ui, d: &ToolDiff) {
    ui.add_space(4.0);
    if !d.path.is_empty() {
        ui.label(
            RichText::new(&d.path)
                .color(Color32::from_gray(150))
                .size(11.0)
                .font(FontId::monospace(11.0)),
        );
    }
    egui::Frame::new()
        .fill(Color32::from_rgb(0x15, 0x15, 0x18))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(6.0)
        .show(ui, |ui| {
            let mono = FontId::monospace(11.0);
            for line in d.old_text.lines().take(12) {
                ui.label(
                    RichText::new(format!("- {line}"))
                        .color(Color32::from_rgb(0xE8, 0x9A, 0x9A))
                        .background_color(Color32::from_rgb(0x33, 0x1A, 0x1C))
                        .font(mono.clone()),
                );
            }
            if d.old_text.lines().count() > 12 {
                ui.label(RichText::new("  …").color(Color32::from_gray(100)));
            }
            for line in d.new_text.lines().take(24) {
                ui.label(
                    RichText::new(format!("+ {line}"))
                        .color(Color32::from_rgb(0x9A, 0xE0, 0xA8))
                        .background_color(Color32::from_rgb(0x16, 0x2A, 0x1A))
                        .font(mono.clone()),
                );
            }
            if d.new_text.lines().count() > 24 {
                ui.label(RichText::new("  …").color(Color32::from_gray(100)));
            }
        });
}
