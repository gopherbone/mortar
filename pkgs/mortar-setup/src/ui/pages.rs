// SPDX-License-Identifier: GPL-2.0-or-later
//! The pages: Welcome (1), Setup file found (2), Connected over USB (3),
//! Wi-Fi networks (replaces the hotspot page 4), joining, Ready (6),
//! Settings > Remote access (8) and the key list.

use super::keyboard::PasswordPage;
use super::{Cx, Mode, Nav, Page};
use crate::control::{KeyView, NetView, Request};
use mortar_ui::gfx::{Color, Face, Rect, TextStyle, Ui};
use mortar_ui::icons::Icon;
use mortar_ui::input::Button;
use mortar_ui::theme;
use mortar_ui::toolkit::{self, hint, Glyph, ListState, MenuItem, PillKind, Row, RowValue};
use std::time::{Duration, Instant};

pub(super) fn t() -> &'static theme::Theme {
    theme::get()
}

// ------------------------------------------------------------ helpers ----

/// Body text in the theme's secondary ink.
pub(super) fn sub(ui: &Ui, cq: f32) -> TextStyle {
    toolkit::st_body(ui, cq, t().line_soft)
}

/// A terminal line: a night block with mono text (`$` in lime).
fn command(ui: &mut Ui, x: f32, y: f32, w: f32, cmd: &str) -> f32 {
    let st = TextStyle::new(Face::Mono400, ui.cq(2.6), Color::hex(0xe1ebf7));
    let h = st.line_px() + ui.cq(2.4);
    toolkit::block(ui, Rect::new(x, y, w, h), t().night);
    let pr = TextStyle::new(Face::Mono600, ui.cq(2.6), t().lime);
    let px = x + ui.cq(2.0);
    let dx = ui.text_vcenter(px, y + h / 2.0, "$", &pr);
    ui.text_fit(px + dx + ui.cq(1.4), y + (h - st.line_px()) / 2.0, cmd, &st, w - ui.cq(6.0));
    h
}

/// A numbered step ("1." in the serif, the motif for setup steps) with
/// wrapped text. Returns the height used.
fn step(ui: &mut Ui, x: f32, y: f32, w: f32, n: u32, text: &str) -> f32 {
    let num = TextStyle::new(Face::SerifItalic, ui.cq(4.0), t().line).line_height(1.0);
    ui.text(x, y - ui.cq(0.4), &format!("{n}."), &num);
    let st = toolkit::st_body(ui, 2.5, t().line);
    let tx = x + ui.cq(5.0);
    toolkit::paragraph(ui, tx, y, w - (tx - x), text, &st).max(num.line_px())
}

/// Signal strength: four rising bars, filled up to `n`.
pub(super) fn signal(ui: &mut Ui, right: f32, cy: f32, n: u8, ink: Color, faint: Color) -> f32 {
    let (bw, gap) = (ui.cq(0.8), ui.cq(0.45));
    let full = ui.cq(2.6);
    let w = 4.0 * bw + 3.0 * gap;
    let x0 = right - w;
    for i in 0..4u8 {
        let h = full * (i as f32 + 1.0) / 4.0;
        let r = Rect::new(x0 + i as f32 * (bw + gap), cy + full / 2.0 - h, bw, h);
        ui.fill_block(r, if i < n { ink } else { faint });
    }
    w
}

fn on_off(b: bool) -> String {
    if b { "On".into() } else { "Off".into() }
}

fn key_label(k: &KeyView) -> String {
    if k.comment.is_empty() { k.fingerprint.clone() } else { format!("{} ({})", k.comment, k.kind) }
}

/// `SHA256:Qx3n…w8Ka`
fn short_fp(fp: &str) -> String {
    let body = fp.strip_prefix("SHA256:").unwrap_or(fp);
    if body.len() > 10 {
        format!("SHA256:{}…{}", &body[..4], &body[body.len() - 4..])
    } else {
        fp.to_string()
    }
}

fn host_local(cx: &Cx) -> String {
    format!("{}.local", cx.st().hostname)
}

/// A two-press confirmation (destructive switches).
#[derive(Default)]
pub(super) struct Confirm(Option<(String, Instant)>);

impl Confirm {
    pub(super) fn ok(&mut self, key: &str, cx: &mut Cx, what: &str) -> bool {
        if self.0.as_ref().is_some_and(|(k, t)| k == key && Instant::now() < *t) {
            self.0 = None;
            return true;
        }
        self.0 = Some((key.into(), Instant::now() + Duration::from_secs(4)));
        cx.toast(format!("Press A again to {what}"));
        false
    }
}

// ----------------------------------------------------------- 1 welcome ----

#[derive(Default)]
pub struct Welcome {
    list: ListState,
}

impl Page for Welcome {
    fn draw(&mut self, ui: &mut Ui, cx: &Cx) {
        let body = cx.frame(ui, false, &[hint("A", "Choose")]);
        let mut y = body.y + ui.cq(1.0);
        y += toolkit::title_h1(ui, body.x, y, "Hello.");
        y += ui.cq(0.6);
        let st = sub(ui, 2.6).line_height(1.4);
        toolkit::paragraph(ui, body.x, y, ui.cq(56.0), "Let's connect this Brick to your computer. It takes about a minute.", &st);
        toolkit::iso_blocks(ui, body.right() - ui.cq(11.0), body.y + ui.cq(8.0), ui.cq(15.0));
        let items = [
            (Icon::Cable, "USB cable", "Fastest. No Wi-Fi needed.", Some(("Recommended", PillKind::Ok))),
            (Icon::Wifi, "Wi-Fi", "Join a network, then connect over it.", None),
            (Icon::Skip, "Skip for now", "You can do this later in Settings.", None),
        ];
        let items: Vec<MenuItem> = items
            .iter()
            .enumerate()
            .map(|(i, (icon, title, desc, pill))| MenuItem {
                glyph: Glyph::Icon(*icon),
                title: (*title).into(),
                desc: Some((*desc).into()),
                pill: pill.map(|(p, k)| (p.to_string(), k)),
                selected: i == self.list.sel,
                dimmed: false,
            })
            .collect();
        let top = body.y + ui.cq(20.5);
        toolkit::menu(ui, Rect::new(body.x, top, body.w, body.bottom() - top), 1.2, &items, &mut self.list);
    }

    fn button(&mut self, b: Button, cx: &mut Cx) -> Nav {
        match b {
            Button::Up => self.list.up(3),
            Button::Down => self.list.down(3),
            Button::A | Button::Start => {
                return match self.list.sel {
                    0 => Nav::Push(Box::new(UsbPair::new(cx))),
                    1 => Nav::Push(Box::new(WifiList::new(cx))),
                    _ => {
                        if cx.mode == Mode::FirstBoot {
                            cx.act(Request::Finish);
                        }
                        Nav::Exit
                    }
                };
            }
            _ => {}
        }
        Nav::Stay
    }
}

// ------------------------------------------------------- 2 setup file ----

#[derive(Default)]
pub struct SetupFile;

impl Page for SetupFile {
    fn draw(&mut self, ui: &mut Ui, cx: &Cx) {
        let body = cx.frame(ui, false, &[hint("B", "Set up by hand"), hint("A", "Continue")]);
        let sf = cx.st().setup_file.clone().unwrap_or_default();
        let mut y = body.y + ui.cq(1.0);
        y += toolkit::title_h2(ui, body.x, y, "Found your setup file");
        y += ui.cq(0.6);
        let st = sub(ui, 2.4);
        let w = ui.text(body.x, y, "Applied from ", &st);
        let mono = toolkit::st_mono(ui, 2.3, t().line);
        let w2 = ui.text(body.x + w, y + ui.cq(0.1), "mortar.toml", &mono);
        ui.text(body.x + w + w2, y, " on this card's boot partition:", &st);
        y += st.line_px() + ui.cq(2.0);

        let keys = if sf.keys.is_empty() {
            "None".to_string()
        } else {
            format!("{} · {}", sf.keys.len(), sf.keys.join(", "))
        };
        let st_now = cx.st();
        let rows = [
            ("Name", format!("{}.local", sf.hostname.clone().unwrap_or_else(|| st_now.hostname.clone()))),
            ("Wi-Fi", sf.wifi_ssid.clone().unwrap_or_else(|| "Not set".into())),
            ("SSH keys", keys),
            ("Password login", on_off(st_now.password_login)),
            ("USB network", on_off(st_now.usb_network)),
        ];
        for (k, v) in rows {
            y += toolkit::row(ui, body.x, y, body.w, &Row::new(k, RowValue::Text(v)));
        }
        y += ui.cq(2.4);
        let note = if sf.wifi_password_scrubbed {
            Some(("The Wi-Fi password was removed from the file after it was saved on the Brick.", PillKind::Lavender))
        } else if sf.wifi_ssid.is_some() {
            Some(("The Wi-Fi password is removed from the file once it is saved on the Brick.", PillKind::Lavender))
        } else {
            None
        };
        if let Some(w0) = sf.warnings.first() {
            note_line(ui, body.x, y, body.w, &format!("Check mortar.toml: {w0}"), PillKind::Orange);
        } else if let Some((n, k)) = note {
            note_line(ui, body.x, y, body.w, n, k);
        }
    }

    fn button(&mut self, b: Button, cx: &mut Cx) -> Nav {
        match b {
            Button::A | Button::Start => {
                if crate_keys(cx) > 0 {
                    Nav::Replace(Box::new(Ready::default()))
                } else {
                    // nothing to log in with yet: offer the cable
                    Nav::Replace(Box::new(UsbPair::new(cx)))
                }
            }
            Button::B => Nav::Replace(Box::new(Welcome::default())),
            _ => Nav::Stay,
        }
    }
}

fn crate_keys(cx: &Cx) -> usize {
    cx.st().keys.len()
}

/// A tag-like note: a small square accent block, then the text.
pub(super) fn note_line(ui: &mut Ui, x: f32, y: f32, w: f32, text: &str, kind: PillKind) -> f32 {
    let (bg, _) = toolkit::kind_colors(kind);
    let s = ui.cq(2.2);
    toolkit::block(ui, Rect::new(x, y + ui.cq(0.35), s, s), bg);
    let st = sub(ui, 2.2);
    toolkit::paragraph(ui, x + s + ui.cq(1.4), y, w - s - ui.cq(1.4), text, &st)
}

// ------------------------------------------------------------ 3 USB ----

pub struct UsbPair {
    keys_at_start: Vec<String>,
}

impl UsbPair {
    pub fn new(cx: &mut Cx) -> UsbPair {
        if cx.st().pairing.is_none() && !cx.st().pairing_locked {
            cx.act(Request::NewCode);
        }
        UsbPair { keys_at_start: cx.st().keys.iter().map(|k| k.fingerprint.clone()).collect() }
    }
}

impl Page for UsbPair {
    fn live(&self) -> bool {
        true
    }

    fn draw(&mut self, ui: &mut Ui, cx: &Cx) {
        let st = cx.st();
        let body = cx.frame(ui, false, &[hint("Y", "New code"), hint("B", "Back")]);
        let colw = ui.cq(50.0);
        let mut y = body.y + ui.cq(1.0);
        let title = if st.usb_connected || !st.usb_network {
            "Connected to your computer"
        } else {
            "Plug in the USB cable"
        };
        y += toolkit::title_h2(ui, body.x, y, title);
        y += ui.cq(2.4);
        let top = y;
        if !st.usb_connected && st.usb_network {
            y += step(ui, body.x, y, colw, 1, "Connect the Brick's bottom USB-C port to your computer.") + ui.cq(2.2);
            y += step(ui, body.x, y, colw, 2, "It shows up as a network adapter. This screen changes when it does.") + ui.cq(2.2);
            step(ui, body.x, y, colw, 3, "Then run mortar pair in a terminal, and type the code.");
        } else {
            y += step(ui, body.x, y, colw, 1, "Open a terminal on your computer.") + ui.cq(2.0);
            y += step(ui, body.x, y, colw, 2, "Run this:") + ui.cq(1.0);
            y += command(ui, body.x + ui.cq(5.0), y, colw - ui.cq(5.0), "mortar pair") + ui.cq(2.4);
            if st.password_login {
                let text = format!("Or just ssh root@{} and use the code as the password.", host_local(cx));
                step(ui, body.x, y, colw, 3, &text);
            } else {
                step(ui, body.x, y, colw, 3, "It asks for the code and checks the host key below for you.");
            }
        }

        // right column: code + fingerprint
        let px = body.x + colw + ui.cq(5.0);
        let pw = body.right() - px;
        let mut py = top - ui.cq(0.4);
        py += toolkit::label(ui, px, py, "Pairing code") + ui.cq(0.8);
        let code_st = TextStyle::new(Face::Mono600, ui.cq(5.4), t().yellow_ink);
        let bh = code_st.line_px() + ui.cq(3.0);
        if st.pairing_locked {
            toolkit::block(ui, Rect::new(px, py, pw, bh), t().orange);
            let s2 = TextStyle::new(Face::Body600, ui.cq(2.4), t().orange_ink);
            ui.text_vcenter(px + ui.cq(2.0), py + bh / 2.0, "Too many wrong codes", &s2);
            py += bh + ui.cq(1.0);
            ui.text(px, py, "Press Y for a new one.", &sub(ui, 2.2));
            py += ui.cq(3.6);
        } else if let Some(p) = &st.pairing {
            toolkit::block(ui, Rect::new(px, py, pw, bh), t().yellow);
            let cw = ui.measure(&p.code, &code_st);
            ui.text_vcenter(px + (pw - cw) / 2.0, py + bh / 2.0, &p.code, &code_st);
            py += bh + ui.cq(1.0);
            let left = p.expires_in_s;
            let txt = format!("Expires in {}:{:02}", left / 60, left % 60);
            ui.text(px, py, &txt, &toolkit::st_mono(ui, 2.1, t().line_soft));
            py += ui.cq(3.6);
        } else {
            toolkit::block(ui, Rect::new(px, py, pw, bh), t().ground_2);
            ui.text_vcenter(px + ui.cq(2.0), py + bh / 2.0, "No code", &sub(ui, 2.4));
            py += bh + ui.cq(4.6);
        }
        py += ui.cq(1.6);
        py += toolkit::label(ui, px, py, "Host key") + ui.cq(0.6);
        let fp = st.host_key_fingerprint.clone().unwrap_or_else(|| "not generated yet".into());
        let fst = toolkit::st_mono(ui, 2.1, t().line).line_height(1.45);
        // SHA256: on the first line, the 43 characters split in two
        let lines: Vec<String> = match fp.strip_prefix("SHA256:") {
            Some(b) if b.len() > 22 => vec!["SHA256:".into(), b[..22].into(), b[22..].into()],
            _ => vec![fp.clone()],
        };
        for l in lines {
            ui.text(px, py, &l, &fst);
            py += fst.line_px();
        }
        py += ui.cq(1.0);
        toolkit::paragraph(ui, px, py, pw, "mortar pair checks this for you.", &sub(ui, 2.0));
    }

    fn button(&mut self, b: Button, cx: &mut Cx) -> Nav {
        match b {
            Button::Y => {
                if cx.act(Request::NewCode) {
                    cx.toast("New code");
                }
                Nav::Stay
            }
            Button::B => {
                // leaving the screen ends a code nobody can see (the daemon
                // keeps one on offer while there are no keys)
                cx.act(Request::EndPairing);
                Nav::Pop
            }
            _ => Nav::Stay,
        }
    }

    fn tick(&mut self, cx: &mut Cx) -> Nav {
        // a key arrived (mortar pair, or ssh-copy-id with the code)
        if cx.st().keys.iter().any(|k| !self.keys_at_start.contains(&k.fingerprint)) {
            return Nav::Replace(Box::new(Ready::default()));
        }
        Nav::Stay
    }
}

// ----------------------------------------------------------- 4 Wi-Fi ----
//
// The plan's page 4 (temporary hotspot + phone) can't exist: the XR829
// has no AP mode. Instead: pick a network here (password on page 5), or
// use the router's WPS button (X), which iwd supports in station mode.

pub struct WifiList {
    list: ListState,
}

impl WifiList {
    pub fn new(cx: &mut Cx) -> WifiList {
        let _ = cx.dev.act(Request::WifiScan);
        WifiList { list: ListState::default() }
    }
}

fn net_row_desc(n: &NetView) -> String {
    let sec = match n.security.as_str() {
        "open" => "Open",
        "psk" => "Password",
        "8021x" => "Enterprise (not supported)",
        "wep" => "WEP (not supported)",
        s => s,
    };
    if n.connected {
        format!("Connected · {sec}")
    } else if n.known {
        format!("Saved · {sec}")
    } else {
        sec.to_string()
    }
}

impl Page for WifiList {
    fn draw(&mut self, ui: &mut Ui, cx: &Cx) {
        let st = cx.st();
        let body = cx.frame(ui, false, &[hint("X", "WPS button"), hint("Y", "Scan"), hint("B", "Back"), hint("A", "Join")]);
        let mut y = body.y + ui.cq(1.0);
        if cx.mode == Mode::Settings {
            y += toolkit::crumb(ui, body.x, y, "Settings /");
        }
        y += toolkit::title_h2(ui, body.x, y, "Choose a Wi-Fi network");
        y += ui.cq(0.4);
        let subst = sub(ui, 2.2);
        let mut line = "2.4 GHz networks. No password typing? Press X, then your router's WPS button.".to_string();
        if st.wifi.scanning {
            line = "Looking for networks…".into();
        }
        ui.text_fit(body.x, y, &line, &subst, body.w);
        y += subst.line_px() + ui.cq(1.6);
        if !st.wifi.available {
            note_line(ui, body.x, y, body.w, "Wi-Fi is not available right now (iwd is not running).", PillKind::Orange);
            return;
        }
        if let Some(e) = &st.wifi.last_error {
            y += note_line(ui, body.x, y, body.w, e, PillKind::Orange) + ui.cq(1.0);
        }
        let nets = &st.wifi.networks;
        if nets.is_empty() {
            note_line(ui, body.x, y, body.w, "No networks found yet.", PillKind::Amber);
            return;
        }
        let rows: Vec<Row> = nets
            .iter()
            .enumerate()
            .map(|(i, n)| Row { title: n.ssid.clone(), desc: Some(net_row_desc(n)), value: RowValue::None, selected: i == self.list.sel })
            .collect();
        let area = Rect::new(body.x, y, body.w, body.bottom() - y - ui.cq(1.0));
        toolkit::rows(ui, area, &rows, &mut self.list);
        // signal bars on the visible rows
        let rh = toolkit::row_h(ui, true);
        let mut ry = y;
        for (i, n) in nets.iter().enumerate().skip(self.list.scroll) {
            if ry + rh > area.bottom() + 0.5 {
                break;
            }
            let sel = i == self.list.sel;
            let ink = if sel { t().lime_ink } else { t().line };
            let faint = if sel { t().lime_ink.with_alpha(0.25) } else { t().line_faint };
            let right = body.right() - ui.cq(2.4);
            let w = signal(ui, right, ry + rh / 2.0, n.bars, ink, faint);
            if n.security != "open" {
                let s = ui.cq(2.6);
                ui.icon(Icon::Lock, right - w - ui.cq(1.6) - s, ry + rh / 2.0 - s / 2.0, s, ink, 1.6, true);
            }
            ry += rh;
        }
    }

    fn button(&mut self, b: Button, cx: &mut Cx) -> Nav {
        let nets = cx.st().wifi.networks.clone();
        match b {
            Button::Up => self.list.up(nets.len()),
            Button::Down => self.list.down(nets.len()),
            Button::Y => {
                cx.act(Request::WifiScan);
            }
            Button::X => {
                if cx.act(Request::WifiWps) {
                    return Nav::Push(Box::new(WifiJoin::new("", true)));
                }
            }
            Button::B => return Nav::Pop,
            Button::A | Button::Start => {
                let Some(n) = nets.get(self.list.sel) else { return Nav::Stay };
                if n.connected {
                    return Nav::Push(Box::new(WifiJoin::new(&n.ssid, false)));
                }
                match n.security.as_str() {
                    "psk" if !n.known => return Nav::Push(Box::new(PasswordPage::new(&n.ssid))),
                    "open" | "psk" => {
                        if cx.act(Request::WifiConnect { ssid: n.ssid.clone(), passphrase: None }) {
                            return Nav::Push(Box::new(WifiJoin::new(&n.ssid, false)));
                        }
                    }
                    _ => cx.toast("This kind of network is not supported here."),
                }
            }
            _ => {}
        }
        Nav::Stay
    }
}

// -------------------------------------------------------- Wi-Fi join ----

pub struct WifiJoin {
    ssid: String,
    wps: bool,
    since: Instant,
    done: bool,
}

impl WifiJoin {
    pub fn new(ssid: &str, wps: bool) -> WifiJoin {
        WifiJoin { ssid: ssid.into(), wps, since: Instant::now(), done: false }
    }
}

impl Page for WifiJoin {
    fn live(&self) -> bool {
        true
    }

    fn draw(&mut self, ui: &mut Ui, cx: &Cx) {
        let st = cx.st();
        let failed = st.wifi.last_error.clone();
        let hints = if failed.is_some() { vec![hint("B", "Back"), hint("A", "Try again")] } else { vec![hint("B", "Cancel")] };
        let body = cx.frame(ui, false, &hints);
        let mut y = body.y + ui.cq(1.0);
        let title = if self.wps {
            "Press your router's WPS button".to_string()
        } else {
            format!("Joining {}", self.ssid)
        };
        y += toolkit::title_h2(ui, body.x, y, &title) + ui.cq(1.0);
        let s = sub(ui, 2.6).line_height(1.4);
        let msg = if let Some(e) = &failed {
            e.clone()
        } else if self.wps {
            "The Brick is listening for a router in WPS mode. Most routers have a button marked WPS: press it now. This works for two minutes.".into()
        } else {
            let n = self.since.elapsed().as_secs() as usize % 4;
            format!("Connecting{}", ".".repeat(n))
        };
        toolkit::paragraph(ui, body.x, y, ui.cq(60.0), &msg, &s);
        toolkit::iso_blocks(ui, body.x + body.w / 2.0, body.y + ui.cq(36.0), ui.cq(22.0));
    }

    fn button(&mut self, b: Button, cx: &mut Cx) -> Nav {
        match b {
            Button::B => Nav::Pop,
            Button::A if cx.st().wifi.last_error.is_some() => {
                if self.wps {
                    cx.act(Request::WifiWps);
                } else if cx.st().wifi.networks.iter().any(|n| n.ssid == self.ssid && n.known) {
                    cx.act(Request::WifiConnect { ssid: self.ssid.clone(), passphrase: None });
                } else {
                    return Nav::Replace(Box::new(PasswordPage::new(&self.ssid)));
                }
                self.since = Instant::now();
                Nav::Stay
            }
            _ => Nav::Stay,
        }
    }

    fn tick(&mut self, cx: &mut Cx) -> Nav {
        let w = &cx.st().wifi;
        let ours = self.wps || w.ssid.as_deref() == Some(self.ssid.as_str());
        if !self.done && w.state == "connected" && ours && w.last_error.is_none() && !w.wps {
            self.done = true;
            if cx.mode == Mode::Settings {
                cx.toast(format!("Connected to {}", w.ssid.clone().unwrap_or_default()));
                return Nav::Pop;
            }
            return Nav::Replace(Box::new(Ready::default()));
        }
        Nav::Stay
    }
}

// ----------------------------------------------------------- 6 ready ----

#[derive(Default)]
pub struct Ready;

impl Page for Ready {
    fn draw(&mut self, ui: &mut Ui, cx: &Cx) {
        let st = cx.st();
        let has_key = !st.keys.is_empty();
        let mut hints = vec![hint("A", "Done")];
        if !has_key {
            hints.insert(0, hint("X", "Pair over USB"));
        }
        let body = cx.frame(ui, has_key, &hints);
        let mut y = body.y + ui.cq(1.0);
        toolkit::iso_blocks(ui, body.right() - ui.cq(11.0), body.y + ui.cq(8.0), ui.cq(15.0));
        let title = if has_key { "SSH is ready" } else if st.wifi.state == "connected" { "Wi-Fi is connected" } else { "Almost there" };
        y += toolkit::title_h1(ui, body.x, y, title) + ui.cq(0.6);
        let s = sub(ui, 2.5).line_height(1.4);
        let text = if let Some(k) = st.last_added.as_ref().or(st.keys.first()) {
            let added = if st.last_added.is_some() { "Key added" } else { "Key" };
            let pw = if st.password_login { "" } else { " Password login is off." };
            format!("{added}: {}.{pw}", key_label(k))
        } else {
            "No SSH key yet. Connect the USB cable and run mortar pair, or put your public key in ssh-keys/ on the card.".into()
        };
        y += toolkit::paragraph(ui, body.x, y, ui.cq(66.0), &text, &s) + ui.cq(3.0);

        if has_key {
            y += toolkit::label(ui, body.x, y, "Connect from your computer") + ui.cq(0.8);
            // `mortar pair` adds "Host brick" to ~/.ssh/config; keys from
            // the card don't, so show the plain form then
            let plain = format!("ssh root@{}", host_local(cx));
            if st.last_added.is_some() {
                y += command(ui, body.x, y, ui.cq(52.0), "ssh brick") + ui.cq(0.8);
                ui.text(body.x, y, &format!("or {plain}"), &toolkit::st_mono(ui, 2.0, t().line_soft));
                y += ui.cq(4.6);
            } else {
                y += command(ui, body.x, y, ui.cq(52.0), &plain) + ui.cq(2.6);
            }
        }
        let wifi = match (&st.wifi.state[..], &st.wifi.address) {
            ("connected", Some(a)) => format!("{} · {a}", host_local(cx)),
            ("connected", None) => host_local(cx),
            _ => "Not connected".into(),
        };
        let usb = if st.usb_network { st.usb_address.clone() } else { "Off".into() };
        for (k, v) in [("Wi-Fi", wifi), ("USB", usb)] {
            y += toolkit::row(ui, body.x, y, body.w, &Row::new(k, RowValue::Text(v)));
        }
    }

    fn button(&mut self, b: Button, cx: &mut Cx) -> Nav {
        match b {
            Button::X if cx.st().keys.is_empty() => Nav::Push(Box::new(UsbPair::new(cx))),
            Button::A | Button::Start | Button::B => {
                if cx.mode == Mode::FirstBoot {
                    cx.act(Request::Finish);
                    Nav::Exit
                } else {
                    Nav::Pop
                }
            }
            _ => Nav::Stay,
        }
    }
}

// --------------------------------------------- 8 settings: remote access ----

#[derive(Clone, Copy, PartialEq)]
enum Item {
    Ssh,
    Password,
    Usb,
    Keys,
    Pair,
    HostKey,
}

const ITEMS: [Item; 6] = [Item::Ssh, Item::Password, Item::Usb, Item::Keys, Item::Pair, Item::HostKey];

#[derive(Default)]
pub struct RemoteAccess {
    list: ListState,
    confirm: Confirm,
}

impl Page for RemoteAccess {
    fn draw(&mut self, ui: &mut Ui, cx: &Cx) {
        let st = cx.st();
        let sel = ITEMS[self.list.sel.min(ITEMS.len() - 1)];
        let act = match sel {
            Item::Ssh | Item::Password | Item::Usb => "Toggle",
            Item::HostKey => "Show",
            _ => "Open",
        };
        let body = cx.frame(ui, false, &[hint("B", "Back"), hint("A", act)]);
        let mut y = body.y + ui.cq(1.0);
        y += toolkit::crumb(ui, body.x, y, "Settings /");
        y += toolkit::title_h2(ui, body.x, y, "Remote access") + ui.cq(1.2);
        let nkeys = st.keys.len();
        let rows: Vec<Row> = ITEMS
            .iter()
            .enumerate()
            .map(|(i, it)| {
                let mut r = match it {
                    Item::Ssh => Row::new("SSH server", RowValue::Toggle(st.ssh_server)),
                    Item::Password => {
                        let mut r = Row::new("Password login", RowValue::Toggle(st.password_login));
                        r.desc = Some(if st.password_login {
                            "The pairing code is root's password over USB".into()
                        } else {
                            "Turning this on needs a button press here".into()
                        });
                        r
                    }
                    Item::Usb => {
                        let mut r = Row::new("USB network", RowValue::Toggle(st.usb_network));
                        r.desc = Some(format!("{} on the cable", st.usb_address));
                        r
                    }
                    Item::Keys => Row::new(
                        "Authorized keys",
                        RowValue::Text(format!("{nkeys} key{} ›", if nkeys == 1 { "" } else { "s" })),
                    ),
                    Item::Pair => Row::new("Pair a new computer", RowValue::Text("Show code ›".into())),
                    Item::HostKey => Row::new(
                        "Host key fingerprint",
                        RowValue::Text(st.host_key_fingerprint.as_deref().map(short_fp).unwrap_or_else(|| "none yet".into())),
                    ),
                };
                r.selected = i == self.list.sel;
                r
            })
            .collect();
        toolkit::rows(ui, Rect::new(body.x, y, body.w, body.bottom() - y), &rows, &mut self.list);
    }

    fn button(&mut self, b: Button, cx: &mut Cx) -> Nav {
        let st = cx.st().clone();
        match b {
            Button::Up => self.list.up(ITEMS.len()),
            Button::Down => self.list.down(ITEMS.len()),
            Button::B => return Nav::Pop,
            Button::A | Button::Start => match ITEMS[self.list.sel.min(ITEMS.len() - 1)] {
                Item::Ssh => {
                    if !st.ssh_server || self.confirm.ok("ssh", cx, "turn the SSH server off") {
                        cx.act(Request::SetSshServer { on: !st.ssh_server });
                    }
                }
                Item::Password => {
                    cx.act(Request::SetPasswordLogin { on: !st.password_login });
                }
                Item::Usb => {
                    if !st.usb_network || self.confirm.ok("usb", cx, "turn the USB network off") {
                        cx.act(Request::SetUsbNetwork { on: !st.usb_network });
                    }
                }
                Item::Keys => return Nav::Push(Box::new(KeysList::default())),
                Item::Pair => return Nav::Push(Box::new(UsbPair::new(cx))),
                Item::HostKey => {
                    if let Some(fp) = &st.host_key_fingerprint {
                        cx.toast(fp.clone());
                    }
                }
            },
            _ => {}
        }
        Nav::Stay
    }
}

// ------------------------------------------------------------ key list ----

#[derive(Default)]
pub struct KeysList {
    list: ListState,
    confirm: Confirm,
}

impl Page for KeysList {
    fn draw(&mut self, ui: &mut Ui, cx: &Cx) {
        let keys = &cx.st().keys;
        let mut hints = vec![hint("B", "Back")];
        if !keys.is_empty() {
            hints.push(hint("X", "Remove"));
        }
        let body = cx.frame(ui, false, &hints);
        let mut y = body.y + ui.cq(1.0);
        y += toolkit::crumb(ui, body.x, y, "Settings / Remote access /");
        y += toolkit::title_h2(ui, body.x, y, "Authorized keys") + ui.cq(1.2);
        if keys.is_empty() {
            note_line(ui, body.x, y, body.w, "No keys yet. Pair a computer to add one.", PillKind::Amber);
            return;
        }
        let rows: Vec<Row> = keys
            .iter()
            .enumerate()
            .map(|(i, k)| {
                let title = if k.comment.is_empty() { "(no comment)".to_string() } else { k.comment.clone() };
                let place = if k.source == "home" { "~/.ssh" } else { "on the card" };
                Row {
                    title,
                    desc: Some(format!("{} · {} · {place}", k.kind, short_fp(&k.fingerprint))),
                    value: RowValue::None,
                    selected: i == self.list.sel,
                }
            })
            .collect();
        toolkit::rows(ui, Rect::new(body.x, y, body.w, body.bottom() - y), &rows, &mut self.list);
    }

    fn button(&mut self, b: Button, cx: &mut Cx) -> Nav {
        let keys = cx.st().keys.clone();
        match b {
            Button::Up => self.list.up(keys.len()),
            Button::Down => self.list.down(keys.len()),
            Button::B => return Nav::Pop,
            Button::X => {
                if let Some(k) = keys.get(self.list.sel) {
                    let last = keys.len() == 1;
                    let what = if last { "remove the last key (SSH stops working)" } else { "remove this key" };
                    if self.confirm.ok(&k.fingerprint, cx, what) && cx.act(Request::RemoveKey { fingerprint: k.fingerprint.clone() }) {
                        cx.toast(format!("Removed {}", key_label(k)));
                        self.list.clamp(keys.len() - 1);
                    }
                }
            }
            _ => {}
        }
        Nav::Stay
    }
}
