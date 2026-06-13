//! 壁纸：Big Sur 风格的多段竖直渐变 + 柔光，色板由 [`Wallpaper`] 主题决定。

use egui::{pos2, Color32, Painter, Rect};

use super::gradient_rect;
use crate::config::Wallpaper;

pub fn draw_wallpaper(p: &Painter, screen: Rect, wp: Wallpaper) {
    draw_wallpaper_alpha(p, screen, wp, 1.0);
}

/// 带透明度的壁纸重绘：Launchpad 用它把窗口「霜化」进背景
pub fn draw_wallpaper_alpha(p: &Painter, screen: Rect, wp: Wallpaper, alpha: f32) {
    for win in wp.stops().windows(2) {
        let (t0, c0) = win[0];
        let (t1, c1) = win[1];
        let band = Rect::from_min_max(
            pos2(screen.left(), screen.top() + screen.height() * t0),
            pos2(screen.right(), screen.top() + screen.height() * t1),
        );
        gradient_rect(p, band, c0.gamma_multiply(alpha), c1.gamma_multiply(alpha));
    }

    // 两团柔光（叠几层低透明度圆模拟径向渐变）
    let (warm, cool) = wp.glows();
    soft_glow(
        p,
        pos2(screen.right() - screen.width() * 0.22, screen.bottom() - screen.height() * 0.18),
        screen.width() * 0.30,
        warm,
        alpha,
    );
    soft_glow(
        p,
        pos2(screen.left() + screen.width() * 0.18, screen.top() + screen.height() * 0.30),
        screen.width() * 0.24,
        cool,
        alpha,
    );
}

fn soft_glow(p: &Painter, center: egui::Pos2, radius: f32, color: Color32, alpha: f32) {
    for i in 0..10 {
        let t = i as f32 / 10.0;
        p.circle_filled(
            center,
            radius * (1.0 - t),
            color.gamma_multiply((0.018 + t * 0.012) * alpha),
        );
    }
}
