//! 极简补间动画：所有动效共用一套基于 egui 时钟的 Tween + easing。

use egui::{pos2, Rect};

pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

pub fn ease_in_cubic(t: f32) -> f32 {
    t.powi(3)
}

pub fn ease_in_out_cubic(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t.powi(3)
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// 回弹收尾（用于窗口打开时轻微过冲）
pub fn ease_out_back(t: f32) -> f32 {
    const C1: f32 = 1.20158;
    const C3: f32 = C1 + 1.0;
    1.0 + C3 * (t - 1.0).powi(3) + C1 * (t - 1.0).powi(2)
}

#[derive(Clone, Copy, Debug)]
pub struct Tween {
    pub start: f64,
    pub dur: f32,
}

impl Tween {
    pub fn new(now: f64, dur: f32) -> Self {
        Self { start: now, dur }
    }

    /// 0..=1 的线性进度
    pub fn progress(&self, now: f64) -> f32 {
        (((now - self.start) as f32) / self.dur).clamp(0.0, 1.0)
    }

    pub fn done(&self, now: f64) -> bool {
        now - self.start >= self.dur as f64
    }
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    Rect::from_min_max(
        pos2(lerp(a.min.x, b.min.x, t), lerp(a.min.y, b.min.y, t)),
        pos2(lerp(a.max.x, b.max.x, t), lerp(a.max.y, b.max.y, t)),
    )
}
