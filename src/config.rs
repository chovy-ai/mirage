//! 桌面全局配置：被系统设置面板修改，实时驱动壁纸 / Dock / 菜单栏。

use egui::Color32;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Wallpaper {
    Aurora,
    Sunset,
    Ocean,
    Graphite,
}

impl Wallpaper {
    pub const ALL: [Wallpaper; 4] = [
        Wallpaper::Aurora,
        Wallpaper::Sunset,
        Wallpaper::Ocean,
        Wallpaper::Graphite,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Wallpaper::Aurora => "极光",
            Wallpaper::Sunset => "暮色",
            Wallpaper::Ocean => "深海",
            Wallpaper::Graphite => "石墨",
        }
    }

    /// 竖直渐变色标（0..1 位置 + RGB）
    pub fn stops(self) -> &'static [(f32, Color32)] {
        const AURORA: &[(f32, Color32)] = &[
            (0.00, Color32::from_rgb(0x0A, 0x0E, 0x26)),
            (0.42, Color32::from_rgb(0x2B, 0x1E, 0x5C)),
            (0.72, Color32::from_rgb(0x7A, 0x2F, 0x63)),
            (1.00, Color32::from_rgb(0xE8, 0x81, 0x4F)),
        ];
        const SUNSET: &[(f32, Color32)] = &[
            (0.00, Color32::from_rgb(0x2A, 0x10, 0x3A)),
            (0.45, Color32::from_rgb(0x8A, 0x2B, 0x5B)),
            (0.78, Color32::from_rgb(0xE3, 0x5D, 0x4E)),
            (1.00, Color32::from_rgb(0xF7, 0xB7, 0x4A)),
        ];
        const OCEAN: &[(f32, Color32)] = &[
            (0.00, Color32::from_rgb(0x05, 0x1A, 0x2E)),
            (0.45, Color32::from_rgb(0x0B, 0x3A, 0x5E)),
            (0.78, Color32::from_rgb(0x16, 0x7A, 0x96)),
            (1.00, Color32::from_rgb(0x5A, 0xD0, 0xC0)),
        ];
        const GRAPHITE: &[(f32, Color32)] = &[
            (0.00, Color32::from_rgb(0x14, 0x15, 0x18)),
            (0.50, Color32::from_rgb(0x26, 0x28, 0x2E)),
            (1.00, Color32::from_rgb(0x44, 0x48, 0x52)),
        ];
        match self {
            Wallpaper::Aurora => AURORA,
            Wallpaper::Sunset => SUNSET,
            Wallpaper::Ocean => OCEAN,
            Wallpaper::Graphite => GRAPHITE,
        }
    }

    /// 两团柔光的颜色（暖光、冷光）
    pub fn glows(self) -> (Color32, Color32) {
        match self {
            Wallpaper::Aurora => (
                Color32::from_rgb(0xFF, 0xB0, 0x60),
                Color32::from_rgb(0x6A, 0x4A, 0xC8),
            ),
            Wallpaper::Sunset => (
                Color32::from_rgb(0xFF, 0xC0, 0x70),
                Color32::from_rgb(0xC0, 0x4A, 0x9A),
            ),
            Wallpaper::Ocean => (
                Color32::from_rgb(0x6A, 0xE0, 0xD0),
                Color32::from_rgb(0x2A, 0x6A, 0xC8),
            ),
            Wallpaper::Graphite => (
                Color32::from_rgb(0x8A, 0x90, 0xA0),
                Color32::from_rgb(0x50, 0x54, 0x60),
            ),
        }
    }
}

pub struct DesktopConfig {
    pub wallpaper: Wallpaper,
    /// Dock 图标基础大小的倍率（1.0 = 54px）
    pub dock_scale: f32,
    /// Dock 邻近放大强度（0 = 关闭）
    pub dock_magnify: f32,
    /// 菜单栏时钟是否显示秒
    pub show_seconds: bool,
}

impl Default for DesktopConfig {
    fn default() -> Self {
        Self {
            wallpaper: Wallpaper::Aurora,
            dock_scale: 1.0,
            dock_magnify: 0.65,
            show_seconds: false,
        }
    }
}

impl DesktopConfig {
    pub const DOCK_BASE: f32 = 54.0;

    pub fn dock_base(&self) -> f32 {
        Self::DOCK_BASE * self.dock_scale
    }
}
