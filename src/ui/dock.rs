//! Dock 栏：磨砂面板、鼠标邻近放大波浪、点击弹跳、运行指示点、
//! 最小化窗口停靠区、悬停 tooltip。
//!
//! 布局每帧重算：先按基础位置求出每个图标与鼠标的水平距离 -> 算缩放，
//! 再按缩放后的尺寸重新排布并整体居中（macOS 同款近似算法）。

use std::collections::{HashMap, HashSet};

use egui::{
    pos2, vec2, FontId, Painter, Pos2, Rect, Stroke, StrokeKind,
};

use super::{black_a, icon, white_a};
use crate::apps::{self, TRASH};
use crate::wm::WindowId;

const GAP: f32 = 6.0;
const PAD_H: f32 = 12.0;
const PAD_V: f32 = 8.0;
const MARGIN_BOTTOM: f32 = 10.0;
/// 放大影响半径（放大倍率由 DesktopConfig.dock_magnify 提供）
const MAG_RANGE: f32 = 150.0;
/// 弹跳时长
pub const BOUNCE_DUR: f64 = 0.95;

#[derive(Clone)]
pub enum DockKind {
    App(&'static str),
    Launchpad,
    Separator,
    Minimized { id: WindowId, app_id: &'static str, title: String },
    Trash,
}

#[derive(Default)]
pub struct DockState {
    /// app_id -> 弹跳开始时刻
    pub bounce: HashMap<&'static str, f64>,
}

pub struct LaidItem {
    pub kind: DockKind,
    pub rect: Rect,
    pub size: f32,
}

pub struct DockGeometry {
    pub panel: Rect,
    pub items: Vec<LaidItem>,
    /// 下一个最小化窗口将落入的位置（最小化动画的飞行目标）
    pub minimized_anchor: Pos2,
}

pub fn layout(
    screen: Rect,
    pointer: Option<Pos2>,
    kinds: &[DockKind],
    base: f32,
    mag: f32,
) -> DockGeometry {
    let icon_bottom = screen.bottom() - MARGIN_BOTTOM - PAD_V;
    let panel_top = icon_bottom - base; // 图标向上放大溢出

    // 第一遍：基础宽度与基础中心（无放大）
    let base_w = |k: &DockKind| match k {
        DockKind::Separator => 10.0,
        _ => base,
    };
    let total_base: f32 =
        kinds.iter().map(base_w).sum::<f32>() + GAP * (kinds.len().saturating_sub(1)) as f32;
    let mut x = screen.center().x - total_base / 2.0;
    let mut base_centers = Vec::with_capacity(kinds.len());
    for k in kinds {
        let w = base_w(k);
        base_centers.push(x + w / 2.0);
        x += w + GAP;
    }

    // 放大系数：鼠标在 Dock 带状区域内才生效，余弦衰减
    let magnify = |cx: f32| -> f32 {
        if mag <= 0.0 {
            return 0.0;
        }
        let Some(p) = pointer else { return 0.0 };
        let band = p.y >= panel_top - base * mag && p.x >= screen.left() && p.x <= screen.right();
        if !band {
            return 0.0;
        }
        let d = (p.x - cx).abs();
        if d >= MAG_RANGE {
            return 0.0;
        }
        let t = (std::f32::consts::PI * d / MAG_RANGE).cos() * 0.5 + 0.5;
        mag * t * t
    };

    // 第二遍：按放大后的尺寸重新排布，整体居中
    let sizes: Vec<f32> = kinds
        .iter()
        .zip(&base_centers)
        .map(|(k, &cx)| match k {
            DockKind::Separator => 10.0,
            _ => base * (1.0 + magnify(cx)),
        })
        .collect();
    let total: f32 = sizes.iter().sum::<f32>() + GAP * (kinds.len().saturating_sub(1)) as f32;
    let mut x = screen.center().x - total / 2.0;
    let mut items = Vec::with_capacity(kinds.len());
    for (k, &size) in kinds.iter().zip(&sizes) {
        let rect = match k {
            DockKind::Separator => Rect::from_min_size(
                pos2(x, icon_bottom - base),
                vec2(size, base),
            ),
            _ => Rect::from_min_size(pos2(x, icon_bottom - size), vec2(size, size)),
        };
        items.push(LaidItem { kind: k.clone(), rect, size });
        x += size + GAP;
    }

    let panel = Rect::from_min_max(
        pos2(
            screen.center().x - total / 2.0 - PAD_H,
            icon_bottom - base - PAD_V,
        ),
        pos2(
            screen.center().x + total / 2.0 + PAD_H,
            screen.bottom() - MARGIN_BOTTOM,
        ),
    );

    // 最小化窗口落点：废纸篓左侧
    let trash_left = items
        .iter()
        .find(|it| matches!(it.kind, DockKind::Trash))
        .map(|it| it.rect.left())
        .unwrap_or(panel.right());
    let minimized_anchor = pos2(trash_left - GAP - base / 2.0, icon_bottom - base / 2.0);

    DockGeometry { panel, items, minimized_anchor }
}

/// 命中检测（用放大后的 rect）
pub fn hit(geom: &DockGeometry, pos: Pos2) -> Option<&LaidItem> {
    geom.items
        .iter()
        .find(|it| !matches!(it.kind, DockKind::Separator) && it.rect.contains(pos))
}

pub fn draw(
    p: &Painter,
    geom: &DockGeometry,
    state: &DockState,
    running: &HashSet<&'static str>,
    now: f64,
    hover: Option<Pos2>,
) {
    // 磨砂面板（半透明 + 高光描边模拟毛玻璃）
    p.rect_filled(geom.panel, 22, white_a(0.16));
    p.rect_filled(geom.panel, 22, black_a(0.10));
    p.rect_stroke(
        geom.panel,
        22,
        Stroke::new(1.0, white_a(0.22)),
        StrokeKind::Inside,
    );

    let mut tooltip: Option<(Pos2, String)> = None;

    for it in &geom.items {
        let center = it.rect.center();
        match &it.kind {
            DockKind::Separator => {
                p.line_segment(
                    [
                        pos2(center.x, geom.panel.top() + 10.0),
                        pos2(center.x, geom.panel.bottom() - 10.0),
                    ],
                    Stroke::new(1.0, white_a(0.22)),
                );
                continue;
            }
            DockKind::App(id) => {
                // 弹跳位移
                let mut c = center;
                if let Some(&t0) = state.bounce.get(id) {
                    let t = ((now - t0) / BOUNCE_DUR) as f32;
                    if t < 1.0 {
                        let h = (t * std::f32::consts::PI * 3.0).sin().abs() * 30.0 * (1.0 - t);
                        c.y -= h;
                    }
                }
                let app = apps::get(id);
                icon::draw_app_icon(p, c, it.size, app, 1.0);
                if running.contains(id) {
                    p.circle_filled(
                        pos2(center.x, geom.panel.bottom() - 4.0),
                        2.0,
                        white_a(0.85),
                    );
                }
                if it.rect.contains(hover.unwrap_or(pos2(-1.0, -1.0))) {
                    tooltip = Some((pos2(center.x, it.rect.top()), app.name.to_owned()));
                }
            }
            DockKind::Launchpad => {
                icon::draw_launchpad_icon(p, center, it.size, 1.0);
                if it.rect.contains(hover.unwrap_or(pos2(-1.0, -1.0))) {
                    tooltip = Some((pos2(center.x, it.rect.top()), "启动台".to_owned()));
                }
            }
            DockKind::Minimized { app_id, title, .. } => {
                let app = apps::get(app_id);
                icon::draw_app_icon(p, center, it.size, app, 0.92);
                // 右下角小标记，表示这是一个最小化的窗口
                p.circle_filled(
                    pos2(it.rect.right() - 7.0, it.rect.bottom() - 7.0),
                    4.5,
                    black_a(0.55),
                );
                p.circle_filled(
                    pos2(it.rect.right() - 7.0, it.rect.bottom() - 7.0),
                    2.2,
                    white_a(0.9),
                );
                if it.rect.contains(hover.unwrap_or(pos2(-1.0, -1.0))) {
                    tooltip = Some((pos2(center.x, it.rect.top()), title.clone()));
                }
            }
            DockKind::Trash => {
                icon::draw_app_icon(p, center, it.size, &TRASH, 1.0);
                if it.rect.contains(hover.unwrap_or(pos2(-1.0, -1.0))) {
                    tooltip = Some((pos2(center.x, it.rect.top()), TRASH.name.to_owned()));
                }
            }
        }
    }

    // tooltip：图标上方的小气泡
    if let Some((anchor, text)) = tooltip {
        let font = FontId::proportional(12.5);
        let galley = p.layout_no_wrap(text, font, white_a(0.95));
        let size = galley.size() + vec2(20.0, 12.0);
        let rect = Rect::from_center_size(pos2(anchor.x, anchor.y - 22.0), size);
        p.rect_filled(rect, 7, black_a(0.66));
        p.rect_stroke(rect, 7, Stroke::new(1.0, white_a(0.14)), StrokeKind::Inside);
        // 小三角
        p.add(egui::Shape::convex_polygon(
            vec![
                pos2(anchor.x - 5.0, rect.bottom()),
                pos2(anchor.x + 5.0, rect.bottom()),
                pos2(anchor.x, rect.bottom() + 5.0),
            ],
            black_a(0.66),
            Stroke::NONE,
        ));
        p.galley(rect.min + vec2(10.0, 6.0), galley, white_a(0.95));
    }
}
