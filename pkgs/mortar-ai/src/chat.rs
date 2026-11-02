// SPDX-License-Identifier: GPL-2.0-or-later
//! The chat screen: a claude session, the model and the view, glued to the
//! window loop (`screen::Screen`).

use crate::model::{Model, NoticeKind, Out};
use crate::proto::{Claude, Msg, Spawn};
use crate::screen::Screen;
use crate::view::View;
use mortar_ui::gfx::Ui;
use mortar_ui::status::Sys;
use std::os::fd::RawFd;
use std::time::{Duration, Instant};

pub struct Chat {
    spawn: Spawn,
    claude: Option<Claude>,
    model: Model,
    view: View,
    sys: Sys,
    sys_at: Instant,
    /// The shell's "Needs you" while an approval waits.
    shell: crate::attention::Notifier,
}

impl Chat {
    pub fn new(spawn: Spawn) -> Chat {
        let mut c = Chat { spawn, claude: None, model: Model::new(), view: View::default(), sys: Sys::read(), sys_at: Instant::now(), shell: Default::default() };
        c.start();
        c
    }

    fn start(&mut self) {
        match Claude::spawn(&self.spawn) {
            Ok(c) => {
                self.claude = Some(c);
                self.model.restarted();
            }
            Err(e) => {
                self.model.notice(NoticeKind::Error, format!("Could not start {}: {e}", self.spawn.program));
                self.model.alive = false;
            }
        }
    }

    fn apply(&mut self, outs: Vec<Out>) {
        for o in outs {
            match o {
                Out::Send(v) => {
                    if let Some(c) = &mut self.claude {
                        c.send(&v);
                    }
                }
                Out::Interrupt => {
                    if let Some(c) = &mut self.claude {
                        c.interrupt();
                    }
                }
                Out::Attention => attention(),
                Out::Restart => self.start(),
            }
        }
    }
}

/// Buzz the rumble motor when Claude needs an answer (MORTAR_AI_RUMBLE=0 to
/// turn it off).
fn attention() {
    if std::env::var("MORTAR_AI_RUMBLE").as_deref() == Ok("0") {
        return;
    }
    if let Ok(mut c) = std::process::Command::new("mortar-rumble").args(["120", "1"]).spawn() {
        std::thread::spawn(move || c.wait());
    }
}

impl Chat {
    /// An approval waits: the shell shows "Needs you" until it is answered.
    fn tell_shell(&mut self) {
        let want = self.model.pending.front().map(|p| match self.model.pending.len() {
            1 => format!("Approve: {}", p.title),
            n => format!("Approve: {} (+{} more)", p.title, n - 1),
        });
        self.shell.update(want);
    }
}

impl Screen for Chat {
    fn title(&self) -> &str {
        "Claude"
    }

    fn wake_fd(&self) -> Option<RawFd> {
        self.claude.as_ref().map(|c| c.wake_fd())
    }

    fn poll(&mut self) -> bool {
        let mut dirty = false;
        let mut outs = Vec::new();
        let mut exited = None;
        if let Some(c) = &mut self.claude {
            c.ack_wake();
            while let Ok(m) = c.rx.try_recv() {
                dirty = true;
                match m {
                    Msg::Json(v) => outs.extend(self.model.event(&v)),
                    Msg::Text(t) => self.model.notice(NoticeKind::Info, t),
                    Msg::Stderr(t) => self.model.stderr(t),
                    Msg::Exit => exited = Some(c.reap()),
                }
            }
        }
        if let Some(code) = exited {
            self.claude = None;
            self.model.exited(code);
        }
        self.apply(outs);
        self.tell_shell();
        if self.sys_at.elapsed() > Duration::from_secs(10) {
            self.sys = Sys::read();
            self.sys_at = Instant::now();
            dirty = true;
        }
        dirty
    }

    fn key(&mut self, code: u16, down: bool) {
        let outs = self.model.key(code, down, self.view.page_lines());
        self.apply(outs);
        self.tell_shell();
    }

    fn reset_keys(&mut self) {
        self.model.mods = Default::default();
    }

    fn draw(&mut self, ui: &mut Ui) {
        self.view.draw(ui, &mut self.model, &self.sys.bar());
    }

    fn done(&self) -> bool {
        false
    }
}
