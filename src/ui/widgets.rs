//! 苹果风格基础控件库（滑杆 / 开关）。
//!
//! 调研结论（2026-06）：egui 生态没有现成的 macOS 控件库——
//! egui-desktop 只做窗口 chrome/菜单（egui 0.33、alpha），第三方主题库
//! （catppuccin 等）只是配色；真正嵌入 NSSlider/NSScroller 原生视图则有
//! 「原生子视图永远浮在 egui 画面之上」的层级问题（与 webview 同款限制），
//! 不适合窗口内控件。因此对照 macOS System Settings 自建这两个控件，
//! 滚动条用 egui 内建的 `ScrollStyle::floating`（即 macOS 覆盖式形态）。

use egui::{pos2, vec2, Color32, Rect, Response, Sense, Stroke, StrokeKind, Ui};

/// macOS 系统蓝（深色模式 accent）
const ACCENT: Color32 = Color32::from_rgb(0x0A, 0x84, 0xFF);
/// 轨道底色（深色模式的灰轨）
const TRACK: Color32 = Color32::from_rgb(0x46, 0x46, 0x4C);
/// 旋钮白
const KNOB: Color32 = Color32::from_rgb(0xE6, 0xE6, 0xEA);

/// macOS 风格滑杆：4px 圆角细轨 + 已走过段填充强调蓝 + 白色圆钮。
/// 占据 `width x 20` 的布局空间，值变化时 `Response::changed()` 为真。
pub fn slider(ui: &mut Ui, value: &mut f32, range: std::ops::RangeInclusive<f32>, width: f32) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, 20.0), Sense::click_and_drag());
    let (lo, hi) = (*range.start(), *range.end());
    // 轨道两端为旋钮半径留位，钮心不越界
    let r_knob = 8.0;
    let track = Rect::from_min_max(
        pos2(rect.left() + r_knob, rect.center().y - 2.0),
        pos2(rect.right() - r_knob, rect.center().y + 2.0),
    );
    if resp.dragged() || resp.clicked() || resp.is_pointer_button_down_on() {
        if let Some(p) = resp.interact_pointer_pos() {
            let t = ((p.x - track.left()) / track.width()).clamp(0.0, 1.0);
            let new = lo + t * (hi - lo);
            if new != *value {
                *value = new;
                resp.mark_changed();
            }
        }
    }
    let t = ((*value - lo) / (hi - lo)).clamp(0.0, 1.0);
    let knob_x = track.left() + t * track.width();

    let p = ui.painter();
    p.rect_filled(track, 2, TRACK);
    p.rect_filled(
        Rect::from_min_max(track.left_top(), pos2(knob_x, track.bottom())),
        2,
        ACCENT,
    );
    let knob_c = pos2(knob_x, track.center().y);
    // 旋钮落影 + 白钮 + 轮廓（macOS 的钮按下时几乎不变色，仅有轮廓感）
    p.circle_filled(knob_c + vec2(0.0, 0.6), r_knob, Color32::from_black_alpha(70));
    p.circle_filled(knob_c, r_knob, KNOB);
    p.circle_stroke(knob_c, r_knob, Stroke::new(0.5, Color32::from_black_alpha(90)));
    resp
}

/// macOS 风格开关（System Settings 同款）：38x22 胶囊，开=蓝底，
/// 旋钮带 macOS 同款的滑动动画。点击切换，切换时 `changed()` 为真。
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(38.0, 22.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    // 0..1 的开合动画进度（egui 内建的 animate_bool，~0.1s 缓动）
    let t = ui.ctx().animate_bool(resp.id, *on);
    let fill = lerp_color(TRACK, ACCENT, t);

    let p = ui.painter();
    p.rect_filled(rect, 11, fill);
    p.rect_stroke(
        rect,
        11,
        Stroke::new(1.0, Color32::from_black_alpha(40)),
        StrokeKind::Inside,
    );
    let r_knob = 9.0;
    let knob_x = egui::lerp(
        (rect.left() + 2.0 + r_knob)..=(rect.right() - 2.0 - r_knob),
        t,
    );
    let knob_c = pos2(knob_x, rect.center().y);
    p.circle_filled(knob_c + vec2(0.0, 0.6), r_knob, Color32::from_black_alpha(60));
    p.circle_filled(knob_c, r_knob, Color32::WHITE);
    resp
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}
