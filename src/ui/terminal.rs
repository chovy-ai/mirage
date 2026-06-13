//! 终端：基于开源 egui_term（Alacritty 抽出的 alacritty_terminal 后端）。
//! 真 PTY 终端——支持 vim / htop / 颜色 / 补全等完整 shell 交互，
//! 不再是手写的「命令执行器」。

use egui::{Align2, Color32, FontId, Ui};
use egui_term::{BackendSettings, PtyEvent, TerminalBackend, TerminalView};
use std::sync::mpsc::{channel, Receiver};

#[derive(Default)]
pub struct TerminalApp {
    backend: Option<TerminalBackend>,
    rx: Option<Receiver<(u64, PtyEvent)>>,
    exited: bool,
    failed: Option<String>,
}

impl TerminalApp {
    fn ensure_started(&mut self, ui: &Ui) {
        if self.backend.is_some() || self.failed.is_some() || self.exited {
            return;
        }
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let (tx, rx) = channel();
        match TerminalBackend::new(
            0,
            ui.ctx().clone(),
            tx,
            BackendSettings {
                shell,
                ..Default::default()
            },
        ) {
            Ok(backend) => {
                self.backend = Some(backend);
                self.rx = Some(rx);
            }
            Err(e) => self.failed = Some(format!("终端启动失败：{e}")),
        }
    }

    pub fn running(&self) -> bool {
        self.backend.is_some()
    }

    pub fn show(&mut self, ui: &mut Ui) {
        self.ensure_started(ui);

        let full = ui.max_rect();
        ui.painter()
            .rect_filled(full, 0, Color32::from_rgb(0x14, 0x14, 0x17));

        // shell 退出（exit / Ctrl-D）→ 显示提示，点击重启会话
        if let Some(rx) = &self.rx {
            if let Ok((_, PtyEvent::Exit)) = rx.try_recv() {
                self.backend = None;
                self.rx = None;
                self.exited = true;
            }
        }

        if let Some(err) = &self.failed {
            ui.painter().text(
                full.center(),
                Align2::CENTER_CENTER,
                err,
                FontId::proportional(13.0),
                Color32::from_rgb(0xE5, 0x6B, 0x6B),
            );
            return;
        }
        if self.exited {
            ui.painter().text(
                full.center(),
                Align2::CENTER_CENTER,
                "会话已结束 · 点击重新开始",
                FontId::proportional(14.0),
                Color32::from_gray(140),
            );
            if ui.input(|i| i.pointer.primary_pressed())
                && ui
                    .input(|i| i.pointer.hover_pos())
                    .is_some_and(|p| full.contains(p))
            {
                self.exited = false;
            }
            return;
        }

        if let Some(backend) = &mut self.backend {
            let view = TerminalView::new(ui, backend)
                .set_focus(ui.is_enabled())
                .set_size(full.size());
            ui.add(view);
        }
    }
}
