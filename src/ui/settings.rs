//! 系统设置：控件实时修改 [`DesktopConfig`]，立刻反映到壁纸 / Dock / 菜单栏。

use egui::{vec2, Align, Color32, CornerRadius, FontId, Layout, RichText, Stroke, StrokeKind, Ui};

use crate::config::{DesktopConfig, Wallpaper};

pub fn show(ui: &mut Ui, cfg: &mut DesktopConfig) {
    ui.painter()
        .rect_filled(ui.max_rect(), 0, Color32::from_rgb(0x1B, 0x1B, 0x1F));

    let full = ui.max_rect();
    let mut body = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(full.shrink2(vec2(22.0, 18.0)))
            .layout(Layout::top_down(Align::Min)),
    );
    let ui = &mut body;
    ui.set_width(ui.available_width());

    ui.label(
        RichText::new("系统设置")
            .size(20.0)
            .strong()
            .color(Color32::from_gray(235)),
    );
    ui.add_space(14.0);

    // ---- 壁纸主题 ----
    section(ui, "桌面壁纸");
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        for wp in Wallpaper::ALL {
            let selected = cfg.wallpaper == wp;
            let (rect, resp) = ui.allocate_exact_size(vec2(96.0, 58.0), egui::Sense::click());
            // 预览：用主题首尾色画个竖直渐变缩略
            let stops = wp.stops();
            let top = stops.first().map(|s| s.1).unwrap_or(Color32::BLACK);
            let bot = stops.last().map(|s| s.1).unwrap_or(Color32::BLACK);
            let p = ui.painter();
            let mut mesh = egui::Mesh::default();
            let i = mesh.vertices.len() as u32;
            mesh.colored_vertex(rect.left_top(), top);
            mesh.colored_vertex(rect.right_top(), top);
            mesh.colored_vertex(rect.right_bottom(), bot);
            mesh.colored_vertex(rect.left_bottom(), bot);
            mesh.add_triangle(i, i + 1, i + 2);
            mesh.add_triangle(i, i + 2, i + 3);
            // 圆角裁剪用 rect_filled 盖边
            p.add(egui::Shape::mesh(mesh));
            p.rect_stroke(
                rect,
                CornerRadius::same(8),
                Stroke::new(
                    if selected { 2.5 } else { 1.0 },
                    if selected {
                        Color32::from_rgb(0x4C, 0x8D, 0xFF)
                    } else {
                        Color32::from_gray(60)
                    },
                ),
                StrokeKind::Inside,
            );
            p.text(
                rect.center_bottom() + vec2(0.0, -8.0),
                egui::Align2::CENTER_CENTER,
                wp.name(),
                FontId::proportional(11.5),
                Color32::from_gray(235),
            );
            if resp.clicked() {
                cfg.wallpaper = wp;
            }
            ui.add_space(4.0);
        }
    });
    ui.add_space(16.0);

    // ---- Dock ----
    section(ui, "程序坞");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("图标大小").color(Color32::from_gray(190)));
        super::widgets::slider(ui, &mut cfg.dock_scale, 0.7..=1.4, 170.0);
        ui.label(
            RichText::new(format!("{:.0}px", cfg.dock_base()))
                .color(Color32::from_gray(140))
                .size(12.0),
        );
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new("邻近放大").color(Color32::from_gray(190)));
        super::widgets::slider(ui, &mut cfg.dock_magnify, 0.0..=1.2, 170.0);
        ui.label(
            RichText::new(if cfg.dock_magnify <= 0.01 {
                "关闭".to_owned()
            } else {
                format!("{:.0}%", cfg.dock_magnify * 100.0)
            })
            .color(Color32::from_gray(140))
            .size(12.0),
        );
    });
    ui.add_space(16.0);

    // ---- 菜单栏 ----
    section(ui, "菜单栏");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        super::widgets::toggle(ui, &mut cfg.show_seconds);
        ui.label(RichText::new("时钟显示秒").color(Color32::from_gray(200)));
    });

    ui.add_space(20.0);
    ui.label(
        RichText::new("以上设置实时生效，影响桌面壁纸、程序坞与菜单栏。")
            .color(Color32::from_gray(90))
            .size(11.5)
            .italics(),
    );
}

fn section(ui: &mut Ui, title: &str) {
    ui.label(
        RichText::new(title)
            .size(13.0)
            .strong()
            .color(Color32::from_rgb(0x8A, 0x9A, 0xD0)),
    );
}
