//! Chrome 浏览器窗口：对标 nextop 的 Browser Node（Electron <webview>），
//! 这里用 wry（macOS 上是 WKWebView）作为 eframe 窗口的原生子视图，
//! 每帧把 bounds 同步到壳窗口的内容区。
//!
//! 原生视图永远浮在 egui 画面之上，所以只有当浏览器窗口是前台聚焦窗口时
//! 才显示 webview，其余时刻显示占位（保持桌面层级假象不破）。

use egui::{
    vec2, Align2, Color32, CornerRadius, FontId, Key, Rect, Stroke, StrokeKind, TextEdit, Ui,
};
use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{WebView, WebViewBuilder};

pub const TOOLBAR_H: f32 = 40.0;
const HOME_URL: &str = "https://www.bing.com";

/// 把 eframe 主窗口的 raw handle 包一层，便于 wry 挂子视图
pub struct HostHandle(pub RawWindowHandle);

impl HasWindowHandle for HostHandle {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        Ok(unsafe { WindowHandle::borrow_raw(self.0) })
    }
}

pub struct BrowserApp {
    webview: Option<WebView>,
    url_input: String,
    failed: Option<String>,
    visible: bool,
    /// webview 尚未创建时暂存目标地址（如从 Agent 链接点入），创建后即加载
    pending_url: Option<String>,
}

impl Default for BrowserApp {
    fn default() -> Self {
        Self {
            webview: None,
            url_input: HOME_URL.to_owned(),
            failed: None,
            visible: false,
            pending_url: None,
        }
    }
}

impl BrowserApp {
    fn ensure_webview(&mut self, host: &HostHandle, content: Rect) {
        if self.webview.is_some() || self.failed.is_some() {
            return;
        }
        let start_url = self.pending_url.take().unwrap_or_else(|| HOME_URL.to_owned());
        self.url_input = start_url.clone();
        let result = WebViewBuilder::new()
            .with_url(&start_url)
            .with_bounds(wry::Rect {
                position: LogicalPosition::new(content.left() as f64, content.top() as f64).into(),
                size: LogicalSize::new(content.width() as f64, content.height() as f64).into(),
            })
            .build_as_child(host);
        match result {
            Ok(wv) => self.webview = Some(wv),
            Err(e) => self.failed = Some(format!("WebView 创建失败：{e}")),
        }
    }

    /// 导航到指定地址：webview 已存在则立即加载，否则暂存到创建时加载。
    /// 供 Agent 对话内链接点击调用（在应用内浏览器打开，而非系统浏览器）。
    pub fn navigate(&mut self, url: &str) {
        self.url_input = url.to_owned();
        if let Some(wv) = &self.webview {
            let _ = wv.load_url(url);
            self.pending_url = None;
        } else {
            self.pending_url = Some(url.to_owned());
        }
    }

    pub fn set_visible(&mut self, visible: bool) {
        if self.visible != visible {
            self.visible = visible;
            if let Some(wv) = &self.webview {
                let _ = wv.set_visible(visible);
            }
        }
    }

    /// 浏览器窗口关闭时释放 webview
    pub fn teardown(&mut self) {
        self.webview = None;
        self.visible = false;
    }

    pub fn has_webview(&self) -> bool {
        self.webview.is_some()
    }

    /// 渲染工具栏 + 同步 webview bounds。content 是壳窗口的内容区（已聚焦时调用）。
    pub fn show(&mut self, ui: &mut Ui, host: &HostHandle, content: Rect) {
        let toolbar = Rect::from_min_size(content.min, vec2(content.width(), TOOLBAR_H));
        let page = Rect::from_min_max(toolbar.left_bottom(), content.max);

        self.ensure_webview(host, page);

        // ---- 工具栏（Chrome 风格：返回/前进/刷新 + 地址栏胶囊） ----
        let p = ui.painter();
        p.rect_filled(toolbar, 0, Color32::from_rgb(0x2A, 0x2A, 0x2E));
        p.line_segment(
            [toolbar.left_bottom(), toolbar.right_bottom()],
            Stroke::new(1.0, Color32::from_gray(20)),
        );

        let mut x = toolbar.left() + 10.0;
        for (glyph, script) in [
            ("←", "history.back()"),
            ("→", "history.forward()"),
            ("⟳", "location.reload()"),
        ] {
            let r = Rect::from_center_size(
                egui::pos2(x + 12.0, toolbar.center().y),
                vec2(24.0, 24.0),
            );
            let resp = ui.interact(
                r,
                ui.id().with(("nav", glyph)),
                egui::Sense::click(),
            );
            if resp.hovered() {
                ui.painter().rect_filled(r, CornerRadius::same(12), Color32::from_gray(60));
            }
            ui.painter().text(
                r.center(),
                Align2::CENTER_CENTER,
                glyph,
                FontId::proportional(15.0),
                Color32::from_gray(200),
            );
            if resp.clicked() {
                if let Some(wv) = &self.webview {
                    let _ = wv.evaluate_script(script);
                }
            }
            x += 28.0;
        }

        // 地址栏
        let url_rect = Rect::from_min_max(
            egui::pos2(x + 6.0, toolbar.top() + 7.0),
            egui::pos2(toolbar.right() - 12.0, toolbar.bottom() - 7.0),
        );
        ui.painter()
            .rect_filled(url_rect, CornerRadius::same(13), Color32::from_rgb(0x1C, 0x1C, 0x20));
        ui.painter().rect_stroke(
            url_rect,
            CornerRadius::same(13),
            Stroke::new(1.0, Color32::from_gray(55)),
            StrokeKind::Inside,
        );
        let te = TextEdit::singleline(&mut self.url_input)
            .frame(egui::Frame::NONE)
            .text_color(Color32::from_gray(215))
            .font(FontId::proportional(12.5));
        let resp = ui.put(url_rect.shrink2(vec2(12.0, 4.0)), te);
        if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            let mut url = self.url_input.trim().to_owned();
            if !url.is_empty() {
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    url = format!("https://{url}");
                }
                self.url_input = url.clone();
                if let Some(wv) = &self.webview {
                    let _ = wv.load_url(&url);
                }
            }
        }

        // ---- webview bounds 同步 ----
        if let Some(wv) = &self.webview {
            let _ = wv.set_bounds(wry::Rect {
                position: LogicalPosition::new(page.left() as f64, page.top() as f64).into(),
                size: LogicalSize::new(page.width() as f64, page.height() as f64).into(),
            });
            // 地址栏跟随导航（非聚焦输入时）
            if !resp.has_focus() {
                if let Ok(url) = wv.url() {
                    if !url.is_empty() && url != "about:blank" {
                        self.url_input = url;
                    }
                }
            }
        } else if let Some(err) = &self.failed {
            ui.painter().text(
                page.center(),
                Align2::CENTER_CENTER,
                err,
                FontId::proportional(13.0),
                Color32::from_rgb(0xE5, 0x6B, 0x6B),
            );
        }
    }

    /// 浏览器窗口未聚焦/动画中时的占位画面
    pub fn draw_placeholder(p: &egui::Painter, content: Rect, alpha: f32) {
        let toolbar = Rect::from_min_size(content.min, vec2(content.width(), TOOLBAR_H));
        p.rect_filled(
            toolbar,
            0,
            Color32::from_rgb(0x2A, 0x2A, 0x2E).gamma_multiply(alpha),
        );
        p.rect_filled(
            Rect::from_min_max(toolbar.left_bottom(), content.max),
            0,
            Color32::from_rgb(0x18, 0x18, 0x1B).gamma_multiply(alpha),
        );
        p.text(
            content.center(),
            Align2::CENTER_CENTER,
            "◎  网页被上方窗口遮挡，移开或置顶后继续显示",
            FontId::proportional(14.0),
            Color32::from_gray(120).gamma_multiply(alpha),
        );
    }

    pub fn show_placeholder_text(ui: &Ui, content: Rect, alpha: f32) {
        Self::draw_placeholder(ui.painter(), content, alpha);
    }
}
