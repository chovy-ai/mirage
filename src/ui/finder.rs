//! 访达：真实浏览本地文件系统。对齐 macOS Finder——左侧「个人收藏」侧边栏、
//! 工具栏前进后退、列表视图（名称/修改日期/大小/种类、斑马纹、选中高亮）、
//! 底部路径栏；双击进目录，双击文件交给系统默认应用打开。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Local};
use egui::{
    pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect,
    ScrollArea, Sense, Stroke, Ui,
};

use super::{human_size, truncate_text};

struct Entry {
    name: String,
    path: PathBuf,
    is_dir: bool,
    size: u64,
    modified: Option<SystemTime>,
    kind: &'static str,
}

struct SidebarItem {
    label: &'static str,
    path: PathBuf,
}

pub struct FinderApp {
    cwd: PathBuf,
    entries: Vec<Entry>,
    selected: Option<usize>,
    back: Vec<PathBuf>,
    fwd: Vec<PathBuf>,
    sidebar: Vec<SidebarItem>,
    loaded: bool,
}

fn kind_of(name: &str, is_dir: bool) -> &'static str {
    if is_dir {
        return "文件夹";
    }
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "heic" | "webp" | "bmp" | "svg" => "图像",
        "mp4" | "mov" | "mkv" | "avi" => "影片",
        "mp3" | "m4a" | "wav" | "flac" | "aac" => "音频",
        "pdf" => "PDF 文稿",
        "txt" | "md" | "rtf" => "文本",
        "rs" | "py" | "js" | "ts" | "go" | "c" | "cpp" | "h" | "java" | "swift" | "sh"
        | "toml" | "json" | "yaml" | "yml" => "源代码",
        "zip" | "dmg" | "tar" | "gz" | "7z" | "rar" => "归档",
        "app" => "应用程序",
        _ => "文稿",
    }
}

impl Default for FinderApp {
    fn default() -> Self {
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()));
        let mut sidebar = vec![SidebarItem {
            label: "个人",
            path: home.clone(),
        }];
        for (label, sub) in [
            ("桌面", "Desktop"),
            ("文稿", "Documents"),
            ("下载", "Downloads"),
            ("图片", "Pictures"),
            ("音乐", "Music"),
            ("影片", "Movies"),
        ] {
            let p = home.join(sub);
            if p.is_dir() {
                sidebar.push(SidebarItem { label, path: p });
            }
        }
        sidebar.push(SidebarItem {
            label: "应用程序",
            path: PathBuf::from("/Applications"),
        });
        Self {
            cwd: home,
            entries: Vec::new(),
            selected: None,
            back: Vec::new(),
            fwd: Vec::new(),
            sidebar,
            loaded: false,
        }
    }
}

impl FinderApp {
    fn reload(&mut self) {
        self.entries.clear();
        self.selected = None;
        if let Ok(rd) = std::fs::read_dir(&self.cwd) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let meta = e.metadata().ok();
                let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                self.entries.push(Entry {
                    kind: kind_of(&name, is_dir),
                    path: e.path(),
                    is_dir,
                    size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
                    modified: meta.and_then(|m| m.modified().ok()),
                    name,
                });
            }
        }
        self.entries.sort_by_key(|e| e.name.to_lowercase());
        self.loaded = true;
    }

    fn navigate(&mut self, to: PathBuf) {
        if to == self.cwd {
            return;
        }
        self.back.push(self.cwd.clone());
        self.fwd.clear();
        self.cwd = to;
        self.reload();
    }

    fn go_back(&mut self) {
        if let Some(p) = self.back.pop() {
            self.fwd.push(self.cwd.clone());
            self.cwd = p;
            self.reload();
        }
    }

    fn go_fwd(&mut self) {
        if let Some(p) = self.fwd.pop() {
            self.back.push(self.cwd.clone());
            self.cwd = p;
            self.reload();
        }
    }

    fn title(&self) -> String {
        self.cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".into())
    }

    pub fn show(&mut self, ui: &mut Ui) {
        if !self.loaded {
            self.reload();
        }
        let full = ui.max_rect();
        ui.painter()
            .rect_filled(full, 0, Color32::from_rgb(0x1E, 0x1E, 0x23));

        const SIDEBAR_W: f32 = 165.0;
        const TOOLBAR_H: f32 = 40.0;
        const PATHBAR_H: f32 = 24.0;

        // ---- 侧边栏 ----
        let side = Rect::from_min_size(full.min, vec2(SIDEBAR_W, full.height()));
        ui.painter()
            .rect_filled(side, 0, Color32::from_rgb(0x26, 0x26, 0x2B));
        ui.painter().line_segment(
            [side.right_top(), side.right_bottom()],
            Stroke::new(1.0, Color32::from_gray(20)),
        );
        ui.painter().text(
            side.left_top() + vec2(14.0, 18.0),
            Align2::LEFT_CENTER,
            "个人收藏",
            FontId::proportional(11.0),
            Color32::from_gray(120),
        );
        let mut nav_to: Option<PathBuf> = None;
        for (i, item) in self.sidebar.iter().enumerate() {
            let row = Rect::from_min_size(
                pos2(side.left() + 8.0, side.top() + 32.0 + i as f32 * 26.0),
                vec2(SIDEBAR_W - 16.0, 24.0),
            );
            let resp = ui.interact(row, ui.id().with(("side", i)), Sense::click());
            let active = item.path == self.cwd;
            if active {
                ui.painter()
                    .rect_filled(row, CornerRadius::same(6), Color32::from_rgb(0x3D, 0x3D, 0x46));
            } else if resp.hovered() {
                ui.painter()
                    .rect_filled(row, CornerRadius::same(6), Color32::from_rgb(0x30, 0x30, 0x37));
            }
            // 小图标：个人=房子，其余=文件夹
            let ic = pos2(row.left() + 14.0, row.center().y);
            if item.label == "个人" {
                draw_mini_home(ui, ic);
            } else {
                draw_mini_folder(ui, ic);
            }
            ui.painter().text(
                pos2(row.left() + 28.0, row.center().y),
                Align2::LEFT_CENTER,
                item.label,
                FontId::proportional(13.0),
                Color32::from_gray(215),
            );
            if resp.clicked() {
                nav_to = Some(item.path.clone());
            }
        }
        if let Some(p) = nav_to {
            self.navigate(p);
        }

        // ---- 工具栏 ----
        let content_x = side.right();
        let toolbar = Rect::from_min_max(
            pos2(content_x, full.top()),
            pos2(full.right(), full.top() + TOOLBAR_H),
        );
        ui.painter()
            .rect_filled(toolbar, 0, Color32::from_rgb(0x2A, 0x2A, 0x30));
        ui.painter().line_segment(
            [toolbar.left_bottom(), toolbar.right_bottom()],
            Stroke::new(1.0, Color32::from_gray(20)),
        );
        for (i, (glyph, enabled)) in [
            ("‹", !self.back.is_empty()),
            ("›", !self.fwd.is_empty()),
        ]
        .iter()
        .enumerate()
        {
            let r = Rect::from_center_size(
                pos2(toolbar.left() + 22.0 + i as f32 * 30.0, toolbar.center().y),
                vec2(24.0, 24.0),
            );
            let resp = ui.interact(r, ui.id().with(("nav", i)), Sense::click());
            if resp.hovered() && *enabled {
                ui.painter()
                    .rect_filled(r, CornerRadius::same(6), Color32::from_gray(58));
            }
            ui.painter().text(
                r.center(),
                Align2::CENTER_CENTER,
                *glyph,
                FontId::proportional(20.0),
                if *enabled {
                    Color32::from_gray(210)
                } else {
                    Color32::from_gray(85)
                },
            );
            if resp.clicked() && *enabled {
                if i == 0 {
                    self.go_back();
                } else {
                    self.go_fwd();
                }
            }
        }
        let title_font = FontId::proportional(14.5);
        let title = truncate_text(
            ui.painter(),
            &self.title(),
            title_font.clone(),
            toolbar.width() - 90.0,
        );
        ui.painter().text(
            pos2(toolbar.left() + 70.0, toolbar.center().y),
            Align2::LEFT_CENTER,
            title,
            title_font,
            Color32::from_gray(230),
        );

        // ---- 列头 ----
        let header_h = 24.0;
        let head = Rect::from_min_max(
            toolbar.left_bottom(),
            pos2(full.right(), toolbar.bottom() + header_h),
        );
        let w = head.width();
        let col_name_w = w * 0.42;
        let col_date_w = w * 0.26;
        let col_size_w = w * 0.13;
        let cols = [
            ("名称", head.left() + 30.0),
            ("修改日期", head.left() + col_name_w),
            ("大小", head.left() + col_name_w + col_date_w),
            ("种类", head.left() + col_name_w + col_date_w + col_size_w),
        ];
        for (label, x) in cols {
            ui.painter().text(
                pos2(x, head.center().y),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(11.0),
                Color32::from_gray(130),
            );
        }
        ui.painter().line_segment(
            [head.left_bottom(), head.right_bottom()],
            Stroke::new(1.0, Color32::from_gray(28)),
        );

        // ---- 文件列表 ----
        let list = Rect::from_min_max(
            head.left_bottom(),
            pos2(full.right(), full.bottom() - PATHBAR_H),
        );
        let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(list));
        // 显式视口高度与裁剪：egui 0.34 下 new_child 的可用高度可能超出
        // max_rect，导致 ScrollArea 误以为内容塞得下而不滚动。
        list_ui.set_clip_rect(list.intersect(ui.clip_rect()));
        let row_h = 24.0;
        let mut open_dir: Option<PathBuf> = None;
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(list.height())
            .show_rows(&mut list_ui, row_h, self.entries.len(), |ui, range| {
                for i in range {
                    let e = &self.entries[i];
                    let (row, resp) =
                        ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                    let selected = self.selected == Some(i);
                    // 斑马纹 / 选中
                    if selected {
                        ui.painter().rect_filled(
                            row.shrink2(vec2(4.0, 0.0)),
                            CornerRadius::same(5),
                            Color32::from_rgb(0x2B, 0x5B, 0xD7),
                        );
                    } else if i % 2 == 1 {
                        ui.painter()
                            .rect_filled(row, 0, Color32::from_rgb(0x23, 0x23, 0x28));
                    }
                    // 图标
                    let ic = pos2(row.left() + 18.0, row.center().y);
                    if e.is_dir {
                        draw_file_folder(ui, ic);
                    } else {
                        draw_file_doc(ui, ic);
                    }
                    let text_c = if selected {
                        Color32::WHITE
                    } else {
                        Color32::from_gray(215)
                    };
                    let dim_c = if selected {
                        Color32::from_gray(230)
                    } else {
                        Color32::from_gray(135)
                    };
                    // 名称列截断：长文件名不越界压到「修改日期」列
                    let name_font = FontId::proportional(13.0);
                    let name = truncate_text(
                        ui.painter(),
                        &e.name,
                        name_font.clone(),
                        col_name_w - 38.0,
                    );
                    ui.painter().text(
                        pos2(row.left() + 30.0, row.center().y),
                        Align2::LEFT_CENTER,
                        name,
                        name_font,
                        text_c,
                    );
                    let date = e
                        .modified
                        .map(|m| {
                            let dt: DateTime<Local> = m.into();
                            dt.format("%Y年%m月%d日 %H:%M").to_string()
                        })
                        .unwrap_or_else(|| "—".into());
                    let dim_font = FontId::proportional(12.0);
                    let date =
                        truncate_text(ui.painter(), &date, dim_font.clone(), col_date_w - 10.0);
                    ui.painter().text(
                        pos2(row.left() + col_name_w, row.center().y),
                        Align2::LEFT_CENTER,
                        date,
                        dim_font,
                        dim_c,
                    );
                    ui.painter().text(
                        pos2(row.left() + col_name_w + col_date_w, row.center().y),
                        Align2::LEFT_CENTER,
                        if e.is_dir {
                            "—".into()
                        } else {
                            human_size(e.size)
                        },
                        FontId::proportional(12.0),
                        dim_c,
                    );
                    ui.painter().text(
                        pos2(
                            row.left() + col_name_w + col_date_w + col_size_w,
                            row.center().y,
                        ),
                        Align2::LEFT_CENTER,
                        e.kind,
                        FontId::proportional(12.0),
                        dim_c,
                    );

                    if resp.clicked() {
                        self.selected = Some(i);
                    }
                    if resp.double_clicked() {
                        if e.is_dir {
                            open_dir = Some(e.path.clone());
                        } else {
                            // 交给系统默认应用（真实 Finder 行为）
                            let _ = std::process::Command::new("open").arg(&e.path).spawn();
                        }
                    }
                }
            });
        if let Some(p) = open_dir {
            self.navigate(p);
        }

        // ---- 底部路径栏（面包屑） ----
        let bar = Rect::from_min_max(pos2(content_x, full.bottom() - PATHBAR_H), full.max);
        ui.painter()
            .rect_filled(bar, 0, Color32::from_rgb(0x26, 0x26, 0x2B));
        ui.painter().line_segment(
            [bar.left_top(), bar.right_top()],
            Stroke::new(1.0, Color32::from_gray(28)),
        );
        let crumbs: Vec<&str> = Path::new(&self.cwd)
            .iter()
            .filter_map(|c| c.to_str())
            .filter(|c| *c != "/")
            .collect();
        // 深路径防溢出：从尾部保留尽量多的层级，放不下的开头折叠成「…」
        // （右侧还要给「N 个项目」留位）
        let crumb_font = FontId::proportional(11.5);
        let crumb_w = |s: &str| {
            ui.painter()
                .layout_no_wrap(s.to_owned(), crumb_font.clone(), Color32::WHITE)
                .size()
                .x
        };
        let avail = bar.width() - 24.0 - (crumb_w(&format!("{} 个项目", self.entries.len())) + 24.0);
        let mut start = 0usize;
        let total = |from: usize| -> f32 {
            crumbs[from..]
                .iter()
                .map(|c| crumb_w(c) + 8.0 + 12.0)
                .sum::<f32>()
                + if from > 0 { 22.0 } else { 0.0 } // 「…›」的宽度
        };
        while start + 1 < crumbs.len() && total(start) > avail {
            start += 1;
        }
        let mut x = bar.left() + 12.0;
        if start > 0 {
            ui.painter().text(
                pos2(x, bar.center().y),
                Align2::LEFT_CENTER,
                "… ›",
                FontId::proportional(11.0),
                Color32::from_gray(100),
            );
            x += 22.0;
        }
        let mut crumb_nav: Option<PathBuf> = None;
        for (i, c) in crumbs.iter().enumerate().skip(start) {
            let galley = ui.painter().layout_no_wrap(
                (*c).to_owned(),
                FontId::proportional(11.5),
                Color32::WHITE,
            );
            let r = Rect::from_min_size(
                pos2(x, bar.top() + 3.0),
                vec2(galley.size().x + 6.0, PATHBAR_H - 6.0),
            );
            let resp = ui.interact(r, ui.id().with(("crumb", i)), Sense::click());
            ui.painter().text(
                pos2(x + 3.0, bar.center().y),
                Align2::LEFT_CENTER,
                *c,
                FontId::proportional(11.5),
                if resp.hovered() {
                    Color32::from_gray(230)
                } else {
                    Color32::from_gray(150)
                },
            );
            x += galley.size().x + 8.0;
            if i + 1 < crumbs.len() {
                ui.painter().text(
                    pos2(x, bar.center().y),
                    Align2::LEFT_CENTER,
                    "›",
                    FontId::proportional(11.0),
                    Color32::from_gray(100),
                );
                x += 12.0;
            }
            if resp.clicked() {
                let p: PathBuf =
                    std::iter::once("/").chain(crumbs[..=i].iter().copied()).collect();
                crumb_nav = Some(p);
            }
        }
        if let Some(p) = crumb_nav {
            self.navigate(p);
        }
        // 右侧项目数
        ui.painter().text(
            pos2(bar.right() - 12.0, bar.center().y),
            Align2::RIGHT_CENTER,
            format!("{} 个项目", self.entries.len()),
            FontId::proportional(11.5),
            Color32::from_gray(130),
        );
    }
}

// ---- 行内小图标 ----

fn draw_mini_folder(ui: &Ui, c: egui::Pos2) {
    let p = ui.painter();
    let body = Rect::from_center_size(c + vec2(0.0, 1.0), vec2(14.0, 10.0));
    p.rect_filled(body, CornerRadius::same(2), Color32::from_rgb(0x4B, 0xA3, 0xF5));
    p.rect_filled(
        Rect::from_min_size(body.min - vec2(0.0, 2.5), vec2(6.0, 3.5)),
        CornerRadius::same(1),
        Color32::from_rgb(0x4B, 0xA3, 0xF5),
    );
}

fn draw_mini_home(ui: &Ui, c: egui::Pos2) {
    let p = ui.painter();
    let blue = Color32::from_rgb(0x4B, 0xA3, 0xF5);
    // 屋顶 + 屋身
    p.add(egui::Shape::convex_polygon(
        vec![c + vec2(-8.0, 0.0), c + vec2(0.0, -7.0), c + vec2(8.0, 0.0)],
        blue,
        Stroke::NONE,
    ));
    p.rect_filled(
        Rect::from_min_size(c + vec2(-5.5, -0.5), vec2(11.0, 7.0)),
        CornerRadius::same(1),
        blue,
    );
}

fn draw_file_folder(ui: &Ui, c: egui::Pos2) {
    let p = ui.painter();
    let body = Rect::from_center_size(c + vec2(0.0, 1.0), vec2(15.0, 11.0));
    p.rect_filled(body, CornerRadius::same(2), Color32::from_rgb(0x52, 0xA8, 0xF0));
    p.rect_filled(
        Rect::from_min_size(body.min - vec2(0.0, 2.5), vec2(7.0, 3.5)),
        CornerRadius::same(1),
        Color32::from_rgb(0x52, 0xA8, 0xF0),
    );
    p.rect_filled(
        Rect::from_min_size(body.min + vec2(0.0, 2.0), vec2(15.0, 9.0)),
        CornerRadius { nw: 0, ne: 0, sw: 2, se: 2 },
        Color32::from_rgb(0x6E, 0xBE, 0xFA),
    );
}

fn draw_file_doc(ui: &Ui, c: egui::Pos2) {
    let p = ui.painter();
    let body = Rect::from_center_size(c, vec2(11.0, 14.0));
    p.rect_filled(body, CornerRadius::same(2), Color32::from_rgb(0xE6, 0xE6, 0xEC));
    // 折角
    p.add(egui::Shape::convex_polygon(
        vec![
            body.right_top() + vec2(-4.0, 0.0),
            body.right_top() + vec2(0.0, 4.0),
            body.right_top(),
        ],
        Color32::from_rgb(0xB8, 0xB8, 0xC2),
        Stroke::NONE,
    ));
    for dy in [-2.0, 1.0, 4.0] {
        p.line_segment(
            [c + vec2(-3.5, dy), c + vec2(3.5, dy)],
            Stroke::new(1.0, Color32::from_rgb(0xB0, 0xB0, 0xBA)),
        );
    }
}
