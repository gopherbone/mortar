// SPDX-License-Identifier: GPL-2.0-or-later
//! Screen stack + event loop shared by every Mortar UI program.

use crate::backend::Backend;
use crate::config::{Config, State};
use crate::gfx::Ui;
use crate::greetd::Greetd;
use crate::input::{Button, Event, Repeater};
use crate::status::{Backlight, Sys};
use crate::toolkit;
use std::time::{Duration, Instant};

/// What a screen wants after handling input.
pub enum Flow {
    Stay,
    Push(Box<dyn Screen>),
    Replace(Box<dyn Screen>),
    Pop,
    PopToRoot,
    /// Leave the program (after handing the display back).
    Exit(i32),
}

pub trait Screen {
    fn draw(&mut self, ui: &mut Ui, ctx: &Ctx);
    fn button(&mut self, b: Button, repeat: bool, ctx: &mut Ctx) -> Flow;
    fn char(&mut self, _c: char, _ctx: &mut Ctx) -> Flow {
        Flow::Stay
    }
    /// Called once when the screen becomes active.
    fn enter(&mut self, _ctx: &mut Ctx) -> Flow {
        Flow::Stay
    }
}

pub struct Ctx {
    pub cfg: Config,
    pub state: State,
    pub greetd: Box<dyn Greetd>,
    pub sys: Sys,
    /// Fixed status values (screenshots/dev) instead of probing the system.
    pub demo: Option<Option<String>>,
    /// Dev mode: no real greetd, actions are only announced.
    pub dev: bool,
    toast: Option<(String, Instant)>,
    backlight: Option<Backlight>,
}

impl Ctx {
    pub fn new(cfg: Config, greetd: Box<dyn Greetd>, demo: Option<Option<String>>, dev: bool) -> Ctx {
        let state = State::load(&cfg.state_dir);
        let sys = match &demo {
            Some(t) => Sys::demo(t.clone()),
            None => Sys::read(),
        };
        Ctx { cfg, state, greetd, sys, demo, dev, toast: None, backlight: Backlight::find() }
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now() + Duration::from_millis(3500)));
    }
    pub fn toast_text(&self) -> Option<&str> {
        self.toast.as_ref().map(|(t, _)| t.as_str())
    }

    /// Re-probe status. Returns true if anything visible changed.
    pub fn refresh(&mut self) -> bool {
        let new = match &self.demo {
            Some(t) => {
                let mut s = Sys::demo(t.clone());
                s.brightness = self.sys.brightness;
                s
            }
            None => Sys::read(),
        };
        let changed = format!("{new:?}") != format!("{:?}", self.sys);
        self.sys = new;
        changed
    }

    pub fn set_brightness(&mut self, pct: u8) {
        let pct = pct.clamp(5, 100);
        match (&self.backlight, self.demo.is_some()) {
            (Some(b), false) => {
                let r = b.set(pct);
                self.sys.brightness = b.get();
                if let Err(e) = r {
                    self.toast(format!("Brightness: {e}"));
                }
            }
            _ => self.sys.brightness = Some(pct),
        }
    }

    /// Run a system command (sleep, reboot, …). Dev mode only announces it.
    pub fn run(&mut self, argv: &[String]) {
        if self.dev || argv.is_empty() {
            self.toast(format!("dev: would run {}", argv.join(" ")));
            return;
        }
        if let Err(e) = std::process::Command::new(&argv[0]).args(&argv[1..]).spawn() {
            self.toast(format!("{}: {e}", argv[0]));
        }
    }
}

/// The screen stack.
pub struct Runner {
    stack: Vec<Box<dyn Screen>>,
    pub exit: Option<i32>,
}

impl Runner {
    pub fn new(root: Box<dyn Screen>, ctx: &mut Ctx) -> Runner {
        let mut r = Runner { stack: Vec::new(), exit: None };
        r.push(root, ctx);
        r
    }

    fn push(&mut self, mut s: Box<dyn Screen>, ctx: &mut Ctx) {
        let f = s.enter(ctx);
        self.stack.push(s);
        self.apply(f, ctx);
    }

    fn apply(&mut self, f: Flow, ctx: &mut Ctx) {
        match f {
            Flow::Stay => {}
            Flow::Push(s) => self.push(s, ctx),
            Flow::Replace(s) => {
                self.stack.pop();
                self.push(s, ctx);
            }
            Flow::Pop => {
                self.stack.pop();
                if self.stack.is_empty() {
                    self.exit = Some(0);
                }
            }
            Flow::PopToRoot => self.stack.truncate(1),
            Flow::Exit(c) => self.exit = Some(c),
        }
    }

    pub fn event(&mut self, ev: &Event, ctx: &mut Ctx) {
        let Some(top) = self.stack.last_mut() else { return };
        let f = match ev {
            Event::Press { button, repeat } => top.button(*button, *repeat, ctx),
            Event::Char(c) => top.char(*c, ctx),
            Event::Release(_) => Flow::Stay,
            Event::Quit => Flow::Exit(0),
        };
        self.apply(f, ctx);
    }

    pub fn draw(&mut self, ui: &mut Ui, ctx: &Ctx) {
        if let Some(top) = self.stack.last_mut() {
            top.draw(ui, ctx);
        }
        if let Some(t) = ctx.toast_text() {
            toolkit::toast(ui, t);
        }
    }

    /// Interactive loop until a screen exits. Redraws only on change.
    pub fn run(&mut self, backend: &mut dyn Backend, ui: &mut Ui, ctx: &mut Ctx) -> i32 {
        let mut rep = Repeater::default();
        let mut dirty = true;
        let mut next_status = Instant::now() + Duration::from_secs(2);
        let mut evs = Vec::new();
        while self.exit.is_none() {
            if dirty {
                self.draw(ui, ctx);
                if let Err(e) = backend.present(ui) {
                    eprintln!("present: {e}");
                }
                dirty = false;
            }
            let now = Instant::now();
            let mut timeout = next_status.saturating_duration_since(now);
            if let Some(t) = rep.timeout() {
                timeout = timeout.min(t);
            }
            if let Some((_, until)) = &ctx.toast {
                timeout = timeout.min(until.saturating_duration_since(now));
            }
            evs.clear();
            backend.wait(Some(timeout), &mut evs);
            rep.poll(&mut evs);
            for ev in &evs {
                match ev {
                    Event::Press { button, repeat: false } => rep.press(*button),
                    Event::Release(b) => rep.release(*b),
                    _ => {}
                }
                if !matches!(ev, Event::Release(_)) {
                    dirty = true;
                }
                self.event(ev, ctx);
                if self.exit.is_some() {
                    break;
                }
            }
            if ctx.toast.as_ref().is_some_and(|(_, u)| Instant::now() >= *u) {
                ctx.toast = None;
                dirty = true;
            }
            if Instant::now() >= next_status {
                dirty |= ctx.refresh();
                next_status = Instant::now() + Duration::from_secs(2);
            }
        }
        backend.release();
        self.exit.unwrap_or(0)
    }
}
