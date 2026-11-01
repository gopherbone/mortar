// SPDX-License-Identifier: GPL-2.0-or-later
//! Screen 10: the quick menu behind the MENU button. Inside a session it
//! offers switch session / sleep / brightness / log out / remote access; on
//! the greeter, sleep / brightness / remote access / restart / power off.
//!
//! In a session it runs as `mortar-ui quick-menu --session <id>`. Switching
//! drops a request for the greeter in the switch dir and logs out; the
//! greeter picks it up and starts the new session without a second press.

use crate::app::{Ctx, Flow, Screen};
use crate::gfx::Ui;
use crate::input::Button;
use crate::toolkit::{self, hint, ListState, Row, RowValue};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Item {
    Switch,
    Sleep,
    Brightness,
    Logout,
    Remote,
    Restart,
    PowerOff,
}

pub struct QuickMenu {
    /// Running session id (None on the greeter).
    session: Option<String>,
    standalone: bool,
    items: Vec<Item>,
    list: ListState,
    confirm: Option<(Item, Instant)>,
}

impl QuickMenu {
    pub fn for_greeter() -> QuickMenu {
        QuickMenu {
            session: None,
            standalone: false,
            items: vec![Item::Sleep, Item::Brightness, Item::Remote, Item::Restart, Item::PowerOff],
            list: ListState::default(),
            confirm: None,
        }
    }
    pub fn for_session(id: &str) -> QuickMenu {
        QuickMenu {
            session: Some(id.to_string()),
            standalone: true,
            items: vec![Item::Switch, Item::Sleep, Item::Brightness, Item::Logout, Item::Remote],
            list: ListState::default(),
            confirm: None,
        }
    }

    fn other_sessions<'a>(&self, ctx: &'a Ctx) -> Vec<&'a crate::config::Session> {
        let user = current_user();
        let all: Vec<_> = match ctx.cfg.profiles.iter().find(|p| Some(&p.user) == user.as_ref()) {
            Some(p) => ctx.cfg.sessions_for(p),
            None => ctx.cfg.sessions.iter().collect(),
        };
        all.into_iter().filter(|s| Some(&s.id) != self.session.as_ref()).collect()
    }

    fn row(&self, it: Item, ctx: &Ctx) -> Row {
        let v = |s: String| RowValue::Text(s);
        match it {
            Item::Switch => {
                let names: Vec<_> = self.other_sessions(ctx).iter().map(|s| s.name.clone()).collect();
                Row::new("Switch session", v(format!("{} ›", if names.is_empty() { "None".into() } else { names.join(", ") })))
            }
            Item::Sleep => Row::new("Sleep", v("Power key".into())),
            Item::Brightness => Row::new(
                "Brightness",
                v(ctx.sys.brightness.map(|b| format!("{b}%")).unwrap_or_else(|| "n/a".into())),
            ),
            Item::Logout => Row::new("Log out", v("Back to greeter".into())),
            Item::Remote => Row::new(
                "Remote access",
                v(if ctx.sys.sshd_listening {
                    match ctx.sys.ssh_sessions {
                        0 => "On".to_string(),
                        1 => "On · 1 session".to_string(),
                        n => format!("On · {n} sessions"),
                    }
                } else {
                    "Off".into()
                }),
            ),
            Item::Restart => Row::new("Restart", v("›".into())),
            Item::PowerOff => Row::new("Power off", v("›".into())),
        }
    }

    fn confirmed(&mut self, it: Item, ctx: &mut Ctx, what: &str) -> bool {
        if self.confirm.is_some_and(|(c, t)| c == it && Instant::now() < t) {
            self.confirm = None;
            return true;
        }
        self.confirm = Some((it, Instant::now() + Duration::from_secs(4)));
        ctx.toast(format!("Press A again to {what}"));
        false
    }

    fn choose(&mut self, ctx: &mut Ctx) -> Flow {
        let it = self.items[self.list.sel];
        match it {
            Item::Switch => {
                let others: Vec<(String, String)> =
                    self.other_sessions(ctx).iter().map(|s| (s.id.clone(), s.name.clone())).collect();
                if others.is_empty() {
                    ctx.toast("No other sessions");
                    return Flow::Stay;
                }
                return Flow::Push(Box::new(SwitchPicker { sessions: others, list: ListState::default() }));
            }
            Item::Sleep => ctx.run(&ctx.cfg.actions.sleep.clone()),
            Item::Brightness => {}
            Item::Logout => return logout(ctx),
            Item::Remote => ctx.toast("Remote access settings arrive with the Settings screens"),
            Item::Restart => {
                if self.confirmed(it, ctx, "restart") {
                    ctx.run(&ctx.cfg.actions.reboot.clone());
                }
            }
            Item::PowerOff => {
                if self.confirmed(it, ctx, "power off") {
                    ctx.run(&ctx.cfg.actions.poweroff.clone());
                }
            }
        }
        Flow::Stay
    }
}

fn current_user() -> Option<String> {
    std::env::var("USER").ok()
}

/// End the current logind session (greetd then shows the greeter again).
fn logout(ctx: &mut Ctx) -> Flow {
    match std::env::var("XDG_SESSION_ID") {
        Ok(id) if !ctx.dev => {
            ctx.run(&["loginctl".into(), "terminate-session".into(), id]);
            Flow::Exit(0)
        }
        _ => {
            ctx.toast("dev: would log out (loginctl terminate-session)");
            Flow::Stay
        }
    }
}

impl Screen for QuickMenu {
    fn draw(&mut self, ui: &mut Ui, ctx: &Ctx) {
        let mut hints = Vec::new();
        if self.items.get(self.list.sel) == Some(&Item::Brightness) {
            hints.push(hint("←→", "Adjust"));
        }
        hints.push(hint("B", "Close"));
        hints.push(hint("A", "Choose"));
        let body = toolkit::frame(ui, &ctx.sys.bar(), &hints);
        let mut y = body.y;
        y += toolkit::crumb(ui, body.x, y, "Quick menu · MENU button");
        let title = match &self.session {
            Some(id) => ctx.cfg.session(id).map(|s| s.name.clone()).unwrap_or_else(|| id.clone()),
            None => "Mortar".into(),
        };
        y += toolkit::title_h2(ui, body.x, y, &title) + ui.cq(2.2);
        let rows: Vec<Row> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, it)| Row { selected: i == self.list.sel, ..self.row(*it, ctx) })
            .collect();
        let r = crate::gfx::Rect::new(body.x, y, body.w, body.bottom() - y);
        toolkit::rows(ui, r, &rows, &mut self.list);
    }

    fn button(&mut self, b: Button, _repeat: bool, ctx: &mut Ctx) -> Flow {
        let n = self.items.len();
        match b {
            Button::Up => self.list.up(n),
            Button::Down => self.list.down(n),
            Button::Left | Button::Right if self.items[self.list.sel] == Item::Brightness => {
                let cur = ctx.sys.brightness.unwrap_or(60) as i32;
                let step = if b == Button::Left { -10 } else { 10 };
                ctx.set_brightness((cur + step).clamp(5, 100) as u8);
            }
            Button::A => return self.choose(ctx),
            Button::B | Button::Menu => return if self.standalone { Flow::Exit(0) } else { Flow::Pop },
            _ => {}
        }
        Flow::Stay
    }
}

/// "Switch session" submenu: pick one, leave a request for the greeter, log
/// out.
struct SwitchPicker {
    sessions: Vec<(String, String)>,
    list: ListState,
}

impl Screen for SwitchPicker {
    fn draw(&mut self, ui: &mut Ui, ctx: &Ctx) {
        let body = toolkit::frame(ui, &ctx.sys.bar(), &[hint("B", "Back"), hint("A", "Switch")]);
        let mut y = body.y;
        y += toolkit::crumb(ui, body.x, y, "Quick menu /");
        y += toolkit::title_h2(ui, body.x, y, "Switch session") + ui.cq(2.2);
        let items: Vec<_> = self
            .sessions
            .iter()
            .enumerate()
            .map(|(i, (id, _))| {
                let s = ctx.cfg.session(id).expect("listed session exists");
                crate::greeter::session_item(ctx, s, i == self.list.sel)
            })
            .collect();
        let r = crate::gfx::Rect::new(body.x, y, body.w, body.bottom() - y);
        toolkit::menu(ui, r, 1.2, &items, &mut self.list);
    }

    fn button(&mut self, b: Button, _repeat: bool, ctx: &mut Ctx) -> Flow {
        let n = self.sessions.len();
        match b {
            Button::Up => self.list.up(n),
            Button::Down => self.list.down(n),
            Button::B => return Flow::Pop,
            Button::A => {
                let id = self.sessions[self.list.sel].0.clone();
                let user = current_user().unwrap_or_default();
                let f = ctx.cfg.switch_dir.join(format!("switch-{user}"));
                if let Err(e) = std::fs::write(&f, &id) {
                    ctx.toast(format!("{}: {e}", f.display()));
                    return Flow::Stay;
                }
                return logout(ctx);
            }
            _ => {}
        }
        Flow::Stay
    }
}
