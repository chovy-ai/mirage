//! Launchpad 应用中心：全屏遮罩 + 应用网格 + 缩放淡入动画 + 搜索过滤。

use egui::{
    pos2, vec2, Align2, FontId, Pos2, Rect, Stroke, StrokeKind, Ui,
};

use super::{black_a, icon, white_a};
use crate::anim::{ease_in_out_cubic, Tween};
use crate::apps;
use crate::config::Wallpaper;

const ICON: f32 = 76.0;
const CELL: egui::Vec2 = vec2(150.0, 128.0);
const COLS: usize = 7;

pub enum LpPress {
    Icon(&'static str),
    Search,
    Empty,
}

pub struct Launchpad {
    open: bool,
    t: Tween,
    pub search: String,
    icon_hits: Vec<(Rect, &'static str)>,
    search_rect: Rect,
    want_focus: bool,
}

impl Default for Launchpad {
    fn default() -> Self {
        Self {
            open: false,
            t: Tween { start: -10.0, dur: 0.28 },
            search: String::new(),
            icon_hits: Vec::new(),
            search_rect: Rect::NOTHING,
            want_focus: false,
        }
    }
}

impl Launchpad {
    pub fn toggle(&mut self, now: f64) {
        if self.open {
            self.close(now);
        } else {
            self.open = true;
            self.t = Tween::new(now, 0.28);
            self.search.clear();
            self.want_focus = true;
        }
    }

    pub fn close(&mut self, now: f64) {
        if self.open {
            self.open = false;
            self.t = Tween::new(now, 0.22);
        }
    }

    /// 打开程度 0..=1（带方向）
    pub fn progress(&self, now: f64) -> f32 {
        let k = ease_in_out_cubic(self.t.progress(now));
        if self.open {
            k
        } else {
            1.0 - k
        }
    }

    pub fn visible(&self, now: f64) -> bool {
        self.open || !self.t.done(now)
    }

    pub fn animating(&self, now: f64) -> bool {
        !self.t.done(now)
    }

    /// 判定一次按下落在哪里（用上一帧记录的命中区域）
    pub fn press_target(&self, pos: Pos2) -> LpPress {
        if let Some((_, id)) = self.icon_hits.iter().find(|(r, _)| r.contains(pos)) {
            LpPress::Icon(id)
        } else if self.search_rect.expand(6.0).contains(pos) {
            LpPress::Search
        } else {
            LpPress::Empty
        }
    }

    pub fn show(&mut self, ui: &mut Ui, screen: Rect, now: f64, wp: Wallpaper) {
        let p = ui.painter().clone();
        let k = self.progress(now);
        if k <= 0.0 {
            self.icon_hits.clear();
            return;
        }

        // 毛玻璃：壁纸重绘把窗口霜化进背景，再叠暗化与霜层
        super::desktop::draw_wallpaper_alpha(&p, screen, wp, 0.88 * k);
        p.rect_filled(screen, 0, black_a(0.30 * k));
        p.rect_filled(screen, 0, white_a(0.05 * k));

        // 搜索框
        let sw = 260.0;
        self.search_rect = Rect::from_center_size(
            pos2(screen.center().x, screen.top() + 64.0),
            vec2(sw, 34.0),
        );
        if k > 0.5 {
            p.rect_filled(self.search_rect, 9, white_a(0.10 * k));
            p.rect_stroke(
                self.search_rect,
                9,
                Stroke::new(1.0, white_a(0.25 * k)),
                StrokeKind::Inside,
            );
            let te = egui::TextEdit::singleline(&mut self.search)
                .frame(egui::Frame::NONE)
                .hint_text(egui::RichText::new("搜索").color(white_a(0.4)))
                .text_color(white_a(0.95))
                .font(FontId::proportional(15.0))
                .horizontal_align(egui::Align::Center);
            let resp = ui.put(self.search_rect.shrink2(vec2(10.0, 5.0)), te);
            if self.want_focus {
                resp.request_focus();
                self.want_focus = false;
            }
        }

        // 应用网格（搜索过滤）
        let needle = self.search.trim().to_lowercase();
        let apps: Vec<&apps::AppInfo> = apps::APPS
            .iter()
            .filter(|a| {
                needle.is_empty()
                    || a.name.to_lowercase().contains(&needle)
                    || a.id.contains(&needle)
            })
            .collect();

        self.icon_hits.clear();
        let n = apps.len();
        if n == 0 {
            p.text(
                screen.center(),
                Align2::CENTER_CENTER,
                "未找到应用",
                FontId::proportional(16.0),
                white_a(0.5 * k),
            );
            return;
        }
        let cols = COLS.min(n);
        let rows = n.div_ceil(cols);
        let grid_size = vec2(cols as f32 * CELL.x, rows as f32 * CELL.y);
        let grid_origin = screen.center() - grid_size / 2.0 + vec2(0.0, -20.0);

        // 整体从 1.12 缩放到 1.0（macOS Launchpad 的 zoom-out 入场）
        let zoom = 1.0 + 0.12 * (1.0 - k);
        let sc = screen.center();
        let transform = |pt: Pos2| -> Pos2 { sc + (pt - sc) * zoom };

        for (i, app) in apps.iter().enumerate() {
            let col = i % cols;
            let row = i / cols;
            // 末行居中
            let row_cols = if row == rows - 1 { n - row * cols } else { cols };
            let row_offset = (cols - row_cols) as f32 * CELL.x / 2.0;
            let cell_center = pos2(
                grid_origin.x + row_offset + (col as f32 + 0.5) * CELL.x,
                grid_origin.y + (row as f32 + 0.5) * CELL.y,
            );
            let c = transform(cell_center);

            let hit = Rect::from_center_size(c, vec2(CELL.x * 0.8, CELL.y * 0.92));
            let hovered = ui
                .input(|inp| inp.pointer.hover_pos())
                .is_some_and(|hp| hit.contains(hp));
            if hovered && k > 0.9 {
                p.rect_filled(hit, 14, white_a(0.08));
            }

            icon::draw_app_icon(&p, c - vec2(0.0, 16.0), ICON * (0.96 + 0.04 * k), app, k);
            p.text(
                c + vec2(0.0, ICON / 2.0 + 4.0),
                Align2::CENTER_CENTER,
                app.name,
                FontId::proportional(13.0),
                white_a(0.92 * k),
            );
            self.icon_hits.push((hit, app.id));
        }
    }
}
