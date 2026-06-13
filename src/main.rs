//! mirage：用 Rust + egui 复刻 macOS 桌面的窗口管理骨架。
//!
//! 分层：
//! - `wm`   窗口管理纯逻辑（z-order / 焦点 / 动画状态机），不依赖渲染
//! - `ui`   egui 适配层（壁纸、菜单栏、窗口 chrome、Dock、Launchpad）
//! - `apps` 应用注册表（壳应用）

mod anim;
mod apps;
mod codex;
mod config;
mod mail;
mod ui;
mod wm;

use std::collections::HashSet;

use eframe::egui;
use egui::{pos2, vec2, CursorIcon, Pos2, Rect, Vec2};
use raw_window_handle::HasWindowHandle;

use codex::{CLAUDE_BACKEND, CODEX_BACKEND};
use config::DesktopConfig;
use ui::agent::AgentApp;
use ui::browser::{BrowserApp, HostHandle};
use ui::dock::{self, DockGeometry, DockKind, DockState};
use ui::finder::FinderApp;
use ui::launchpad::{Launchpad, LpPress};
use ui::mail::MailApp;
use ui::maps::MapsApp;
use ui::menubar::{MenuAction, MenuPress, MenuState};
use ui::music::MusicApp;
use ui::photos::PhotosApp;
use ui::reminders::RemindersApp;
use ui::terminal::TerminalApp;
use ui::trash::TrashApp;
use ui::wechat::WeChatApp;
use ui::{chrome, desktop, menubar};
use wm::{EdgeHit, WindowId, WindowManager};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Mirage")
            .with_inner_size(vec2(1440.0, 900.0))
            .with_maximized(true),
        ..Default::default()
    };
    eframe::run_native(
        "Mirage",
        options,
        Box::new(|cc| Ok(Box::new(DesktopApp::new(cc)))),
    )
}

enum DragOp {
    Move {
        id: WindowId,
        grab: Vec2,
    },
    Resize {
        id: WindowId,
        edge: EdgeHit,
        start_rect: Rect,
        start_ptr: Pos2,
    },
}

struct DesktopApp {
    wm: WindowManager,
    dock: DockState,
    launchpad: Launchpad,
    menu: MenuState,
    drag: Option<DragOp>,
    /// 拖拽中的边缘吸附预览（松手后窗口归位到这个 rect）
    snap: Option<Rect>,
    cascade: usize,
    selftest: Option<SelfTest>,
    /// 真实应用
    agent_codex: AgentApp,
    agent_claude: AgentApp,
    browser: BrowserApp,
    mail: MailApp,
    maps_app: MapsApp,
    finder: FinderApp,
    reminders: RemindersApp,
    photos: PhotosApp,
    music: MusicApp,
    terminal: TerminalApp,
    trash: TrashApp,
    wechat_app: WeChatApp,
    host: Option<HostHandle>,
    /// 桌面配置（被系统设置面板修改）
    cfg: DesktopConfig,
    appshot: Option<AppShot>,
    /// MIRAGE_OPEN="wechat,maps"：首帧自动打开这些应用后正常运行
    /// （webview 是原生子视图，egui 截图抓不到，真机验证用系统截屏 + 本变量）
    open_on_start: Vec<&'static str>,
}

/// 开发自检：MIRAGE_SHOT=<前缀> 启动后自动走一遍关键场景并截图退出。
/// 输出：<前缀>-{desktop,genie,launchpad,menu,tiled}.png
struct SelfTest {
    prefix: String,
    stage: u8,
    t0: Option<f64>,
    /// 最近一次状态切换时的 elapsed，用于相对计时
    mark: f64,
}

/// 应用截图：MIRAGE_APPSHOT="terminal,trash,settings" 逐个打开应用窗口、
/// 截图保存 /tmp/appshot-<id>.png 后退出。与 MIRAGE_SHOT 互斥使用。
struct AppShot {
    apps: Vec<&'static str>,
    idx: usize,
    stage: u8,
    t0: Option<f64>,
    mark: f64,
}

impl DesktopApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_macos_fonts(&cc.egui_ctx);
        // macOS 式滚动条：悬浮细圆条，滚动时浮现，不占布局、无轨道底色
        cc.egui_ctx.global_style_mut(|s| {
            s.spacing.scroll = egui::style::ScrollStyle::floating();
            s.spacing.scroll.bar_width = 8.0;
        });
        Self {
            wm: WindowManager::default(),
            dock: DockState::default(),
            launchpad: Launchpad::default(),
            menu: MenuState::default(),
            drag: None,
            snap: None,
            cascade: 0,
            selftest: std::env::var("MIRAGE_SHOT").ok().map(|prefix| SelfTest {
                prefix,
                stage: 0,
                t0: None,
                mark: 0.0,
            }),
            appshot: std::env::var("MIRAGE_APPSHOT").ok().map(|list| AppShot {
                apps: list
                    .split(',')
                    .map(|t| apps::get(t.trim()).id)
                    .collect(),
                idx: 0,
                stage: 0,
                t0: None,
                mark: 0.0,
            }),
            agent_codex: AgentApp::new(&CODEX_BACKEND, "Codex Agent"),
            agent_claude: AgentApp::new(&CLAUDE_BACKEND, "Claude Code"),
            browser: BrowserApp::default(),
            mail: MailApp::default(),
            maps_app: MapsApp::default(),
            finder: FinderApp::default(),
            reminders: RemindersApp::default(),
            photos: PhotosApp::default(),
            music: MusicApp::default(),
            terminal: TerminalApp::default(),
            trash: TrashApp::default(),
            wechat_app: WeChatApp::default(),
            host: None,
            cfg: DesktopConfig::default(),
            open_on_start: std::env::var("MIRAGE_OPEN")
                .map(|list| list.split(',').map(|t| apps::get(t.trim()).id).collect())
                .unwrap_or_default(),
        }
    }

    fn run_appshot(&mut self, ctx: &egui::Context, screen: Rect, now: f64) {
        let Some(st) = &mut self.appshot else { return };
        let t0 = *st.t0.get_or_insert(now);
        let elapsed = now - t0;
        let (stage, mark, idx) = (st.stage, st.mark, st.idx);
        if idx >= st.apps.len() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let app_id = st.apps[idx];

        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            if let Some(buf) =
                image::RgbaImage::from_raw(img.size[0] as u32, img.size[1] as u32, bytes)
            {
                let _ = buf.save(format!("/tmp/appshot-{app_id}.png"));
            }
            // 关掉当前窗口，进入下一个应用
            if let Some(front) = self.wm.front_id() {
                self.wm.close(front, now);
            }
            if let Some(st) = &mut self.appshot {
                st.idx += 1;
                st.stage = 0;
            }
            ctx.request_repaint();
            return;
        }

        match stage {
            0 => {
                self.open_window(app_id, screen, now);
                if let Some(st) = &mut self.appshot {
                    st.stage = 1;
                    st.mark = elapsed;
                }
            }
            // 等 2.5s：PTY 终端的 login shell 启动较慢；
            // 邮件要等 IMAP 连接 + 拉取列表，给 8s
            1 if elapsed > mark + if app_id == "mail" { 8.0 } else { 2.5 } => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
                if let Some(st) = &mut self.appshot {
                    st.stage = 2;
                }
            }
            _ => {}
        }
        ctx.request_repaint();
    }

    fn run_selftest(&mut self, ctx: &egui::Context, screen: Rect, geom: &DockGeometry, now: f64) {
        let Some(st) = &mut self.selftest else { return };
        let t0 = *st.t0.get_or_insert(now);
        let elapsed = now - t0;
        let stage = st.stage;
        let mark = st.mark;
        let prefix = st.prefix.clone();

        let set_stage = |me: &mut Self, s: u8, elapsed: f64| {
            if let Some(st) = &mut me.selftest {
                st.stage = s;
                st.mark = elapsed;
            }
        };
        let shoot = |ctx: &egui::Context| {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        };

        // 收截图事件
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let name = match stage {
                2 => "desktop",
                4 => "genie",
                6 => "launchpad",
                8 => "menu",
                10 => "tiled",
                12 => "codex",
                _ => "chrome",
            };
            let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            if let Some(buf) =
                image::RgbaImage::from_raw(img.size[0] as u32, img.size[1] as u32, bytes)
            {
                let _ = buf.save(format!("{prefix}-{name}.png"));
            }
            match stage {
                2 => {
                    // 桌面截完 -> 最小化前窗，抓 genie 中段
                    if let Some(front) = self.wm.front_id() {
                        self.wm.minimize(front, geom.minimized_anchor, now);
                    }
                    set_stage(self, 3, elapsed);
                }
                4 => {
                    self.launchpad.toggle(now);
                    set_stage(self, 5, elapsed);
                }
                6 => {
                    self.launchpad.close(now);
                    set_stage(self, 7, elapsed);
                }
                8 => {
                    self.menu.open = None;
                    // 平铺：前窗去左半屏
                    let work = Self::work_rect(screen, geom);
                    let half = Rect::from_min_max(work.min, pos2(work.center().x, work.bottom()));
                    if let Some(front) = self.wm.front_id() {
                        self.wm.morph_to(front, half, now);
                    }
                    set_stage(self, 9, elapsed);
                }
                10 => {
                    // 打开 Codex 窗口，等会话就绪后自动发一轮 prompt
                    self.open_window("codex", screen, now);
                    set_stage(self, 11, elapsed);
                }
                12 => {
                    self.open_window("chrome", screen, now);
                    set_stage(self, 13, elapsed);
                }
                _ => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
            return;
        }

        match stage {
            0 => {
                self.open_window("notes", screen, now);
                self.open_window("finder", screen, now);
                set_stage(self, 1, elapsed);
            }
            1 if elapsed > 1.0 => {
                shoot(ctx);
                set_stage(self, 2, elapsed);
            }
            // genie 中段（最小化时长 0.38s，取 ~45%）
            3 if elapsed > mark + 0.17 => {
                shoot(ctx);
                set_stage(self, 4, elapsed);
            }
            5 if elapsed > mark + 0.6 => {
                shoot(ctx);
                set_stage(self, 6, elapsed);
            }
            // Launchpad 关完后再开菜单（早开会被 alpha=0 的菜单栏清掉）
            7 if elapsed > mark + 0.5 => {
                self.menu.open = Some(2); // 文件菜单
                shoot(ctx);
                set_stage(self, 8, elapsed);
            }
            9 if elapsed > mark + 0.5 => {
                shoot(ctx);
                set_stage(self, 10, elapsed);
            }
            // Codex：会话就绪后发一轮真实 prompt，等回复（或 60s 超时）再截
            11 => {
                let prompted = self
                    .selftest
                    .as_ref()
                    .is_some_and(|st| st.mark < 0.0);
                if !prompted {
                    if self.agent_codex.auto_prompt(
                        "请运行 shell 命令 `ls -la` 查看当前工作目录，然后用一句话总结看到了什么。",
                    ) {
                        if let Some(st) = &mut self.selftest {
                            st.mark = -elapsed; // 负值标记「已发送」，绝对值为发送时刻
                        }
                    } else if elapsed > mark + 20.0 {
                        // 会话一直没就绪（如未登录）：直接截当前状态
                        shoot(ctx);
                        set_stage(self, 12, elapsed);
                    }
                } else {
                    let sent_at = -mark;
                    let done =
                        !self.agent_codex.running() && self.agent_codex.has_assistant_reply();
                    if (done && elapsed > sent_at + 1.0) || elapsed > sent_at + 100.0 {
                        // 确保 Codex 窗口聚焦，否则只画占位（run_selftest 在窗口绘制前，本帧即生效）
                        if let Some(w) = self.wm.window_of_app("codex") {
                            let id = w.id;
                            self.wm.focus(id);
                        }
                        self.agent_codex.expand_all(); // 展开思考/工具块再截图
                        shoot(ctx);
                        set_stage(self, 12, elapsed);
                    }
                }
            }
            13 if elapsed > mark + 1.5 => {
                shoot(ctx);
                set_stage(self, 14, elapsed);
            }
            _ => {}
        }
        ctx.request_repaint();
    }

    // ---------- 窗口操作 ----------

    fn open_window(&mut self, app_id: &'static str, screen: Rect, now: f64) {
        // 外部原生应用：用系统 open -a 拉起（无法嵌入窗口）
        if let Some(native_name) = apps::external_app(app_id) {
            let _ = std::process::Command::new("open")
                .arg("-a")
                .arg(native_name)
                .spawn();
            return;
        }
        // 自带真实内容的应用是单例：已有窗口则聚焦
        if chrome::has_custom_content(app_id) {
            if let Some(w) = self.wm.window_of_app(app_id) {
                let id = w.id;
                self.wm.focus(id);
                return;
            }
        }
        let app = apps::get(app_id);
        let size = match app_id {
            "codex" | "claude" => vec2(560.0, 660.0),
            "chrome" => vec2(1000.0, 680.0),
            "mail" => vec2(1060.0, 660.0),
            "maps" => vec2(900.0, 620.0),
            "wechat" => vec2(1000.0, 700.0),
            "finder" => vec2(920.0, 560.0),
            "reminders" => vec2(460.0, 540.0),
            "photos" => vec2(980.0, 640.0),
            "music" => vec2(1000.0, 660.0),
            "terminal" => vec2(760.0, 460.0),
            "trash" => vec2(560.0, 440.0),
            "settings" => vec2(600.0, 540.0),
            _ => vec2(820.0, 540.0),
        };
        let n = (self.cascade % 7) as f32;
        self.cascade += 1;
        let min = screen.center() - size / 2.0 + vec2(n * 32.0 - 90.0, n * 26.0 - 60.0);
        let min = pos2(min.x.max(8.0), min.y.max(menubar::HEIGHT + 6.0));
        self.wm
            .open(app_id, app.name.to_owned(), Rect::from_min_size(min, size), now);
    }

    /// 打开应用：已有窗口则聚焦/恢复，否则新建
    fn open_or_focus(&mut self, app_id: &'static str, screen: Rect, from: Pos2, now: f64) {
        match self.wm.window_of_app(app_id) {
            Some(w) => {
                let (id, minimized) = (w.id, w.minimized);
                if minimized {
                    self.wm.restore(id, from, now);
                } else {
                    self.wm.focus(id);
                }
            }
            None => self.open_window(app_id, screen, now),
        }
    }

    fn dock_entries(&self) -> Vec<DockKind> {
        let mut v: Vec<DockKind> = Vec::new();
        v.push(DockKind::App("finder"));
        v.push(DockKind::Launchpad);
        for app in apps::APPS.iter().skip(1) {
            v.push(DockKind::App(app.id));
        }
        v.push(DockKind::Separator);
        for w in self.wm.windows.iter().filter(|w| w.minimized) {
            v.push(DockKind::Minimized {
                id: w.id,
                app_id: w.app_id,
                title: w.title.clone(),
            });
        }
        v.push(DockKind::Trash);
        v
    }

    /// 最大化目标区域：菜单栏之下、Dock 之上
    fn work_rect(screen: Rect, geom: &DockGeometry) -> Rect {
        Rect::from_min_max(
            pos2(screen.left(), screen.top() + menubar::HEIGHT),
            pos2(screen.right(), geom.panel.top() - 8.0),
        )
    }

    // ---------- 输入路由 ----------

    fn handle_input(&mut self, ctx: &egui::Context, screen: Rect, geom: &DockGeometry, now: f64) {
        let (pos_opt, pressed, released, double, esc, cmd_w, cmd_m, cmd_n) = ctx.input(|i| {
            (
                i.pointer.hover_pos(),
                i.pointer.primary_pressed(),
                i.pointer.primary_released(),
                i.pointer.button_double_clicked(egui::PointerButton::Primary),
                i.key_pressed(egui::Key::Escape),
                i.modifiers.command && i.key_pressed(egui::Key::W),
                i.modifiers.command && i.key_pressed(egui::Key::M),
                i.modifiers.command && i.key_pressed(egui::Key::N),
            )
        });

        let lp_visible = self.launchpad.visible(now);

        if esc {
            if lp_visible {
                self.launchpad.close(now);
            }
            self.menu.open = None;
        }
        if let Some(front) = self.wm.front_id() {
            if cmd_w {
                self.wm.close(front, now);
            }
            if cmd_m {
                self.wm.minimize(front, geom.minimized_anchor, now);
            }
        }
        if cmd_n {
            self.do_menu_action(MenuAction::NewWindow, screen, geom, now);
        }

        if released {
            // 拖拽到边缘松手：归位到吸附预览
            if let (Some(DragOp::Move { id, .. }), Some(target)) = (&self.drag, self.snap) {
                let id = *id;
                self.wm.morph_to(id, target, now);
            }
            self.snap = None;
            self.drag = None;
        }

        let Some(pos) = pos_opt else { return };

        // 进行中的拖拽优先
        if self.drag.is_some() {
            let work = Self::work_rect(screen, geom);
            self.apply_drag(ctx, pos, screen, work);
            return;
        }

        // 悬停时的缩放光标
        if !lp_visible {
            if let Some((_, edge)) = self.hit_resize(pos) {
                ctx.output_mut(|o| o.cursor_icon = edge_cursor(edge));
            }
        }

        // 双击标题栏 = zoom
        if double && !lp_visible {
            if let Some(w) = self.wm.topmost_at(pos) {
                let (id, rect) = (w.id, w.rect);
                if chrome::titlebar_rect(rect).contains(pos)
                    && chrome::traffic_hit(rect, pos).is_none()
                {
                    let target = Self::work_rect(screen, geom);
                    self.wm.toggle_maximize(id, target, now);
                    self.drag = None;
                    return;
                }
            }
        }

        if !pressed {
            return;
        }

        // 1) 菜单栏与下拉菜单（Launchpad 打开时菜单栏不可见）
        if !lp_visible {
            match self.menu.handle_press(pos) {
                MenuPress::Title(i) => {
                    self.menu.open = if self.menu.open == Some(i) { None } else { Some(i) };
                    return;
                }
                MenuPress::Action(a) => {
                    self.menu.open = None;
                    self.do_menu_action(a, screen, geom, now);
                    return;
                }
                MenuPress::Swallow => {
                    self.menu.open = None;
                    return;
                }
                MenuPress::Pass => {}
            }
        }

        // 2) Dock（Launchpad 打开时 Dock 仍可用）
        if let Some(item) = dock::hit(geom, pos) {
            let (kind, center) = (item.kind.clone(), item.rect.center());
            self.dock_click(kind, center, screen, now);
            return;
        }
        if geom.panel.contains(pos) {
            return; // 点在 Dock 面板空白处：吞掉
        }

        // 3) Launchpad 层
        if lp_visible {
            match self.launchpad.press_target(pos) {
                LpPress::Icon(app_id) => {
                    self.open_or_focus(app_id, screen, geom.minimized_anchor, now);
                    self.launchpad.close(now);
                }
                LpPress::Search => {}
                LpPress::Empty => self.launchpad.close(now),
            }
            return;
        }

        // 4) 窗口：先边缘缩放，再标题栏/红绿灯/内容
        if let Some((id, edge)) = self.hit_resize(pos) {
            self.wm.focus(id);
            let rect = self.wm.get(id).unwrap().rect;
            self.drag = Some(DragOp::Resize {
                id,
                edge,
                start_rect: rect,
                start_ptr: pos,
            });
            return;
        }

        if let Some(w) = self.wm.topmost_at(pos) {
            let (id, rect) = (w.id, w.rect);
            self.wm.focus(id);
            if let Some(btn) = chrome::traffic_hit(rect, pos) {
                match btn {
                    0 => self.wm.close(id, now),
                    1 => self.wm.minimize(id, geom.minimized_anchor, now),
                    _ => {
                        let target = Self::work_rect(screen, geom);
                        self.wm.toggle_maximize(id, target, now);
                    }
                }
            } else if chrome::titlebar_rect(rect).contains(pos) {
                self.drag = Some(DragOp::Move {
                    id,
                    grab: pos - rect.min,
                });
            }
        }
    }

    fn apply_drag(&mut self, ctx: &egui::Context, pos: Pos2, screen: Rect, work: Rect) {
        match self.drag.as_ref().unwrap() {
            DragOp::Move { id, grab } => {
                let id = *id;
                let grab = *grab;
                if let Some(w) = self.wm.get(id) {
                    let size = w.rect.size();
                    let mut min = pos - grab;
                    min.y = min.y.max(menubar::HEIGHT);
                    min.x = min.x.clamp(-size.x + 80.0, screen.right() - 80.0);
                    self.wm.set_rect(id, Rect::from_min_size(min, size));
                }
                self.snap = snap_target(pos, screen, work);
            }
            DragOp::Resize {
                id,
                edge,
                start_rect,
                start_ptr,
            } => {
                let (id, edge, start_rect, start_ptr) = (*id, *edge, *start_rect, *start_ptr);
                let d = pos - start_ptr;
                let mut r = start_rect;
                let min = chrome::MIN_SIZE;
                if edge.w {
                    r.min.x = (start_rect.min.x + d.x).min(r.max.x - min.x);
                }
                if edge.e {
                    r.max.x = (start_rect.max.x + d.x).max(r.min.x + min.x);
                }
                if edge.n {
                    r.min.y = (start_rect.min.y + d.y).clamp(menubar::HEIGHT, r.max.y - min.y);
                }
                if edge.s {
                    r.max.y = (start_rect.max.y + d.y).max(r.min.y + min.y);
                }
                ctx.output_mut(|o| o.cursor_icon = edge_cursor(edge));
                self.wm.set_rect(id, r);
            }
        }
    }

    /// 找到最前面可命中缩放边缘的窗口
    fn hit_resize(&self, pos: Pos2) -> Option<(WindowId, EdgeHit)> {
        for w in self.wm.windows.iter().rev().filter(|w| w.interactive()) {
            if let Some(edge) = wm::hit_edges(w.rect, pos) {
                return Some((w.id, edge));
            }
            if w.rect.contains(pos) {
                return None; // 被这个窗口挡住
            }
        }
        None
    }

    fn dock_click(&mut self, kind: DockKind, icon_center: Pos2, screen: Rect, now: f64) {
        match kind {
            DockKind::App(app_id) => {
                if self.wm.window_of_app(app_id).is_none() {
                    self.dock.bounce.insert(app_id, now);
                }
                self.open_or_focus(app_id, screen, icon_center, now);
                self.launchpad.close(now);
            }
            DockKind::Launchpad => self.launchpad.toggle(now),
            DockKind::Minimized { id, .. } => {
                self.wm.restore(id, icon_center, now);
                self.launchpad.close(now);
            }
            DockKind::Trash => {
                self.open_or_focus("trash", screen, icon_center, now);
                self.launchpad.close(now);
            }
            DockKind::Separator => {}
        }
    }

    fn do_menu_action(&mut self, a: MenuAction, screen: Rect, geom: &DockGeometry, now: f64) {
        let front = self.wm.front_id();
        match a {
            MenuAction::NewWindow => {
                let app = front
                    .and_then(|id| self.wm.get(id))
                    .map(|w| w.app_id)
                    .unwrap_or("finder");
                self.open_window(app, screen, now);
            }
            MenuAction::CloseWindow => {
                if let Some(id) = front {
                    self.wm.close(id, now);
                }
            }
            MenuAction::MinimizeWindow => {
                if let Some(id) = front {
                    self.wm.minimize(id, geom.minimized_anchor, now);
                }
            }
            MenuAction::ZoomWindow => {
                if let Some(id) = front {
                    let target = Self::work_rect(screen, geom);
                    self.wm.toggle_maximize(id, target, now);
                }
            }
            MenuAction::OpenApp(app_id) => {
                self.open_or_focus(app_id, screen, geom.minimized_anchor, now);
            }
            MenuAction::Noop => {}
        }
    }

    fn front_app_name(&self) -> &'static str {
        self.wm
            .front_id()
            .and_then(|id| self.wm.get(id))
            .map(|w| apps::get(w.app_id).name)
            .unwrap_or("访达")
    }
}

impl eframe::App for DesktopApp {
    fn ui(&mut self, root_ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        // eframe 0.34 的入口是 ui()；我们沿用 ctx 级的 CentralPanel 流程
        let ctx = &root_ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        self.wm.cleanup(now);
        self.dock
            .bounce
            .retain(|_, t0| now - *t0 < dock::BOUNCE_DUR);

        // 主窗口句柄（wry 子视图挂载用）
        if self.host.is_none() {
            if let Ok(h) = frame.window_handle() {
                self.host = Some(HostHandle(h.as_raw()));
            }
        }
        // Agent 事件持续消化（窗口关了 turn 也照常推进）
        self.agent_codex.pump();
        self.agent_claude.pump();
        // 曲终自动切歌（窗口关着音乐也继续放）
        self.music.tick();

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show_inside(root_ui, |ui| {
                let screen = ui.max_rect();
                let painter = ui.painter().clone();
                let pointer = ctx.input(|i| i.pointer.hover_pos());

                desktop::draw_wallpaper(&painter, screen, self.cfg.wallpaper);

                // 先布局 Dock：输入路由和最小化动画目标都依赖它
                let entries = self.dock_entries();
                let geom = dock::layout(
                    screen,
                    pointer,
                    &entries,
                    self.cfg.dock_base(),
                    self.cfg.dock_magnify,
                );

                self.run_selftest(ctx, screen, &geom, now);
                self.run_appshot(ctx, screen, now);
                for app_id in std::mem::take(&mut self.open_on_start) {
                    self.open_window(app_id, screen, now);
                }
                self.handle_input(ctx, screen, &geom, now);

                // 绘制窗口（Vec 顺序即 z-order）。
                // 应用内容默认可见：未聚焦时仍渲染，只是禁用交互（egui 的命中测试
                // 不知道上层手绘窗口的遮挡关系，禁用可防止被盖住的控件误响应）。
                // webview 类（chrome/maps）例外：原生视图永远浮在最上层——
                // 只要没被上方窗口遮挡就一直显示（不要求聚焦），被遮挡时退占位。
                let focused = self.wm.front_id();
                let lp_vis = self.launchpad.visible(now);
                let menu_open = self.menu.open.is_some();

                let mut chrome_eligible = false;
                let mut maps_eligible = false;
                let mut wechat_eligible = false;
                for i in 0..self.wm.windows.len() {
                    // 先画窗口 chrome，再就地画自定义内容（保证 z-order 正确）
                    let (win_id, app_id, interactive, steady, content, alpha_hint, covered) = {
                        let win = &self.wm.windows[i];
                        let is_focused = Some(win.id) == focused;
                        chrome::draw_window(&painter, win, is_focused, now, pointer);
                        if !chrome::has_custom_content(win.app_id) {
                            continue;
                        }
                        let Some((rect, alpha)) = chrome::effective_rect(win, now) else {
                            continue;
                        };
                        let steady = matches!(win.anim, wm::WindowAnim::None) && !win.minimized;
                        let content = chrome::content_rect(rect);
                        // 是否被 z-order 更高的窗口遮挡（webview 显隐用）。
                        // shrink(1) 避免平铺窗口共享边界被误判为相交。
                        let probe = content.shrink(1.0);
                        let covered = self.wm.windows[i + 1..]
                            .iter()
                            .any(|w| w.interactive() && w.rect.intersects(probe));
                        (
                            win.id,
                            win.app_id,
                            is_focused && steady && !lp_vis && !menu_open,
                            steady,
                            content,
                            alpha > 0.5,
                            covered,
                        )
                    };
                    // 非 webview 应用：steady 即渲染，未聚焦禁用。
                    // id_salt 用窗口 id：不同窗口里的 ScrollArea 等控件
                    // 否则会生成相同 ID 互相打架（滚动状态串台甚至失效）。
                    let child_ui = |ui: &mut egui::Ui| {
                        let mut child = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(content)
                                .id_salt(("win-content", win_id)),
                        );
                        if !interactive {
                            child.disable();
                        }
                        child
                    };
                    // webview 可见 = 稳定 && 未被上方窗口遮挡 && 无全屏遮罩（不要求聚焦）
                    let webview_visible = steady && !covered && !lp_vis && !menu_open;
                    match app_id {
                        "chrome" => {
                            if webview_visible {
                                chrome_eligible = true;
                                if let Some(host) = &self.host {
                                    let mut child = ui.new_child(
                                        egui::UiBuilder::new()
                                            .max_rect(content)
                                            .id_salt(("win-content", win_id)),
                                    );
                                    if !interactive {
                                        child.disable(); // 工具栏只读，webview 内容照常显示
                                    }
                                    self.browser.show(&mut child, host, content);
                                }
                            } else if alpha_hint {
                                BrowserApp::show_placeholder_text(ui, content, 1.0);
                            }
                        }
                        "maps" => {
                            if webview_visible {
                                maps_eligible = true;
                                if let Some(host) = &self.host {
                                    let mut child = ui.new_child(
                                        egui::UiBuilder::new()
                                            .max_rect(content)
                                            .id_salt(("win-content", win_id)),
                                    );
                                    if !interactive {
                                        child.disable();
                                    }
                                    self.maps_app.show(&mut child, host, content);
                                }
                            } else if alpha_hint {
                                MapsApp::draw_placeholder(&painter, content, 1.0);
                            }
                        }
                        "wechat" => {
                            if webview_visible {
                                wechat_eligible = true;
                                if let Some(host) = &self.host {
                                    let mut child = ui.new_child(
                                        egui::UiBuilder::new()
                                            .max_rect(content)
                                            .id_salt(("win-content", win_id)),
                                    );
                                    if !interactive {
                                        child.disable();
                                    }
                                    self.wechat_app.show(&mut child, host, content);
                                }
                            } else if alpha_hint {
                                WeChatApp::draw_placeholder(&painter, content, 1.0);
                            }
                        }
                        "codex" if steady => {
                            let mut child = child_ui(ui);
                            self.agent_codex.show(&mut child, now);
                        }
                        "claude" if steady => {
                            let mut child = child_ui(ui);
                            self.agent_claude.show(&mut child, now);
                        }
                        "terminal" if steady => {
                            let mut child = child_ui(ui);
                            self.terminal.show(&mut child);
                        }
                        "trash" if steady => {
                            let mut child = child_ui(ui);
                            self.trash.show(&mut child);
                        }
                        "settings" if steady => {
                            let mut child = child_ui(ui);
                            ui::settings::show(&mut child, &mut self.cfg);
                        }
                        "finder" if steady => {
                            let mut child = child_ui(ui);
                            self.finder.show(&mut child);
                        }
                        "photos" if steady => {
                            let mut child = child_ui(ui);
                            self.photos.show(&mut child, now);
                        }
                        "music" if steady => {
                            let mut child = child_ui(ui);
                            self.music.show(&mut child);
                        }
                        "reminders" if steady => {
                            let mut child = child_ui(ui);
                            self.reminders.show(&mut child);
                        }
                        "mail" if steady => {
                            let mut child = child_ui(ui);
                            self.mail.show(&mut child);
                        }
                        _ => {}
                    }
                }

                // 链接点击（Agent 对话 / 任意 egui hyperlink）：拦下 egui 的 OpenUrl 命令，
                // 改在应用内 Chrome 浏览器打开，而不是弹系统浏览器。egui 0.34 把
                // open_url 并进了 PlatformOutput::commands，这里抽出 OpenUrl、保留其余命令。
                let opened_url = ctx.output_mut(|o| {
                    let mut url = None;
                    o.commands.retain(|cmd| {
                        if let egui::OutputCommand::OpenUrl(open) = cmd {
                            url = Some(open.url.clone());
                            false
                        } else {
                            true
                        }
                    });
                    url
                });
                if let Some(url) = opened_url {
                    if url.starts_with("http://") || url.starts_with("https://") {
                        self.open_or_focus("chrome", screen, pointer.unwrap_or(screen.center()), now);
                        self.browser.navigate(&url);
                    }
                }

                // webview 显隐与生命周期
                if self.wm.window_of_app("chrome").is_none() && self.browser.has_webview() {
                    self.browser.teardown();
                } else {
                    self.browser.set_visible(chrome_eligible);
                }
                if self.wm.window_of_app("maps").is_none() && self.maps_app.has_webview() {
                    self.maps_app.teardown();
                } else {
                    self.maps_app.set_visible(maps_eligible);
                }
                if self.wm.window_of_app("wechat").is_none() && self.wechat_app.has_webview() {
                    self.wechat_app.teardown();
                } else {
                    self.wechat_app.set_visible(wechat_eligible);
                }

                // 边缘吸附预览
                if let Some(target) = self.snap {
                    painter.rect_filled(target, 12, ui::white_a(0.12));
                    painter.rect_filled(target, 12, ui::black_a(0.08));
                    painter.rect_stroke(
                        target,
                        12,
                        egui::Stroke::new(1.5, ui::white_a(0.55)),
                        egui::StrokeKind::Inside,
                    );
                }

                // Launchpad 盖在窗口上、Dock 之下（macOS 中 Dock 始终可见）
                self.launchpad.show(ui, screen, now, self.cfg.wallpaper);

                let running: HashSet<&'static str> =
                    self.wm.windows.iter().map(|w| w.app_id).collect();
                dock::draw(&painter, &geom, &self.dock, &running, now, pointer);

                // Launchpad 打开时菜单栏淡出
                let mb_alpha = 1.0 - self.launchpad.progress(now);
                let has_window = self.wm.front_id().is_some();
                menubar::draw(
                    &painter,
                    screen,
                    self.front_app_name(),
                    mb_alpha,
                    &mut self.menu,
                    pointer,
                    has_window,
                    self.cfg.show_seconds,
                );
            });

        let animating = self.wm.animating()
            || self.launchpad.animating(now)
            || !self.dock.bounce.is_empty()
            || self.agent_codex.animating()
            || self.agent_claude.animating()
            || self.mail.animating()
            || self.terminal.running();
        if animating {
            ctx.request_repaint();
        } else if self.music.playing() {
            // 播放中：中频重绘，保证曲终能及时切歌
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        } else {
            // 菜单栏时钟需要更新，低频重绘即可
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }
}

/// 拖拽吸附判定：顶部 = 最大化，左右边 = 半屏，角落 = 四分之一屏
fn snap_target(pos: Pos2, screen: Rect, work: Rect) -> Option<Rect> {
    const EDGE: f32 = 14.0;
    const CORNER: f32 = 140.0;
    let cx = work.center().x;
    let cy = work.center().y;

    let left = pos.x <= screen.left() + EDGE;
    let right = pos.x >= screen.right() - EDGE;
    let top_zone = pos.y <= menubar::HEIGHT + 4.0;
    let near_top = pos.y <= work.top() + CORNER;
    let near_bottom = pos.y >= work.bottom() - CORNER;

    if left {
        return Some(if near_top {
            Rect::from_min_max(work.min, pos2(cx, cy))
        } else if near_bottom {
            Rect::from_min_max(pos2(work.left(), cy), pos2(cx, work.bottom()))
        } else {
            Rect::from_min_max(work.min, pos2(cx, work.bottom()))
        });
    }
    if right {
        return Some(if near_top {
            Rect::from_min_max(pos2(cx, work.top()), pos2(work.right(), cy))
        } else if near_bottom {
            Rect::from_min_max(pos2(cx, cy), work.max)
        } else {
            Rect::from_min_max(pos2(cx, work.top()), work.max)
        });
    }
    if top_zone {
        return Some(work);
    }
    None
}

fn edge_cursor(e: EdgeHit) -> CursorIcon {
    match (e.n, e.s, e.e, e.w) {
        (true, _, _, true) | (_, true, true, _) => CursorIcon::ResizeNwSe,
        (true, _, true, _) | (_, true, _, true) => CursorIcon::ResizeNeSw,
        (true, ..) | (_, true, ..) => CursorIcon::ResizeVertical,
        _ => CursorIcon::ResizeHorizontal,
    }
}

/// 加载 macOS 系统字体，让界面排版与真 macOS 一致：
/// 比例文本 = SF Pro（西文/数字）→ 苹方（中文）；等宽 = SF Mono → Menlo → 苹方。
/// Arial Unicode 殿后补冷门字形；任何文件读不到就顺位跳过，egui 默认字体保底。
/// 注意：MIRAGE_PLAIN_FONTS=1 可退回旧的 Arial Unicode 单回退（排查字体问题用）。
fn install_macos_fonts(ctx: &egui::Context) {
    if std::env::var("MIRAGE_PLAIN_FONTS").is_ok() {
        install_cjk_fallback(ctx);
        return;
    }
    // (字体键, 文件路径, 加入比例族, 加入等宽族, 基线下移系数)
    // 基线系数：egui 不对 fallback 字体做基线对齐，苹方的字形相对 SF 偏高，
    // 用 y_offset_factor 按字号比例下移到与 SF 同一基线（数值靠截图校准）。
    const PLAN: &[(&str, &str, bool, bool, f32)] = &[
        ("sf-pro", "/System/Library/Fonts/SFNS.ttf", true, false, 0.0),
        ("sf-mono", "/System/Library/Fonts/SFNSMono.ttf", false, true, 0.0),
        ("menlo", "/System/Library/Fonts/Menlo.ttc", false, true, 0.0),
        // 0.26 由实测校准：与真 macOS 菜单栏对比，使汉字相对数字基线
        // 上下对称地各超出 ~2 物理像素（见 README 自检一节的字体对齐验证）
        ("pingfang", "/System/Library/Fonts/PingFang.ttc", true, true, 0.26),
        (
            "hiragino-gb",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            true,
            true,
            0.26,
        ),
        (
            "arial-unicode",
            "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
            true,
            true,
            0.0,
        ),
    ];
    let mut fonts = egui::FontDefinitions::default();
    let mut prop_front: Vec<String> = Vec::new();
    let mut mono_front: Vec<String> = Vec::new();
    for (key, path, prop, mono, y_shift) in PLAN {
        let Ok(bytes) = std::fs::read(path) else { continue };
        let mut data = egui::FontData::from_owned(bytes);
        if *y_shift != 0.0 {
            data = data.tweak(egui::epaint::text::FontTweak {
                y_offset_factor: *y_shift,
                ..Default::default()
            });
        }
        fonts.font_data.insert(
            (*key).to_owned(),
            std::sync::Arc::new(data),
        );
        if *prop {
            prop_front.push((*key).to_owned());
        }
        if *mono {
            mono_front.push((*key).to_owned());
        }
    }
    if prop_front.is_empty() {
        // 系统字体一个都没读到（异常环境）：退回旧逻辑
        install_cjk_fallback(ctx);
        return;
    }
    // 系统字体排在最前，egui 默认字体保留在后面兜底
    let prop = fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default();
    for key in prop_front.iter().rev() {
        prop.insert(0, key.clone());
    }
    let mono = fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default();
    for key in mono_front.iter().rev() {
        mono.insert(0, key.clone());
    }
    ctx.set_fonts(fonts);
}

/// 旧逻辑：单个 CJK 回退字体（egui 默认字体不含 CJK）。
fn install_cjk_fallback(ctx: &egui::Context) {
    const CANDIDATES: &[&str] = &[
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
    ];
    for path in CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "cjk".to_owned(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts
                    .families
                    .entry(family)
                    .or_default()
                    .push("cjk".to_owned());
            }
            ctx.set_fonts(fonts);
            return;
        }
    }
}
