// SPDX-License-Identifier: GPL-2.0-or-later
//! The shell's own screens, drawn with the mortar-ui toolkit:
//!
//! - Home (no app visible): the launcher as tiles (mockup 7).
//! - MENU overlay over an app (mockup 10, "switch session" became "switch
//!   app"): three tabs, L1/R1 to change.
//!     Apps      running apps: A switch, X close (press twice)
//!     Launch    launcher tiles: A start (or switch if running)
//!     Settings  brightness, volume, keyboard, Wi-Fi, remote access, sleep,
//!               log out, power off
//!
//! Pure UI state; the compositor applies the returned `Action`s.

use mortar_ui::gfx::{Rect, Ui};
use mortar_ui::icons::Icon;
use mortar_ui::input::Button;
use mortar_ui::status::Sys;
use mortar_ui::toolkit::{self, hint, Glyph, ListState, MenuItem, PillKind, Row, RowValue};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct RunningInfo {
    pub uid: u32,
    pub name: String,
    pub icon: Icon,
    pub visible: bool,
    pub windows: usize,
    pub closing: bool,
}

#[derive(Clone, Debug)]
pub struct LauncherInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: Icon,
    pub running: bool,
}

/// What the overlay shows; rebuilt by the compositor for each draw/input.
pub struct View<'a> {
    pub running: &'a [RunningInfo],
    pub launcher: &'a [LauncherInfo],
    pub sys: &'a Sys,
    pub volume: Option<u8>,
    /// The visible app (None = home screen).
    pub current: Option<&'a RunningInfo>,
    /// The visible app takes keyboard input (on-screen keyboard available).
    pub keyboard_app: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    None,
    Hide,
    Focus(u32),
    Close(u32),
    Launch(String),
    Brightness(i32),
    Volume(i32),
    ShowKeyboard,
    Sleep,
    Logout,
    Reboot,
    PowerOff,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Apps,
    Launch,
    Settings,
}

const TABS: [(Tab, &str); 3] = [(Tab::Apps, "Apps"), (Tab::Launch, "Launch"), (Tab::Settings, "Settings")];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setting {
    Brightness,
    Volume,
    Keyboard,
    Wifi,
    Remote,
    Sleep,
    Logout,
    Reboot,
    PowerOff,
}

pub struct Overlay {
    /// Shown over an app (MENU). The home screen shows whenever no app is
    /// visible, regardless.
    pub visible: bool,
    pub tab: Tab,
    running: ListState,
    tile: usize,
    settings: ListState,
    confirm: Option<(String, Instant)>,
    toast: Option<(String, Instant)>,
}

impl Default for Overlay {
    fn default() -> Self {
        Overlay {
            visible: false,
            tab: Tab::Apps,
            running: ListState::default(),
            tile: 0,
            settings: ListState::default(),
            confirm: None,
            toast: None,
        }
    }
}

const TILES_PER_ROW: usize = 4;

fn settings(v: &View) -> Vec<Setting> {
    let mut s = vec![Setting::Brightness, Setting::Volume];
    if v.keyboard_app {
        s.push(Setting::Keyboard);
    }
    s.extend([Setting::Wifi, Setting::Remote, Setting::Sleep, Setting::Logout, Setting::Reboot, Setting::PowerOff]);
    s
}

impl Overlay {
    /// Open over the visible app (MENU): start on the app list with the
    /// previous app selected, so MENU, A switches back and forth.
    pub fn open(&mut self, v: &View) {
        self.visible = true;
        self.tab = Tab::Apps;
        self.running.sel = if v.running.len() > 1 { 1 } else { 0 };
        self.confirm = None;
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now() + Duration::from_millis(3000)));
    }

    pub fn clear_toast(&mut self) {
        self.toast = None;
    }

    /// When the overlay next needs a redraw for a timeout (toast expiry).
    pub fn deadline(&self) -> Option<Instant> {
        self.toast.as_ref().map(|t| t.1)
    }
    /// Drop expired toasts; true if something changed.
    pub fn expire(&mut self) -> bool {
        if self.toast.as_ref().is_some_and(|t| Instant::now() >= t.1) {
            self.toast = None;
            return true;
        }
        false
    }

    fn confirmed(&mut self, key: &str, what: &str) -> bool {
        if self.confirm.as_ref().is_some_and(|(k, t)| k == key && Instant::now() < *t) {
            self.confirm = None;
            return true;
        }
        self.confirm = Some((key.to_string(), Instant::now() + Duration::from_secs(4)));
        self.toast(format!("Press again to {what}"));
        false
    }

    fn home(v: &View) -> bool {
        v.current.is_none()
    }

    pub fn button(&mut self, b: Button, v: &View) -> Action {
        let tabs: Vec<Tab> = if Self::home(v) { vec![Tab::Launch, Tab::Settings] } else { TABS.iter().map(|t| t.0).collect() };
        if !tabs.contains(&self.tab) {
            self.tab = tabs[0];
        }
        match b {
            Button::L1 | Button::R1 => {
                let i = tabs.iter().position(|t| *t == self.tab).unwrap_or(0);
                let n = tabs.len();
                self.tab = tabs[if b == Button::L1 { (i + n - 1) % n } else { (i + 1) % n }];
                return Action::None;
            }
            Button::Menu | Button::B if !Self::home(v) => return Action::Hide,
            Button::Menu if Self::home(v) => {
                self.tab = if self.tab == Tab::Settings { Tab::Launch } else { Tab::Settings };
                return Action::None;
            }
            _ => {}
        }
        match self.tab {
            Tab::Apps => self.button_apps(b, v),
            Tab::Launch => self.button_launch(b, v),
            Tab::Settings => self.button_settings(b, v),
        }
    }

    fn button_apps(&mut self, b: Button, v: &View) -> Action {
        let n = v.running.len();
        match b {
            Button::Up => self.running.up(n),
            Button::Down => self.running.down(n),
            Button::A if n > 0 => return Action::Focus(v.running[self.running.sel.min(n - 1)].uid),
            Button::X if n > 0 => {
                let a = &v.running[self.running.sel.min(n - 1)];
                if self.confirmed(&format!("close-{}", a.uid), &format!("close {}", a.name)) {
                    return Action::Close(a.uid);
                }
            }
            _ => {}
        }
        Action::None
    }

    fn button_launch(&mut self, b: Button, v: &View) -> Action {
        let n = v.launcher.len();
        if n == 0 {
            return Action::None;
        }
        self.tile = self.tile.min(n - 1);
        match b {
            Button::Left => self.tile = (self.tile + n - 1) % n,
            Button::Right => self.tile = (self.tile + 1) % n,
            Button::Up if self.tile >= TILES_PER_ROW => self.tile -= TILES_PER_ROW,
            Button::Down => self.tile = (self.tile + TILES_PER_ROW).min(n - 1),
            Button::A | Button::Start => return Action::Launch(v.launcher[self.tile].id.clone()),
            _ => {}
        }
        Action::None
    }

    fn button_settings(&mut self, b: Button, v: &View) -> Action {
        let items = settings(v);
        let n = items.len();
        self.settings.clamp(n);
        let it = items[self.settings.sel];
        match b {
            Button::Up => self.settings.up(n),
            Button::Down => self.settings.down(n),
            Button::Left | Button::Right => {
                let d = if b == Button::Left { -10 } else { 10 };
                match it {
                    Setting::Brightness => return Action::Brightness(d),
                    Setting::Volume => return Action::Volume(d / 2),
                    _ => {}
                }
            }
            Button::A => match it {
                Setting::Keyboard => return Action::ShowKeyboard,
                Setting::Sleep => return Action::Sleep,
                Setting::Logout => {
                    if self.confirmed("logout", "log out (all apps close)") {
                        return Action::Logout;
                    }
                }
                Setting::Reboot => {
                    if self.confirmed("reboot", "restart") {
                        return Action::Reboot;
                    }
                }
                Setting::PowerOff => {
                    if self.confirmed("poweroff", "power off") {
                        return Action::PowerOff;
                    }
                }
                Setting::Wifi | Setting::Remote => self.toast("Wi-Fi and SSH settings arrive with the Settings screens"),
                Setting::Brightness | Setting::Volume => {}
            },
            _ => {}
        }
        Action::None
    }

    // ------------------------------------------------------------ draw ----

    pub fn draw(&mut self, ui: &mut Ui, v: &View) {
        let home = Self::home(v);
        let tabs: Vec<(Tab, &str)> = TABS.iter().copied().filter(|(t, _)| !home || *t != Tab::Apps).collect();
        if !tabs.iter().any(|t| t.0 == self.tab) {
            self.tab = tabs[0].0;
        }
        let mut hints = vec![hint("L1/R1", "Tabs")];
        match self.tab {
            Tab::Apps => {
                if !v.running.is_empty() {
                    hints.push(hint("X", "Close"));
                    hints.push(hint("A", "Switch"));
                }
            }
            Tab::Launch => hints.push(hint("A", "Open")),
            Tab::Settings => {
                let items = settings(v);
                if matches!(items.get(self.settings.sel), Some(Setting::Brightness | Setting::Volume)) {
                    hints.push(hint("←→", "Adjust"));
                }
                hints.push(hint("A", "Choose"));
            }
        }
        if home {
            hints.insert(0, hint("MENU", if self.tab == Tab::Settings { "Apps" } else { "Settings" }));
        } else {
            hints.insert(1, hint("B", "Back"));
        }
        let mut status = v.sys.bar();
        status.left = v.sys.time.clone();
        let body = toolkit::frame(ui, &status, &hints);
        let t = mortar_ui::theme::get();

        // header: crumb + title on the left, tab tags on the right
        let mut y = body.y;
        let crumb = match v.current {
            Some(_) => format!("MENU · {} running", v.running.len()),
            None => "Mortar".to_string(),
        };
        y += toolkit::crumb(ui, body.x, y, &crumb);
        let title = v.current.map(|c| c.name.as_str()).unwrap_or("Home");
        let th = toolkit::title_h2(ui, body.x, y, title);
        let mut right = body.right();
        let cy = y + th / 2.0;
        for (tab, name) in tabs.iter().rev() {
            let kind = if *tab == self.tab { PillKind::Ok } else { PillKind::Off };
            right -= toolkit::tag(ui, right, cy, name, kind) + ui.cq(1.0);
        }
        y += th + ui.cq(2.4);
        let area = Rect::new(body.x, y, body.w, body.bottom() - y);

        match self.tab {
            Tab::Apps => self.draw_apps(ui, area, v),
            Tab::Launch => self.draw_launch(ui, area, v),
            Tab::Settings => self.draw_settings(ui, area, v),
        }
        if home && self.tab == Tab::Launch {
            // the strip from mockup 7: who is connected
            let st = toolkit::st_mono(ui, 1.9, t.line_soft);
            let msg = match v.sys.ssh_sessions {
                0 if v.sys.sshd_listening => "Remote access on · no one connected".to_string(),
                0 => "Remote access off".to_string(),
                1 => "1 SSH session connected".to_string(),
                n => format!("{n} SSH sessions connected"),
            };
            ui.text(body.x, body.bottom() - st.line_px(), &msg, &st);
        }
        if let Some((m, _)) = &self.toast {
            toolkit::toast(ui, m);
        }
    }

    fn draw_apps(&mut self, ui: &mut Ui, area: Rect, v: &View) {
        if v.running.is_empty() {
            let t = mortar_ui::theme::get();
            let st = toolkit::st_body(ui, 2.6, t.line_soft);
            ui.text(area.x, area.y, "Nothing running. Start something from Launch.", &st);
            return;
        }
        let items: Vec<MenuItem> = v
            .running
            .iter()
            .enumerate()
            .map(|(i, a)| MenuItem {
                glyph: Glyph::Icon(a.icon),
                title: a.name.clone(),
                desc: Some(if a.closing {
                    "Closing…".into()
                } else if a.windows == 0 {
                    "Starting…".into()
                } else if a.visible {
                    "On screen".into()
                } else {
                    "In the background, still running".into()
                }),
                pill: if a.visible { Some(("NOW".into(), PillKind::Lavender)) } else { None },
                selected: i == self.running.sel,
                dimmed: a.closing,
            })
            .collect();
        toolkit::menu(ui, area, 1.2, &items, &mut self.running);
    }

    fn draw_launch(&mut self, ui: &mut Ui, area: Rect, v: &View) {
        let n = v.launcher.len();
        if n == 0 {
            let t = mortar_ui::theme::get();
            toolkit::iso_blocks(ui, area.x + area.w / 2.0, area.y + area.h / 2.0 - ui.cq(4.0), ui.cq(22.0));
            let st = toolkit::st_body(ui, 2.4, t.line_soft);
            let msg = "No apps configured (mortar.shell.apps).";
            let w = ui.measure(msg, &st);
            ui.text(area.x + (area.w - w) / 2.0, area.bottom() - ui.cq(8.0), msg, &st);
            return;
        }
        self.tile = self.tile.min(n - 1);
        let gap = ui.cq(2.4);
        let tw = (area.w - gap * (TILES_PER_ROW as f32 - 1.0)) / TILES_PER_ROW as f32;
        let th = (tw * 1.05).min(area.h - ui.cq(5.0));
        let rows_fit = (((area.h - ui.cq(4.0)) + gap) / (th + gap)).floor().max(1.0) as usize;
        let sel_row = self.tile / TILES_PER_ROW;
        let first_row = sel_row.saturating_sub(rows_fit - 1);
        for (i, l) in v.launcher.iter().enumerate() {
            let r = i / TILES_PER_ROW;
            if r < first_row || r >= first_row + rows_fit {
                continue;
            }
            let c = i % TILES_PER_ROW;
            let rect = Rect::new(area.x + c as f32 * (tw + gap), area.y + (r - first_row) as f32 * (th + gap), tw, th);
            let desc = if l.running { "Running" } else { l.description.as_str() };
            toolkit::tile(ui, rect, l.icon, &l.name, desc, i == self.tile);
            if l.running {
                let p = ui.cq(1.6);
                toolkit::tag(ui, rect.right() - p, rect.y + p + ui.cq(1.4), "ON", PillKind::Lavender);
            }
        }
    }

    fn draw_settings(&mut self, ui: &mut Ui, area: Rect, v: &View) {
        let items = settings(v);
        let pct = |p: Option<u8>| p.map(|b| format!("‹ {b}% ›")).unwrap_or_else(|| "n/a".into());
        let rows: Vec<Row> = items
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let txt = |s: &str| RowValue::Text(s.to_string());
                let r = match s {
                    Setting::Brightness => Row::new("Brightness", RowValue::Text(pct(v.sys.brightness))),
                    Setting::Volume => Row::new("Volume", RowValue::Text(pct(v.volume))),
                    Setting::Keyboard => Row::new("On-screen keyboard", txt("START in the app ›")),
                    Setting::Wifi => Row::new("Wi-Fi", txt(if v.sys.wifi { "Connected" } else { "Off" })),
                    Setting::Remote => Row::new(
                        "Remote access",
                        RowValue::Text(if v.sys.sshd_listening {
                            match v.sys.ssh_sessions {
                                0 => "On".to_string(),
                                1 => "On · 1 session".to_string(),
                                n => format!("On · {n} sessions"),
                            }
                        } else {
                            "Off".into()
                        }),
                    ),
                    Setting::Sleep => Row::new("Sleep", txt("Power key")),
                    Setting::Logout => Row::new("Log out", txt("Closes all apps")),
                    Setting::Reboot => Row::new("Restart", txt("›")),
                    Setting::PowerOff => Row::new("Power off", txt("›")),
                };
                Row { selected: i == self.settings.sel, ..r }
            })
            .collect();
        toolkit::rows(ui, area, &rows, &mut self.settings);
    }
}

/// The small volume/brightness indicator shown over an app: a tag-sized
/// block in the top-right corner. Returns the area used.
pub fn draw_osd(ui: &mut Ui, text: &str) -> Rect {
    let t = mortar_ui::theme::get();
    let st = mortar_ui::gfx::TextStyle::new(mortar_ui::gfx::Face::Mono600, ui.cq(2.4), t.lime_ink);
    let w = (ui.measure(text, &st) + ui.cq(3.0)).ceil();
    let h = (st.line_px() + ui.cq(1.6)).ceil();
    let r = Rect::new(0.0, 0.0, w, h);
    toolkit::block(ui, r, t.lime);
    ui.text_vcenter(ui.cq(1.5), h / 2.0, text, &st);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(uid: u32, name: &str, visible: bool) -> RunningInfo {
        RunningInfo { uid, name: name.into(), icon: Icon::Pad, visible, windows: 1, closing: false }
    }

    #[test]
    fn menu_a_switches_to_previous_app() {
        let running = [run(2, "Terminal", true), run(1, "M8", false)];
        let sys = Sys::demo(None);
        let v = View { running: &running, launcher: &[], sys: &sys, volume: None, current: Some(&running[0]), keyboard_app: true };
        let mut o = Overlay::default();
        o.open(&v);
        assert_eq!(o.button(Button::A, &v), Action::Focus(1));
        assert_eq!(o.button(Button::B, &v), Action::Hide);
    }

    #[test]
    fn close_needs_two_presses() {
        let running = [run(2, "Terminal", true)];
        let sys = Sys::demo(None);
        let v = View { running: &running, launcher: &[], sys: &sys, volume: None, current: Some(&running[0]), keyboard_app: false };
        let mut o = Overlay::default();
        o.open(&v);
        assert_eq!(o.button(Button::X, &v), Action::None);
        assert_eq!(o.button(Button::X, &v), Action::Close(2));
    }

    #[test]
    fn home_launches_tiles() {
        let launcher: Vec<LauncherInfo> = (0..6)
            .map(|i| LauncherInfo { id: format!("a{i}"), name: format!("A{i}"), description: String::new(), icon: Icon::Pad, running: false })
            .collect();
        let sys = Sys::demo(None);
        let v = View { running: &[], launcher: &launcher, sys: &sys, volume: None, current: None, keyboard_app: false };
        let mut o = Overlay::default();
        o.button(Button::Down, &v);
        o.button(Button::Right, &v);
        assert_eq!(o.button(Button::A, &v), Action::Launch("a5".into()));
        // B does not leave the home screen; MENU flips to settings
        assert_eq!(o.button(Button::B, &v), Action::None);
        o.button(Button::Menu, &v);
        assert_eq!(o.tab, Tab::Settings);
        o.button(Button::Right, &v);
        assert_eq!(o.button(Button::Right, &v), Action::Brightness(10));
    }
}
