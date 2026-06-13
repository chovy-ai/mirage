//! 提醒事项：对齐 macOS——圆形勾选框、添加/完成/删除、计数徽标，
//! JSON 持久化到 ~/MirageWorkspace/reminders.json。

use egui::{
    pos2, vec2, Align, Align2, Color32, FontId, Key, Layout, Rect, RichText,
    ScrollArea, Sense, Stroke, TextEdit, Ui,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
struct Item {
    text: String,
    done: bool,
}

#[derive(Default)]
pub struct RemindersApp {
    items: Vec<Item>,
    draft: String,
    loaded: bool,
}

fn store_path() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    format!("{home}/MirageWorkspace/reminders.json")
}

impl RemindersApp {
    fn load(&mut self) {
        if let Ok(bytes) = std::fs::read(store_path()) {
            if let Ok(items) = serde_json::from_slice::<Vec<Item>>(&bytes) {
                self.items = items;
            }
        }
        self.loaded = true;
    }

    fn save(&self) {
        let path = store_path();
        if let Some(dir) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec_pretty(&self.items) {
            let _ = std::fs::write(path, json);
        }
    }

    pub fn show(&mut self, ui: &mut Ui) {
        if !self.loaded {
            self.load();
        }
        let full = ui.max_rect();
        ui.painter()
            .rect_filled(full, 0, Color32::from_rgb(0x1C, 0x1C, 0x21));

        let accent = Color32::from_rgb(0xFF, 0x9F, 0x0A);
        let head_h = 64.0;
        let input_h = 44.0;

        // ---- 头部：标题 + 未完成计数 ----
        let pending = self.items.iter().filter(|i| !i.done).count();
        ui.painter().text(
            pos2(full.left() + 20.0, full.top() + 30.0),
            Align2::LEFT_CENTER,
            "提醒事项",
            FontId::proportional(22.0),
            accent,
        );
        ui.painter().text(
            pos2(full.right() - 20.0, full.top() + 30.0),
            Align2::RIGHT_CENTER,
            format!("{pending}"),
            FontId::proportional(26.0),
            Color32::from_gray(120),
        );

        // ---- 列表 ----
        let list = Rect::from_min_max(
            pos2(full.left(), full.top() + head_h),
            pos2(full.right(), full.bottom() - input_h),
        );
        let list_inner = list.shrink2(vec2(14.0, 2.0));
        let mut list_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(list_inner)
                .layout(Layout::top_down(Align::Min)),
        );
        list_ui.set_clip_rect(list_inner.intersect(ui.clip_rect()));
        let mut toggle: Option<usize> = None;
        let mut remove: Option<usize> = None;
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(list_inner.height())
            .show(&mut list_ui, |ui| {
                ui.set_width(ui.available_width());
                if self.items.is_empty() {
                    ui.add_space(30.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new("没有提醒事项")
                                .color(Color32::from_gray(110))
                                .size(14.0),
                        );
                        ui.label(
                            RichText::new("在下方输入后回车添加")
                                .color(Color32::from_gray(80))
                                .size(12.0),
                        );
                    });
                }
                for (i, item) in self.items.iter().enumerate() {
                    let (row, resp) = ui.allocate_exact_size(
                        vec2(ui.available_width(), 34.0),
                        Sense::hover(),
                    );
                    // 圆形勾选框
                    let circle = pos2(row.left() + 14.0, row.center().y);
                    let cresp = ui.interact(
                        Rect::from_center_size(circle, vec2(26.0, 26.0)),
                        ui.id().with(("chk", i)),
                        Sense::click(),
                    );
                    if item.done {
                        ui.painter().circle_filled(circle, 9.0, accent);
                        ui.painter().circle_filled(circle, 4.0, Color32::from_rgb(0x1C, 0x1C, 0x21));
                        ui.painter().circle_filled(circle, 3.2, accent);
                    } else {
                        ui.painter().circle_stroke(
                            circle,
                            9.0,
                            Stroke::new(
                                1.5,
                                if cresp.hovered() {
                                    accent
                                } else {
                                    Color32::from_gray(95)
                                },
                            ),
                        );
                    }
                    if cresp.clicked() {
                        toggle = Some(i);
                    }
                    // 文本：截断省略，给右侧悬停删除按钮留位，长文本不与 ✕ 重叠
                    let text_font = FontId::proportional(14.0);
                    let shown = super::truncate_text(
                        ui.painter(),
                        &item.text,
                        text_font.clone(),
                        row.width() - 34.0 - 36.0,
                    );
                    ui.painter().text(
                        pos2(row.left() + 34.0, row.center().y),
                        Align2::LEFT_CENTER,
                        shown.clone(),
                        text_font.clone(),
                        if item.done {
                            Color32::from_gray(100)
                        } else {
                            Color32::from_gray(225)
                        },
                    );
                    if item.done {
                        // 删除线（按显示出来的截断文本量宽）
                        let tw = ui
                            .painter()
                            .layout_no_wrap(shown, text_font, Color32::WHITE)
                            .size()
                            .x;
                        ui.painter().line_segment(
                            [
                                pos2(row.left() + 34.0, row.center().y),
                                pos2(row.left() + 34.0 + tw, row.center().y),
                            ],
                            Stroke::new(1.0, Color32::from_gray(100)),
                        );
                    }
                    // hover 显示删除按钮
                    if resp.hovered() || cresp.hovered() {
                        let del = Rect::from_center_size(
                            pos2(row.right() - 18.0, row.center().y),
                            vec2(22.0, 22.0),
                        );
                        let dresp = ui.interact(del, ui.id().with(("del", i)), Sense::click());
                        ui.painter().text(
                            del.center(),
                            Align2::CENTER_CENTER,
                            "✕",
                            FontId::proportional(13.0),
                            if dresp.hovered() {
                                Color32::from_rgb(0xE5, 0x5B, 0x5B)
                            } else {
                                Color32::from_gray(110)
                            },
                        );
                        if dresp.clicked() {
                            remove = Some(i);
                        }
                    }
                    // 分隔线
                    ui.painter().line_segment(
                        [
                            pos2(row.left() + 34.0, row.bottom()),
                            pos2(row.right(), row.bottom()),
                        ],
                        Stroke::new(1.0, Color32::from_gray(38)),
                    );
                }
            });
        let mut dirty = false;
        if let Some(i) = toggle {
            self.items[i].done = !self.items[i].done;
            dirty = true;
        }
        if let Some(i) = remove {
            self.items.remove(i);
            dirty = true;
        }

        // ---- 底部添加行 ----
        let bar = Rect::from_min_max(pos2(full.left(), full.bottom() - input_h), full.max);
        ui.painter().line_segment(
            [bar.left_top(), bar.right_top()],
            Stroke::new(1.0, Color32::from_gray(42)),
        );
        ui.painter().text(
            pos2(bar.left() + 20.0, bar.center().y),
            Align2::CENTER_CENTER,
            "＋",
            FontId::proportional(16.0),
            accent,
        );
        let field = Rect::from_min_max(
            pos2(bar.left() + 36.0, bar.top() + 8.0),
            pos2(bar.right() - 12.0, bar.bottom() - 8.0),
        );
        let te = TextEdit::singleline(&mut self.draft)
            .frame(egui::Frame::NONE)
            .hint_text(RichText::new("新提醒事项").color(Color32::from_gray(95)))
            .text_color(Color32::from_gray(228))
            .font(FontId::proportional(13.5))
            .desired_width(field.width());
        let resp = ui.put(field, te);
        if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            let text = self.draft.trim().to_owned();
            if !text.is_empty() {
                self.items.push(Item { text, done: false });
                self.draft.clear();
                dirty = true;
                if ui.is_enabled() {
                    resp.request_focus();
                }
            }
        }

        if dirty {
            self.save();
        }
    }
}
