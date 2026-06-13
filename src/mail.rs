//! 邮件核心逻辑：IMAP 收信 + SMTP 发信。
//!
//! 零 egui 依赖（对齐 wm.rs 原则）：所有网络操作在一个后台 worker 线程里
//! 同步阻塞执行，UI 通过 mpsc 通道下发命令 / 收取事件。
//! 账户配置持久化到 ~/MirageWorkspace/mail_account.json。

use std::net::TcpStream;
use std::sync::mpsc::{channel, Receiver, Sender};

use serde::{Deserialize, Serialize};

// ---------- 账户 ----------

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Account {
    pub display_name: String,
    pub email: String,
    pub password: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: String,
    /// 587 走 STARTTLS，465 走隐式 TLS
    pub smtp_port: u16,
}

pub fn account_path() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    format!("{home}/MirageWorkspace/mail_account.json")
}

pub fn load_account() -> Option<Account> {
    let bytes = std::fs::read(account_path()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn save_account(acc: &Account) {
    let path = account_path();
    if let Some(dir) = std::path::Path::new(&path).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_vec_pretty(acc) {
        let _ = std::fs::write(path, json);
    }
}

/// 常见邮箱服务商的服务器预设：按邮箱域名自动填充
pub fn provider_preset(email: &str) -> Option<(&'static str, &'static str, u16)> {
    let domain = email.rsplit('@').next()?.to_ascii_lowercase();
    let (imap, smtp, smtp_port) = match domain.as_str() {
        "qq.com" | "vip.qq.com" => ("imap.qq.com", "smtp.qq.com", 465),
        "163.com" => ("imap.163.com", "smtp.163.com", 465),
        "126.com" => ("imap.126.com", "smtp.126.com", 465),
        "gmail.com" => ("imap.gmail.com", "smtp.gmail.com", 587),
        "outlook.com" | "hotmail.com" | "live.com" => {
            ("outlook.office365.com", "smtp-mail.outlook.com", 587)
        }
        "icloud.com" | "me.com" => ("imap.mail.me.com", "smtp.mail.me.com", 587),
        "ethereal.email" => ("imap.ethereal.email", "smtp.ethereal.email", 587),
        _ => return None,
    };
    Some((imap, smtp, smtp_port))
}

// ---------- 消息模型 ----------

#[derive(Clone)]
pub struct Summary {
    pub uid: u32,
    pub from: String,
    pub subject: String,
    /// 已格式化的本地时间（列表展示用）
    pub date: String,
    pub timestamp: i64,
    pub seen: bool,
}

// ---------- UI <-> worker 协议 ----------

pub enum Cmd {
    Connect(Account),
    Refresh,
    FetchBody(u32),
    Send {
        to: String,
        subject: String,
        body: String,
    },
}

pub enum Evt {
    Status(String),
    Connected,
    List(Vec<Summary>),
    Body { uid: u32, text: String },
    Sent { to: String, subject: String },
    Error(String),
}

pub struct MailWorker {
    pub tx: Sender<Cmd>,
    pub rx: Receiver<Evt>,
}

pub fn spawn() -> MailWorker {
    let (tx_cmd, rx_cmd) = channel::<Cmd>();
    let (tx_evt, rx_evt) = channel::<Evt>();
    std::thread::Builder::new()
        .name("mail-worker".into())
        .spawn(move || run(rx_cmd, tx_evt))
        .expect("spawn mail worker");
    MailWorker { tx: tx_cmd, rx: rx_evt }
}

type ImapSession = imap::Session<native_tls::TlsStream<TcpStream>>;

fn run(rx: Receiver<Cmd>, tx: Sender<Evt>) {
    let mut session: Option<ImapSession> = None;
    let mut account: Option<Account> = None;
    // 通道断开（UI 侧整体退出）时线程结束
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Connect(acc) => {
                let _ = tx.send(Evt::Status(format!("正在连接 {} …", acc.imap_host)));
                match imap_connect(&acc) {
                    Ok(s) => {
                        session = Some(s);
                        account = Some(acc);
                        let _ = tx.send(Evt::Connected);
                        refresh(&mut session, &tx);
                    }
                    Err(e) => {
                        session = None;
                        let _ = tx.send(Evt::Error(format!("连接失败：{e}")));
                    }
                }
            }
            Cmd::Refresh => refresh(&mut session, &tx),
            Cmd::FetchBody(uid) => match &mut session {
                Some(s) => match fetch_body(s, uid) {
                    Ok(text) => {
                        let _ = tx.send(Evt::Body { uid, text });
                    }
                    Err(e) => {
                        let _ = tx.send(Evt::Error(format!("读取正文失败：{e}")));
                    }
                },
                None => {
                    let _ = tx.send(Evt::Error("未连接".into()));
                }
            },
            Cmd::Send { to, subject, body } => match &account {
                Some(acc) => {
                    let _ = tx.send(Evt::Status(format!("正在发送给 {to} …")));
                    match smtp_send(acc, &to, &subject, &body) {
                        Ok(()) => {
                            let _ = tx.send(Evt::Sent {
                                to: to.clone(),
                                subject: subject.clone(),
                            });
                            // Ethereal 等测试服务会把发出的信捕获进 INBOX，顺手刷新
                            refresh(&mut session, &tx);
                        }
                        Err(e) => {
                            let _ = tx.send(Evt::Error(format!("发送失败：{e}")));
                        }
                    }
                }
                None => {
                    let _ = tx.send(Evt::Error("未配置账户".into()));
                }
            },
        }
    }
}

// ---------- IMAP ----------

fn imap_connect(acc: &Account) -> Result<ImapSession, String> {
    let tls = native_tls::TlsConnector::builder()
        .build()
        .map_err(|e| e.to_string())?;
    let client = imap::connect(
        (acc.imap_host.as_str(), acc.imap_port),
        acc.imap_host.as_str(),
        &tls,
    )
    .map_err(|e| e.to_string())?;
    client
        .login(&acc.email, &acc.password)
        .map_err(|(e, _)| e.to_string())
}

fn refresh(session: &mut Option<ImapSession>, tx: &Sender<Evt>) {
    let Some(s) = session else {
        let _ = tx.send(Evt::Error("未连接".into()));
        return;
    };
    let _ = tx.send(Evt::Status("正在收取邮件…".into()));
    match fetch_list(s) {
        Ok(list) => {
            let _ = tx.send(Evt::List(list));
            let _ = tx.send(Evt::Status(String::new()));
        }
        Err(e) => {
            // 连接可能已超时断开：标记掉线，下次 Connect 重建
            *session = None;
            let _ = tx.send(Evt::Error(format!("收取失败：{e}")));
        }
    }
}

/// 取 INBOX 最近 50 封的头部，解析出列表摘要（新的在前）
fn fetch_list(s: &mut ImapSession) -> Result<Vec<Summary>, String> {
    let mb = s.select("INBOX").map_err(|e| e.to_string())?;
    let total = mb.exists;
    if total == 0 {
        return Ok(Vec::new());
    }
    let from = total.saturating_sub(49).max(1);
    let fetches = s
        .fetch(format!("{from}:{total}"), "(UID FLAGS RFC822.HEADER)")
        .map_err(|e| e.to_string())?;
    let mut out: Vec<Summary> = Vec::new();
    for f in fetches.iter() {
        let Some(uid) = f.uid else { continue };
        let Some(header) = f.header() else { continue };
        let (headers, _) = mailparse::parse_headers(header).map_err(|e| e.to_string())?;
        use mailparse::MailHeaderMap;
        let subject = headers
            .get_first_value("Subject")
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "（无主题）".into());
        let from_hdr = headers.get_first_value("From").unwrap_or_default();
        let timestamp = headers
            .get_first_value("Date")
            .and_then(|d| mailparse::dateparse(&d).ok())
            .unwrap_or(0);
        let seen = f.flags().iter().any(|fl| matches!(fl, imap::types::Flag::Seen));
        out.push(Summary {
            uid,
            from: friendly_from(&from_hdr),
            subject,
            date: format_date(timestamp),
            timestamp,
            seen,
        });
    }
    out.sort_by_key(|m| -m.timestamp);
    Ok(out)
}

fn fetch_body(s: &mut ImapSession, uid: u32) -> Result<String, String> {
    let fetches = s
        .uid_fetch(uid.to_string(), "(BODY[])")
        .map_err(|e| e.to_string())?;
    let f = fetches.iter().next().ok_or("邮件不存在")?;
    let raw = f.body().ok_or("空正文")?;
    let parsed = mailparse::parse_mail(raw).map_err(|e| e.to_string())?;
    Ok(extract_text(&parsed))
}

/// 深度优先找 text/plain；没有就拿 text/html 去标签
fn extract_text(mail: &mailparse::ParsedMail) -> String {
    fn find<'a>(m: &'a mailparse::ParsedMail<'a>, want: &str) -> Option<String> {
        if m.ctype.mimetype.eq_ignore_ascii_case(want) {
            return m.get_body().ok();
        }
        m.subparts.iter().find_map(|p| find(p, want))
    }
    if let Some(t) = find(mail, "text/plain") {
        return t;
    }
    if let Some(h) = find(mail, "text/html") {
        return strip_html(&h);
    }
    mail.get_body().unwrap_or_else(|_| "（无法解析正文）".into())
}

fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut tag = String::new();
    for c in html.chars() {
        match (in_tag, c) {
            (false, '<') => {
                in_tag = true;
                tag.clear();
            }
            (true, '>') => {
                in_tag = false;
                let t = tag.trim_start_matches('/').to_ascii_lowercase();
                if t.starts_with("br") || t.starts_with("p") || t.starts_with("div")
                    || t.starts_with("tr") || t.starts_with("li")
                {
                    out.push('\n');
                }
            }
            (true, c) => tag.push(c),
            (false, c) => out.push(c),
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    // 压掉连续空行
    let mut lines: Vec<&str> = Vec::new();
    let mut blank = 0;
    for l in out.lines() {
        if l.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        lines.push(l.trim_end());
    }
    lines.join("\n").trim().to_owned()
}

/// "张三 <a@b.com>" -> "张三"；纯地址原样返回
fn friendly_from(from: &str) -> String {
    let f = from.trim();
    if let Some(i) = f.find('<') {
        let name = f[..i].trim().trim_matches('"').trim();
        if !name.is_empty() {
            return name.to_owned();
        }
        return f[i + 1..].trim_end_matches('>').to_owned();
    }
    if f.is_empty() {
        "（未知发件人）".into()
    } else {
        f.to_owned()
    }
}

fn format_date(ts: i64) -> String {
    use chrono::{DateTime, Datelike, Local};
    if ts <= 0 {
        return String::new();
    }
    let Some(dt) = DateTime::from_timestamp(ts, 0) else {
        return String::new();
    };
    let local = dt.with_timezone(&Local);
    let now = Local::now();
    if local.date_naive() == now.date_naive() {
        local.format("%H:%M").to_string()
    } else if local.year() == now.year() {
        local.format("%m月%d日").to_string()
    } else {
        local.format("%Y/%m/%d").to_string()
    }
}

// ---------- SMTP ----------

fn smtp_send(acc: &Account, to: &str, subject: &str, body: &str) -> Result<(), String> {
    use lettre::message::header::ContentType;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{Message, SmtpTransport, Transport};

    let from_mbox = if acc.display_name.trim().is_empty() {
        acc.email.parse()
    } else {
        format!("{} <{}>", acc.display_name.trim(), acc.email).parse()
    }
    .map_err(|e| format!("发件人地址无效：{e}"))?;
    let to_mbox = to
        .trim()
        .parse()
        .map_err(|e| format!("收件人地址无效：{e}"))?;
    let msg = Message::builder()
        .from(from_mbox)
        .to(to_mbox)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body.to_owned())
        .map_err(|e| e.to_string())?;

    let creds = Credentials::new(acc.email.clone(), acc.password.clone());
    let builder = if acc.smtp_port == 465 {
        SmtpTransport::relay(&acc.smtp_host)
    } else {
        SmtpTransport::starttls_relay(&acc.smtp_host)
    }
    .map_err(|e| e.to_string())?
    .port(acc.smtp_port);
    let mailer = builder.credentials(creds).build();
    mailer.send(&msg).map_err(|e| e.to_string())?;
    Ok(())
}

// ---------- 端到端回归 ----------

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实收发回归：SMTP 发一封带唯一标记的信给自己，再用 IMAP 轮询收到并核对正文。
    /// 需要环境变量（推荐 Ethereal 测试账号，发出的信会被捕获进 INBOX）：
    ///   MAILTEST_EMAIL / MAILTEST_PASS
    ///   [MAILTEST_IMAP_HOST] [MAILTEST_SMTP_HOST] [MAILTEST_SMTP_PORT]
    /// 运行：cargo test mail_roundtrip -- --ignored --nocapture
    #[test]
    #[ignore]
    fn mail_roundtrip() {
        let email = std::env::var("MAILTEST_EMAIL").expect("MAILTEST_EMAIL");
        let pass = std::env::var("MAILTEST_PASS").expect("MAILTEST_PASS");
        let preset = provider_preset(&email);
        let imap_host = std::env::var("MAILTEST_IMAP_HOST")
            .unwrap_or_else(|_| preset.expect("需要 MAILTEST_IMAP_HOST").0.into());
        let smtp_host = std::env::var("MAILTEST_SMTP_HOST")
            .unwrap_or_else(|_| preset.expect("需要 MAILTEST_SMTP_HOST").1.into());
        let smtp_port: u16 = std::env::var("MAILTEST_SMTP_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .or(preset.map(|p| p.2))
            .unwrap_or(587);
        let acc = Account {
            display_name: "Mirage 自检".into(),
            email: email.clone(),
            password: pass,
            imap_host,
            imap_port: 993,
            smtp_host,
            smtp_port,
        };

        let marker = format!(
            "mirage-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        );
        let subject = format!("Mirage 邮件自检 {marker}");
        let body = format!("这是 mirage 邮件应用的端到端自检。\n标记：{marker}\n");

        smtp_send(&acc, &email, &subject, &body).expect("SMTP 发送失败");
        println!("已发送，开始 IMAP 轮询…");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
        loop {
            let mut s = imap_connect(&acc).expect("IMAP 连接失败");
            let list = fetch_list(&mut s).expect("拉取列表失败");
            if let Some(m) = list.iter().find(|m| m.subject.contains(&marker)) {
                let text = fetch_body(&mut s, m.uid).expect("拉取正文失败");
                assert!(text.contains(&marker), "正文不含标记：{text}");
                println!("✓ 回环成功：uid={} subject={}", m.uid, m.subject);
                let _ = s.logout();
                return;
            }
            let _ = s.logout();
            assert!(
                std::time::Instant::now() < deadline,
                "90 秒内未收到自检邮件（已见 {} 封）",
                list.len()
            );
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    }
}
