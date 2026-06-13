//! 邮件 App：对齐 macOS Mail 的三栏布局（侧栏 / 列表 / 阅读窗格）。
//! 真实 IMAP/SMTP（src/mail.rs 后台线程），首次使用进入账户设置页，
//! 支持服务商预设自动填充与一键创建 Ethereal 测试账号。

use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, Mutex};

use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Key, Layout, Rect, RichText,
    ScrollArea, Sense, Stroke, StrokeKind, TextEdit, Ui,
};

use crate::mail::{self, Account, Cmd, Evt, MailWorker, Summary};

const BG: Color32 = Color32::from_rgb(0x1E, 0x1E, 0x23);
const SIDEBAR_BG: Color32 = Color32::from_rgb(0x26, 0x26, 0x2C);
const LIST_BG: Color32 = Color32::from_rgb(0x21, 0x21, 0x26);
const SEP: Color32 = Color32::from_rgb(0x38, 0x38, 0x3E);
const ACCENT: Color32 = Color32::from_rgb(0x1A, 0x7C, 0xF0);
const TEXT: Color32 = Color32::from_gray(228);
const DIM: Color32 = Color32::from_gray(140);
const FAINT: Color32 = Color32::from_gray(95);

#[derive(PartialEq, Clone, Copy)]
enum Mailbox {
    Inbox,
    Sent,
}

struct SentMail {
    to: String,
    subject: String,
    body: String,
    date: String,
}

struct Compose {
    to: String,
    subject: String,
    body: String,
    sending: bool,
}

pub struct MailApp {
    worker: Option<MailWorker>,
    account: Option<Account>,
    /// 设置表单（account 为 None 或主动进设置时展示）
    form: Account,
    show_setup: bool,
    /// Ethereal 一键创建：后台 curl 的结果槽
    ethereal: Arc<Mutex<Option<Result<Account, String>>>>,
    ethereal_busy: bool,

    connected: bool,
    busy: bool,
    status: String,
    error: String,

    mailbox: Mailbox,
    list: Vec<Summary>,
    selected: Option<u32>,
    /// 当前阅读的正文（uid, 文本）；None = 加载中或未选
    body: Option<(u32, String)>,
    sent: Vec<SentMail>,
    selected_sent: Option<usize>,
    compose: Option<Compose>,
    loaded: bool,
}

impl Default for MailApp {
    fn default() -> Self {
        Self {
            worker: None,
            account: None,
            form: Account {
                imap_port: 993,
                smtp_port: 587,
                ..Default::default()
            },
            show_setup: false,
            ethereal: Arc::new(Mutex::new(None)),
            ethereal_busy: false,
            connected: false,
            busy: false,
            status: String::new(),
            error: String::new(),
            mailbox: Mailbox::Inbox,
            list: Vec::new(),
            selected: None,
            body: None,
            sent: Vec::new(),
            selected_sent: None,
            compose: None,
            loaded: false,
        }
    }
}

impl MailApp {
    /// 有网络往来时让主循环保持重绘
    pub fn animating(&self) -> bool {
        self.busy || self.ethereal_busy
    }

    fn worker(&mut self) -> &MailWorker {
        self.worker.get_or_insert_with(mail::spawn)
    }

    fn connect(&mut self, acc: Account) {
        self.account = Some(acc.clone());
        self.busy = true;
        self.error.clear();
        let _ = self.worker().tx.send(Cmd::Connect(acc));
    }

    fn pump(&mut self) {
        // Ethereal 创建结果
        if self.ethereal_busy {
            let done = self.ethereal.lock().unwrap().take();
            if let Some(res) = done {
                self.ethereal_busy = false;
                match res {
                    Ok(acc) => {
                        self.form = acc.clone();
                        mail::save_account(&acc);
                        self.show_setup = false;
                        self.connect(acc);
                    }
                    Err(e) => self.error = format!("创建测试账号失败：{e}"),
                }
            }
        }
        let Some(w) = &self.worker else { return };
        loop {
            match w.rx.try_recv() {
                Ok(evt) => match evt {
                    Evt::Status(s) => {
                        self.busy = !s.is_empty();
                        self.status = s;
                    }
                    Evt::Connected => {
                        self.connected = true;
                        self.error.clear();
                    }
                    Evt::List(list) => {
                        self.busy = false;
                        // 保持选中
                        if let Some(uid) = self.selected {
                            if !list.iter().any(|m| m.uid == uid) {
                                self.selected = None;
                                self.body = None;
                            }
                        }
                        self.list = list;
                    }
                    Evt::Body { uid, text } => {
                        self.busy = false;
                        if self.selected == Some(uid) {
                            self.body = Some((uid, text));
                        }
                    }
                    Evt::Sent { to, subject } => {
                        self.busy = false;
                        self.status = format!("已发送给 {to}");
                        let body = self
                            .compose
                            .as_ref()
                            .map(|c| c.body.clone())
                            .unwrap_or_default();
                        self.sent.push(SentMail {
                            to,
                            subject,
                            body,
                            date: chrono::Local::now().format("%H:%M").to_string(),
                        });
                        self.compose = None;
                    }
                    Evt::Error(e) => {
                        self.busy = false;
                        self.connected = false;
                        self.error = e;
                        if let Some(c) = &mut self.compose {
                            c.sending = false;
                        }
                    }
                },
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
    }

    pub fn show(&mut self, ui: &mut Ui) {
        if !self.loaded {
            self.loaded = true;
            match mail::load_account() {
                Some(acc) => {
                    self.form = acc.clone();
                    self.connect(acc);
                }
                None => self.show_setup = true,
            }
        }
        self.pump();

        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0, BG);

        if self.show_setup || self.account.is_none() {
            self.draw_setup(ui, full);
            return;
        }
        self.draw_main(ui, full);
        if self.compose.is_some() {
            self.draw_compose(ui, full);
        }
    }

    // ---------- 账户设置页 ----------

    fn draw_setup(&mut self, ui: &mut Ui, full: Rect) {
        let p = ui.painter().clone();
        p.text(
            pos2(full.center().x, full.top() + 64.0),
            Align2::CENTER_CENTER,
            "欢迎使用邮件",
            FontId::proportional(24.0),
            TEXT,
        );
        p.text(
            pos2(full.center().x, full.top() + 94.0),
            Align2::CENTER_CENTER,
            "添加一个 IMAP/SMTP 邮件账户开始使用",
            FontId::proportional(13.0),
            DIM,
        );

        let card_w = 420.0_f32.min(full.width() - 48.0);
        let card = Rect::from_center_size(
            pos2(full.center().x, full.top() + 130.0 + 170.0),
            vec2(card_w, 340.0),
        );
        p.rect_filled(card, 10, SIDEBAR_BG);
        p.rect_stroke(card, 10, Stroke::new(1.0, SEP), StrokeKind::Inside);

        let inner = card.shrink(18.0);
        let mut cui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::top_down(Align::Min)),
        );
        cui.set_clip_rect(inner.intersect(ui.clip_rect()));
        cui.spacing_mut().item_spacing.y = 8.0;

        let field = |cui: &mut Ui, label: &str, value: &mut String, password: bool| {
            cui.horizontal(|cui| {
                cui.add_sized(
                    vec2(86.0, 24.0),
                    egui::Label::new(RichText::new(label).color(DIM).size(12.5)),
                );
                cui.add_sized(
                    vec2(cui.available_width(), 24.0),
                    TextEdit::singleline(value)
                        .password(password)
                        .text_color(TEXT)
                        .font(FontId::proportional(13.0)),
                );
            });
        };

        field(&mut cui, "显示名", &mut self.form.display_name, false);
        let email_before = self.form.email.clone();
        field(&mut cui, "邮箱地址", &mut self.form.email, false);
        // 输完地址自动按域名填服务器预设
        if self.form.email != email_before {
            if let Some((imap, smtp, smtp_port)) = mail::provider_preset(&self.form.email) {
                if self.form.imap_host.is_empty() || self.form.smtp_host.is_empty() {
                    self.form.imap_host = imap.into();
                    self.form.smtp_host = smtp.into();
                    self.form.smtp_port = smtp_port;
                    self.form.imap_port = 993;
                }
            }
        }
        field(&mut cui, "密码/授权码", &mut self.form.password, true);
        field(&mut cui, "IMAP 服务器", &mut self.form.imap_host, false);
        field(&mut cui, "SMTP 服务器", &mut self.form.smtp_host, false);
        cui.horizontal(|cui| {
            cui.add_sized(
                vec2(86.0, 24.0),
                egui::Label::new(RichText::new("SMTP 端口").color(DIM).size(12.5)),
            );
            let mut port = self.form.smtp_port.to_string();
            cui.add_sized(
                vec2(72.0, 24.0),
                TextEdit::singleline(&mut port).text_color(TEXT),
            );
            if let Ok(v) = port.trim().parse() {
                self.form.smtp_port = v;
            }
            cui.label(RichText::new("587=STARTTLS / 465=TLS").color(FAINT).size(11.0));
        });

        cui.add_space(6.0);
        cui.horizontal(|cui| {
            let ready = !self.form.email.trim().is_empty()
                && !self.form.password.is_empty()
                && !self.form.imap_host.trim().is_empty()
                && !self.form.smtp_host.trim().is_empty();
            let login = egui::Button::new(RichText::new("登录").color(Color32::WHITE).size(13.0))
                .fill(ACCENT)
                .corner_radius(CornerRadius::same(6))
                .min_size(vec2(96.0, 28.0));
            if cui.add_enabled(ready && !self.busy, login).clicked() {
                let mut acc = self.form.clone();
                acc.email = acc.email.trim().to_owned();
                acc.imap_host = acc.imap_host.trim().to_owned();
                acc.smtp_host = acc.smtp_host.trim().to_owned();
                acc.imap_port = 993;
                mail::save_account(&acc);
                self.form = acc.clone();
                self.show_setup = false;
                self.connect(acc);
            }
            let eth = egui::Button::new(
                RichText::new(if self.ethereal_busy {
                    "正在创建…"
                } else {
                    "创建 Ethereal 测试账号"
                })
                .color(TEXT)
                .size(12.5),
            )
            .corner_radius(CornerRadius::same(6))
            .min_size(vec2(170.0, 28.0));
            if cui.add_enabled(!self.ethereal_busy, eth).clicked() {
                self.spawn_ethereal();
            }
        });
        if !self.error.is_empty() {
            cui.add_space(4.0);
            cui.label(
                RichText::new(&self.error)
                    .color(Color32::from_rgb(0xE5, 0x5B, 0x5B))
                    .size(12.0),
            );
        }
        if self.busy {
            cui.add_space(4.0);
            cui.label(RichText::new(&self.status).color(DIM).size(12.0));
        }
    }

    /// Ethereal（nodemailer 官方测试邮箱）一键开号：发出的信会被捕获进自己的 INBOX，
    /// 无真实凭据也能完整体验收发。用 curl 避免引入 HTTP 客户端依赖。
    fn spawn_ethereal(&mut self) {
        self.ethereal_busy = true;
        self.error.clear();
        let slot = self.ethereal.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<Account, String> {
                let out = std::process::Command::new("curl")
                    .args([
                        "-s",
                        "--max-time",
                        "20",
                        "-X",
                        "POST",
                        "https://api.nodemailer.com/user",
                        "-H",
                        "Content-Type: application/json",
                        "-d",
                        r#"{"requestor":"mirage","version":"0.1.0"}"#,
                    ])
                    .output()
                    .map_err(|e| e.to_string())?;
                let v: serde_json::Value =
                    serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
                if v["status"] != "success" {
                    return Err(v["error"].as_str().unwrap_or("接口返回失败").to_owned());
                }
                Ok(Account {
                    display_name: "Mirage".into(),
                    email: v["user"].as_str().unwrap_or_default().to_owned(),
                    password: v["pass"].as_str().unwrap_or_default().to_owned(),
                    imap_host: v["imap"]["host"].as_str().unwrap_or_default().to_owned(),
                    imap_port: v["imap"]["port"].as_u64().unwrap_or(993) as u16,
                    smtp_host: v["smtp"]["host"].as_str().unwrap_or_default().to_owned(),
                    smtp_port: v["smtp"]["port"].as_u64().unwrap_or(587) as u16,
                })
            })();
            *slot.lock().unwrap() = Some(result);
        });
    }

    // ---------- 主界面 ----------

    fn draw_main(&mut self, ui: &mut Ui, full: Rect) {
        let toolbar_h = 44.0;
        let sidebar_w = 150.0;
        let list_w = (full.width() * 0.32).clamp(220.0, 320.0);

        let toolbar = Rect::from_min_max(full.min, pos2(full.right(), full.top() + toolbar_h));
        let below = Rect::from_min_max(pos2(full.left(), toolbar.bottom()), full.max);
        let sidebar = Rect::from_min_max(below.min, pos2(below.left() + sidebar_w, below.bottom()));
        let list = Rect::from_min_max(
            pos2(sidebar.right(), below.top()),
            pos2(sidebar.right() + list_w, below.bottom()),
        );
        let pane = Rect::from_min_max(pos2(list.right(), below.top()), below.max);

        self.draw_toolbar(ui, toolbar);
        self.draw_sidebar(ui, sidebar);
        match self.mailbox {
            Mailbox::Inbox => {
                self.draw_inbox_list(ui, list);
                self.draw_inbox_pane(ui, pane);
            }
            Mailbox::Sent => {
                self.draw_sent_list(ui, list);
                self.draw_sent_pane(ui, pane);
            }
        }
        let p = ui.painter();
        p.line_segment([sidebar.right_top(), sidebar.right_bottom()], Stroke::new(1.0, SEP));
        p.line_segment([list.right_top(), list.right_bottom()], Stroke::new(1.0, SEP));
        p.line_segment([toolbar.left_bottom(), toolbar.right_bottom()], Stroke::new(1.0, SEP));
    }

    fn draw_toolbar(&mut self, ui: &mut Ui, bar: Rect) {
        let p = ui.painter().clone();
        p.rect_filled(bar, 0, SIDEBAR_BG);

        let mut x = bar.left() + 14.0;
        let mut button = |ui: &mut Ui, label: &str, w: f32| -> bool {
            let r = Rect::from_min_size(pos2(x, bar.center().y - 13.0), vec2(w, 26.0));
            x += w + 8.0;
            let resp = ui.interact(r, ui.id().with(("mailtb", label)), Sense::click());
            p.rect_filled(
                r,
                6,
                if resp.hovered() {
                    Color32::from_gray(70)
                } else {
                    Color32::from_gray(55)
                },
            );
            p.text(
                r.center(),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(12.5),
                TEXT,
            );
            resp.clicked()
        };
        if button(ui, "✎ 写邮件", 84.0) {
            self.compose = Some(Compose {
                to: String::new(),
                subject: String::new(),
                body: String::new(),
                sending: false,
            });
        }
        if button(ui, "⟳ 收取", 70.0) {
            self.refresh();
        }
        if button(ui, "账户", 52.0) {
            self.show_setup = true;
            self.error.clear();
        }

        // 右侧状态
        let status = if !self.error.is_empty() {
            (self.error.clone(), Color32::from_rgb(0xE5, 0x5B, 0x5B))
        } else if self.busy {
            (self.status.clone(), DIM)
        } else {
            (
                self.account
                    .as_ref()
                    .map(|a| a.email.clone())
                    .unwrap_or_default(),
                FAINT,
            )
        };
        p.text(
            pos2(bar.right() - 14.0, bar.center().y),
            Align2::RIGHT_CENTER,
            status.0,
            FontId::proportional(11.5),
            status.1,
        );
    }

    fn refresh(&mut self) {
        self.error.clear();
        self.busy = true;
        if self.connected {
            let _ = self.worker().tx.send(Cmd::Refresh);
        } else if let Some(acc) = self.account.clone() {
            // 掉线重连
            self.connect(acc);
        }
    }

    fn draw_sidebar(&mut self, ui: &mut Ui, side: Rect) {
        let p = ui.painter().clone();
        p.rect_filled(side, 0, SIDEBAR_BG);
        p.text(
            pos2(side.left() + 14.0, side.top() + 20.0),
            Align2::LEFT_CENTER,
            "邮箱",
            FontId::proportional(11.0),
            FAINT,
        );
        let unread = self.list.iter().filter(|m| !m.seen).count();
        let rows = [
            (Mailbox::Inbox, "📥 收件箱", unread),
            (Mailbox::Sent, "📤 已发送", 0),
        ];
        let mut y = side.top() + 36.0;
        for (mb, label, badge) in rows {
            let r = Rect::from_min_size(
                pos2(side.left() + 8.0, y),
                vec2(side.width() - 16.0, 30.0),
            );
            let resp = ui.interact(r, ui.id().with(("mailbox", label)), Sense::click());
            if self.mailbox == mb {
                p.rect_filled(r, 6, ACCENT.gamma_multiply(0.32));
            } else if resp.hovered() {
                p.rect_filled(r, 6, Color32::from_gray(58));
            }
            p.text(
                pos2(r.left() + 10.0, r.center().y),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(13.0),
                TEXT,
            );
            if badge > 0 {
                p.text(
                    pos2(r.right() - 10.0, r.center().y),
                    Align2::RIGHT_CENTER,
                    badge.to_string(),
                    FontId::proportional(12.0),
                    DIM,
                );
            }
            if resp.clicked() {
                self.mailbox = mb;
            }
            y += 34.0;
        }
    }

    fn draw_inbox_list(&mut self, ui: &mut Ui, list: Rect) {
        let p = ui.painter().clone();
        p.rect_filled(list, 0, LIST_BG);
        if self.list.is_empty() {
            p.text(
                list.center(),
                Align2::CENTER_CENTER,
                if self.busy { "正在收取…" } else { "没有邮件" },
                FontId::proportional(13.0),
                FAINT,
            );
            return;
        }
        let inner = list.shrink2(vec2(0.0, 2.0));
        let mut lui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::top_down(Align::Min)),
        );
        lui.set_clip_rect(inner.intersect(ui.clip_rect()));
        let mut clicked: Option<u32> = None;
        ScrollArea::vertical()
            .id_salt("mail_list")
            .auto_shrink([false, false])
            .max_height(inner.height())
            .show(&mut lui, |ui| {
                ui.set_width(ui.available_width());
                for m in &self.list {
                    let (row, resp) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::click());
                    let selected = self.selected == Some(m.uid);
                    if selected {
                        ui.painter().rect_filled(row.shrink2(vec2(6.0, 2.0)), 8, ACCENT);
                    } else if resp.hovered() {
                        ui.painter()
                            .rect_filled(row.shrink2(vec2(6.0, 2.0)), 8, Color32::from_gray(50));
                    }
                    let (fg, fg2) = if selected {
                        (Color32::WHITE, Color32::from_gray(225))
                    } else {
                        (TEXT, DIM)
                    };
                    // 未读蓝点
                    if !m.seen {
                        ui.painter().circle_filled(
                            pos2(row.left() + 16.0, row.top() + 18.0),
                            4.0,
                            if selected { Color32::WHITE } else { ACCENT },
                        );
                    }
                    let left = row.left() + 28.0;
                    ui.painter().text(
                        pos2(left, row.top() + 18.0),
                        Align2::LEFT_CENTER,
                        truncate(&m.from, 18),
                        FontId::proportional(13.5),
                        fg,
                    );
                    ui.painter().text(
                        pos2(row.right() - 14.0, row.top() + 18.0),
                        Align2::RIGHT_CENTER,
                        &m.date,
                        FontId::proportional(11.0),
                        fg2,
                    );
                    ui.painter().text(
                        pos2(left, row.top() + 40.0),
                        Align2::LEFT_CENTER,
                        truncate(&m.subject, 34),
                        FontId::proportional(12.5),
                        fg2,
                    );
                    ui.painter().line_segment(
                        [
                            pos2(row.left() + 28.0, row.bottom()),
                            pos2(row.right() - 6.0, row.bottom()),
                        ],
                        Stroke::new(1.0, SEP.gamma_multiply(0.6)),
                    );
                    if resp.clicked() {
                        clicked = Some(m.uid);
                    }
                }
            });
        if let Some(uid) = clicked {
            self.selected = Some(uid);
            self.body = None;
            if let Some(m) = self.list.iter_mut().find(|m| m.uid == uid) {
                m.seen = true; // BODY[] 抓取会在服务端置 \Seen，本地同步标掉
            }
            self.busy = true;
            let _ = self.worker().tx.send(Cmd::FetchBody(uid));
        }
    }

    fn draw_inbox_pane(&mut self, ui: &mut Ui, pane: Rect) {
        let p = ui.painter().clone();
        let Some(uid) = self.selected else {
            p.text(
                pane.center(),
                Align2::CENTER_CENTER,
                "未选择邮件",
                FontId::proportional(14.0),
                FAINT,
            );
            return;
        };
        let Some(m) = self.list.iter().find(|m| m.uid == uid).cloned() else {
            return;
        };
        // 头部
        let head = Rect::from_min_size(pane.min, vec2(pane.width(), 74.0));
        p.text(
            pos2(head.left() + 18.0, head.top() + 24.0),
            Align2::LEFT_CENTER,
            &m.subject,
            FontId::proportional(16.0),
            TEXT,
        );
        p.text(
            pos2(head.left() + 18.0, head.top() + 50.0),
            Align2::LEFT_CENTER,
            format!("{} · {}", m.from, m.date),
            FontId::proportional(12.0),
            DIM,
        );
        p.line_segment(
            [
                pos2(head.left() + 14.0, head.bottom()),
                pos2(head.right() - 14.0, head.bottom()),
            ],
            Stroke::new(1.0, SEP),
        );
        // 正文
        let body_rect = Rect::from_min_max(pos2(pane.left(), head.bottom()), pane.max).shrink(16.0);
        let mut bui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(body_rect)
                .layout(Layout::top_down(Align::Min)),
        );
        bui.set_clip_rect(body_rect.intersect(ui.clip_rect()));
        ScrollArea::vertical()
            .id_salt("mail_body")
            .auto_shrink([false, false])
            .max_height(body_rect.height())
            .show(&mut bui, |ui| {
                ui.set_width(ui.available_width());
                match &self.body {
                    Some((buid, text)) if *buid == uid => {
                        ui.label(RichText::new(text).color(Color32::from_gray(210)).size(13.5));
                    }
                    _ => {
                        ui.add_space(20.0);
                        ui.label(RichText::new("正在加载正文…").color(FAINT).size(13.0));
                    }
                }
            });
    }

    fn draw_sent_list(&mut self, ui: &mut Ui, list: Rect) {
        let p = ui.painter().clone();
        p.rect_filled(list, 0, LIST_BG);
        if self.sent.is_empty() {
            p.text(
                list.center(),
                Align2::CENTER_CENTER,
                "本次会话还没有发出的邮件",
                FontId::proportional(12.5),
                FAINT,
            );
            return;
        }
        let inner = list.shrink2(vec2(0.0, 2.0));
        let mut lui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::top_down(Align::Min)),
        );
        lui.set_clip_rect(inner.intersect(ui.clip_rect()));
        ScrollArea::vertical()
            .id_salt("sent_list")
            .auto_shrink([false, false])
            .max_height(inner.height())
            .show(&mut lui, |ui| {
                ui.set_width(ui.available_width());
                for (i, m) in self.sent.iter().enumerate().rev() {
                    let (row, resp) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::click());
                    let selected = self.selected_sent == Some(i);
                    if selected {
                        ui.painter().rect_filled(row.shrink2(vec2(6.0, 2.0)), 8, ACCENT);
                    } else if resp.hovered() {
                        ui.painter()
                            .rect_filled(row.shrink2(vec2(6.0, 2.0)), 8, Color32::from_gray(50));
                    }
                    let (fg, fg2) = if selected {
                        (Color32::WHITE, Color32::from_gray(225))
                    } else {
                        (TEXT, DIM)
                    };
                    ui.painter().text(
                        pos2(row.left() + 16.0, row.top() + 16.0),
                        Align2::LEFT_CENTER,
                        format!("发往 {}", truncate(&m.to, 22)),
                        FontId::proportional(13.0),
                        fg,
                    );
                    ui.painter().text(
                        pos2(row.right() - 14.0, row.top() + 16.0),
                        Align2::RIGHT_CENTER,
                        &m.date,
                        FontId::proportional(11.0),
                        fg2,
                    );
                    ui.painter().text(
                        pos2(row.left() + 16.0, row.top() + 38.0),
                        Align2::LEFT_CENTER,
                        truncate(&m.subject, 34),
                        FontId::proportional(12.5),
                        fg2,
                    );
                    if resp.clicked() {
                        self.selected_sent = Some(i);
                    }
                }
            });
    }

    fn draw_sent_pane(&mut self, ui: &mut Ui, pane: Rect) {
        let p = ui.painter().clone();
        let Some(m) = self.selected_sent.and_then(|i| self.sent.get(i)) else {
            p.text(
                pane.center(),
                Align2::CENTER_CENTER,
                "未选择邮件",
                FontId::proportional(14.0),
                FAINT,
            );
            return;
        };
        let head = Rect::from_min_size(pane.min, vec2(pane.width(), 74.0));
        p.text(
            pos2(head.left() + 18.0, head.top() + 24.0),
            Align2::LEFT_CENTER,
            &m.subject,
            FontId::proportional(16.0),
            TEXT,
        );
        p.text(
            pos2(head.left() + 18.0, head.top() + 50.0),
            Align2::LEFT_CENTER,
            format!("发往 {} · {}", m.to, m.date),
            FontId::proportional(12.0),
            DIM,
        );
        p.line_segment(
            [
                pos2(head.left() + 14.0, head.bottom()),
                pos2(head.right() - 14.0, head.bottom()),
            ],
            Stroke::new(1.0, SEP),
        );
        let body_rect = Rect::from_min_max(pos2(pane.left(), head.bottom()), pane.max).shrink(16.0);
        let text = m.body.clone();
        let mut bui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(body_rect)
                .layout(Layout::top_down(Align::Min)),
        );
        bui.set_clip_rect(body_rect.intersect(ui.clip_rect()));
        ScrollArea::vertical()
            .id_salt("sent_body")
            .auto_shrink([false, false])
            .max_height(body_rect.height())
            .show(&mut bui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(text).color(Color32::from_gray(210)).size(13.5));
            });
    }

    // ---------- 写邮件浮层 ----------

    fn draw_compose(&mut self, ui: &mut Ui, full: Rect) {
        let p = ui.painter().clone();
        p.rect_filled(full, 0, crate::ui::black_a(0.35));
        let card = Rect::from_center_size(
            full.center(),
            vec2(
                520.0_f32.min(full.width() - 40.0),
                420.0_f32.min(full.height() - 40.0),
            ),
        );
        p.rect_filled(card, 10, Color32::from_rgb(0x2A, 0x2A, 0x30));
        p.rect_stroke(card, 10, Stroke::new(1.0, SEP), StrokeKind::Inside);
        p.text(
            pos2(card.center().x, card.top() + 22.0),
            Align2::CENTER_CENTER,
            "新邮件",
            FontId::proportional(14.0),
            TEXT,
        );

        let inner = Rect::from_min_max(
            pos2(card.left() + 16.0, card.top() + 42.0),
            card.max - vec2(16.0, 16.0),
        );
        let mut cui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(inner)
                .layout(Layout::top_down(Align::Min)),
        );
        cui.set_clip_rect(inner.intersect(ui.clip_rect()));
        cui.spacing_mut().item_spacing.y = 8.0;

        let c = self.compose.as_mut().unwrap();
        let sending = c.sending;
        cui.horizontal(|cui| {
            cui.add_sized(
                vec2(56.0, 24.0),
                egui::Label::new(RichText::new("收件人").color(DIM).size(12.5)),
            );
            cui.add_enabled(
                !sending,
                TextEdit::singleline(&mut c.to)
                    .hint_text("name@example.com")
                    .text_color(TEXT)
                    .desired_width(cui.available_width()),
            );
        });
        cui.horizontal(|cui| {
            cui.add_sized(
                vec2(56.0, 24.0),
                egui::Label::new(RichText::new("主题").color(DIM).size(12.5)),
            );
            cui.add_enabled(
                !sending,
                TextEdit::singleline(&mut c.subject)
                    .text_color(TEXT)
                    .desired_width(cui.available_width()),
            );
        });
        let body_h = inner.height() - 24.0 * 2.0 - 8.0 * 3.0 - 32.0;
        cui.add_enabled(
            !sending,
            TextEdit::multiline(&mut c.body)
                .text_color(TEXT)
                .desired_width(inner.width())
                .desired_rows(1)
                .min_size(vec2(inner.width(), body_h.max(80.0))),
        );

        let mut do_send: Option<(String, String, String)> = None;
        let mut do_cancel = false;
        cui.horizontal(|cui| {
            let can_send =
                !sending && c.to.trim().contains('@') && !(c.subject.is_empty() && c.body.is_empty());
            let send = egui::Button::new(
                RichText::new(if sending { "发送中…" } else { "发送" })
                    .color(Color32::WHITE)
                    .size(13.0),
            )
            .fill(ACCENT)
            .corner_radius(CornerRadius::same(6))
            .min_size(vec2(88.0, 26.0));
            if cui.add_enabled(can_send, send).clicked() {
                do_send = Some((c.to.clone(), c.subject.clone(), c.body.clone()));
            }
            let cancel = egui::Button::new(RichText::new("取消").color(TEXT).size(13.0))
                .corner_radius(CornerRadius::same(6))
                .min_size(vec2(64.0, 26.0));
            if cui.add_enabled(!sending, cancel).clicked() || cui.input(|i| i.key_pressed(Key::Escape)) {
                do_cancel = true;
            }
        });
        if let Some((to, subject, body)) = do_send {
            if let Some(c) = &mut self.compose {
                c.sending = true;
            }
            self.busy = true;
            self.error.clear();
            let _ = self.worker().tx.send(Cmd::Send { to, subject, body });
        } else if do_cancel {
            self.compose = None;
        }
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_owned();
    }
    let cut: String = s.chars().take(max_chars).collect();
    format!("{cut}…")
}
