// SPDX-License-Identifier: GPL-2.0-or-later
//! Screen 9: the greeter. Profiles on the left (L1/R1), sessions on the
//! right (D-pad), A starts. Each profile's last session is remembered, so a
//! normal boot is one press. Also the PIN pad for profiles PAM asks about.

use crate::app::{Ctx, Flow, Screen};
use crate::config::{Auth, Profile, Session};
use crate::gfx::{Rect, Ui};
use crate::greetd::Reply;
use crate::input::Button;
use crate::quickmenu::QuickMenu;
use crate::toolkit::{self, hint, Glyph, ListState, MenuItem, PillKind};
use std::time::{Duration, Instant, SystemTime};

pub struct Greeter {
    profile: usize,
    sessions: ListState,
    /// "No Wi-Fi: press A again" confirmation deadline.
    confirm: Option<(String, Instant)>,
}

impl Greeter {
    pub fn new(ctx: &Ctx) -> Greeter {
        let profile = ctx
            .state
            .last_user
            .as_ref()
            .and_then(|u| ctx.cfg.profiles.iter().position(|p| &p.user == u))
            .unwrap_or(0);
        let mut g = Greeter { profile, sessions: ListState::default(), confirm: None };
        g.select_last(ctx);
        g
    }

    fn profile<'a>(&self, ctx: &'a Ctx) -> &'a Profile {
        &ctx.cfg.profiles[self.profile]
    }

    fn select_last(&mut self, ctx: &Ctx) {
        let p = self.profile(ctx);
        let list = ctx.cfg.sessions_for(p);
        self.sessions = ListState::default();
        if let Some(last) = ctx.state.last_session.get(&p.user)
            && let Some(i) = list.iter().position(|s| &s.id == last) {
                self.sessions.sel = i;
            }
    }

    fn profile_desc(ctx: &Ctx, p: &Profile) -> String {
        let list = ctx.cfg.sessions_for(p);
        let mut d = if list.len() == 1 {
            format!("{} only", list[0].name)
        } else if let Some(s) = ctx.state.last_session.get(&p.user).and_then(|id| ctx.cfg.session(id)) {
            format!("Last: {}", s.name)
        } else if list.is_empty() {
            "No sessions".into()
        } else {
            format!("{} sessions", list.len())
        };
        if p.auth == Auth::Pin {
            d.push_str(" · PIN");
        }
        d
    }

    fn start(&mut self, ctx: &mut Ctx, force: bool) -> Flow {
        let p = self.profile(ctx).clone();
        let list = ctx.cfg.sessions_for(&p);
        let Some(s) = list.get(self.sessions.sel).map(|s| (*s).clone()) else {
            ctx.toast("No session to start");
            return Flow::Stay;
        };
        if s.needs_wifi && !ctx.sys.wifi && !force {
            let armed = self.confirm.as_ref().is_some_and(|(id, t)| *id == s.id && Instant::now() < *t);
            if !armed {
                self.confirm = Some((s.id.clone(), Instant::now() + Duration::from_secs(4)));
                ctx.toast(format!("{} needs Wi-Fi. Press A again to start anyway.", s.name));
                return Flow::Stay;
            }
        }
        self.confirm = None;
        login(ctx, &p.user, &s.id)
    }

    /// A pending "switch to session X" left by the quick menu of a session
    /// that just logged out: start it without asking (only works if PAM lets
    /// the profile in without a prompt).
    fn take_switch_request(ctx: &mut Ctx) -> Option<(String, String)> {
        let dir = ctx.cfg.switch_dir.clone();
        for p in &ctx.cfg.profiles {
            let f = dir.join(format!("switch-{}", p.user));
            let Ok(meta) = std::fs::metadata(&f) else { continue };
            let content = std::fs::read_to_string(&f).unwrap_or_default();
            let _ = std::fs::remove_file(&f);
            let fresh = meta.modified().ok().and_then(|m| SystemTime::now().duration_since(m).ok()).is_some_and(|a| a < Duration::from_secs(120));
            if !fresh || !(ctx.dev || owned_by(&meta, &p.user)) {
                continue;
            }
            let id = content.trim().to_string();
            if ctx.cfg.sessions_for(p).iter().any(|s| s.id == id) {
                return Some((p.user.clone(), id));
            }
        }
        None
    }
}

#[cfg(unix)]
fn owned_by(meta: &std::fs::Metadata, user: &str) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(name) = std::ffi::CString::new(user) else { return false };
    let pw = unsafe { libc::getpwnam(name.as_ptr()) };
    !pw.is_null() && unsafe { (*pw).pw_uid } == meta.uid()
}

impl Screen for Greeter {
    fn enter(&mut self, ctx: &mut Ctx) -> Flow {
        if let Some((user, id)) = Self::take_switch_request(ctx) {
            if let Some(i) = ctx.cfg.profiles.iter().position(|p| p.user == user) {
                self.profile = i;
                self.select_last(ctx);
            }
            return login(ctx, &user, &id);
        }
        Flow::Stay
    }

    fn draw(&mut self, ui: &mut Ui, ctx: &Ctx) {
        let mut hints = Vec::new();
        if ctx.cfg.profiles.len() > 1 {
            hints.push(hint("L1/R1", "Profile"));
        }
        hints.push(hint("A", "Start"));
        let body = toolkit::frame(ui, &ctx.sys.bar(), &hints);
        let gap = ui.cq(5.0);
        let lw = (body.w - gap) * 0.8 / 2.0;
        let left = Rect::new(body.x, body.y, lw, body.h);
        let right = Rect::new(body.x + lw + gap, body.y, body.w - lw - gap, body.h);
        let col_gap = ui.cq(1.6);

        // profiles
        let lh = toolkit::label(ui, left.x, left.y, "Profile");
        let items: Vec<MenuItem> = ctx
            .cfg
            .profiles
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let initial = p.display().chars().next().unwrap_or('?').to_uppercase().to_string();
                MenuItem {
                    glyph: Glyph::Letter(initial),
                    title: p.display().to_string(),
                    desc: Some(Self::profile_desc(ctx, p)),
                    pill: None,
                    selected: i == self.profile,
                    dimmed: false,
                }
            })
            .collect();
        let mut pl = ListState { sel: self.profile, scroll: 0 };
        let (_, rest) = left.take_top(lh + col_gap);
        toolkit::menu(ui, rest, 1.6, &items, &mut pl);

        // sessions
        let lh = toolkit::label(ui, right.x, right.y, "Session");
        let p = self.profile(ctx);
        let list = ctx.cfg.sessions_for(p);
        let items: Vec<MenuItem> = list
            .iter()
            .enumerate()
            .map(|(i, s)| session_item(ctx, s, i == self.sessions.sel))
            .collect();
        let (_, rest) = right.take_top(lh + col_gap);
        if items.is_empty() {
            let st = toolkit::st_body(ui, 2.6, mortar_ui::theme::get().line_soft);
            ui.text(rest.x, rest.y, "No sessions for this profile.", &st);
        }
        toolkit::menu(ui, rest, 1.2, &items, &mut self.sessions);
    }

    fn button(&mut self, b: Button, _repeat: bool, ctx: &mut Ctx) -> Flow {
        let n = ctx.cfg.sessions_for(self.profile(ctx)).len();
        let np = ctx.cfg.profiles.len();
        match b {
            Button::Up => self.sessions.up(n),
            Button::Down => self.sessions.down(n),
            Button::L1 | Button::Left => {
                self.profile = (self.profile + np - 1) % np;
                self.select_last(ctx);
            }
            Button::R1 | Button::Right => {
                self.profile = (self.profile + 1) % np;
                self.select_last(ctx);
            }
            Button::A | Button::Start => return self.start(ctx, false),
            Button::Menu => return Flow::Push(Box::new(QuickMenu::for_greeter())),
            _ => {}
        }
        self.confirm = None;
        Flow::Stay
    }
}

pub fn session_item(ctx: &Ctx, s: &Session, selected: bool) -> MenuItem {
    let no_wifi = s.needs_wifi && !ctx.sys.wifi;
    MenuItem {
        glyph: Glyph::Icon(s.icon),
        title: s.name.clone(),
        desc: if s.description.is_empty() { None } else { Some(s.description.clone()) },
        pill: if no_wifi { Some(("No Wi-Fi".into(), PillKind::Off)) } else { None },
        selected,
        dimmed: false,
    }
}

// ------------------------------------------------------- login flow ----

/// Begin a greetd login for `user` and, once authenticated, start session
/// `sid`. Prompts push the PIN pad.
pub fn login(ctx: &mut Ctx, user: &str, sid: &str) -> Flow {
    let r = ctx.greetd.create_session(user);
    advance(ctx, user, sid, r)
}

fn advance(ctx: &mut Ctx, user: &str, sid: &str, mut r: Result<Reply, String>) -> Flow {
    loop {
        match r {
            Err(e) => {
                let _ = ctx.greetd.cancel();
                ctx.toast(format!("greetd: {e}"));
                return Flow::PopToRoot;
            }
            Ok(Reply::Info(t)) => {
                ctx.toast(t);
                r = ctx.greetd.respond(None);
            }
            Ok(Reply::Prompt { text, secret }) => {
                return Flow::Push(Box::new(PinPad::new(user, sid, &text, secret)));
            }
            Ok(Reply::Error { text, auth }) => {
                let _ = ctx.greetd.cancel();
                ctx.toast(if auth { format!("Login failed: {text}") } else { text });
                return Flow::PopToRoot;
            }
            Ok(Reply::Success) => return start_session(ctx, user, sid),
        }
    }
}

fn start_session(ctx: &mut Ctx, user: &str, sid: &str) -> Flow {
    let Some(s) = ctx.cfg.session(sid).cloned() else {
        let _ = ctx.greetd.cancel();
        ctx.toast(format!("Unknown session {sid}"));
        return Flow::PopToRoot;
    };
    let mut env: Vec<String> = s.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
    env.push(format!("MORTAR_SESSION={}", s.id));
    env.push(format!("XDG_SESSION_DESKTOP={}", s.id));
    // remember before handing over: the greeter exits right after this
    ctx.state.last_user = Some(user.to_string());
    ctx.state.last_session.insert(user.to_string(), s.id.clone());
    if let Err(e) = ctx.state.save(&ctx.cfg.state_dir) {
        eprintln!("mortar-greeter: saving state: {e}");
    }
    match ctx.greetd.start_session(s.command.clone(), env) {
        Ok(Reply::Success) => {
            if ctx.dev {
                ctx.toast(format!("dev: started {} as {user}", s.name));
                Flow::PopToRoot
            } else {
                Flow::Exit(0)
            }
        }
        Ok(Reply::Error { text, .. }) => {
            let _ = ctx.greetd.cancel();
            ctx.toast(text);
            Flow::PopToRoot
        }
        Ok(other) => {
            let _ = ctx.greetd.cancel();
            ctx.toast(format!("unexpected reply {other:?}"));
            Flow::PopToRoot
        }
        Err(e) => {
            let _ = ctx.greetd.cancel();
            ctx.toast(format!("greetd: {e}"));
            Flow::PopToRoot
        }
    }
}

// ---------------------------------------------------------- PIN pad ----

const PAD: [[&str; 3]; 4] = [["1", "2", "3"], ["4", "5", "6"], ["7", "8", "9"], ["del", "0", "OK"]];
const MAX_PIN: usize = 16;

/// Answers a PAM prompt with the D-pad (or a keyboard's digits).
pub struct PinPad {
    user: String,
    sid: String,
    prompt: String,
    secret: bool,
    entry: String,
    row: usize,
    col: usize,
}

impl PinPad {
    pub fn new(user: &str, sid: &str, prompt: &str, secret: bool) -> PinPad {
        PinPad {
            user: user.into(),
            sid: sid.into(),
            prompt: prompt.trim().trim_end_matches(':').to_string(),
            secret,
            entry: String::new(),
            row: 0,
            col: 0,
        }
    }
    fn submit(&mut self, ctx: &mut Ctx) -> Flow {
        let answer = std::mem::take(&mut self.entry);
        let r = ctx.greetd.respond(Some(answer));
        let (user, sid) = (self.user.clone(), self.sid.clone());
        match advance(ctx, &user, &sid, r) {
            Flow::Push(next) => Flow::Replace(next), // another prompt
            f => f,
        }
    }
    fn type_digit(&mut self, d: char) {
        if self.entry.len() < MAX_PIN {
            self.entry.push(d);
        }
    }
}

impl Screen for PinPad {
    fn draw(&mut self, ui: &mut Ui, ctx: &Ctx) {
        let hints = [hint("B", "Delete"), hint("A", "Type"), hint("START", "OK")];
        let body = toolkit::frame(ui, &ctx.sys.bar(), &hints);
        let gap = ui.cq(5.0);
        let lw = (body.w - gap) * 0.5;
        let mut y = body.y + ui.cq(1.0);
        y += toolkit::crumb(ui, body.x, y, &format!("Profile / {}", self.user));
        let title = if self.prompt.is_empty() || self.prompt.eq_ignore_ascii_case("pin") { "Enter PIN" } else { &self.prompt };
        y += toolkit::title_h2(ui, body.x, y, title) + ui.cq(2.2);
        let shown: String = if self.secret { "•".repeat(self.entry.chars().count()) } else { self.entry.clone() };
        y += toolkit::text_field(ui, body.x, y, lw, &shown, None) + ui.cq(2.2);
        let sub = toolkit::st_body(ui, 2.4, mortar_ui::theme::get().line_soft);
        toolkit::paragraph(ui, body.x, y, lw, "Use the D-pad and A, or type digits on a keyboard. B with nothing typed goes back.", &sub);

        // keypad, 3x4, vertically centred in the right column
        let kx = body.x + lw + gap;
        let kw = body.w - lw - gap;
        let kg = ui.cq(1.2);
        let cw = (kw - 2.0 * kg) / 3.0;
        let ch = ui.cq(9.0);
        let total_h = 4.0 * ch + 3.0 * kg;
        let ky = body.y + (body.h - total_h) / 2.0;
        for (r, row) in PAD.iter().enumerate() {
            for (c, k) in row.iter().enumerate() {
                let rect = Rect::new(kx + c as f32 * (cw + kg), ky + r as f32 * (ch + kg), cw, ch);
                toolkit::key_cap(ui, rect, k, r == self.row && c == self.col, k.len() > 1);
            }
        }
    }

    fn button(&mut self, b: Button, _repeat: bool, ctx: &mut Ctx) -> Flow {
        match b {
            Button::Up => self.row = (self.row + 3) % 4,
            Button::Down => self.row = (self.row + 1) % 4,
            Button::Left => self.col = (self.col + 2) % 3,
            Button::Right => self.col = (self.col + 1) % 3,
            Button::A => match PAD[self.row][self.col] {
                "del" => {
                    self.entry.pop();
                }
                "OK" => return self.submit(ctx),
                d => self.type_digit(d.chars().next().unwrap()),
            },
            Button::Start => return self.submit(ctx),
            Button::B
                if self.entry.pop().is_none() => {
                    let _ = ctx.greetd.cancel();
                    return Flow::Pop;
                }
            _ => {}
        }
        Flow::Stay
    }

    fn char(&mut self, c: char, _ctx: &mut Ctx) -> Flow {
        if c.is_ascii_digit() {
            self.type_digit(c);
        }
        Flow::Stay
    }
}
