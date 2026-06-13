//! 窗口外观与几何：阴影、圆角、标题栏、红绿灯按钮、壳内容。
//! 同时负责把窗口动画状态换算成「实际绘制的 rect + 透明度」。

use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Stroke, StrokeKind,
    Vec2,
};

use super::{black_a, icon, white_a};
use crate::anim::{
    ease_in_cubic, ease_in_out_cubic, ease_out_back, ease_out_cubic, lerp, lerp_rect, smoothstep,
};
use crate::apps;
use crate::wm::{WindowAnim, WindowState};

pub const TITLEBAR_H: f32 = 36.0;
pub const MIN_SIZE: Vec2 = vec2(380.0, 260.0);
const RADIUS: u8 = 11;

/// 红绿灯
const RED: Color32 = Color32::from_rgb(0xFF, 0x5F, 0x57);
const YELLOW: Color32 = Color32::from_rgb(0xFE, 0xBC, 0x2E);
const GREEN: Color32 = Color32::from_rgb(0x28, 0xC8, 0x40);
const DIM: Color32 = Color32::from_rgb(0x4E, 0x4E, 0x54);

pub fn titlebar_rect(rect: Rect) -> Rect {
    Rect::from_min_size(rect.min, vec2(rect.width(), TITLEBAR_H))
}

/// 这些应用的窗口内容由专门模块渲染（webview / agent 聊天），不画通用壳
pub fn has_custom_content(app_id: &str) -> bool {
    matches!(
        app_id,
        "chrome"
            | "codex"
            | "claude"
            | "terminal"
            | "trash"
            | "settings"
            | "finder"
            | "maps"
            | "reminders"
            | "photos"
            | "music"
            | "mail"
            | "wechat"
    )
}

pub fn content_rect(rect: Rect) -> Rect {
    Rect::from_min_max(pos2(rect.left(), rect.top() + TITLEBAR_H), rect.max)
}

pub fn traffic_centers(rect: Rect) -> [Pos2; 3] {
    let cy = rect.top() + TITLEBAR_H / 2.0;
    [
        pos2(rect.left() + 20.0, cy),
        pos2(rect.left() + 40.0, cy),
        pos2(rect.left() + 60.0, cy),
    ]
}

/// 命中红绿灯：返回 0=关闭 1=最小化 2=最大化
pub fn traffic_hit(rect: Rect, pos: Pos2) -> Option<usize> {
    traffic_centers(rect)
        .iter()
        .position(|c| c.distance(pos) <= 8.0)
}

/// 把动画状态换算为绘制 rect 与透明度；None 表示完全不可见
pub fn effective_rect(win: &WindowState, now: f64) -> Option<(Rect, f32)> {
    match &win.anim {
        WindowAnim::None => {
            if win.minimized {
                None
            } else {
                Some((win.rect, 1.0))
            }
        }
        WindowAnim::Opening(t) => {
            let k = t.progress(now);
            let s = 0.82 + 0.18 * ease_out_back(k);
            Some((scale_about_center(win.rect, s), ease_out_cubic(k)))
        }
        WindowAnim::Closing(t) => {
            let k = ease_in_cubic(t.progress(now));
            Some((scale_about_center(win.rect, 1.0 - 0.15 * k), 1.0 - k))
        }
        // genie 动画走专用绘制路径，不在这里处理
        WindowAnim::Minimizing { .. } | WindowAnim::Restoring { .. } => None,
        WindowAnim::Morph { t, from } => {
            let k = ease_in_out_cubic(t.progress(now));
            Some((lerp_rect(*from, win.rect, k), 1.0))
        }
    }
}

pub fn draw_window(p: &Painter, win: &WindowState, focused: bool, now: f64, hover: Option<Pos2>) {
    // genie：最小化吸入 / 恢复吐出
    match &win.anim {
        WindowAnim::Minimizing { t, to } => {
            draw_genie(p, win, t.progress(now), *to);
            return;
        }
        WindowAnim::Restoring { t, from } => {
            draw_genie(p, win, 1.0 - t.progress(now), *from);
            return;
        }
        _ => {}
    }

    let Some((rect, alpha)) = effective_rect(win, now) else {
        return;
    };
    let cr = CornerRadius::same(RADIUS);

    // 多层柔和投影，聚焦窗口阴影更深更大
    let (layers, base_a) = if focused { (6, 0.16) } else { (4, 0.10) };
    for i in 0..layers {
        let f = i as f32;
        p.rect_filled(
            rect.expand(1.5 + f * 2.6).translate(vec2(0.0, 2.0 + f * 1.2)),
            CornerRadius::same(RADIUS + 2 + i as u8 * 2),
            black_a(base_a * alpha / (f + 1.5)),
        );
    }

    // 窗体
    p.rect_filled(rect, cr, Color32::from_rgb(0x20, 0x20, 0x25).gamma_multiply(alpha * 0.99));

    // 标题栏
    let tb = titlebar_rect(rect);
    let tb_fill = if focused {
        Color32::from_rgb(0x2E, 0x2E, 0x34)
    } else {
        Color32::from_rgb(0x26, 0x26, 0x2B)
    };
    p.rect_filled(
        tb,
        CornerRadius {
            nw: RADIUS,
            ne: RADIUS,
            sw: 0,
            se: 0,
        },
        tb_fill.gamma_multiply(alpha),
    );
    p.line_segment(
        [tb.left_bottom(), tb.right_bottom()],
        Stroke::new(1.0, black_a(0.5 * alpha)),
    );

    // 红绿灯：聚焦时彩色，失焦灰色；指针悬停在按钮区时显示符号
    let centers = traffic_centers(rect);
    let hover_zone = hover.is_some_and(|h| {
        tb.contains(h) && h.x <= rect.left() + 76.0 && alpha > 0.9 && !win.minimized
    });
    let colors = if focused || hover_zone {
        [RED, YELLOW, GREEN]
    } else {
        [DIM, DIM, DIM]
    };
    for (i, (&c, col)) in centers.iter().zip(colors).enumerate() {
        p.circle_filled(c, 6.2, col.gamma_multiply(alpha));
        p.circle_stroke(c, 6.2, Stroke::new(0.5, black_a(0.18 * alpha)));
        if hover_zone {
            draw_traffic_glyph(p, i, c, alpha);
        }
    }

    // 标题
    p.text(
        tb.center(),
        Align2::CENTER_CENTER,
        &win.title,
        FontId::proportional(13.0),
        white_a(if focused { 0.85 } else { 0.42 } * alpha),
    );

    // 内容区
    let content = Rect::from_min_max(pos2(rect.left(), tb.bottom()), rect.max);
    if has_custom_content(win.app_id) {
        // chrome/codex 自带真实内容，这里只铺内容底色（聚焦时由各自模块渲染）
        p.rect_filled(
            content,
            CornerRadius {
                nw: 0,
                ne: 0,
                sw: RADIUS,
                se: RADIUS,
            },
            Color32::from_rgb(0x1B, 0x1B, 0x1F).gamma_multiply(alpha),
        );
    } else if content.height() > 130.0 {
        let app = apps::get(win.app_id);
        let c = content.center();
        icon::draw_app_icon(p, c - vec2(0.0, 34.0), 64.0, app, alpha);
        p.text(
            c + vec2(0.0, 22.0),
            Align2::CENTER_CENTER,
            app.name,
            FontId::proportional(17.0),
            white_a(0.88 * alpha),
        );
        p.text(
            c + vec2(0.0, 46.0),
            Align2::CENTER_CENTER,
            "这只是一个应用壳 · 内容待接入",
            FontId::proportional(12.5),
            white_a(0.36 * alpha),
        );
    }

    // 外描边
    p.rect_stroke(rect, cr, Stroke::new(1.0, white_a(0.12 * alpha)), StrokeKind::Inside);
}

fn draw_traffic_glyph(p: &Painter, idx: usize, c: Pos2, alpha: f32) {
    let g = black_a(0.55 * alpha);
    let s = Stroke::new(1.4, g);
    match idx {
        // ×
        0 => {
            let d = 2.6;
            p.line_segment([pos2(c.x - d, c.y - d), pos2(c.x + d, c.y + d)], s);
            p.line_segment([pos2(c.x - d, c.y + d), pos2(c.x + d, c.y - d)], s);
        }
        // −
        1 => {
            p.line_segment([pos2(c.x - 3.0, c.y), pos2(c.x + 3.0, c.y)], s);
        }
        // zoom：左上 + 右下两个角三角，中间留斜缝（同 macOS 全屏符号 ⤡）
        _ => {
            let r = 3.1; // 三角直角边到中心的距离
            let cut = 1.0; // 斜缝半宽
            p.add(egui::Shape::convex_polygon(
                vec![
                    pos2(c.x - r, c.y - r),
                    pos2(c.x + r - 2.0 * cut, c.y - r),
                    pos2(c.x - r, c.y + r - 2.0 * cut),
                ],
                g,
                Stroke::NONE,
            ));
            p.add(egui::Shape::convex_polygon(
                vec![
                    pos2(c.x + r, c.y + r),
                    pos2(c.x - r + 2.0 * cut, c.y + r),
                    pos2(c.x + r, c.y - r + 2.0 * cut),
                ],
                g,
                Stroke::NONE,
            ));
        }
    }
}

fn scale_about_center(rect: Rect, s: f32) -> Rect {
    Rect::from_center_size(rect.center(), rect.size() * s)
}

/// genie 曲面变形：把窗口切成横向切片，左右边缘沿曲线汇聚到 Dock 图标。
/// k=0 完整窗口，k=1 完全吸入 Dock。双相位：先弯折（bend）再下坠（drop）。
fn draw_genie(p: &Painter, win: &WindowState, k: f32, anchor: Pos2) {
    let from = win.rect;
    let to = Rect::from_center_size(anchor, vec2(54.0, 54.0));

    let bend = smoothstep((k / 0.5).min(1.0));
    let drop = smoothstep(((k - 0.30) / 0.70).clamp(0.0, 1.0));
    let alpha = 1.0 - smoothstep(((k - 0.72) / 0.28).clamp(0.0, 1.0)) * 0.75;

    let top_y = lerp(from.top(), to.top(), drop);
    let bot_y = lerp(from.bottom(), to.bottom(), smoothstep((k * 1.7).min(1.0)));

    let body = Color32::from_rgb(0x20, 0x20, 0x25);
    let tbar = Color32::from_rgb(0x2E, 0x2E, 0x34);
    let titlebar_frac = (TITLEBAR_H / from.height()).min(0.3);

    const ROWS: usize = 28;
    let mut mesh = egui::Mesh::default();
    for r in 0..=ROWS {
        let f = r as f32 / ROWS as f32;
        let y = lerp(top_y, bot_y, f);
        // 越靠下的切片越早被拽向图标；drop 相位把整体收口
        let s = (bend * f.powf(1.7) + drop).clamp(0.0, 1.0);
        let left = lerp(from.left(), to.left(), s);
        let right = lerp(from.right(), to.right(), s);
        let col = if f < titlebar_frac { tbar } else { body };
        let col = col.gamma_multiply(alpha * 0.97);
        mesh.colored_vertex(pos2(left, y), col);
        mesh.colored_vertex(pos2(right, y), col);
        if r > 0 {
            let i = (r as u32 - 1) * 2;
            mesh.add_triangle(i, i + 1, i + 2);
            mesh.add_triangle(i + 1, i + 3, i + 2);
        }
    }
    p.add(egui::Shape::mesh(mesh));

    // 应用图标随气流飞向 Dock，让吸入更有方向感
    let app = apps::get(win.app_id);
    let icon_center = pos2(
        lerp(from.center().x, to.center().x, smoothstep(k)),
        lerp(from.center().y, to.center().y, ease_in_cubic(k)),
    );
    let icon_size = lerp(64.0, 40.0, k);
    icon::draw_app_icon(p, icon_center, icon_size, app, alpha);
}
