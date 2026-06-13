//! 应用注册表：每个应用只是一个「壳」——图标、名字、配色。

use egui::Color32;

pub struct AppInfo {
    pub id: &'static str,
    pub name: &'static str,
    /// 图标上的字形（依赖默认字体 + 系统 CJK 字体回退）
    pub glyph: &'static str,
    pub color: Color32,
    pub glyph_color: Color32,
}

const WHITE: Color32 = Color32::from_rgb(0xFA, 0xFA, 0xFC);

pub const APPS: &[AppInfo] = &[
    AppInfo {
        id: "finder",
        name: "访达",
        glyph: "☺",
        color: Color32::from_rgb(0x1E, 0x9B, 0xF6),
        glyph_color: WHITE,
    },
    AppInfo {
        id: "chrome",
        name: "Chrome",
        glyph: "◎",
        color: Color32::from_rgb(0xF0, 0xF4, 0xF8),
        glyph_color: Color32::from_rgb(0x42, 0x85, 0xF4),
    },
    AppInfo {
        id: "codex",
        name: "Codex",
        glyph: "✳",
        color: Color32::from_rgb(0x10, 0x10, 0x12),
        glyph_color: WHITE,
    },
    AppInfo {
        id: "claude",
        name: "Claude Code",
        glyph: "✳",
        color: Color32::from_rgb(0xD9, 0x77, 0x57),
        glyph_color: WHITE,
    },
    AppInfo {
        id: "mail",
        name: "邮件",
        glyph: "✉",
        color: Color32::from_rgb(0x1A, 0x7C, 0xF0),
        glyph_color: WHITE,
    },
    AppInfo {
        id: "music",
        name: "音乐",
        glyph: "♫",
        color: Color32::from_rgb(0xFC, 0x3C, 0x44),
        glyph_color: WHITE,
    },
    AppInfo {
        id: "photos",
        name: "照片",
        glyph: "❀",
        color: Color32::from_rgb(0xFC, 0xFC, 0xFE),
        glyph_color: Color32::from_rgb(0xFF, 0x6A, 0x9C),
    },
    AppInfo {
        id: "notes",
        name: "备忘录",
        glyph: "✎",
        color: Color32::from_rgb(0xFF, 0xE1, 0x64),
        glyph_color: Color32::from_rgb(0x5C, 0x4A, 0x18),
    },
    AppInfo {
        id: "calendar",
        name: "日历",
        glyph: "12",
        color: Color32::from_rgb(0xFC, 0xFC, 0xFE),
        glyph_color: Color32::from_rgb(0xFF, 0x3B, 0x30),
    },
    AppInfo {
        id: "reminders",
        name: "提醒事项",
        glyph: "☑",
        color: Color32::from_rgb(0xFC, 0xFC, 0xFE),
        glyph_color: Color32::from_rgb(0xFF, 0x9F, 0x0A),
    },
    AppInfo {
        id: "maps",
        name: "地图",
        glyph: "➤",
        color: Color32::from_rgb(0x32, 0xC7, 0x59),
        glyph_color: WHITE,
    },
    AppInfo {
        id: "wechat",
        name: "微信",
        glyph: "💬",
        color: Color32::from_rgb(0x07, 0xC1, 0x60),
        glyph_color: WHITE,
    },
    AppInfo {
        id: "terminal",
        name: "终端",
        glyph: ">_",
        color: Color32::from_rgb(0x1C, 0x1C, 0x1F),
        glyph_color: Color32::from_rgb(0xD0, 0xF0, 0xD8),
    },
    AppInfo {
        id: "settings",
        name: "系统设置",
        glyph: "⚙",
        color: Color32::from_rgb(0x7D, 0x7D, 0x85),
        glyph_color: WHITE,
    },
];

pub const TRASH: AppInfo = AppInfo {
    id: "trash",
    name: "废纸篓",
    glyph: "♺",
    color: Color32::from_rgb(0x9A, 0x9A, 0xA4),
    glyph_color: WHITE,
};

pub fn get(id: &str) -> &'static AppInfo {
    if id == "trash" {
        return &TRASH;
    }
    APPS.iter().find(|a| a.id == id).unwrap_or(&APPS[0])
}

/// 外部原生应用：无法嵌入窗口（闭源 GUI 进程），点击图标用系统 `open -a` 拉起。
/// 返回 Some(系统应用名)。微信已改为内嵌 webview（网页版），不再外部拉起。
pub fn external_app(_id: &str) -> Option<&'static str> {
    None
}
