//! 音乐：一比一还原 macOS「音乐」——顶部播放工具栏（走带控制 + LCD 显示屏 +
//! 音量），左侧资料库侧边栏（最近添加/艺人/专辑/歌曲 + 搜索过滤），
//! 歌曲表格视图与专辑网格视图。真实播放：rodio(symphonia) 解码本机音频，
//! lofty 后台线程读标签（标题/艺人/专辑/时长）与内嵌封面。
//! 扫描 ~/Music 与 ~/Downloads 的 mp3/m4a/flac/wav/ogg/aiff。

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Local};
use egui::{
    pos2, vec2, Align2, Color32, ColorImage, CornerRadius, FontId, Rect, Sense, Shape, Stroke,
    StrokeKind, TextureHandle, TextureOptions, Ui,
};

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::Accessor;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};

const MAX_TRACKS: usize = 500;
const ART_PX: u32 = 256;
/// Apple Music 红
const ACCENT: Color32 = Color32::from_rgb(0xFC, 0x3C, 0x44);

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Recent,
    Artists,
    Albums,
    Songs,
}

struct Track {
    path: PathBuf,
    /// 标签未到前用文件名
    title: String,
    artist: String,
    album: String,
    /// 秒；0 = 未知
    duration: f32,
    added: DateTime<Local>,
}

/// 后台标签线程产出
struct Meta {
    idx: usize,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    duration: f32,
    art: Option<ColorImage>,
}

pub struct MusicApp {
    scanned: bool,
    tracks: Vec<Track>,
    rx: Option<Receiver<Meta>>,
    arts: HashMap<usize, TextureHandle>,
    view: View,
    search: String,
    selected: Option<usize>,
    /// 播放
    stream: Option<(OutputStream, OutputStreamHandle)>,
    stream_failed: bool,
    sink: Option<Sink>,
    current: Option<usize>,
    paused: bool,
    volume: f32,
    /// 播放队列（开播时的可见顺序）
    queue: Vec<usize>,
    autoplayed: bool,
}

impl Default for MusicApp {
    fn default() -> Self {
        Self {
            scanned: false,
            tracks: Vec::new(),
            rx: None,
            arts: HashMap::new(),
            // 截图回归可指定初始视图：MIRAGE_MUSIC_VIEW=albums|artists|recent
            view: match std::env::var("MIRAGE_MUSIC_VIEW").as_deref() {
                Ok("albums") => View::Albums,
                Ok("artists") => View::Artists,
                Ok("recent") => View::Recent,
                _ => View::Songs,
            },
            search: String::new(),
            selected: None,
            stream: None,
            stream_failed: false,
            sink: None,
            current: None,
            paused: false,
            volume: 0.8,
            queue: Vec::new(),
            autoplayed: false,
        }
    }
}

fn is_audio(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    matches!(
        ext.as_str(),
        "mp3" | "m4a" | "aac" | "flac" | "wav" | "ogg" | "aif" | "aiff" | "alac"
    )
}

fn fmt_time(secs: f32) -> String {
    if secs <= 0.0 {
        return "-:--".into();
    }
    let s = secs.round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

impl MusicApp {
    // ---------- 库扫描与标签 ----------

    fn scan(&mut self) {
        self.scanned = true;
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into()));
        let mut out = Vec::new();
        // iTunes 媒体库实际在 ~/Music/Music/Media.localized/Music/艺人/专辑/x.m4a，给足深度
        collect(&home.join("Music"), 6, &mut out);
        collect(&home.join("Downloads"), 2, &mut out);
        out.truncate(MAX_TRACKS);
        out.sort_by_key(|a| a.title.to_lowercase());
        self.tracks = out;

        fn collect(dir: &Path, depth: usize, out: &mut Vec<Track>) {
            if out.len() >= MAX_TRACKS {
                return;
            }
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    // .musiclibrary / .photoslibrary 等包不进
                    if depth > 0 && !name.contains(".musiclibrary") {
                        collect(&e.path(), depth - 1, out);
                    }
                } else if is_audio(&name) {
                    let added: DateTime<Local> = e
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH)
                        .into();
                    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(&name);
                    out.push(Track {
                        path: e.path(),
                        title: stem.to_owned(),
                        artist: "未知艺人".into(),
                        album: "未知专辑".into(),
                        duration: 0.0,
                        added,
                    });
                }
            }
        }
    }

    /// 后台线程一次性读完所有标签与封面
    fn spawn_tagger(&mut self) {
        let (tx, rx) = channel::<Meta>();
        self.rx = Some(rx);
        let jobs: Vec<(usize, PathBuf)> = self
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (i, t.path.clone()))
            .collect();
        std::thread::spawn(move || {
            for (idx, path) in jobs {
                let meta = read_meta(idx, &path);
                if tx.send(meta).is_err() {
                    return;
                }
            }
        });

        fn read_meta(idx: usize, path: &Path) -> Meta {
            let mut m = Meta {
                idx,
                title: None,
                artist: None,
                album: None,
                duration: 0.0,
                art: None,
            };
            let Ok(probe) = Probe::open(path) else { return m };
            let Ok(tagged) = probe.read() else { return m };
            m.duration = tagged.properties().duration().as_secs_f32();
            if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
                m.title = tag.title().map(|s| s.into_owned());
                m.artist = tag.artist().map(|s| s.into_owned());
                m.album = tag.album().map(|s| s.into_owned());
                if let Some(pic) = tag.pictures().first() {
                    if let Ok(img) = image::load_from_memory(pic.data()) {
                        let img = img.thumbnail(ART_PX, ART_PX).to_rgba8();
                        let size = [img.width() as usize, img.height() as usize];
                        m.art = Some(ColorImage::from_rgba_unmultiplied(size, img.as_raw()));
                    }
                }
            }
            m
        }
    }

    fn drain(&mut self, ui: &Ui) {
        let Some(rx) = &self.rx else { return };
        let mut got = Vec::new();
        while let Ok(m) = rx.try_recv() {
            got.push(m);
        }
        for m in got {
            if let Some(t) = self.tracks.get_mut(m.idx) {
                if let Some(v) = m.title {
                    t.title = v;
                }
                if let Some(v) = m.artist {
                    t.artist = v;
                }
                if let Some(v) = m.album {
                    t.album = v;
                }
                if m.duration > 0.0 {
                    t.duration = m.duration;
                }
            }
            if let Some(img) = m.art {
                let tex = ui.ctx().load_texture(
                    format!("music-art-{}", m.idx),
                    img,
                    TextureOptions::LINEAR,
                );
                self.arts.insert(m.idx, tex);
            }
        }
    }

    // ---------- 播放控制 ----------

    fn ensure_stream(&mut self) {
        if self.stream.is_some() || self.stream_failed {
            return;
        }
        match OutputStream::try_default() {
            Ok(s) => self.stream = Some(s),
            Err(_) => self.stream_failed = true,
        }
    }

    fn play(&mut self, idx: usize, queue: Vec<usize>) {
        self.ensure_stream();
        let Some((_, handle)) = &self.stream else { return };
        let Ok(file) = File::open(&self.tracks[idx].path) else { return };
        // rodio 0.20 的 symphonia 初始化遇到 Seek 错误会直接 unreachable! panic
        // （Result 接不住），坏文件/不支持的封装会带崩整个桌面——catch_unwind 兜底。
        let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Decoder::new(BufReader::new(file))
        }));
        let Ok(Ok(src)) = decoded else { return };
        if let Some(old) = self.sink.take() {
            old.stop();
        }
        let Ok(sink) = Sink::try_new(handle) else { return };
        sink.set_volume(self.volume);
        sink.append(src);
        self.sink = Some(sink);
        self.current = Some(idx);
        self.selected = Some(idx);
        self.paused = false;
        if !queue.is_empty() {
            self.queue = queue;
        }
    }

    fn toggle_pause(&mut self) {
        let Some(sink) = &self.sink else {
            // 没在播：从选中或第一首开始
            let order = self.songs_order();
            let pick = self
                .selected
                .filter(|s| order.contains(s))
                .or_else(|| order.first().copied());
            if let Some(first) = pick {
                self.play(first, order);
            }
            return;
        };
        if self.paused {
            sink.play();
            self.paused = false;
        } else {
            sink.pause();
            self.paused = true;
        }
    }

    fn stop_playback(&mut self) {
        if let Some(s) = self.sink.take() {
            s.stop();
        }
        self.current = None;
        self.paused = false;
    }

    /// step = ±1。上一曲：播放超过 3s 先回到曲首（macOS 行为）。
    fn skip(&mut self, step: i64) {
        let Some(cur) = self.current else { return };
        if step < 0 {
            if let Some(sink) = &self.sink {
                if sink.get_pos().as_secs_f32() > 3.0 {
                    let _ = sink.try_seek(Duration::ZERO);
                    return;
                }
            }
        }
        let pos = self.queue.iter().position(|&i| i == cur);
        let next = match pos {
            Some(p) => {
                let n = p as i64 + step;
                if n < 0 || n >= self.queue.len() as i64 {
                    None
                } else {
                    Some(self.queue[n as usize])
                }
            }
            None => None,
        };
        match next {
            Some(idx) => self.play(idx, Vec::new()),
            None => self.stop_playback(),
        }
    }

    /// 每帧驱动：曲终自动下一首（窗口关着也照常走）
    pub fn tick(&mut self) {
        if self.paused || self.current.is_none() {
            return;
        }
        if self.sink.as_ref().is_some_and(|s| s.empty()) {
            self.skip(1);
        }
    }

    pub fn playing(&self) -> bool {
        self.current.is_some() && !self.paused
    }

    // ---------- 过滤与分组 ----------

    fn matches_search(&self, t: &Track) -> bool {
        if self.search.is_empty() {
            return true;
        }
        let q = self.search.to_lowercase();
        t.title.to_lowercase().contains(&q)
            || t.artist.to_lowercase().contains(&q)
            || t.album.to_lowercase().contains(&q)
    }

    /// 歌曲视图顺序（按标题排序后的过滤索引）
    fn songs_order(&self) -> Vec<usize> {
        (0..self.tracks.len())
            .filter(|&i| self.matches_search(&self.tracks[i]))
            .collect()
    }

    /// 专辑分组：(专辑名, 艺人, 曲目下标列表)，曲目按文件名序
    fn albums(&self, recent_first: bool) -> Vec<(String, String, Vec<usize>)> {
        let mut map: Vec<(String, String, Vec<usize>)> = Vec::new();
        for i in self.songs_order() {
            let t = &self.tracks[i];
            match map.iter_mut().find(|(a, ..)| *a == t.album) {
                Some((_, _, v)) => v.push(i),
                None => map.push((t.album.clone(), t.artist.clone(), vec![i])),
            }
        }
        if recent_first {
            map.sort_by_key(|(_, _, v)| {
                std::cmp::Reverse(v.iter().map(|&i| self.tracks[i].added).max())
            });
        }
        map
    }

    // ---------- UI ----------

    pub fn show(&mut self, ui: &mut Ui) {
        if !self.scanned {
            self.scan();
            if !self.tracks.is_empty() {
                self.spawn_tagger();
            }
        }
        self.drain(ui);

        // 自检/演示：MIRAGE_AUTOPLAY=1 时自动播放第一首
        if !self.autoplayed && std::env::var("MIRAGE_AUTOPLAY").is_ok() {
            self.autoplayed = true;
            let order = self.songs_order();
            if let Some(&first) = order.first() {
                self.play(first, order);
            }
        }

        let full = ui.max_rect();
        ui.painter()
            .rect_filled(full, 0, Color32::from_rgb(0x1C, 0x1C, 0x20));

        const TOOLBAR_H: f32 = 56.0;
        const SIDEBAR_W: f32 = 185.0;

        let toolbar = Rect::from_min_size(full.min, vec2(full.width(), TOOLBAR_H));
        self.show_toolbar(ui, toolbar);

        let side = Rect::from_min_max(
            pos2(full.left(), toolbar.bottom()),
            pos2(full.left() + SIDEBAR_W, full.bottom()),
        );
        self.show_sidebar(ui, side);

        let content = Rect::from_min_max(
            pos2(side.right(), toolbar.bottom()),
            full.max,
        );
        if self.tracks.is_empty() {
            let c = content.center();
            ui.painter().circle_filled(c - vec2(0.0, 30.0), 34.0, Color32::from_gray(45));
            ui.painter().text(
                c - vec2(0.0, 30.0),
                Align2::CENTER_CENTER,
                "♫",
                FontId::proportional(34.0),
                Color32::from_gray(120),
            );
            ui.painter().text(
                c + vec2(0.0, 24.0),
                Align2::CENTER_CENTER,
                "没有音乐",
                FontId::proportional(16.0),
                Color32::from_gray(200),
            );
            ui.painter().text(
                c + vec2(0.0, 46.0),
                Align2::CENTER_CENTER,
                "将音频文件放入「音乐」或「下载」文件夹后重新打开",
                FontId::proportional(12.0),
                Color32::from_gray(120),
            );
            return;
        }
        match self.view {
            View::Songs => self.show_songs(ui, content),
            View::Albums => self.show_albums(ui, content, false),
            View::Recent => self.show_albums(ui, content, true),
            View::Artists => self.show_artists(ui, content),
        }

        // 播放中持续刷新进度
        if self.playing() {
            ui.ctx()
                .request_repaint_after(Duration::from_millis(200));
        }
    }

    fn show_toolbar(&mut self, ui: &mut Ui, bar: Rect) {
        let p = ui.painter().clone();
        p.rect_filled(bar, 0, Color32::from_rgb(0x28, 0x28, 0x2D));
        p.line_segment(
            [bar.left_bottom(), bar.right_bottom()],
            Stroke::new(1.0, Color32::from_gray(15)),
        );

        // -- 左：走带控制 --
        let cy = bar.center().y;
        let mut clicked: Option<&str> = None;
        for (i, kind) in ["prev", "play", "next"].iter().enumerate() {
            let cx = bar.left() + 36.0 + i as f32 * 44.0;
            let r = Rect::from_center_size(pos2(cx, cy), vec2(36.0, 32.0));
            let resp = ui.interact(r, ui.id().with(("mu-tp", i)), Sense::click());
            let col = if resp.hovered() {
                Color32::WHITE
            } else {
                Color32::from_gray(205)
            };
            draw_transport(&p, r.center(), kind, self.playing(), col);
            if resp.clicked() {
                clicked = Some(kind);
            }
        }
        match clicked {
            Some("play") => self.toggle_pause(),
            Some("prev") => self.skip(-1),
            Some("next") => self.skip(1),
            _ => {}
        }

        // -- 右：音量 --
        let vol_rect = Rect::from_min_max(
            pos2(bar.right() - 140.0, cy - 10.0),
            pos2(bar.right() - 14.0, cy + 10.0),
        );
        // 小喇叭（箱体 + 喇叭口分开画，整体非凸不能用一个 convex_polygon）
        let sp = pos2(vol_rect.left() - 14.0, cy);
        let spk = Color32::from_gray(170);
        p.rect_filled(
            Rect::from_min_max(pos2(sp.x - 6.0, sp.y - 2.5), pos2(sp.x - 1.0, sp.y + 2.5)),
            CornerRadius::same(1),
            spk,
        );
        p.add(Shape::convex_polygon(
            vec![
                pos2(sp.x - 2.0, sp.y - 2.5),
                pos2(sp.x + 3.0, sp.y - 7.0),
                pos2(sp.x + 3.0, sp.y + 7.0),
                pos2(sp.x - 2.0, sp.y + 2.5),
            ],
            spk,
            Stroke::NONE,
        ));
        let mut vol = self.volume;
        let mut vui = ui.new_child(egui::UiBuilder::new().max_rect(vol_rect));
        vui.spacing_mut().slider_width = vol_rect.width() - 6.0;
        if vui
            .add(egui::Slider::new(&mut vol, 0.0..=1.0).show_value(false))
            .changed()
        {
            self.volume = vol;
            if let Some(sink) = &self.sink {
                sink.set_volume(vol);
            }
        }

        // -- 中：LCD 显示屏 --
        let lcd_w = (bar.width() - 420.0).clamp(260.0, 460.0);
        let lcd = Rect::from_center_size(
            pos2(bar.center().x, cy),
            vec2(lcd_w, bar.height() - 14.0),
        );
        p.rect_filled(lcd, CornerRadius::same(6), Color32::from_rgb(0x1E, 0x1E, 0x23));
        p.rect_stroke(
            lcd,
            CornerRadius::same(6),
            Stroke::new(1.0, Color32::from_gray(20)),
            StrokeKind::Inside,
        );
        let Some(cur) = self.current else {
            p.text(
                lcd.center(),
                Align2::CENTER_CENTER,
                "♫",
                FontId::proportional(18.0),
                Color32::from_gray(95),
            );
            return;
        };

        // 封面缩略
        let art_r = Rect::from_min_size(
            pos2(lcd.left(), lcd.top()),
            vec2(lcd.height(), lcd.height()),
        );
        if let Some(tex) = self.arts.get(&cur) {
            p.image(
                tex.id(),
                art_r,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            p.rect_filled(art_r, 0, Color32::from_gray(40));
            p.text(
                art_r.center(),
                Align2::CENTER_CENTER,
                "♫",
                FontId::proportional(15.0),
                Color32::from_gray(110),
            );
        }
        let (title, sub, dur) = {
            let t = &self.tracks[cur];
            (
                t.title.clone(),
                format!("{} — {}", t.artist, t.album),
                t.duration,
            )
        };
        let tx = art_r.right() + 10.0;
        p.text(
            pos2(lcd.left() + (lcd.width() + art_r.width()) / 2.0, lcd.top() + 11.0),
            Align2::CENTER_CENTER,
            title,
            FontId::proportional(12.5),
            Color32::from_gray(235),
        );
        p.text(
            pos2(lcd.left() + (lcd.width() + art_r.width()) / 2.0, lcd.top() + 25.0),
            Align2::CENTER_CENTER,
            sub,
            FontId::proportional(10.5),
            Color32::from_gray(140),
        );

        // 进度条（可点击拖动跳转）
        let pos_s = self
            .sink
            .as_ref()
            .map(|s| s.get_pos().as_secs_f32())
            .unwrap_or(0.0);
        let total = if dur > 0.0 { dur } else { pos_s.max(1.0) };
        let bar_r = Rect::from_min_max(
            pos2(tx + 30.0, lcd.bottom() - 9.0),
            pos2(lcd.right() - 38.0, lcd.bottom() - 6.0),
        );
        let resp = ui.interact(
            bar_r.expand2(vec2(0.0, 5.0)),
            ui.id().with("mu-seek"),
            Sense::click_and_drag(),
        );
        p.rect_filled(bar_r, CornerRadius::same(2), Color32::from_gray(55));
        let frac = (pos_s / total).clamp(0.0, 1.0);
        let mut fill = bar_r;
        fill.set_right(bar_r.left() + bar_r.width() * frac);
        p.rect_filled(fill, CornerRadius::same(2), Color32::from_gray(165));
        if resp.clicked() || resp.dragged() {
            if let (Some(ptr), Some(sink)) = (resp.interact_pointer_pos(), &self.sink) {
                if dur > 0.0 {
                    let f = ((ptr.x - bar_r.left()) / bar_r.width()).clamp(0.0, 1.0);
                    let _ = sink.try_seek(Duration::from_secs_f32(f * dur));
                }
            }
        }
        p.text(
            pos2(tx, bar_r.center().y),
            Align2::LEFT_CENTER,
            fmt_time(pos_s),
            FontId::proportional(9.5),
            Color32::from_gray(130),
        );
        p.text(
            pos2(lcd.right() - 6.0, bar_r.center().y),
            Align2::RIGHT_CENTER,
            format!("-{}", fmt_time((total - pos_s).max(0.0))),
            FontId::proportional(9.5),
            Color32::from_gray(130),
        );
    }

    fn show_sidebar(&mut self, ui: &mut Ui, side: Rect) {
        let p = ui.painter().clone();
        p.rect_filled(side, 0, Color32::from_rgb(0x25, 0x25, 0x2A));
        p.line_segment(
            [side.right_top(), side.right_bottom()],
            Stroke::new(1.0, Color32::from_gray(18)),
        );

        // 搜索框
        let search_r = Rect::from_min_size(
            pos2(side.left() + 10.0, side.top() + 10.0),
            vec2(side.width() - 20.0, 24.0),
        );
        p.rect_filled(search_r, CornerRadius::same(6), Color32::from_rgb(0x1B, 0x1B, 0x1F));
        let edit_r = search_r.shrink2(vec2(6.0, 2.0));
        let resp = ui.put(
            edit_r,
            egui::TextEdit::singleline(&mut self.search)
                .frame(egui::Frame::NONE)
                .hint_text("搜索")
                .font(egui::TextStyle::Small),
        );
        let _ = resp;

        let items: [(&str, View); 4] = [
            ("最近添加", View::Recent),
            ("艺人", View::Artists),
            ("专辑", View::Albums),
            ("歌曲", View::Songs),
        ];
        let mut y = search_r.bottom() + 16.0;
        p.text(
            pos2(side.left() + 14.0, y),
            Align2::LEFT_CENTER,
            "资料库",
            FontId::proportional(11.0),
            Color32::from_gray(115),
        );
        y += 22.0;
        for (label, v) in items {
            let row = Rect::from_min_size(
                pos2(side.left() + 8.0, y - 12.0),
                vec2(side.width() - 16.0, 26.0),
            );
            let resp = ui.interact(row, ui.id().with(("mu-side", label)), Sense::click());
            if self.view == v {
                p.rect_filled(row, CornerRadius::same(6), Color32::from_rgb(0x3D, 0x3D, 0x46));
            } else if resp.hovered() {
                p.rect_filled(row, CornerRadius::same(6), Color32::from_rgb(0x2E, 0x2E, 0x35));
            }
            // 红色小图标点缀（Music 侧边栏图标都是红的）
            p.circle_filled(pos2(row.left() + 14.0, row.center().y), 5.0, ACCENT);
            p.text(
                pos2(row.left() + 26.0, row.center().y),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(13.0),
                Color32::from_gray(220),
            );
            if resp.clicked() {
                self.view = v;
            }
            y += 28.0;
        }

        // 底部统计
        p.text(
            pos2(side.left() + 14.0, side.bottom() - 16.0),
            Align2::LEFT_CENTER,
            format!("{} 首歌曲", self.tracks.len()),
            FontId::proportional(11.0),
            Color32::from_gray(110),
        );
    }

    fn show_songs(&mut self, ui: &mut Ui, content: Rect) {
        let p = ui.painter().clone();
        const HEADER_H: f32 = 26.0;
        const ROW_H: f32 = 30.0;

        // 列：标题 45% / 艺人 20% / 专辑 25% / 时长 10%
        let w = content.width();
        let cols = [
            (content.left() + 12.0, "歌名"),
            (content.left() + w * 0.45, "艺人"),
            (content.left() + w * 0.65, "专辑"),
            (content.left() + w * 0.90, "时长"),
        ];
        let header = Rect::from_min_size(content.min, vec2(w, HEADER_H));
        p.rect_filled(header, 0, Color32::from_rgb(0x22, 0x22, 0x27));
        for (x, label) in cols {
            p.text(
                pos2(x, header.center().y),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(11.0),
                Color32::from_gray(130),
            );
        }
        p.line_segment(
            [header.left_bottom(), header.right_bottom()],
            Stroke::new(1.0, Color32::from_gray(18)),
        );

        let list_rect = Rect::from_min_max(pos2(content.left(), header.bottom()), content.max);
        let order = self.songs_order();
        let mut play_req: Option<usize> = None;
        let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(list_rect));
        list_ui.set_clip_rect(list_rect.intersect(ui.clip_rect()));
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(list_rect.height())
            .show_rows(&mut list_ui, ROW_H, order.len(), |ui, range| {
                ui.set_width(ui.available_width());
                for vi in range {
                    let idx = order[vi];
                    let (rect, resp) = ui.allocate_exact_size(
                        vec2(list_rect.width(), ROW_H),
                        Sense::click(),
                    );
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    let playing_this = self.current == Some(idx);
                    // 斑马纹 + 选中高亮
                    if self.selected == Some(idx) {
                        ui.painter().rect_filled(rect, 0, Color32::from_rgb(0x3A, 0x3A, 0x44));
                    } else if vi % 2 == 1 {
                        ui.painter().rect_filled(rect, 0, Color32::from_rgb(0x20, 0x20, 0x25));
                    }
                    if resp.hovered() && self.selected != Some(idx) {
                        ui.painter().rect_filled(rect, 0, Color32::from_white_alpha(6));
                    }
                    let t = &self.tracks[idx];
                    // 播放中：扬声器红点
                    if playing_this {
                        ui.painter().text(
                            pos2(rect.left() + 4.0, rect.center().y),
                            Align2::LEFT_CENTER,
                            if self.paused { "❚❚" } else { "♪" },
                            FontId::proportional(10.0),
                            ACCENT,
                        );
                    }
                    let title_col = if playing_this {
                        ACCENT
                    } else {
                        Color32::from_gray(225)
                    };
                    let dur_s = fmt_time(t.duration);
                    let texts = [
                        (cols[0].0 + 6.0, t.title.as_str(), title_col, 13.0),
                        (cols[1].0, t.artist.as_str(), Color32::from_gray(160), 12.5),
                        (cols[2].0, t.album.as_str(), Color32::from_gray(160), 12.5),
                        (cols[3].0, dur_s.as_str(), Color32::from_gray(140), 12.0),
                    ];
                    for (x, s, col, size) in texts {
                        ui.painter().text(
                            pos2(x, rect.center().y),
                            Align2::LEFT_CENTER,
                            s,
                            FontId::proportional(size),
                            col,
                        );
                    }
                    if resp.clicked() {
                        self.selected = Some(idx);
                    }
                    if resp.double_clicked() {
                        play_req = Some(idx);
                    }
                }
            });
        if let Some(idx) = play_req {
            self.play(idx, order);
        }
    }

    /// 专辑网格（recent_first = 「最近添加」视图）
    fn show_albums(&mut self, ui: &mut Ui, content: Rect, recent_first: bool) {
        let albums = self.albums(recent_first);
        let p = ui.painter().clone();
        let _ = &p;
        const CARD: f32 = 150.0;
        const CARD_H: f32 = CARD + 42.0;
        let gap = 18.0;
        let pad = 18.0;
        let cols = (((content.width() - pad * 2.0) + gap) / (CARD + gap))
            .floor()
            .max(1.0) as usize;

        let mut play_req: Option<(usize, Vec<usize>)> = None;
        let mut grid_ui = ui.new_child(egui::UiBuilder::new().max_rect(content));
        grid_ui.set_clip_rect(content.intersect(ui.clip_rect()));
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(content.height())
            .show(&mut grid_ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space(pad);
                    ui.label(
                        egui::RichText::new(if recent_first { "最近添加" } else { "专辑" })
                            .size(20.0)
                            .strong()
                            .color(Color32::from_gray(235)),
                    );
                });
                ui.add_space(8.0);
                let rows = albums.len().div_ceil(cols);
                for r in 0..rows {
                    ui.horizontal(|ui| {
                        ui.add_space(pad);
                        ui.spacing_mut().item_spacing = vec2(gap, gap);
                        for c in 0..cols {
                            let Some((album, artist, ids)) = albums.get(r * cols + c) else {
                                break;
                            };
                            let (rect, resp) = ui.allocate_exact_size(
                                vec2(CARD, CARD_H),
                                Sense::click(),
                            );
                            if !ui.is_rect_visible(rect) {
                                continue;
                            }
                            let art = Rect::from_min_size(rect.min, vec2(CARD, CARD));
                            let tex = ids.iter().find_map(|i| self.arts.get(i));
                            if let Some(tex) = tex {
                                ui.painter().image(
                                    tex.id(),
                                    art,
                                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                                    Color32::WHITE,
                                );
                            } else {
                                ui.painter().rect_filled(
                                    art,
                                    CornerRadius::same(8),
                                    Color32::from_rgb(0x2E, 0x2E, 0x36),
                                );
                                ui.painter().text(
                                    art.center(),
                                    Align2::CENTER_CENTER,
                                    "♫",
                                    FontId::proportional(40.0),
                                    Color32::from_gray(95),
                                );
                            }
                            ui.painter().rect_stroke(
                                art,
                                CornerRadius::same(8),
                                Stroke::new(1.0, Color32::from_white_alpha(14)),
                                StrokeKind::Inside,
                            );
                            if resp.hovered() {
                                ui.painter().rect_filled(
                                    art,
                                    CornerRadius::same(8),
                                    Color32::from_white_alpha(10),
                                );
                            }
                            ui.painter().text(
                                pos2(rect.left() + 2.0, art.bottom() + 12.0),
                                Align2::LEFT_CENTER,
                                truncate(album, 16),
                                FontId::proportional(12.5),
                                Color32::from_gray(225),
                            );
                            ui.painter().text(
                                pos2(rect.left() + 2.0, art.bottom() + 28.0),
                                Align2::LEFT_CENTER,
                                truncate(artist, 18),
                                FontId::proportional(11.5),
                                Color32::from_gray(140),
                            );
                            if resp.double_clicked() {
                                if let Some(&first) = ids.first() {
                                    play_req = Some((first, ids.clone()));
                                }
                            }
                        }
                    });
                }
                ui.add_space(14.0);
            });
        if let Some((idx, q)) = play_req {
            self.play(idx, q);
        }
    }

    /// 艺人视图：按艺人分组的歌曲列表
    fn show_artists(&mut self, ui: &mut Ui, content: Rect) {
        let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
        for i in self.songs_order() {
            let a = self.tracks[i].artist.clone();
            match groups.iter_mut().find(|(g, _)| *g == a) {
                Some((_, v)) => v.push(i),
                None => groups.push((a, vec![i])),
            }
        }
        groups.sort_by_key(|a| a.0.to_lowercase());

        let mut play_req: Option<(usize, Vec<usize>)> = None;
        let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(content));
        list_ui.set_clip_rect(content.intersect(ui.clip_rect()));
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(content.height())
            .show(&mut list_ui, |ui| {
                ui.set_width(ui.available_width());
                for (artist, ids) in &groups {
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        // 圆形头像占位
                        let (av, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
                        ui.painter().circle_filled(av.center(), 17.0, Color32::from_rgb(0x3A, 0x3A, 0x44));
                        ui.painter().text(
                            av.center(),
                            Align2::CENTER_CENTER,
                            artist.chars().next().unwrap_or('♪').to_string(),
                            FontId::proportional(15.0),
                            Color32::from_gray(200),
                        );
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(artist)
                                .size(16.0)
                                .strong()
                                .color(Color32::from_gray(230)),
                        );
                        ui.label(
                            egui::RichText::new(format!("　{} 首", ids.len()))
                                .size(12.0)
                                .color(Color32::from_gray(130)),
                        );
                    });
                    ui.add_space(4.0);
                    for &idx in ids {
                        let (rect, resp) = ui.allocate_exact_size(
                            vec2(ui.available_width(), 26.0),
                            Sense::click(),
                        );
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        let playing_this = self.current == Some(idx);
                        if resp.hovered() {
                            ui.painter().rect_filled(rect, 0, Color32::from_white_alpha(6));
                        }
                        let t = &self.tracks[idx];
                        ui.painter().text(
                            pos2(rect.left() + 58.0, rect.center().y),
                            Align2::LEFT_CENTER,
                            &t.title,
                            FontId::proportional(12.5),
                            if playing_this { ACCENT } else { Color32::from_gray(210) },
                        );
                        ui.painter().text(
                            pos2(rect.right() - 16.0, rect.center().y),
                            Align2::RIGHT_CENTER,
                            fmt_time(t.duration),
                            FontId::proportional(11.5),
                            Color32::from_gray(130),
                        );
                        if resp.double_clicked() {
                            play_req = Some((idx, ids.clone()));
                        }
                    }
                }
                ui.add_space(12.0);
            });
        if let Some((idx, q)) = play_req {
            self.play(idx, q);
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let cut: String = s.chars().take(max - 1).collect();
        format!("{cut}…")
    }
}

/// 走带按钮图形：prev/next 双三角，play 三角，pause 双竖条
fn draw_transport(p: &egui::Painter, c: egui::Pos2, kind: &str, playing: bool, col: Color32) {
    let tri = |p: &egui::Painter, cx: f32, dir: f32, h: f32| {
        p.add(Shape::convex_polygon(
            vec![
                pos2(cx - dir * h * 0.5, c.y - h * 0.62),
                pos2(cx + dir * h * 0.5, c.y),
                pos2(cx - dir * h * 0.5, c.y + h * 0.62),
            ],
            col,
            Stroke::NONE,
        ));
    };
    match kind {
        "prev" => {
            tri(p, c.x - 4.0, -1.0, 9.0);
            tri(p, c.x + 5.0, -1.0, 9.0);
        }
        "next" => {
            tri(p, c.x - 5.0, 1.0, 9.0);
            tri(p, c.x + 4.0, 1.0, 9.0);
        }
        _ if playing => {
            // 暂停：双竖条
            for dx in [-4.0, 4.0] {
                p.rect_filled(
                    Rect::from_center_size(pos2(c.x + dx, c.y), vec2(4.5, 16.0)),
                    CornerRadius::same(2),
                    col,
                );
            }
        }
        _ => tri(p, c.x + 1.0, 1.0, 13.0),
    }
}
