//! 回收站：浏览 macOS 的 ~/.Trash。
//! 只读——清空废纸篓属于永久删除，本应用不做，需要用户自行在访达里操作。

use std::path::PathBuf;

use egui::{vec2, Align, Color32, FontId, Layout, RichText, ScrollArea, Ui};

struct Entry {
    name: String,
    is_dir: bool,
    size: u64,
}

#[derive(Default)]
pub struct TrashApp {
    entries: Vec<Entry>,
    total: u64,
    loaded: bool,
}

fn dir_size(path: &PathBuf) -> u64 {
    let mut total = 0;
    if let Ok(rd) = std::fs::read_dir(path) {
        for e in rd.flatten() {
            if let Ok(ft) = e.file_type() {
                if ft.is_dir() {
                    total += dir_size(&e.path());
                } else if let Ok(m) = e.metadata() {
                    total += m.len();
                }
            }
        }
    }
    total
}

fn human(bytes: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

impl TrashApp {
    fn trash_dir() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(home).join(".Trash")
    }

    pub fn reload(&mut self) {
        self.entries.clear();
        self.total = 0;
        if let Ok(rd) = std::fs::read_dir(Self::trash_dir()) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with(".DS_Store") {
                    continue;
                }
                let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                let size = if is_dir {
                    dir_size(&e.path())
                } else {
                    e.metadata().map(|m| m.len()).unwrap_or(0)
                };
                self.total += size;
                self.entries.push(Entry { name, is_dir, size });
            }
        }
        self.entries.sort_by_key(|e| e.name.to_lowercase());
        self.loaded = true;
    }

    pub fn show(&mut self, ui: &mut Ui) {
        if !self.loaded {
            self.reload();
        }
        ui.painter()
            .rect_filled(ui.max_rect(), 0, Color32::from_rgb(0x1B, 0x1B, 0x1F));

        let full = ui.max_rect();
        let head_h = 44.0;

        // ---- 头部：标题 + 统计 + 刷新 ----
        let head = egui::Rect::from_min_size(full.min, vec2(full.width(), head_h));
        ui.painter().rect_filled(head, 0, Color32::from_rgb(0x24, 0x24, 0x29));
        ui.painter().text(
            head.left_center() + vec2(16.0, 0.0),
            egui::Align2::LEFT_CENTER,
            format!("废纸篓 · {} 个项目 · {}", self.entries.len(), human(self.total)),
            FontId::proportional(13.5),
            Color32::from_gray(210),
        );
        let btn = egui::Rect::from_center_size(
            egui::pos2(head.right() - 44.0, head.center().y),
            vec2(60.0, 26.0),
        );
        if ui
            .put(btn, egui::Button::new(RichText::new("刷新").size(12.0)))
            .clicked()
        {
            self.reload();
        }

        // ---- 列表 ----
        let body = egui::Rect::from_min_max(head.left_bottom(), full.max);
        let body_inner = body.shrink2(vec2(12.0, 8.0));
        let mut bu = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(body_inner)
                .layout(Layout::top_down(Align::Min)),
        );
        bu.set_clip_rect(body_inner.intersect(ui.clip_rect()));
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(body_inner.height())
            .show(&mut bu, |ui| {
                ui.set_width(ui.available_width());
                if self.entries.is_empty() {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new("🗑")
                                .size(48.0)
                                .color(Color32::from_gray(70)),
                        );
                        ui.label(
                            RichText::new("废纸篓是空的").color(Color32::from_gray(120)),
                        );
                    });
                    return;
                }
                for e in &self.entries {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(if e.is_dir { "📁" } else { "📄" }).size(15.0),
                        );
                        // 名称占满中间但截断省略，长文件名不挤压右侧大小、不换行撑高行
                        let name_w = (ui.available_width() - 80.0).max(40.0);
                        ui.allocate_ui_with_layout(
                            vec2(name_w, 18.0),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&e.name).color(Color32::from_gray(220)),
                                    )
                                    .truncate(),
                                );
                            },
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(human(e.size))
                                    .color(Color32::from_gray(120))
                                    .size(12.0),
                            );
                        });
                    });
                    ui.separator();
                }
                ui.add_space(8.0);
                ui.label(
                    RichText::new("只读浏览 · 清空废纸篓请在访达中操作")
                        .color(Color32::from_gray(90))
                        .size(11.5)
                        .italics(),
                );
            });
    }
}
