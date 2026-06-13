pub mod agent;
pub mod browser;
pub mod chrome;
pub mod desktop;
pub mod dock;
pub mod finder;
pub mod icon;
pub mod launchpad;
pub mod mail;
pub mod maps;
pub mod menubar;
pub mod music;
pub mod photos;
pub mod reminders;
pub mod settings;
pub mod terminal;
pub mod trash;
pub mod wechat;
pub mod widgets;

use egui::Color32;

/// 字节数 -> 人类可读
pub fn human_size(bytes: u64) -> String {
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

pub fn white_a(a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(255, 255, 255, (a.clamp(0.0, 1.0) * 255.0) as u8)
}

pub fn black_a(a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(0, 0, 0, (a.clamp(0.0, 1.0) * 255.0) as u8)
}

/// 把文本截断到指定像素宽，放不下时以「…」结尾（macOS 列表/标题的省略行为）。
/// painter 绘制的文本没有 Label::truncate 可用，列内容统一走这里防止越界堆叠。
pub fn truncate_text(
    painter: &egui::Painter,
    text: &str,
    font: egui::FontId,
    max_w: f32,
) -> String {
    let width = |s: String| {
        painter
            .layout_no_wrap(s, font.clone(), Color32::WHITE)
            .size()
            .x
    };
    if width(text.to_owned()) <= max_w {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    // 二分可容纳的字符数（按字符截断对 CJK/西文都安全）
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let candidate: String = chars[..mid].iter().collect::<String>() + "…";
        if width(candidate) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    if lo == 0 {
        "…".to_owned()
    } else {
        chars[..lo].iter().collect::<String>() + "…"
    }
}

/// 在 rect 上画一个竖直线性渐变（egui 没有渐变填充，用顶点着色 Mesh 实现）
pub fn gradient_rect(painter: &egui::Painter, rect: egui::Rect, top: Color32, bottom: Color32) {
    let mut mesh = egui::Mesh::default();
    let i = mesh.vertices.len() as u32;
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.add_triangle(i, i + 1, i + 2);
    mesh.add_triangle(i, i + 2, i + 3);
    painter.add(egui::Shape::mesh(mesh));
}
