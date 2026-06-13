//! 地图：第二个 wry webview 实例，加载 OpenStreetMap（无地址栏的专用容器）。
//! 与 Chrome 的 webview 并存，bounds 跟随各自窗口。

use egui::{vec2, Align2, Color32, FontId, Rect, Ui};
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{WebView, WebViewBuilder};

use super::browser::HostHandle;

const MAP_URL: &str = "https://www.openstreetmap.org/#map=12/31.2304/121.4737";

#[derive(Default)]
pub struct MapsApp {
    webview: Option<WebView>,
    failed: Option<String>,
    visible: bool,
}

impl MapsApp {
    fn ensure_webview(&mut self, host: &HostHandle, content: Rect) {
        if self.webview.is_some() || self.failed.is_some() {
            return;
        }
        let result = WebViewBuilder::new()
            .with_url(MAP_URL)
            .with_bounds(wry::Rect {
                position: LogicalPosition::new(content.left() as f64, content.top() as f64)
                    .into(),
                size: LogicalSize::new(content.width() as f64, content.height() as f64).into(),
            })
            .build_as_child(host);
        match result {
            Ok(wv) => self.webview = Some(wv),
            Err(e) => self.failed = Some(format!("地图 WebView 创建失败：{e}")),
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

    pub fn teardown(&mut self) {
        self.webview = None;
        self.visible = false;
    }

    pub fn has_webview(&self) -> bool {
        self.webview.is_some()
    }

    /// 聚焦时调用：创建并同步 webview bounds（占满整个内容区）
    pub fn show(&mut self, ui: &mut Ui, host: &HostHandle, content: Rect) {
        self.ensure_webview(host, content);
        if let Some(wv) = &self.webview {
            let _ = wv.set_bounds(wry::Rect {
                position: LogicalPosition::new(content.left() as f64, content.top() as f64)
                    .into(),
                size: LogicalSize::new(content.width() as f64, content.height() as f64).into(),
            });
        } else if let Some(err) = &self.failed {
            ui.painter().text(
                content.center(),
                Align2::CENTER_CENTER,
                err,
                FontId::proportional(13.0),
                Color32::from_rgb(0xE5, 0x6B, 0x6B),
            );
        }
    }

    /// 未聚焦占位
    pub fn draw_placeholder(p: &egui::Painter, content: Rect, alpha: f32) {
        p.rect_filled(
            content,
            0,
            Color32::from_rgb(0x1A, 0x22, 0x1A).gamma_multiply(alpha),
        );
        p.text(
            content.center(),
            Align2::CENTER_CENTER,
            "🗺  地图被上方窗口遮挡，移开或置顶后继续显示",
            FontId::proportional(14.0),
            Color32::from_gray(120).gamma_multiply(alpha),
        );
        let _ = vec2(0.0, 0.0);
    }
}
