//! 窗口管理核心：纯逻辑，不做任何绘制。
//! `windows` 的 Vec 顺序即 z-order（最后一个最靠前）。

use egui::{Pos2, Rect};

use crate::anim::Tween;

pub type WindowId = u64;

pub enum WindowAnim {
    None,
    /// 打开：缩放淡入
    Opening(Tween),
    /// 关闭：缩小淡出，结束后从列表移除
    Closing(Tween),
    /// 最小化：飞向 Dock（简化版 genie）
    Minimizing { t: Tween, to: Pos2 },
    /// 从 Dock 恢复
    Restoring { t: Tween, from: Pos2 },
    /// 最大化 / 还原的尺寸过渡
    Morph { t: Tween, from: Rect },
}

pub struct WindowState {
    pub id: WindowId,
    pub app_id: &'static str,
    pub title: String,
    pub rect: Rect,
    /// Some(原始 rect) 表示当前处于最大化状态
    pub restore_rect: Option<Rect>,
    pub minimized: bool,
    pub anim: WindowAnim,
}

impl WindowState {
    /// 是否参与点击命中 / 算作可见窗口
    pub fn interactive(&self) -> bool {
        !self.minimized
            && !matches!(
                self.anim,
                WindowAnim::Closing(_) | WindowAnim::Minimizing { .. }
            )
    }
}

#[derive(Default)]
pub struct WindowManager {
    pub windows: Vec<WindowState>,
    next_id: WindowId,
}

impl WindowManager {
    pub fn open(&mut self, app_id: &'static str, title: String, rect: Rect, now: f64) -> WindowId {
        self.next_id += 1;
        let id = self.next_id;
        self.windows.push(WindowState {
            id,
            app_id,
            title,
            rect,
            restore_rect: None,
            minimized: false,
            anim: WindowAnim::Opening(Tween::new(now, 0.26)),
        });
        id
    }

    pub fn get(&self, id: WindowId) -> Option<&WindowState> {
        self.windows.iter().find(|w| w.id == id)
    }

    pub fn get_mut(&mut self, id: WindowId) -> Option<&mut WindowState> {
        self.windows.iter_mut().find(|w| w.id == id)
    }

    /// 置顶并聚焦
    pub fn focus(&mut self, id: WindowId) {
        if let Some(i) = self.windows.iter().position(|w| w.id == id) {
            let w = self.windows.remove(i);
            self.windows.push(w);
        }
    }

    /// 当前聚焦窗口（最前面的可交互窗口）
    pub fn front_id(&self) -> Option<WindowId> {
        self.windows.iter().rev().find(|w| w.interactive()).map(|w| w.id)
    }

    /// pos 命中的最前窗口
    pub fn topmost_at(&self, pos: Pos2) -> Option<&WindowState> {
        self.windows
            .iter()
            .rev()
            .find(|w| w.interactive() && w.rect.contains(pos))
    }

    pub fn window_of_app(&self, app_id: &str) -> Option<&WindowState> {
        self.windows.iter().rev().find(|w| w.app_id == app_id)
    }

    pub fn set_rect(&mut self, id: WindowId, rect: Rect) {
        if let Some(w) = self.get_mut(id) {
            w.rect = rect;
        }
    }

    pub fn close(&mut self, id: WindowId, now: f64) {
        if let Some(w) = self.get_mut(id) {
            w.anim = WindowAnim::Closing(Tween::new(now, 0.16));
        }
    }

    pub fn minimize(&mut self, id: WindowId, to: Pos2, now: f64) {
        if let Some(w) = self.get_mut(id) {
            if !w.minimized {
                w.anim = WindowAnim::Minimizing {
                    t: Tween::new(now, 0.38),
                    to,
                };
            }
        }
    }

    pub fn restore(&mut self, id: WindowId, from: Pos2, now: f64) {
        if let Some(w) = self.get_mut(id) {
            if w.minimized {
                w.minimized = false;
                w.anim = WindowAnim::Restoring {
                    t: Tween::new(now, 0.32),
                    from,
                };
            }
        }
        self.focus(id);
    }

    /// 动画式移动到目标 rect（边缘平铺归位用）
    pub fn morph_to(&mut self, id: WindowId, target: Rect, now: f64) {
        if let Some(w) = self.get_mut(id) {
            let from = w.rect;
            w.rect = target;
            w.restore_rect = None;
            w.anim = WindowAnim::Morph {
                t: Tween::new(now, 0.26),
                from,
            };
        }
    }

    /// 最大化 <-> 还原（macOS 的 zoom）
    pub fn toggle_maximize(&mut self, id: WindowId, target: Rect, now: f64) {
        if let Some(w) = self.get_mut(id) {
            let from = w.rect;
            if let Some(prev) = w.restore_rect.take() {
                w.rect = prev;
            } else {
                w.restore_rect = Some(w.rect);
                w.rect = target;
            }
            w.anim = WindowAnim::Morph {
                t: Tween::new(now, 0.30),
                from,
            };
        }
    }

    /// 推进动画状态机：移除已关闭窗口、落定最小化等
    pub fn cleanup(&mut self, now: f64) {
        self.windows.retain(|w| match &w.anim {
            WindowAnim::Closing(t) => !t.done(now),
            _ => true,
        });
        for w in &mut self.windows {
            let done = match &w.anim {
                WindowAnim::None | WindowAnim::Closing(_) => false,
                WindowAnim::Opening(t) | WindowAnim::Restoring { t, .. } | WindowAnim::Morph { t, .. } => {
                    t.done(now)
                }
                WindowAnim::Minimizing { t, .. } => {
                    if t.done(now) {
                        w.minimized = true;
                        true
                    } else {
                        false
                    }
                }
            };
            if done {
                w.anim = WindowAnim::None;
            }
        }
    }

    pub fn animating(&self) -> bool {
        self.windows.iter().any(|w| !matches!(w.anim, WindowAnim::None))
    }
}

/// 窗口边缘命中结果（用于 8 向缩放）
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EdgeHit {
    pub n: bool,
    pub s: bool,
    pub e: bool,
    pub w: bool,
}

pub const RESIZE_OUT: f32 = 5.0;
pub const RESIZE_IN: f32 = 4.0;

/// 边框带状区域命中检测：窗口边缘内 4px / 外 5px
pub fn hit_edges(rect: Rect, pos: Pos2) -> Option<EdgeHit> {
    if !rect.expand(RESIZE_OUT).contains(pos) {
        return None;
    }
    let w = pos.x <= rect.left() + RESIZE_IN;
    let e = pos.x >= rect.right() - RESIZE_IN;
    let n = pos.y <= rect.top() + RESIZE_IN;
    let s = pos.y >= rect.bottom() - RESIZE_IN;
    if n || s || e || w {
        Some(EdgeHit { n, s, e, w })
    } else {
        None
    }
}
