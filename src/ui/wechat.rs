//! 微信：第三个 wry webview 实例。
//!
//! 桌面版微信（/Applications/WeChat.app）是闭源独立进程，macOS 不支持把
//! 别的进程的 NSWindow reparent 进自己的窗口层级，无法「无缝嵌入」——
//! 所以走 WebView 方案：默认加载微信网页版（wx.qq.com，扫码登录）；
//! 部分账号被腾讯禁用网页版登录，提供「文件传输助手」标签兜底
//! （filehelper.weixin.qq.com，几乎所有账号可用，可收发消息/文件）。

use egui::{
    vec2, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, StrokeKind, Ui,
};
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{WebView, WebViewBuilder};

use super::browser::HostHandle;

pub const TOOLBAR_H: f32 = 38.0;

const WECHAT_GREEN: Color32 = Color32::from_rgb(0x07, 0xC1, 0x60);

/// 标签页：网页版微信 / 文件传输助手
const TABS: &[(&str, &str)] = &[
    ("微信网页版", "https://wx.qq.com/"),
    ("文件传输助手", "https://filehelper.weixin.qq.com/"),
];

/// 桌面 Safari UA：WKWebView 默认 UA 带 AppleWebKit 但缺 Safari 后缀，
/// 微信网页版会按「非常见浏览器」降级提示，补全成 Safari 即可。
const SAFARI_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15";

#[derive(Default)]
pub struct WeChatApp {
    webview: Option<WebView>,
    failed: Option<String>,
    visible: bool,
    tab: usize,
}

impl WeChatApp {
    fn ensure_webview(&mut self, host: &HostHandle, content: Rect) {
        if self.webview.is_some() || self.failed.is_some() {
            return;
        }
        let result = WebViewBuilder::new()
            .with_url(TABS[self.tab].1)
            .with_user_agent(SAFARI_UA)
            .with_bounds(wry::Rect {
                position: LogicalPosition::new(content.left() as f64, content.top() as f64)
                    .into(),
                size: LogicalSize::new(content.width() as f64, content.height() as f64).into(),
            })
            .build_as_child(host);
        match result {
            Ok(wv) => self.webview = Some(wv),
            Err(e) => self.failed = Some(format!("微信 WebView 创建失败：{e}")),
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

    /// 聚焦时调用：标签栏 + webview bounds 同步
    pub fn show(&mut self, ui: &mut Ui, host: &HostHandle, content: Rect) {
        let toolbar = Rect::from_min_size(content.min, vec2(content.width(), TOOLBAR_H));
        let page = Rect::from_min_max(toolbar.left_bottom(), content.max);

        self.ensure_webview(host, page);

        // ---- 标签栏（微信绿点缀的深色工具条） ----
        let p = ui.painter();
        p.rect_filled(toolbar, 0, Color32::from_rgb(0x26, 0x29, 0x27));
        p.line_segment(
            [toolbar.left_bottom(), toolbar.right_bottom()],
            Stroke::new(1.0, Color32::from_gray(18)),
        );

        let mut x = toolbar.left() + 10.0;
        for (i, (label, url)) in TABS.iter().enumerate() {
            let w = 110.0;
            let r = Rect::from_min_size(
                egui::pos2(x, toolbar.top() + 6.0),
                vec2(w, TOOLBAR_H - 12.0),
            );
            let resp = ui.interact(r, ui.id().with(("wx-tab", i)), Sense::click());
            let active = self.tab == i;
            if active {
                ui.painter()
                    .rect_filled(r, CornerRadius::same(13), WECHAT_GREEN.gamma_multiply(0.22));
                ui.painter().rect_stroke(
                    r,
                    CornerRadius::same(13),
                    Stroke::new(1.0, WECHAT_GREEN.gamma_multiply(0.8)),
                    StrokeKind::Inside,
                );
            } else if resp.hovered() {
                ui.painter()
                    .rect_filled(r, CornerRadius::same(13), Color32::from_gray(55));
            }
            ui.painter().text(
                r.center(),
                Align2::CENTER_CENTER,
                *label,
                FontId::proportional(12.5),
                if active {
                    Color32::from_rgb(0x9F, 0xEF, 0xC4)
                } else {
                    Color32::from_gray(190)
                },
            );
            if resp.clicked() && !active {
                self.tab = i;
                if let Some(wv) = &self.webview {
                    let _ = wv.load_url(url);
                }
            }
            x += w + 8.0;
        }

        // 刷新按钮（扫码超时后重新拉登录页用）
        let r = Rect::from_center_size(
            egui::pos2(toolbar.right() - 24.0, toolbar.center().y),
            vec2(24.0, 24.0),
        );
        let resp = ui.interact(r, ui.id().with("wx-reload"), Sense::click());
        if resp.hovered() {
            ui.painter()
                .rect_filled(r, CornerRadius::same(12), Color32::from_gray(55));
        }
        ui.painter().text(
            r.center(),
            Align2::CENTER_CENTER,
            "⟳",
            FontId::proportional(15.0),
            Color32::from_gray(200),
        );
        if resp.clicked() {
            if let Some(wv) = &self.webview {
                let _ = wv.load_url(TABS[self.tab].1);
            }
        }

        // ---- webview bounds 同步 ----
        if let Some(wv) = &self.webview {
            let _ = wv.set_bounds(wry::Rect {
                position: LogicalPosition::new(page.left() as f64, page.top() as f64).into(),
                size: LogicalSize::new(page.width() as f64, page.height() as f64).into(),
            });
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

    /// 未聚焦/被遮挡时的占位
    pub fn draw_placeholder(p: &egui::Painter, content: Rect, alpha: f32) {
        let toolbar = Rect::from_min_size(content.min, vec2(content.width(), TOOLBAR_H));
        p.rect_filled(
            toolbar,
            0,
            Color32::from_rgb(0x26, 0x29, 0x27).gamma_multiply(alpha),
        );
        p.rect_filled(
            Rect::from_min_max(toolbar.left_bottom(), content.max),
            0,
            Color32::from_rgb(0x16, 0x1B, 0x17).gamma_multiply(alpha),
        );
        p.text(
            content.center(),
            Align2::CENTER_CENTER,
            "微信被上方窗口遮挡，移开或置顶后继续显示",
            FontId::proportional(14.0),
            Color32::from_gray(120).gamma_multiply(alpha),
        );
    }
}
