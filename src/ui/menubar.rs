//! 顶部菜单栏：苹果 logo、应用菜单、下拉菜单、右侧状态区与时钟。
//!
//! 下拉菜单交互复刻 macOS：点击标题打开，打开状态下悬停其他标题自动切换，
//! 点击菜单项执行动作，点击其他位置关闭。

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Once;

use chrono::{Datelike, Local, Timelike};
use egui::{pos2, vec2, Align2, FontId, Painter, Pos2, Rect, Stroke, StrokeKind};

use super::{black_a, white_a};

pub const HEIGHT: f32 = 26.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuAction {
    NewWindow,
    CloseWindow,
    MinimizeWindow,
    ZoomWindow,
    OpenApp(&'static str),
    Noop,
}

struct Item {
    label: String,
    shortcut: &'static str,
    action: MenuAction,
    enabled: bool,
    sep_after: bool,
}

impl Item {
    fn new(label: impl Into<String>, action: MenuAction, enabled: bool) -> Self {
        Self {
            label: label.into(),
            shortcut: "",
            action,
            enabled,
            sep_after: false,
        }
    }
    fn key(mut self, s: &'static str) -> Self {
        self.shortcut = s;
        self
    }
    fn sep(mut self) -> Self {
        self.sep_after = true;
        self
    }
}

/// 菜单定义：has_window 决定窗口类动作是否可用
fn menu_defs(app_name: &str, has_window: bool) -> Vec<(String, Vec<Item>)> {
    let hw = has_window;
    vec![
        (
            "\u{f8ff}".to_owned(), // 占位，苹果菜单的标题单独绘制 logo
            vec![
                Item::new("关于本机", MenuAction::OpenApp("settings"), true).sep(),
                Item::new("系统设置…", MenuAction::OpenApp("settings"), true),
                Item::new("App Store…", MenuAction::Noop, true).sep(),
                Item::new("强制退出…", MenuAction::Noop, true).sep(),
                Item::new("睡眠", MenuAction::Noop, true),
                Item::new("重新启动…", MenuAction::Noop, true),
                Item::new("关机…", MenuAction::Noop, true).sep(),
                Item::new("锁定屏幕", MenuAction::Noop, true),
                Item::new("退出登录…", MenuAction::Noop, true),
            ],
        ),
        (
            app_name.to_owned(),
            vec![
                Item::new(format!("关于{app_name}"), MenuAction::Noop, true).sep(),
                Item::new("设置…", MenuAction::Noop, true).sep(),
                Item::new(format!("隐藏{app_name}"), MenuAction::MinimizeWindow, hw),
                Item::new(format!("退出{app_name}"), MenuAction::CloseWindow, hw).key("⌘Q"),
            ],
        ),
        (
            "文件".to_owned(),
            vec![
                Item::new("新建窗口", MenuAction::NewWindow, true).key("⌘N").sep(),
                Item::new("关闭窗口", MenuAction::CloseWindow, hw).key("⌘W"),
            ],
        ),
        (
            "编辑".to_owned(),
            vec![
                Item::new("撤销", MenuAction::Noop, false).key("⌘Z"),
                Item::new("重做", MenuAction::Noop, false).sep(),
                Item::new("剪切", MenuAction::Noop, false).key("⌘X"),
                Item::new("拷贝", MenuAction::Noop, false).key("⌘C"),
                Item::new("粘贴", MenuAction::Noop, false).key("⌘V"),
                Item::new("全选", MenuAction::Noop, false).key("⌘A"),
            ],
        ),
        (
            "显示".to_owned(),
            vec![Item::new("进入全屏幕", MenuAction::ZoomWindow, hw)],
        ),
        (
            "前往".to_owned(),
            vec![
                Item::new("最近使用", MenuAction::Noop, false).sep(),
                Item::new("文稿", MenuAction::OpenApp("finder"), true),
                Item::new("桌面", MenuAction::OpenApp("finder"), true),
                Item::new("下载", MenuAction::OpenApp("finder"), true),
            ],
        ),
        (
            "窗口".to_owned(),
            vec![
                Item::new("最小化", MenuAction::MinimizeWindow, hw).key("⌘M"),
                Item::new("缩放", MenuAction::ZoomWindow, hw).sep(),
                Item::new("前置全部窗口", MenuAction::Noop, false),
            ],
        ),
        (
            "帮助".to_owned(),
            vec![Item::new("mirage 帮助", MenuAction::Noop, true)],
        ),
    ]
}

#[derive(Default)]
pub struct MenuState {
    pub open: Option<usize>,
    titles: Vec<Rect>,
    items: Vec<(Rect, MenuAction, bool)>,
}

pub enum MenuPress {
    Title(usize),
    Action(MenuAction),
    /// 点击被菜单栏吞掉（关闭菜单 / 点在条上）
    Swallow,
    Pass,
}

impl MenuState {
    pub fn handle_press(&mut self, pos: Pos2) -> MenuPress {
        if self.open.is_some() {
            if let Some((_, action, enabled)) = self.items.iter().find(|(r, ..)| r.contains(pos)) {
                if *enabled {
                    return MenuPress::Action(*action);
                }
                return MenuPress::Swallow;
            }
            if let Some(i) = self.titles.iter().position(|r| r.contains(pos)) {
                return MenuPress::Title(i);
            }
            // 菜单打开时点其他任何地方：关闭并吞掉这次点击
            return MenuPress::Swallow;
        }
        if let Some(i) = self.titles.iter().position(|r| r.contains(pos)) {
            return MenuPress::Title(i);
        }
        if pos.y < HEIGHT {
            return MenuPress::Swallow;
        }
        MenuPress::Pass
    }
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    p: &Painter,
    screen: Rect,
    app_name: &str,
    alpha: f32,
    state: &mut MenuState,
    pointer: Option<Pos2>,
    has_window: bool,
    show_seconds: bool,
) {
    if alpha <= 0.0 {
        state.open = None;
        state.titles.clear();
        state.items.clear();
        return;
    }
    let bar = Rect::from_min_size(screen.min, vec2(screen.width(), HEIGHT));
    p.rect_filled(bar, 0, black_a(0.38 * alpha));
    p.line_segment(
        [bar.left_bottom(), bar.right_bottom()],
        Stroke::new(1.0, white_a(0.06 * alpha)),
    );

    let cy = bar.center().y;
    let defs = menu_defs(app_name, has_window);

    // 打开状态下悬停切换菜单
    if let (Some(open), Some(hp)) = (state.open, pointer) {
        if let Some(j) = state.titles.iter().position(|r| r.contains(hp)) {
            if j != open {
                state.open = Some(j);
            }
        }
    }

    // ---- 标题行 ----
    state.titles.clear();
    let mut x = bar.left() + 10.0;
    for (i, (title, _)) in defs.iter().enumerate() {
        let (w, is_logo) = if i == 0 {
            (22.0, true)
        } else {
            let galley = p.layout_no_wrap(
                title.clone(),
                FontId::proportional(13.0),
                white_a(1.0),
            );
            (galley.size().x, false)
        };
        let title_rect = Rect::from_min_max(
            pos2(x - 6.0, bar.top() + 2.0),
            pos2(x + w + 6.0, bar.bottom() - 2.0),
        );
        if state.open == Some(i) {
            p.rect_filled(title_rect, 4, white_a(0.22 * alpha));
        }
        if is_logo {
            // 真苹果 logo 字形（U+F8FF，SF Pro / 苹方均收录）
            p.text(
                pos2(x + 8.0, cy - 0.5),
                Align2::CENTER_CENTER,
                "\u{f8ff}",
                FontId::proportional(15.0),
                white_a(0.95 * alpha),
            );
        } else {
            let strong = i == 1; // 应用名加亮
            p.text(
                pos2(x, cy),
                Align2::LEFT_CENTER,
                title,
                FontId::proportional(13.0),
                white_a(if strong { 0.95 } else { 0.80 } * alpha),
            );
        }
        state.titles.push(title_rect);
        x += w + 18.0;
    }

    // ---- 右侧：电池 + 日期时间 ----
    let now = Local::now();
    let weekday = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"]
        [now.weekday().num_days_from_monday() as usize];
    let clock = if show_seconds {
        format!(
            "{}月{}日 {} {:02}:{:02}:{:02}",
            now.month(),
            now.day(),
            weekday,
            now.hour(),
            now.minute(),
            now.second()
        )
    } else {
        format!(
            "{}月{}日 {} {:02}:{:02}",
            now.month(),
            now.day(),
            weekday,
            now.hour(),
            now.minute()
        )
    };
    let clock_rect = p.text(
        pos2(bar.right() - 16.0, cy),
        Align2::RIGHT_CENTER,
        clock,
        FontId::proportional(13.0),
        white_a(0.92 * alpha),
    );
    draw_battery(p, pos2(clock_rect.left() - 30.0, cy), alpha);

    // ---- 下拉菜单 ----
    state.items.clear();
    let Some(open) = state.open else { return };
    let (_, items) = &defs[open];
    let anchor = state.titles[open];

    let font = FontId::proportional(13.0);
    let mut width: f32 = 180.0;
    for it in items.iter() {
        let lw = p
            .layout_no_wrap(it.label.clone(), font.clone(), white_a(1.0))
            .size()
            .x;
        let sw = if it.shortcut.is_empty() {
            0.0
        } else {
            p.layout_no_wrap(it.shortcut.to_owned(), font.clone(), white_a(1.0))
                .size()
                .x + 24.0
        };
        width = width.max(lw + sw + 40.0);
    }
    const ITEM_H: f32 = 24.0;
    const SEP_H: f32 = 9.0;
    let height: f32 = items
        .iter()
        .map(|it| ITEM_H + if it.sep_after { SEP_H } else { 0.0 })
        .sum::<f32>()
        + 10.0;
    let panel = Rect::from_min_size(
        pos2(anchor.left(), bar.bottom() + 2.0),
        vec2(width, height),
    );

    // 阴影 + 面板
    for i in 0..4 {
        p.rect_filled(
            panel.expand(2.0 + i as f32 * 2.0).translate(vec2(0.0, 2.0)),
            10,
            black_a(0.10 / (i as f32 + 1.0)),
        );
    }
    p.rect_filled(panel, 8, egui::Color32::from_rgb(0x2C, 0x2C, 0x31).gamma_multiply(0.96));
    p.rect_stroke(panel, 8, Stroke::new(1.0, white_a(0.14)), StrokeKind::Inside);

    let mut y = panel.top() + 5.0;
    for it in items.iter() {
        let row = Rect::from_min_size(pos2(panel.left(), y), vec2(width, ITEM_H));
        let inner = row.shrink2(vec2(5.0, 1.0));
        let hovered = it.enabled && pointer.is_some_and(|hp| inner.contains(hp));
        if hovered {
            p.rect_filled(inner, 5, egui::Color32::from_rgb(0x2C, 0x62, 0xD6));
        }
        let text_c = if !it.enabled {
            white_a(0.30)
        } else {
            white_a(0.92)
        };
        p.text(
            pos2(row.left() + 16.0, row.center().y),
            Align2::LEFT_CENTER,
            &it.label,
            font.clone(),
            text_c,
        );
        if !it.shortcut.is_empty() {
            p.text(
                pos2(row.right() - 14.0, row.center().y),
                Align2::RIGHT_CENTER,
                it.shortcut,
                font.clone(),
                if hovered { white_a(0.8) } else { white_a(0.38) },
            );
        }
        state.items.push((inner, it.action, it.enabled));
        y += ITEM_H;
        if it.sep_after {
            p.line_segment(
                [
                    pos2(panel.left() + 12.0, y + SEP_H / 2.0),
                    pos2(panel.right() - 12.0, y + SEP_H / 2.0),
                ],
                Stroke::new(1.0, white_a(0.12)),
            );
            y += SEP_H;
        }
    }
}

// ---- 真实电池状态：后台线程定期读取，绘制时无阻塞 ----

static BATTERY_PERCENT: AtomicU32 = AtomicU32::new(100);
static BATTERY_ON_AC: AtomicBool = AtomicBool::new(false);
static BATTERY_POLLER: Once = Once::new();

/// 返回 (电量 0~100, 是否接通电源)。首次调用启动轮询线程。
fn battery_status() -> (u32, bool) {
    BATTERY_POLLER.call_once(|| {
        std::thread::spawn(|| loop {
            if let Some((pct, on_ac)) = read_battery() {
                BATTERY_PERCENT.store(pct, Ordering::Relaxed);
                BATTERY_ON_AC.store(on_ac, Ordering::Relaxed);
            }
            std::thread::sleep(std::time::Duration::from_secs(30));
        });
    });
    (
        BATTERY_PERCENT.load(Ordering::Relaxed),
        BATTERY_ON_AC.load(Ordering::Relaxed),
    )
}

#[cfg(target_os = "macos")]
fn read_battery() -> Option<(u32, bool)> {
    let out = std::process::Command::new("pmset")
        .args(["-g", "batt"])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    // 形如：Now drawing from 'Battery Power'\n -InternalBattery-0 ...\t93%; discharging; ...
    let pct_end = s.find('%')?;
    let pct_start = s[..pct_end]
        .rfind(|c: char| !c.is_ascii_digit())
        .map_or(0, |i| i + 1);
    let pct: u32 = s[pct_start..pct_end].parse().ok()?;
    let on_ac = s.contains("'AC Power'");
    Some((pct.min(100), on_ac))
}

#[cfg(not(target_os = "macos"))]
fn read_battery() -> Option<(u32, bool)> {
    None
}

fn draw_battery(p: &Painter, center: egui::Pos2, alpha: f32) {
    let (pct, on_ac) = battery_status();
    let body = Rect::from_center_size(center, vec2(22.0, 11.0));
    p.rect_stroke(
        body,
        3,
        Stroke::new(1.0, white_a(0.6 * alpha)),
        egui::StrokeKind::Inside,
    );
    p.rect_filled(
        Rect::from_center_size(pos2(body.right() + 2.0, center.y), vec2(2.0, 4.0)),
        1,
        white_a(0.6 * alpha),
    );
    // 填充按实际电量缩放；充电显示绿色，低电量显示红色
    let inner = body.shrink(2.0);
    let fill_w = (inner.width() * pct as f32 / 100.0).max(1.5);
    let fill = Rect::from_min_size(inner.min, vec2(fill_w, inner.height()));
    let color = if on_ac {
        egui::Color32::from_rgb(0x4C, 0xD9, 0x64).gamma_multiply(alpha)
    } else if pct <= 20 {
        egui::Color32::from_rgb(0xFF, 0x45, 0x3A).gamma_multiply(alpha)
    } else {
        white_a(0.85 * alpha)
    };
    p.rect_filled(fill, 2, color);
    // 充电闪电（凹多边形拆成上下两个三角形）
    if on_ac {
        let c = center;
        let ink = black_a(0.9 * alpha);
        p.add(egui::Shape::convex_polygon(
            vec![
                pos2(c.x + 2.0, c.y - 4.8),
                pos2(c.x - 2.6, c.y + 1.0),
                pos2(c.x + 0.6, c.y + 0.2),
            ],
            ink,
            Stroke::NONE,
        ));
        p.add(egui::Shape::convex_polygon(
            vec![
                pos2(c.x - 2.0, c.y + 4.8),
                pos2(c.x + 2.6, c.y - 1.0),
                pos2(c.x - 0.6, c.y - 0.2),
            ],
            ink,
            Stroke::NONE,
        ));
    }
    // 电量百分比文字（图标左侧，同 macOS“显示百分比”样式）
    p.text(
        pos2(body.left() - 5.0, center.y),
        Align2::RIGHT_CENTER,
        format!("{pct}%"),
        FontId::proportional(12.0),
        white_a(0.85 * alpha),
    );
}
