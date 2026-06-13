//! 照片：一比一还原 macOS「照片」——左侧图库/相簿侧边栏、
//! 顶部「年份/月份/日期/所有照片」分段控件 + 缩放滑杆、正方形缩略图网格
//! （后台线程懒加载、可见才解码）、双击进入单张查看器（←/→ 切换、Esc 返回）。
//! 扫描 ~/Pictures、~/Desktop、~/Downloads 的本机图片；HEIC 经 `sips` 转码。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use chrono::{DateTime, Datelike, Local};
use egui::{
    pos2, vec2, Align2, Color32, ColorImage, CornerRadius, FontId, Rect, Sense, Stroke,
    TextureHandle, TextureOptions, Ui,
};

const THUMB_PX: u32 = 256;
const FULL_PX: u32 = 2048;
const MAX_PHOTOS: usize = 800;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Segment {
    Years,
    Months,
    Days,
    All,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    All,
    Pictures,
    Desktop,
    Downloads,
}

struct Photo {
    path: PathBuf,
    name: String,
    modified: DateTime<Local>,
    source: Source,
}

enum Loaded {
    Thumb(usize, ColorImage),
    Full(usize, ColorImage),
    Failed(usize),
}

pub struct PhotosApp {
    scanned: bool,
    photos: Vec<Photo>,
    thumbs: HashMap<usize, TextureHandle>,
    failed: HashSet<usize>,
    pending: Arc<Mutex<Vec<(usize, PathBuf, bool)>>>, // (idx, path, is_full)
    requested: HashSet<usize>,
    rx: Option<Receiver<Loaded>>,
    tx: Option<Sender<Loaded>>,
    workers_started: bool,
    /// 单张查看器：当前索引
    viewer: Option<usize>,
    full_tex: Option<(usize, TextureHandle)>,
    full_requested: Option<usize>,
    segment: Segment,
    source: Source,
    /// 缩略图边长
    cell: f32,
}

impl Default for PhotosApp {
    fn default() -> Self {
        Self {
            scanned: false,
            photos: Vec::new(),
            thumbs: HashMap::new(),
            failed: HashSet::new(),
            pending: Arc::new(Mutex::new(Vec::new())),
            requested: HashSet::new(),
            rx: None,
            tx: None,
            workers_started: false,
            viewer: None,
            full_tex: None,
            full_requested: None,
            segment: Segment::All,
            source: Source::All,
            cell: 120.0,
        }
    }
}

fn is_image(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    matches!(
        ext.as_str(),
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tiff" | "tif" | "heic"
    )
}

fn decode(path: &Path, max_px: u32) -> Option<ColorImage> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    // HEIC：image crate 不支持，经 sips 转临时 jpeg
    let img = if ext == "heic" {
        let tmp = std::env::temp_dir().join(format!(
            "mirage-heic-{}.jpg",
            path.file_stem().and_then(|s| s.to_str()).unwrap_or("x")
        ));
        let ok = std::process::Command::new("sips")
            .args(["-s", "format", "jpeg", "--resampleHeightWidthMax"])
            .arg(max_px.to_string())
            .arg(path)
            .arg("--out")
            .arg(&tmp)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            return None;
        }
        let r = image::open(&tmp).ok()?;
        let _ = std::fs::remove_file(&tmp);
        r
    } else {
        image::open(path).ok()?
    };
    let img = img.thumbnail(max_px, max_px).to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

impl PhotosApp {
    fn scan(&mut self) {
        self.scanned = true;
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()));
        let roots = [
            (home.join("Pictures"), Source::Pictures, 2usize),
            (home.join("Desktop"), Source::Desktop, 1),
            (home.join("Downloads"), Source::Downloads, 1),
        ];
        let mut out = Vec::new();
        for (root, source, depth) in roots {
            collect(&root, source, depth, &mut out);
        }
        out.sort_by_key(|p| std::cmp::Reverse(p.modified));
        out.truncate(MAX_PHOTOS);
        self.photos = out;

        fn collect(dir: &Path, source: Source, depth: usize, out: &mut Vec<Photo>) {
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    if depth > 0 && !name.ends_with(".photoslibrary") {
                        collect(&e.path(), source, depth - 1, out);
                    }
                } else if is_image(&name) {
                    let modified: DateTime<Local> = e
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH)
                        .into();
                    out.push(Photo {
                        path: e.path(),
                        name,
                        modified,
                        source,
                    });
                }
            }
        }
    }

    fn ensure_workers(&mut self) {
        if self.workers_started {
            return;
        }
        self.workers_started = true;
        let (tx, rx) = channel::<Loaded>();
        self.rx = Some(rx);
        self.tx = Some(tx.clone());
        for _ in 0..3 {
            let queue = self.pending.clone();
            let tx = tx.clone();
            std::thread::spawn(move || loop {
                let job = queue.lock().unwrap().pop();
                match job {
                    Some((idx, path, is_full)) => {
                        let max = if is_full { FULL_PX } else { THUMB_PX };
                        let msg = match decode(&path, max) {
                            Some(img) if is_full => Loaded::Full(idx, img),
                            Some(img) => Loaded::Thumb(idx, img),
                            None => Loaded::Failed(idx),
                        };
                        if tx.send(msg).is_err() {
                            return;
                        }
                    }
                    None => std::thread::sleep(std::time::Duration::from_millis(60)),
                }
            });
        }
    }

    fn drain(&mut self, ui: &Ui) {
        let Some(rx) = &self.rx else { return };
        let mut got = Vec::new();
        while let Ok(m) = rx.try_recv() {
            got.push(m);
        }
        for m in got {
            match m {
                Loaded::Thumb(idx, img) => {
                    let tex = ui.ctx().load_texture(
                        format!("photo-thumb-{idx}"),
                        img,
                        TextureOptions::LINEAR,
                    );
                    self.thumbs.insert(idx, tex);
                }
                Loaded::Full(idx, img) => {
                    let tex = ui.ctx().load_texture(
                        format!("photo-full-{idx}"),
                        img,
                        TextureOptions::LINEAR,
                    );
                    self.full_tex = Some((idx, tex));
                }
                Loaded::Failed(idx) => {
                    self.failed.insert(idx);
                }
            }
        }
    }

    fn request_thumb(&mut self, idx: usize) {
        if self.requested.contains(&idx) || self.failed.contains(&idx) {
            return;
        }
        self.requested.insert(idx);
        self.pending
            .lock()
            .unwrap()
            .push((idx, self.photos[idx].path.clone(), false));
    }

    fn request_full(&mut self, idx: usize) {
        if self.full_requested == Some(idx) {
            return;
        }
        self.full_requested = Some(idx);
        // 插队到最前
        self.pending
            .lock()
            .unwrap()
            .push((idx, self.photos[idx].path.clone(), true));
    }

    /// 当前侧边栏来源过滤后的索引集
    fn visible_indices(&self) -> Vec<usize> {
        (0..self.photos.len())
            .filter(|&i| self.source == Source::All || self.photos[i].source == self.source)
            .collect()
    }

    pub fn show(&mut self, ui: &mut Ui, now: f64) {
        let _ = now;
        if !self.scanned {
            self.scan();
        }
        self.ensure_workers();
        self.drain(ui);

        let full = ui.max_rect();
        ui.painter()
            .rect_filled(full, 0, Color32::from_rgb(0x1C, 0x1C, 0x20));

        // 单张查看器盖全区
        if self.viewer.is_some() {
            self.show_viewer(ui, full);
            return;
        }

        const SIDEBAR_W: f32 = 150.0;
        const TOOLBAR_H: f32 = 44.0;

        // ---- 侧边栏 ----
        let side = Rect::from_min_size(full.min, vec2(SIDEBAR_W, full.height()));
        ui.painter()
            .rect_filled(side, 0, Color32::from_rgb(0x25, 0x25, 0x2A));
        ui.painter().line_segment(
            [side.right_top(), side.right_bottom()],
            Stroke::new(1.0, Color32::from_gray(18)),
        );
        let groups: [(&str, &[(&str, Source)]); 2] = [
            ("图库", &[("照片", Source::All)]),
            (
                "相簿",
                &[
                    ("图片", Source::Pictures),
                    ("桌面", Source::Desktop),
                    ("下载", Source::Downloads),
                ],
            ),
        ];
        let mut y = side.top() + 14.0;
        for (title, items) in groups {
            ui.painter().text(
                pos2(side.left() + 14.0, y),
                Align2::LEFT_CENTER,
                title,
                FontId::proportional(11.0),
                Color32::from_gray(115),
            );
            y += 22.0;
            for (label, src) in items {
                let row = Rect::from_min_size(
                    pos2(side.left() + 8.0, y - 11.0),
                    vec2(SIDEBAR_W - 16.0, 24.0),
                );
                let resp = ui.interact(row, ui.id().with(("ph-side", label)), Sense::click());
                if self.source == *src {
                    ui.painter().rect_filled(
                        row,
                        CornerRadius::same(6),
                        Color32::from_rgb(0x3D, 0x3D, 0x46),
                    );
                } else if resp.hovered() {
                    ui.painter().rect_filled(
                        row,
                        CornerRadius::same(6),
                        Color32::from_rgb(0x2E, 0x2E, 0x35),
                    );
                }
                // 小图标：彩色花瓣缩微
                ui.painter().circle_filled(
                    pos2(row.left() + 14.0, row.center().y),
                    5.0,
                    Color32::from_rgb(0xFF, 0x6A, 0x9C),
                );
                ui.painter().text(
                    pos2(row.left() + 26.0, row.center().y),
                    Align2::LEFT_CENTER,
                    *label,
                    FontId::proportional(13.0),
                    Color32::from_gray(215),
                );
                if resp.clicked() {
                    self.source = *src;
                }
                y += 26.0;
            }
            y += 10.0;
        }

        // ---- 工具栏 ----
        let content_x = side.right();
        let toolbar = Rect::from_min_max(
            pos2(content_x, full.top()),
            pos2(full.right(), full.top() + TOOLBAR_H),
        );
        ui.painter()
            .rect_filled(toolbar, 0, Color32::from_rgb(0x22, 0x22, 0x27));
        ui.painter().line_segment(
            [toolbar.left_bottom(), toolbar.right_bottom()],
            Stroke::new(1.0, Color32::from_gray(18)),
        );
        ui.painter().text(
            pos2(toolbar.left() + 14.0, toolbar.center().y),
            Align2::LEFT_CENTER,
            "照片",
            FontId::proportional(15.0),
            Color32::from_gray(235),
        );

        // 分段控件（macOS Photos 的 年份/月份/日期/所有照片）
        let segs: [(&str, Segment); 4] = [
            ("年份", Segment::Years),
            ("月份", Segment::Months),
            ("日期", Segment::Days),
            ("所有照片", Segment::All),
        ];
        let seg_w = 66.0;
        let seg_total = seg_w * segs.len() as f32;
        let seg_rect = Rect::from_center_size(
            pos2(toolbar.center().x + 20.0, toolbar.center().y),
            vec2(seg_total, 26.0),
        );
        ui.painter().rect_filled(
            seg_rect,
            CornerRadius::same(7),
            Color32::from_rgb(0x2C, 0x2C, 0x32),
        );
        for (i, (label, seg)) in segs.iter().enumerate() {
            let r = Rect::from_min_size(
                pos2(seg_rect.left() + i as f32 * seg_w, seg_rect.top()),
                vec2(seg_w, seg_rect.height()),
            );
            let resp = ui.interact(r, ui.id().with(("ph-seg", i)), Sense::click());
            if self.segment == *seg {
                ui.painter().rect_filled(
                    r.shrink(2.0),
                    CornerRadius::same(6),
                    Color32::from_rgb(0x5A, 0x5A, 0x64),
                );
            }
            ui.painter().text(
                r.center(),
                Align2::CENTER_CENTER,
                *label,
                FontId::proportional(12.0),
                if self.segment == *seg {
                    Color32::WHITE
                } else {
                    Color32::from_gray(170)
                },
            );
            if resp.clicked() {
                self.segment = *seg;
            }
        }

        // 缩放滑杆（苹果风格控件）
        let slider_rect = Rect::from_min_max(
            pos2(toolbar.right() - 150.0, toolbar.top() + 12.0),
            pos2(toolbar.right() - 16.0, toolbar.bottom() - 12.0),
        );
        let mut zoom = self.cell;
        let mut zui = ui.new_child(egui::UiBuilder::new().max_rect(slider_rect));
        if super::widgets::slider(&mut zui, &mut zoom, 70.0..=220.0, slider_rect.width())
            .changed()
        {
            self.cell = zoom;
        }

        // ---- 网格 ----
        let grid_rect = Rect::from_min_max(
            pos2(content_x, toolbar.bottom()),
            pos2(full.right(), full.bottom()),
        );
        let indices = self.visible_indices();
        if indices.is_empty() {
            ui.painter().text(
                grid_rect.center(),
                Align2::CENTER_CENTER,
                "没有照片",
                FontId::proportional(15.0),
                Color32::from_gray(110),
            );
            return;
        }

        // 分组（年/月/日/全部）
        let group_key = |p: &Photo| -> String {
            match self.segment {
                Segment::Years => format!("{}年", p.modified.year()),
                Segment::Months => format!("{}年{}月", p.modified.year(), p.modified.month()),
                Segment::Days => format!(
                    "{}年{}月{}日",
                    p.modified.year(),
                    p.modified.month(),
                    p.modified.day()
                ),
                Segment::All => String::new(),
            }
        };
        let mut groups_list: Vec<(String, Vec<usize>)> = Vec::new();
        for &i in &indices {
            let key = group_key(&self.photos[i]);
            match groups_list.last_mut() {
                Some((k, v)) if *k == key => v.push(i),
                _ => groups_list.push((key, vec![i])),
            }
        }

        let mut grid_ui = ui.new_child(egui::UiBuilder::new().max_rect(grid_rect));
        grid_ui.set_clip_rect(grid_rect.intersect(ui.clip_rect()));
        let gap = 2.0;
        let pad = 10.0;
        let cols = (((grid_rect.width() - pad * 2.0) + gap) / (self.cell + gap))
            .floor()
            .max(1.0) as usize;

        let mut open_viewer: Option<usize> = None;
        let mut want_thumbs: Vec<usize> = Vec::new();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(grid_rect.height())
            .show(&mut grid_ui, |ui| {
                ui.set_width(ui.available_width());
                for (title, items) in &groups_list {
                    if !title.is_empty() {
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.add_space(pad);
                            ui.label(
                                egui::RichText::new(title)
                                    .size(16.0)
                                    .strong()
                                    .color(Color32::from_gray(230)),
                            );
                        });
                        ui.add_space(4.0);
                    } else {
                        ui.add_space(8.0);
                    }
                    let rows = items.len().div_ceil(cols);
                    for r in 0..rows {
                        ui.horizontal(|ui| {
                            ui.add_space(pad);
                            ui.spacing_mut().item_spacing = vec2(gap, gap);
                            for c in 0..cols {
                                let Some(&idx) = items.get(r * cols + c) else { break };
                                let (rect, resp) = ui.allocate_exact_size(
                                    vec2(self.cell, self.cell),
                                    Sense::click(),
                                );
                                if ui.is_rect_visible(rect) {
                                    if let Some(tex) = self.thumbs.get(&idx) {
                                        draw_cover(ui, rect, tex);
                                    } else {
                                        ui.painter().rect_filled(
                                            rect,
                                            0,
                                            Color32::from_rgb(0x2A, 0x2A, 0x30),
                                        );
                                        if self.failed.contains(&idx) {
                                            ui.painter().text(
                                                rect.center(),
                                                Align2::CENTER_CENTER,
                                                "✕",
                                                FontId::proportional(16.0),
                                                Color32::from_gray(80),
                                            );
                                        } else {
                                            want_thumbs.push(idx);
                                        }
                                    }
                                    if resp.hovered() {
                                        ui.painter().rect_filled(
                                            rect,
                                            0,
                                            Color32::from_white_alpha(10),
                                        );
                                    }
                                }
                                if resp.double_clicked() {
                                    open_viewer = Some(idx);
                                }
                            }
                        });
                    }
                }
                ui.add_space(12.0);
                // 底部计数（macOS Photos 风格）
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{} 张照片", indices.len()))
                            .size(12.5)
                            .color(Color32::from_gray(130)),
                    );
                });
                ui.add_space(10.0);
            });
        for idx in want_thumbs {
            self.request_thumb(idx);
        }
        if let Some(idx) = open_viewer {
            self.viewer = Some(idx);
            self.full_tex = None;
            self.full_requested = None;
            self.request_full(idx);
        }
        // 后台加载中保持重绘
        if !self.pending.lock().unwrap().is_empty() || !self.requested.is_empty() && self.thumbs.len() + self.failed.len() < self.requested.len() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(80));
        }
    }

    /// 单张查看器：黑底大图 + 顶栏返回 + 左右切换
    fn show_viewer(&mut self, ui: &mut Ui, full: Rect) {
        let Some(cur) = self.viewer else { return };
        let indices = self.visible_indices();
        let pos_in_list = indices.iter().position(|&i| i == cur).unwrap_or(0);

        ui.painter().rect_filled(full, 0, Color32::from_rgb(0x0E, 0x0E, 0x10));

        // 图片区
        let img_rect = Rect::from_min_max(
            pos2(full.left(), full.top() + 40.0),
            pos2(full.right(), full.bottom() - 36.0),
        );
        match &self.full_tex {
            Some((idx, tex)) if *idx == cur => {
                let ts = tex.size_vec2();
                let scale = (img_rect.width() / ts.x)
                    .min(img_rect.height() / ts.y)
                    .min(1.0);
                let draw = Rect::from_center_size(img_rect.center(), ts * scale);
                ui.painter().image(
                    tex.id(),
                    draw,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            _ => {
                // 退化：先用缩略图占位
                if let Some(tex) = self.thumbs.get(&cur) {
                    let ts = tex.size_vec2();
                    let scale = (img_rect.width() / ts.x).min(img_rect.height() / ts.y);
                    let draw = Rect::from_center_size(img_rect.center(), ts * scale);
                    ui.painter().image(
                        tex.id(),
                        draw,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                ui.painter().text(
                    pos2(img_rect.center().x, img_rect.bottom() - 16.0),
                    Align2::CENTER_CENTER,
                    "正在载入…",
                    FontId::proportional(12.0),
                    Color32::from_gray(120),
                );
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(80));
            }
        }

        // 顶栏：返回 + 文件名
        let back = Rect::from_min_size(pos2(full.left() + 10.0, full.top() + 8.0), vec2(74.0, 26.0));
        let bresp = ui.interact(back, ui.id().with("ph-back"), Sense::click());
        ui.painter().rect_filled(
            back,
            CornerRadius::same(7),
            if bresp.hovered() {
                Color32::from_gray(60)
            } else {
                Color32::from_gray(42)
            },
        );
        ui.painter().text(
            back.center(),
            Align2::CENTER_CENTER,
            "‹ 照片",
            FontId::proportional(12.5),
            Color32::from_gray(225),
        );
        let p = &self.photos[cur];
        // 文件名截断：长名不与左侧「‹ 照片」返回按钮重叠
        let name_font = FontId::proportional(13.0);
        let shown = super::truncate_text(ui.painter(), &p.name, name_font.clone(), full.width() - 220.0);
        ui.painter().text(
            pos2(full.center().x, full.top() + 21.0),
            Align2::CENTER_CENTER,
            shown,
            name_font,
            Color32::from_gray(220),
        );
        // 底部：日期 + 位置序号
        ui.painter().text(
            pos2(full.center().x, full.bottom() - 18.0),
            Align2::CENTER_CENTER,
            format!(
                "{}　·　{} / {}",
                p.modified.format("%Y年%m月%d日 %H:%M"),
                pos_in_list + 1,
                indices.len()
            ),
            FontId::proportional(12.0),
            Color32::from_gray(140),
        );

        // 左右切换按钮
        let mut step: i64 = 0;
        for (dx, glyph, d) in [(28.0, "‹", -1i64), (full.width() - 28.0, "›", 1i64)] {
            let r = Rect::from_center_size(
                pos2(full.left() + dx, full.center().y),
                vec2(36.0, 56.0),
            );
            let resp = ui.interact(r, ui.id().with(("ph-nav", d)), Sense::click());
            ui.painter().text(
                r.center(),
                Align2::CENTER_CENTER,
                glyph,
                FontId::proportional(34.0),
                if resp.hovered() {
                    Color32::WHITE
                } else {
                    Color32::from_gray(120)
                },
            );
            if resp.clicked() {
                step = d;
            }
        }
        if ui.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
            step = -1;
        }
        if ui.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
            step = 1;
        }
        if step != 0 && !indices.is_empty() {
            let next = (pos_in_list as i64 + step).rem_euclid(indices.len() as i64) as usize;
            let idx = indices[next];
            self.viewer = Some(idx);
            self.request_full(idx);
            self.request_thumb(idx);
        }
        if bresp.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.viewer = None;
        }
    }
}

/// 方块内 cover 模式绘制（居中裁切，macOS Photos 网格同款）
fn draw_cover(ui: &Ui, rect: Rect, tex: &TextureHandle) {
    let ts = tex.size_vec2();
    let (uw, uh) = if ts.x / ts.y > 1.0 {
        // 宽图：裁左右
        let w = ts.y / ts.x;
        ((1.0 - w) / 2.0, 0.0)
    } else {
        let h = ts.x / ts.y;
        (0.0, (1.0 - h) / 2.0)
    };
    let uv = Rect::from_min_max(pos2(uw, uh), pos2(1.0 - uw, 1.0 - uh));
    ui.painter().image(tex.id(), rect, uv, Color32::WHITE);
}
