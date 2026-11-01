// SPDX-License-Identifier: GPL-2.0-or-later
//! The setup screens (plan mockups 1-6 and 8), drawn with the mortar-ui
//! toolkit in the shared theme (style v2: pale ground, ink outlines, lime
//! selection blocks, serif italic titles).
//!
//! `SetupUi` is a small screen stack with no I/O of its own: it draws into
//! a `Ui` canvas, takes buttons, and talks to a `Device` (the daemon over
//! its control socket, or a demo stand-in for screenshots). The first-boot
//! program (`mortar-setup ui`) drives it with a KMS backend; mortar-shell
//! embeds it for Settings > Remote access and Settings > Wi-Fi.

use crate::control::{Client, Request, Status};
use mortar_ui::gfx::Ui;
use mortar_ui::input::Button;
use mortar_ui::status::Sys;
use mortar_ui::toolkit::{self, Hint, StatusInfo};
use std::time::{Duration, Instant};

pub mod demo;
pub mod keyboard;
pub mod pages;

/// Where the Brick's state comes from.
pub trait Device {
    fn status(&self) -> &Status;
    /// Re-read the state. True if anything changed.
    fn refresh(&mut self) -> bool;
    fn act(&mut self, req: Request) -> Result<(), String>;
}

/// The daemon, over its control socket.
pub struct SocketDevice {
    client: Client,
    status: Status,
    pub error: Option<String>,
}

impl SocketDevice {
    pub fn new(path: impl AsRef<std::path::Path>) -> SocketDevice {
        let mut d = SocketDevice { client: Client::new(path), status: Status::default(), error: None };
        d.refresh();
        d
    }
}

impl Device for SocketDevice {
    fn status(&self) -> &Status {
        &self.status
    }
    fn refresh(&mut self) -> bool {
        match self.client.status() {
            Ok(s) => {
                self.error = None;
                let changed = s != self.status;
                self.status = s;
                changed
            }
            Err(e) => {
                let changed = self.error.as_deref() != Some(e.as_str());
                self.error = Some(e);
                changed
            }
        }
    }
    fn act(&mut self, req: Request) -> Result<(), String> {
        let r = self.client.act(req);
        self.refresh();
        r
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// First boot: "mortar" in the status bar, finishing marks setup done.
    FirstBoot,
    /// Re-opened from Settings: the clock in the status bar, leaving
    /// returns to whoever embedded us.
    Settings,
}

pub struct Cx {
    pub dev: Box<dyn Device>,
    pub sys: Sys,
    pub mode: Mode,
    toast: Option<(String, Instant)>,
    /// Demo/screenshot mode: no real clock, fixed countdowns.
    pub frozen: bool,
}

impl Cx {
    pub fn st(&self) -> &Status {
        self.dev.status()
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now() + Duration::from_millis(3500)));
    }

    /// Run an action; failures become a toast.
    pub fn act(&mut self, req: Request) -> bool {
        match self.dev.act(req) {
            Ok(()) => true,
            Err(e) => {
                self.toast(e);
                false
            }
        }
    }

    /// The status bar, mockup style: "mortar" during first boot, the clock
    /// afterwards; a yellow USB tag while a computer is on the cable.
    pub fn bar(&self, ssh_always: bool) -> StatusInfo {
        let st = self.st();
        StatusInfo {
            left: if self.mode == Mode::FirstBoot { "mortar".into() } else { self.sys.time.clone() },
            wifi: st.wifi.state == "connected",
            battery: self.sys.battery,
            ssh_sessions: self.sys.ssh_sessions,
            ssh_badge_always: ssh_always,
            note: (st.usb_connected && st.usb_network).then(|| format!("USB {}", st.usb_address)),
        }
    }

    /// Clear and draw the frame; returns the body area.
    pub fn frame(&self, ui: &mut Ui, ssh_always: bool, hints: &[Hint]) -> mortar_ui::gfx::Rect {
        toolkit::frame(ui, &self.bar(ssh_always), hints)
    }
}

/// What a page wants after input or a tick.
pub enum Nav {
    Stay,
    Push(Box<dyn Page>),
    Replace(Box<dyn Page>),
    Pop,
    /// Leave the setup UI (first boot: setup is finished).
    Exit,
}

pub trait Page {
    fn draw(&mut self, ui: &mut Ui, cx: &Cx);
    fn button(&mut self, b: Button, cx: &mut Cx) -> Nav;
    /// After the device state was refreshed.
    fn tick(&mut self, _cx: &mut Cx) -> Nav {
        Nav::Stay
    }
    /// Shows something that changes every second (a countdown).
    fn live(&self) -> bool {
        false
    }
}

pub struct SetupUi {
    stack: Vec<Box<dyn Page>>,
    pub cx: Cx,
    exited: bool,
}

impl SetupUi {
    fn with(root: impl FnOnce(&mut Cx) -> Box<dyn Page>, dev: Box<dyn Device>, sys: Sys, mode: Mode) -> SetupUi {
        let mut cx = Cx { dev, sys, mode, toast: None, frozen: false };
        let r = root(&mut cx);
        SetupUi { stack: vec![r], cx, exited: false }
    }

    /// First boot: the setup-file summary if the card had one, else Welcome.
    pub fn first_boot(dev: Box<dyn Device>, sys: Sys) -> SetupUi {
        SetupUi::with(
            |cx| {
                if cx.st().setup_file.as_ref().is_some_and(|s| s.has_content) {
                    Box::new(pages::SetupFile::default())
                } else {
                    Box::new(pages::Welcome::default())
                }
            },
            dev,
            sys,
            Mode::FirstBoot,
        )
    }

    /// Settings > Remote access (mockup 8).
    pub fn remote_access(dev: Box<dyn Device>, sys: Sys) -> SetupUi {
        SetupUi::with(|_| Box::new(pages::RemoteAccess::default()), dev, sys, Mode::Settings)
    }

    /// Settings > Wi-Fi.
    pub fn wifi(dev: Box<dyn Device>, sys: Sys) -> SetupUi {
        SetupUi::with(|cx| Box::new(pages::WifiList::new(cx)), dev, sys, Mode::Settings)
    }

    /// Any page by name, for screenshots and tests.
    pub fn named(name: &str, dev: Box<dyn Device>, sys: Sys, mode: Mode) -> Option<SetupUi> {
        let mut err = false;
        let ui = SetupUi::with(
            |cx| -> Box<dyn Page> {
                match name {
                    "welcome" => Box::new(pages::Welcome::default()),
                    "setup-file" => Box::new(pages::SetupFile::default()),
                    "usb" => Box::new(pages::UsbPair::new(cx)),
                    "wifi" => Box::new(pages::WifiList::new(cx)),
                    "keyboard" => Box::new(keyboard::PasswordPage::new("Gloria_5G")),
                    "joining" => Box::new(pages::WifiJoin::new("Gloria_5G", false)),
                    "ready" => Box::new(pages::Ready::default()),
                    "remote" => Box::new(pages::RemoteAccess::default()),
                    "keys" => Box::new(pages::KeysList::default()),
                    _ => {
                        err = true;
                        Box::new(pages::Welcome::default())
                    }
                }
            },
            dev,
            sys,
            mode,
        );
        (!err).then_some(ui)
    }

    pub fn exited(&self) -> bool {
        self.exited
    }

    pub fn freeze(&mut self) {
        self.cx.frozen = true;
    }

    fn apply(&mut self, n: Nav) {
        match n {
            Nav::Stay => {}
            Nav::Push(p) => self.stack.push(p),
            Nav::Replace(p) => {
                self.stack.pop();
                self.stack.push(p);
            }
            Nav::Pop => {
                self.stack.pop();
                if self.stack.is_empty() {
                    self.exited = true;
                }
            }
            Nav::Exit => self.exited = true,
        }
    }

    pub fn draw(&mut self, ui: &mut Ui) {
        if let Some(top) = self.stack.last_mut() {
            top.draw(ui, &self.cx);
        }
        if let Some((t, _)) = &self.cx.toast {
            toolkit::toast(ui, t);
        }
    }

    /// Handle a button. Returns true when the UI is done.
    pub fn button(&mut self, b: Button) -> bool {
        if self.exited {
            return true;
        }
        let n = match self.stack.last_mut() {
            Some(top) => top.button(b, &mut self.cx),
            None => Nav::Exit,
        };
        self.apply(n);
        // pages react to their own actions right away
        if !self.exited {
            self.after_change();
        }
        self.exited
    }

    fn after_change(&mut self) {
        if let Some(top) = self.stack.last_mut() {
            let n = top.tick(&mut self.cx);
            self.apply(n);
        }
    }

    /// Refresh the device state and let the page react. Returns true if
    /// the screen needs a redraw. Call about once a second.
    pub fn tick(&mut self) -> bool {
        let mut redraw = self.cx.dev.refresh();
        if self.cx.toast.as_ref().is_some_and(|(_, until)| Instant::now() >= *until) {
            self.cx.toast = None;
            redraw = true;
        }
        let depth = self.stack.len();
        self.after_change();
        redraw |= self.stack.len() != depth || self.exited;
        redraw |= self.stack.last().is_some_and(|p| p.live());
        redraw
    }

    /// When the next `tick` is due.
    pub fn next_tick(&self) -> Duration {
        Duration::from_millis(1000)
    }
}
