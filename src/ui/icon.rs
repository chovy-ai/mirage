//! 应用图标：优先使用 macOS 系统的真实 .icns（访达/邮件/音乐/照片/地图/终端/
//! 设置/Chrome/微信…），废纸篓用 Dock 自己的素材并区分空/满。系统图标缺失时
//! 回退到程序化精绘（squircle + 专属图形）。日历始终程序化——真 macOS 的 Dock
//! 日历图标就是动态显示当天日期的，静态 icns 反而不忠实。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use chrono::{Datelike, Local};
use egui::{
    pos2, vec2, Color32, ColorImage, CornerRadius, FontId, Painter, Pos2, Rect, Stroke,
    StrokeKind, TextureHandle, TextureOptions,
};

use super::{black_a, gradient_rect, white_a};
use crate::apps::AppInfo;

// ---------- 系统真实图标（.icns / Dock png 素材） ----------

fn system_icon_path(id: &str) -> Option<&'static str> {
    Some(match id {
        "finder" => "/System/Library/CoreServices/Finder.app/Contents/Resources/Finder.icns",
        "chrome" => "/Applications/Google Chrome.app/Contents/Resources/app.icns",
        "wechat" => "/Applications/WeChat.app/Contents/Resources/AppIcon.icns",
        "mail" => "/System/Applications/Mail.app/Contents/Resources/ApplicationIcon.icns",
        "music" => "/System/Applications/Music.app/Contents/Resources/AppIcon.icns",
        "photos" => "/System/Applications/Photos.app/Contents/Resources/AppIcon.icns",
        "notes" => "/System/Applications/Notes.app/Contents/Resources/AppIcon.icns",
        "reminders" => "/System/Applications/Reminders.app/Contents/Resources/AppIcon.icns",
        "maps" => "/System/Applications/Maps.app/Contents/Resources/AppIcon.icns",
        "terminal" => {
            "/System/Applications/Utilities/Terminal.app/Contents/Resources/Terminal.icns"
        }
        "settings" => {
            "/System/Applications/System Settings.app/Contents/Resources/SystemSettings.icns"
        }
        _ => return None,
    })
}

/// 解析 .icns 容器，取出最接近 256px 的内嵌 PNG。
/// 容器格式：8 字节头（"icns" + 总长），随后是 4 字节类型 + 4 字节长度的块序列。
fn decode_icns(bytes: &[u8]) -> Option<image::DynamicImage> {
    if bytes.len() < 8 || &bytes[0..4] != b"icns" {
        return None;
    }
    const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut best: Option<(i64, &[u8])> = None; // (与 256 的距离, png 数据)
    let mut off = 8usize;
    while off + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[off + 4..off + 8].try_into().ok()?) as usize;
        if len < 8 || off + len > bytes.len() {
            break;
        }
        let data = &bytes[off + 8..off + len];
        if data.len() > 24 && data[..8] == PNG_MAGIC {
            // IHDR 宽度在偏移 16..20
            let w = u32::from_be_bytes(data[16..20].try_into().ok()?) as i64;
            let score = (w - 256).abs();
            if best.is_none_or(|(s, _)| score < s) {
                best = Some((score, data));
            }
        }
        off += len;
    }
    best.and_then(|(_, png)| image::load_from_memory(png).ok())
}

fn load_system_icon(ctx: &egui::Context, id: &str) -> Option<TextureHandle> {
    let img = if id == "trash" {
        // Dock 的废纸篓素材，按当前 ~/.Trash 是否为空选图（macOS 同款行为）
        let home = std::env::var("HOME").ok()?;
        let empty = std::fs::read_dir(format!("{home}/.Trash"))
            .map(|rd| {
                !rd.flatten()
                    .any(|e| !e.file_name().to_string_lossy().starts_with('.'))
            })
            .unwrap_or(true);
        let name = if empty { "trashempty@2x.png" } else { "trashfull@2x.png" };
        image::open(format!(
            "/System/Library/CoreServices/Dock.app/Contents/Resources/{name}"
        ))
        .ok()?
    } else {
        decode_icns(&std::fs::read(system_icon_path(id)?).ok()?)?
    };
    let img = img.thumbnail(256, 256).to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let ci = ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    Some(ctx.load_texture(format!("sys-icon-{id}"), ci, TextureOptions::LINEAR))
}

/// 进程级纹理缓存：每个 app 只解码/上传一次，失败也记住（不反复读盘）。
fn system_icon(ctx: &egui::Context, id: &'static str) -> Option<TextureHandle> {
    static CACHE: OnceLock<Mutex<HashMap<&'static str, Option<TextureHandle>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = cache.lock().unwrap();
    if let Some(t) = map.get(id) {
        return t.clone();
    }
    let loaded = load_system_icon(ctx, id);
    map.insert(id, loaded.clone());
    loaded
}

// ---------- 通用小工具 ----------

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// 竖直渐变的圆角方块：上下圆角带用近似纯色，中段用顶点着色 Mesh，
/// 小尺寸下视觉完全连续。
fn gradient_squircle(p: &Painter, rect: Rect, radius: f32, top: Color32, bottom: Color32) {
    let r = radius.min(rect.height() / 2.0).min(rect.width() / 2.0);
    let h = rect.height();
    let t_band = (r / h).clamp(0.0, 0.5);
    let ru = r as u8;
    // 顶带
    p.rect_filled(
        Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + r)),
        CornerRadius { nw: ru, ne: ru, sw: 0, se: 0 },
        lerp_color(top, bottom, t_band * 0.5),
    );
    // 中段渐变
    gradient_rect(
        p,
        Rect::from_min_max(
            pos2(rect.left(), rect.top() + r),
            pos2(rect.right(), rect.bottom() - r),
        ),
        lerp_color(top, bottom, t_band),
        lerp_color(top, bottom, 1.0 - t_band),
    );
    // 底带
    p.rect_filled(
        Rect::from_min_max(pos2(rect.left(), rect.bottom() - r), rect.max),
        CornerRadius { nw: 0, ne: 0, sw: ru, se: ru },
        lerp_color(top, bottom, 1.0 - t_band * 0.5),
    );
}

fn rot(c: Pos2, pt: Pos2, a: f32) -> Pos2 {
    let (s, cs) = a.sin_cos();
    pos2(
        c.x + (pt.x - c.x) * cs - (pt.y - c.y) * s,
        c.y + (pt.x - c.x) * s + (pt.y - c.y) * cs,
    )
}

/// 扇形（采样成凸多边形）
fn sector(p: &Painter, center: Pos2, r: f32, a0: f32, a1: f32, color: Color32) {
    let n = 24;
    let mut pts = Vec::with_capacity(n + 2);
    pts.push(center);
    for i in 0..=n {
        let a = a0 + (a1 - a0) * i as f32 / n as f32;
        pts.push(center + vec2(a.cos(), a.sin()) * r);
    }
    p.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
}

/// 圆弧折线
fn arc(p: &Painter, center: Pos2, r: f32, a0: f32, a1: f32, stroke: Stroke) {
    let n = 24;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / n as f32;
            center + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    p.add(egui::Shape::line(pts, stroke));
}

/// 旋转椭圆（花瓣）
fn petal(p: &Painter, center: Pos2, rx: f32, ry: f32, angle: f32, color: Color32) {
    let n = 24;
    let pts: Vec<Pos2> = (0..n)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / n as f32;
            rot(center, center + vec2(a.cos() * rx, a.sin() * ry), angle)
        })
        .collect();
    p.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
}

// ---------- 主入口 ----------

pub fn draw_app_icon(p: &Painter, center: Pos2, size: f32, app: &AppInfo, alpha: f32) {
    // 优先用系统真实图标。.icns 的方圆区约占画布 82% 并自带投影留白，
    // 放大 1.22 补偿，使其与程序化全幅图标视觉等大。
    if let Some(tex) = system_icon(p.ctx(), app.id) {
        let draw = Rect::from_center_size(center, vec2(size, size) * 1.22);
        p.image(
            tex.id(),
            draw,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE.gamma_multiply(alpha),
        );
        return;
    }

    let rect = Rect::from_center_size(center, vec2(size, size));
    let radius = size * 0.225;
    let cr = CornerRadius::same(radius as u8);

    // 投影
    p.rect_filled(
        rect.translate(vec2(0.0, size * 0.045)),
        cr,
        black_a(0.26 * alpha),
    );

    // 专属图形（在不透明层上绘制，最后统一处理 alpha 不现实，
    // 这里 alpha 直接乘进每个颜色——子函数约定接收 a 系数）
    let a = alpha;
    match app.id {
        "finder" => finder(p, rect, radius, a),
        "chrome" => chrome_icon(p, rect, radius, a),
        "codex" => codex_icon(p, rect, radius, a),
        "claude" => claude_icon(p, rect, radius, a),
        "wechat" => wechat_icon(p, rect, radius, a),
        "mail" => mail(p, rect, radius, a),
        "music" => music(p, rect, radius, a),
        "photos" => photos(p, rect, radius, a),
        "notes" => notes(p, rect, radius, a),
        "calendar" => calendar(p, rect, radius, a),
        "reminders" => reminders(p, rect, radius, a),
        "maps" => maps(p, rect, radius, a),
        "terminal" => terminal(p, rect, radius, a),
        "settings" => settings_icon(p, rect, radius, a),
        "trash" => trash_icon(p, rect, radius, a),
        _ => {
            gradient_squircle(
                p,
                rect,
                radius,
                app.color.gamma_multiply(a),
                lerp_color(app.color, Color32::BLACK, 0.25).gamma_multiply(a),
            );
            p.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                app.glyph,
                FontId::proportional(size * 0.46),
                app.glyph_color.gamma_multiply(a),
            );
        }
    }

    // 统一质感：顶部细高光 + 内描边
    p.line_segment(
        [
            pos2(rect.left() + radius, rect.top() + 1.0),
            pos2(rect.right() - radius, rect.top() + 1.0),
        ],
        Stroke::new(1.2, white_a(0.30 * a)),
    );
    p.rect_stroke(rect, cr, Stroke::new(1.0, white_a(0.14 * a)), StrokeKind::Inside);
}

// ---------- 各应用 ----------

fn finder(p: &Painter, r: Rect, radius: f32, a: f32) {
    let s = r.width();
    // 右半蓝渐变打底
    gradient_squircle(
        p,
        r,
        radius,
        Color32::from_rgb(0x2B, 0xA9, 0xF8).gamma_multiply(a),
        Color32::from_rgb(0x0B, 0x6F, 0xE0).gamma_multiply(a),
    );
    // 左半白脸
    let ru = radius as u8;
    p.rect_filled(
        Rect::from_min_max(r.min, pos2(r.center().x, r.bottom())),
        CornerRadius { nw: ru, sw: ru, ne: 0, se: 0 },
        Color32::from_rgb(0xF2, 0xF5, 0xF9).gamma_multiply(a),
    );
    let ink = Color32::from_rgb(0x14, 0x26, 0x40).gamma_multiply(a);
    // 眼睛：两条圆头竖线
    let eye = Stroke::new(s * 0.055, ink);
    p.line_segment(
        [pos2(r.left() + s * 0.32, r.top() + s * 0.30), pos2(r.left() + s * 0.32, r.top() + s * 0.45)],
        eye,
    );
    p.line_segment(
        [pos2(r.left() + s * 0.68, r.top() + s * 0.30), pos2(r.left() + s * 0.68, r.top() + s * 0.45)],
        eye,
    );
    // 笑脸（下半圆弧）
    arc(
        p,
        pos2(r.center().x, r.top() + s * 0.42),
        s * 0.30,
        0.35,
        std::f32::consts::PI - 0.35,
        Stroke::new(s * 0.05, ink),
    );
}

fn chrome_icon(p: &Painter, r: Rect, radius: f32, a: f32) {
    gradient_squircle(
        p,
        r,
        radius,
        Color32::from_rgb(0xFC, 0xFC, 0xFE).gamma_multiply(a),
        Color32::from_rgb(0xE4, 0xE6, 0xEC).gamma_multiply(a),
    );
    let c = r.center();
    let s = r.width();
    let rr = s * 0.30;
    use std::f32::consts::TAU;
    // 三色环（红顶、绿左下、黄右下）
    let red = Color32::from_rgb(0xEA, 0x43, 0x35).gamma_multiply(a);
    let green = Color32::from_rgb(0x34, 0xA8, 0x53).gamma_multiply(a);
    let yellow = Color32::from_rgb(0xFB, 0xBC, 0x05).gamma_multiply(a);
    sector(p, c, rr, TAU * 0.583, TAU * 0.917, red);
    sector(p, c, rr, TAU * 0.25, TAU * 0.583, green);
    sector(p, c, rr, -TAU * 0.083, TAU * 0.25, yellow);
    // 白圈 + 蓝心
    p.circle_filled(c, s * 0.155, Color32::from_rgb(0xFC, 0xFC, 0xFE).gamma_multiply(a));
    p.circle_filled(c, s * 0.115, Color32::from_rgb(0x42, 0x85, 0xF4).gamma_multiply(a));
}

fn codex_icon(p: &Painter, r: Rect, radius: f32, a: f32) {
    gradient_squircle(
        p,
        r,
        radius,
        Color32::from_rgb(0x24, 0x24, 0x2A).gamma_multiply(a),
        Color32::from_rgb(0x09, 0x09, 0x0B).gamma_multiply(a),
    );
    let c = r.center();
    let s = r.width();
    let w = white_a(0.95 * a);
    // 六向雪花
    for k in 0..6 {
        let ang = std::f32::consts::TAU * k as f32 / 6.0;
        let dir = vec2(ang.cos(), ang.sin());
        p.line_segment(
            [c + dir * s * 0.07, c + dir * s * 0.27],
            Stroke::new(s * 0.05, w),
        );
    }
    p.circle_filled(c, s * 0.045, w);
}

fn claude_icon(p: &Painter, r: Rect, radius: f32, a: f32) {
    gradient_squircle(
        p,
        r,
        radius,
        Color32::from_rgb(0xE8, 0x8A, 0x66).gamma_multiply(a),
        Color32::from_rgb(0xC2, 0x55, 0x32).gamma_multiply(a),
    );
    let c = r.center();
    let s = r.width();
    let w = white_a(0.96 * a);
    // Claude 星芒：12 条长短交替的圆头射线
    for k in 0..12 {
        let ang = std::f32::consts::TAU * k as f32 / 12.0;
        let dir = vec2(ang.cos(), ang.sin());
        let len = if k % 2 == 0 { 0.28 } else { 0.19 };
        p.line_segment(
            [c + dir * s * 0.06, c + dir * s * len],
            Stroke::new(s * 0.045, w),
        );
    }
}

fn wechat_icon(p: &Painter, r: Rect, radius: f32, a: f32) {
    gradient_squircle(
        p,
        r,
        radius,
        Color32::from_rgb(0x2F, 0xD1, 0x6E).gamma_multiply(a),
        Color32::from_rgb(0x06, 0xA6, 0x52).gamma_multiply(a),
    );
    let s = r.width();
    let w = white_a(0.96 * a);
    let dot = Color32::from_rgb(0x21, 0xB6, 0x5E).gamma_multiply(a);
    // 大气泡（左上）
    let c1 = r.center() + vec2(-s * 0.08, -s * 0.06);
    petal(p, c1, s * 0.21, s * 0.17, 0.0, w);
    p.add(egui::Shape::convex_polygon(
        vec![
            c1 + vec2(-s * 0.10, s * 0.12),
            c1 + vec2(-s * 0.02, s * 0.13),
            c1 + vec2(-s * 0.14, s * 0.22),
        ],
        w,
        Stroke::NONE,
    ));
    p.circle_filled(c1 + vec2(-s * 0.07, -s * 0.02), s * 0.025, dot);
    p.circle_filled(c1 + vec2(0.02 * s, -s * 0.02), s * 0.025, dot);
    // 小气泡（右下）
    let c2 = r.center() + vec2(s * 0.15, s * 0.12);
    petal(p, c2, s * 0.14, s * 0.115, 0.0, w);
    p.add(egui::Shape::convex_polygon(
        vec![
            c2 + vec2(s * 0.06, s * 0.08),
            c2 + vec2(s * 0.12, s * 0.16),
            c2 + vec2(s * 0.00, s * 0.10),
        ],
        w,
        Stroke::NONE,
    ));
    p.circle_filled(c2 + vec2(-s * 0.045, -s * 0.01), s * 0.02, dot);
    p.circle_filled(c2 + vec2(0.025 * s, -s * 0.01), s * 0.02, dot);
}

fn mail(p: &Painter, r: Rect, radius: f32, a: f32) {
    gradient_squircle(
        p,
        r,
        radius,
        Color32::from_rgb(0x33, 0xA5, 0xFF).gamma_multiply(a),
        Color32::from_rgb(0x0A, 0x60, 0xE8).gamma_multiply(a),
    );
    let s = r.width();
    let env = Rect::from_min_max(
        pos2(r.left() + s * 0.18, r.top() + s * 0.30),
        pos2(r.right() - s * 0.18, r.bottom() - s * 0.30),
    );
    p.rect_filled(env, CornerRadius::same((s * 0.035) as u8), white_a(0.96 * a));
    // 封盖 V 线
    let ink = Color32::from_rgb(0x0A, 0x55, 0xC8).gamma_multiply(a);
    let st = Stroke::new(s * 0.030, ink);
    let mid = pos2(env.center().x, env.top() + env.height() * 0.55);
    p.line_segment([env.left_top() + vec2(s * 0.012, s * 0.012), mid], st);
    p.line_segment([env.right_top() + vec2(-s * 0.012, s * 0.012), mid], st);
}

fn music(p: &Painter, r: Rect, radius: f32, a: f32) {
    gradient_squircle(
        p,
        r,
        radius,
        Color32::from_rgb(0xFB, 0x5C, 0x74).gamma_multiply(a),
        Color32::from_rgb(0xEF, 0x23, 0x44).gamma_multiply(a),
    );
    let s = r.width();
    let w = white_a(0.97 * a);
    let n1 = pos2(r.left() + s * 0.36, r.top() + s * 0.66);
    let n2 = pos2(r.left() + s * 0.64, r.top() + s * 0.62);
    // 音符头
    p.circle_filled(n1, s * 0.075, w);
    p.circle_filled(n2, s * 0.075, w);
    // 杆
    let st = Stroke::new(s * 0.045, w);
    let t1 = pos2(n1.x + s * 0.065, n1.y - s * 0.33);
    let t2 = pos2(n2.x + s * 0.065, n2.y - s * 0.33);
    p.line_segment([pos2(n1.x + s * 0.065, n1.y), t1], st);
    p.line_segment([pos2(n2.x + s * 0.065, n2.y), t2], st);
    // 横梁
    p.line_segment([t1, t2], Stroke::new(s * 0.065, w));
}

fn photos(p: &Painter, r: Rect, radius: f32, a: f32) {
    gradient_squircle(
        p,
        r,
        radius,
        Color32::from_rgb(0xFD, 0xFD, 0xFE).gamma_multiply(a),
        Color32::from_rgb(0xEC, 0xEC, 0xF2).gamma_multiply(a),
    );
    let c = r.center();
    let s = r.width();
    let colors = [
        Color32::from_rgb(0xFF, 0xCC, 0x00),
        Color32::from_rgb(0xFF, 0x95, 0x00),
        Color32::from_rgb(0xFF, 0x3B, 0x30),
        Color32::from_rgb(0xFF, 0x2D, 0x55),
        Color32::from_rgb(0xAF, 0x52, 0xDE),
        Color32::from_rgb(0x00, 0x7A, 0xFF),
        Color32::from_rgb(0x5A, 0xC8, 0xFA),
        Color32::from_rgb(0x34, 0xC7, 0x59),
    ];
    for (i, col) in colors.iter().enumerate() {
        let ang = std::f32::consts::TAU * i as f32 / 8.0;
        let pc = c + vec2(ang.cos(), ang.sin()) * s * 0.16;
        petal(
            p,
            pc,
            s * 0.155,
            s * 0.085,
            ang,
            col.gamma_multiply(0.78 * a),
        );
    }
}

fn notes(p: &Painter, r: Rect, radius: f32, a: f32) {
    let s = r.width();
    gradient_squircle(
        p,
        r,
        radius,
        Color32::from_rgb(0xFC, 0xFC, 0xF6).gamma_multiply(a),
        Color32::from_rgb(0xF0, 0xEF, 0xE4).gamma_multiply(a),
    );
    // 顶部黄条
    let ru = radius as u8;
    p.rect_filled(
        Rect::from_min_size(r.min, vec2(s, s * 0.24)),
        CornerRadius { nw: ru, ne: ru, sw: 0, se: 0 },
        Color32::from_rgb(0xFF, 0xD6, 0x0A).gamma_multiply(a),
    );
    // 横线
    let line = Color32::from_rgb(0xD2, 0xD0, 0xC2).gamma_multiply(a);
    for (i, y) in [0.46, 0.60, 0.74].iter().enumerate() {
        let wfrac = if i == 2 { 0.42 } else { 0.60 };
        p.line_segment(
            [
                pos2(r.left() + s * 0.20, r.top() + s * y),
                pos2(r.left() + s * (0.20 + wfrac), r.top() + s * y),
            ],
            Stroke::new(s * 0.035, line),
        );
    }
}

fn calendar(p: &Painter, r: Rect, radius: f32, a: f32) {
    let s = r.width();
    gradient_squircle(
        p,
        r,
        radius,
        Color32::from_rgb(0xFD, 0xFD, 0xFE).gamma_multiply(a),
        Color32::from_rgb(0xEE, 0xEE, 0xF4).gamma_multiply(a),
    );
    let ru = radius as u8;
    p.rect_filled(
        Rect::from_min_size(r.min, vec2(s, s * 0.27)),
        CornerRadius { nw: ru, ne: ru, sw: 0, se: 0 },
        Color32::from_rgb(0xFF, 0x3B, 0x30).gamma_multiply(a),
    );
    let now = Local::now();
    p.text(
        pos2(r.center().x, r.top() + s * 0.135),
        egui::Align2::CENTER_CENTER,
        format!("{}月", now.month()),
        FontId::proportional(s * 0.14),
        white_a(0.96 * a),
    );
    p.text(
        pos2(r.center().x, r.top() + s * 0.62),
        egui::Align2::CENTER_CENTER,
        format!("{}", now.day()),
        FontId::proportional(s * 0.42),
        Color32::from_rgb(0x36, 0x36, 0x3C).gamma_multiply(a),
    );
}

fn reminders(p: &Painter, r: Rect, radius: f32, a: f32) {
    let s = r.width();
    gradient_squircle(
        p,
        r,
        radius,
        Color32::from_rgb(0xFD, 0xFD, 0xFE).gamma_multiply(a),
        Color32::from_rgb(0xEC, 0xEC, 0xF2).gamma_multiply(a),
    );
    let dots = [
        Color32::from_rgb(0x00, 0x7A, 0xFF),
        Color32::from_rgb(0xFF, 0x9F, 0x0A),
        Color32::from_rgb(0xFF, 0x3B, 0x30),
    ];
    let line = Color32::from_rgb(0xD9, 0xD9, 0xDF).gamma_multiply(a);
    for (i, col) in dots.iter().enumerate() {
        let y = r.top() + s * (0.30 + 0.20 * i as f32);
        p.circle_filled(pos2(r.left() + s * 0.27, y), s * 0.05, col.gamma_multiply(a));
        p.line_segment(
            [pos2(r.left() + s * 0.40, y), pos2(r.left() + s * 0.76, y)],
            Stroke::new(s * 0.04, line),
        );
    }
}

fn maps(p: &Painter, r: Rect, radius: f32, a: f32) {
    let s = r.width();
    gradient_squircle(
        p,
        r,
        radius,
        Color32::from_rgb(0x96, 0xD8, 0x60).gamma_multiply(a),
        Color32::from_rgb(0x4F, 0xAE, 0x3E).gamma_multiply(a),
    );
    // 白色道路
    let road: Vec<Pos2> = [
        (0.16, 0.88),
        (0.40, 0.62),
        (0.36, 0.40),
        (0.58, 0.18),
    ]
    .iter()
    .map(|(x, y)| pos2(r.left() + s * x, r.top() + s * y))
    .collect();
    p.add(egui::Shape::line(road, Stroke::new(s * 0.09, white_a(0.92 * a))));
    // 红色 pin
    let pin = pos2(r.left() + s * 0.66, r.top() + s * 0.40);
    let red = Color32::from_rgb(0xFF, 0x3B, 0x30).gamma_multiply(a);
    p.add(egui::Shape::convex_polygon(
        vec![
            pos2(pin.x - s * 0.055, pin.y + s * 0.04),
            pos2(pin.x + s * 0.055, pin.y + s * 0.04),
            pos2(pin.x, pin.y + s * 0.20),
        ],
        red,
        Stroke::NONE,
    ));
    p.circle_filled(pin, s * 0.095, red);
    p.circle_filled(pin, s * 0.04, white_a(0.95 * a));
}

fn terminal(p: &Painter, r: Rect, radius: f32, a: f32) {
    gradient_squircle(
        p,
        r,
        radius,
        Color32::from_rgb(0x3A, 0x3A, 0x40).gamma_multiply(a),
        Color32::from_rgb(0x0F, 0x0F, 0x12).gamma_multiply(a),
    );
    let s = r.width();
    let w = white_a(0.95 * a);
    let st = Stroke::new(s * 0.05, w);
    // ">"
    p.line_segment(
        [pos2(r.left() + s * 0.22, r.top() + s * 0.30), pos2(r.left() + s * 0.36, r.top() + s * 0.41)],
        st,
    );
    p.line_segment(
        [pos2(r.left() + s * 0.36, r.top() + s * 0.41), pos2(r.left() + s * 0.22, r.top() + s * 0.52)],
        st,
    );
    // "_"
    p.line_segment(
        [pos2(r.left() + s * 0.44, r.top() + s * 0.54), pos2(r.left() + s * 0.64, r.top() + s * 0.54)],
        st,
    );
}

fn settings_icon(p: &Painter, r: Rect, radius: f32, a: f32) {
    gradient_squircle(
        p,
        r,
        radius,
        Color32::from_rgb(0xAC, 0xAC, 0xB4).gamma_multiply(a),
        Color32::from_rgb(0x6E, 0x6E, 0x78).gamma_multiply(a),
    );
    let c = r.center();
    let s = r.width();
    let metal = Color32::from_rgb(0xE9, 0xE9, 0xEE).gamma_multiply(a);
    // 8 齿（旋转梯形）
    for k in 0..8 {
        let ang = std::f32::consts::TAU * k as f32 / 8.0;
        let w1 = s * 0.052;
        let w2 = s * 0.034;
        let r1 = s * 0.20;
        let r2 = s * 0.335;
        let pts = vec![
            rot(c, c + vec2(r1, -w1), ang),
            rot(c, c + vec2(r2, -w2), ang),
            rot(c, c + vec2(r2, w2), ang),
            rot(c, c + vec2(r1, w1), ang),
        ];
        p.add(egui::Shape::convex_polygon(pts, metal, Stroke::NONE));
    }
    p.circle_filled(c, s * 0.245, metal);
    // 中孔（用底渐变中间色填）
    p.circle_filled(c, s * 0.115, Color32::from_rgb(0x86, 0x86, 0x90).gamma_multiply(a));
}

fn trash_icon(p: &Painter, r: Rect, radius: f32, a: f32) {
    gradient_squircle(
        p,
        r,
        radius,
        Color32::from_rgb(0xD4, 0xD5, 0xDB).gamma_multiply(a),
        Color32::from_rgb(0x9E, 0x9F, 0xA8).gamma_multiply(a),
    );
    let s = r.width();
    let ink = Color32::from_rgb(0x4E, 0x4F, 0x58).gamma_multiply(a);
    let st = Stroke::new(s * 0.035, ink);
    let cx = r.center().x;
    // 盖
    p.line_segment(
        [pos2(cx - s * 0.20, r.top() + s * 0.30), pos2(cx + s * 0.20, r.top() + s * 0.30)],
        Stroke::new(s * 0.04, ink),
    );
    // 提手
    arc(
        p,
        pos2(cx, r.top() + s * 0.30),
        s * 0.07,
        std::f32::consts::PI,
        std::f32::consts::TAU,
        st,
    );
    // 桶身（梯形）
    let top_w = s * 0.17;
    let bot_w = s * 0.135;
    let y0 = r.top() + s * 0.36;
    let y1 = r.top() + s * 0.74;
    p.add(egui::Shape::closed_line(
        vec![
            pos2(cx - top_w, y0),
            pos2(cx + top_w, y0),
            pos2(cx + bot_w, y1),
            pos2(cx - bot_w, y1),
        ],
        st,
    ));
    // 竖纹
    for dx in [-0.07_f32, 0.0, 0.07] {
        p.line_segment(
            [pos2(cx + s * dx, y0 + s * 0.06), pos2(cx + s * dx * 0.9, y1 - s * 0.06)],
            Stroke::new(s * 0.028, ink),
        );
    }
}

/// Launchpad 图标：深色渐变底 + 3x3 彩色圆点阵
pub fn draw_launchpad_icon(p: &Painter, center: Pos2, size: f32, alpha: f32) {
    let rect = Rect::from_center_size(center, vec2(size, size));
    let radius = size * 0.225;
    let cr = CornerRadius::same(radius as u8);
    p.rect_filled(
        rect.translate(vec2(0.0, size * 0.045)),
        cr,
        black_a(0.26 * alpha),
    );
    gradient_squircle(
        p,
        rect,
        radius,
        Color32::from_rgb(0x32, 0x32, 0x3A).gamma_multiply(alpha),
        Color32::from_rgb(0x18, 0x18, 0x1E).gamma_multiply(alpha),
    );
    p.rect_stroke(
        rect,
        cr,
        Stroke::new(1.0, white_a(0.14 * alpha)),
        StrokeKind::Inside,
    );

    let colors = [
        Color32::from_rgb(0xFF, 0x9F, 0x0A),
        Color32::from_rgb(0x32, 0xC7, 0x59),
        Color32::from_rgb(0x0A, 0x84, 0xFF),
        Color32::from_rgb(0xFF, 0x37, 0x5F),
        Color32::from_rgb(0xBF, 0x5A, 0xF2),
        Color32::from_rgb(0x64, 0xD2, 0xFF),
        Color32::from_rgb(0xFF, 0xD6, 0x0A),
        Color32::from_rgb(0xFF, 0x45, 0x3A),
        Color32::from_rgb(0x30, 0xD1, 0x58),
    ];
    let step = size * 0.24;
    let r = size * 0.075;
    for (i, c) in colors.iter().enumerate() {
        let dx = (i % 3) as f32 - 1.0;
        let dy = (i / 3) as f32 - 1.0;
        p.circle_filled(
            pos2(center.x + dx * step, center.y + dy * step),
            r,
            c.gamma_multiply(alpha),
        );
    }
}
