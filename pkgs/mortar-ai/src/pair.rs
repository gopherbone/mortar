// SPDX-License-Identifier: GPL-2.0-or-later
//! `mortar-ai pair --url LOGIN_URL`: get a sign-in done from a phone.
//!
//! `claude auth login` on a machine without a browser prints a long OAuth
//! link, and after signing in claude.ai shows a long code to paste back.
//! Neither is fun on an on-screen keyboard. This screen shows a QR code of a
//! one-time page served by the Brick on the local network: the page links to
//! the sign-in URL and has a field to paste the code into. The code is
//! printed on stdout and the program exits; the caller (the login script)
//! types it into the waiting `claude auth login` with `tmux send-keys`.
//!
//! X switches the QR to the sign-in link itself (phone not on the same
//! network: then type the code with START). B closes without a code.
//!
//! The page is only served while this screen is open, under a random
//! 128-bit path, accepts one code, and only codes made of URL-safe
//! characters, so it can't be used to type anything else.

use crate::keys;
use crate::screen::Screen;
use mortar_ui::gfx::{Face, Rect, TextStyle, Ui};
use mortar_ui::theme;
use mortar_ui::toolkit::{self, PillKind, hint};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

pub enum Event {
    PageOpened,
    Code(String),
}

pub struct Pair {
    login_url: String,
    /// The Brick's page, if there is a network address to serve it on.
    page: Option<String>,
    other_addrs: Vec<String>,
    direct: bool,
    phone_seen: bool,
    pub code: Option<String>,
    sent_at: Option<Instant>,
    cancelled: bool,
    rx: Option<Receiver<Event>>,
    wake_r: Option<OwnedFd>,
    qr_cache: Option<(String, usize, Vec<bool>)>,
}

/// IPv4 addresses of up interfaces, Wi-Fi first, loopback skipped.
pub fn local_addrs() -> Vec<(String, String)> {
    let mut out = Vec::new();
    unsafe {
        let mut ifa: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifa) != 0 {
            return out;
        }
        let mut p = ifa;
        while !p.is_null() {
            let a = &*p;
            if !a.ifa_addr.is_null()
                && (*a.ifa_addr).sa_family as i32 == libc::AF_INET
                && a.ifa_flags & libc::IFF_UP as u32 != 0
                && a.ifa_flags & libc::IFF_LOOPBACK as u32 == 0
            {
                let sin = &*(a.ifa_addr as *const libc::sockaddr_in);
                let ip = std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                let name = std::ffi::CStr::from_ptr(a.ifa_name).to_string_lossy().into_owned();
                out.push((name, ip.to_string()));
            }
            p = a.ifa_next;
        }
        libc::freeifaddrs(ifa);
    }
    out.sort_by_key(|(n, _)| if n.starts_with("wl") { 0 } else if n.starts_with("usb") { 2 } else { 1 });
    out
}

fn token() -> String {
    let mut b = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut b);
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;")
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let h = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                match h {
                    Some(v) => {
                        out.push(v);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A pasted code: trimmed, and only characters an OAuth code#state uses.
pub fn clean_code(raw: &str) -> Option<String> {
    let c = raw.trim();
    let ok = (8..=512).contains(&c.len()) && c.chars().all(|ch| ch.is_ascii_alphanumeric() || "#-_.~".contains(ch));
    ok.then(|| c.to_string())
}

fn page_html(login_url: &str, msg: Option<&str>) -> String {
    let body = match msg {
        Some(m) => format!("<p class=done>{}</p>", html_escape(m)),
        None => format!(
            r#"<ol>
<li><a class=btn href="{}" target="_blank" rel="noreferrer">Sign in to Claude</a></li>
<li>Copy the code Claude shows after you sign in, paste it here and send it:
<form method=post><input name=code autocomplete=off autocapitalize=off spellcheck=false placeholder="Paste the code"><button>Send to the Brick</button></form></li>
</ol>"#,
            html_escape(login_url)
        ),
    };
    format!(
        r#"<!doctype html><meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1"><title>Mortar sign-in</title>
<style>body{{font:17px system-ui,sans-serif;background:#e1ebf7;color:#1d1d1d;margin:0;padding:20px;max-width:520px}}
h1{{font-weight:600;font-size:24px}}li{{margin:0 0 22px}}.btn,button{{display:inline-block;background:#cef058;color:#1b322d;border:1px solid #1d1d1d;padding:12px 16px;font:inherit;font-weight:600;text-decoration:none}}
input{{display:block;width:100%;box-sizing:border-box;margin:10px 0;padding:12px;font:15px ui-monospace,monospace;border:1px solid #1d1d1d;background:#fff}}.done{{background:#cef058;border:1px solid #1d1d1d;padding:14px}}</style>
<h1>Sign in to Claude on your Brick</h1>{body}"#
    )
}

fn respond(s: &mut TcpStream, status: &str, body: &str) {
    let _ = write!(
        s,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// One request: GET the page, POST the code. Returns what happened.
fn handle(mut s: TcpStream, path: &str, login_url: &str) -> Option<Event> {
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let mut buf = Vec::new();
    let mut tmp = [0u8; 2048];
    let header_end = loop {
        let n = s.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > 16384 {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut first = head.lines().next().unwrap_or("").split(' ');
    let (method, target) = (first.next().unwrap_or(""), first.next().unwrap_or(""));
    if target != path {
        respond(&mut s, "404 Not Found", "not here");
        return None;
    }
    match method {
        "GET" => {
            respond(&mut s, "200 OK", &page_html(login_url, None));
            Some(Event::PageOpened)
        }
        "POST" => {
            let len = head
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                .unwrap_or(0)
                .min(8192);
            while buf.len() < header_end + len {
                let n = s.read(&mut tmp).ok()?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            let body = String::from_utf8_lossy(&buf[header_end..buf.len().min(header_end + len)]).into_owned();
            let raw = body.split('&').find_map(|kv| kv.strip_prefix("code=")).map(url_decode).unwrap_or_default();
            match clean_code(&raw) {
                Some(code) => {
                    respond(&mut s, "200 OK", &page_html(login_url, Some("Sent. Look at the Brick: Claude Code should say Login successful.")));
                    Some(Event::Code(code))
                }
                None => {
                    respond(&mut s, "400 Bad Request", &page_html(login_url, Some("That doesn't look like a sign-in code. Go back and paste the code exactly as shown.")));
                    None
                }
            }
        }
        _ => {
            respond(&mut s, "405 Method Not Allowed", "no");
            None
        }
    }
}

fn serve(listener: TcpListener, path: String, login_url: String, tx: Sender<Event>, wake: OwnedFd) {
    for conn in listener.incoming() {
        let Ok(s) = conn else { continue };
        if let Some(ev) = handle(s, &path, &login_url) {
            let last = matches!(ev, Event::Code(_));
            let _ = tx.send(ev);
            unsafe {
                libc::write(wake.as_raw_fd(), b"x".as_ptr() as *const libc::c_void, 1);
            }
            if last {
                return; // one code, then the page is gone
            }
        }
    }
}

impl Pair {
    /// Without `serve`, only the direct link is shown (previews, no network).
    pub fn new(login_url: &str, serve_page: bool) -> Pair {
        let mut p = Pair {
            login_url: login_url.to_string(),
            page: None,
            other_addrs: Vec::new(),
            direct: false,
            phone_seen: false,
            code: None,
            sent_at: None,
            cancelled: false,
            rx: None,
            wake_r: None,
            qr_cache: None,
        };
        if !serve_page {
            p.direct = true;
            return p;
        }
        // MORTAR_AI_PAIR_ADDR: serve on this address (tests use 127.0.0.1)
        let addrs = match std::env::var("MORTAR_AI_PAIR_ADDR") {
            Ok(a) if !a.is_empty() => vec![("env".to_string(), a)],
            _ => local_addrs(),
        };
        let Some((_, ip)) = addrs.first().cloned() else {
            p.direct = true;
            return p;
        };
        // MORTAR_AI_PAIR_PORT: fixed port so the firewall can allow it (0 = any)
        let want_port: u16 = std::env::var("MORTAR_AI_PAIR_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
        let Ok(listener) = TcpListener::bind(("0.0.0.0", want_port)) else {
            p.direct = true;
            return p;
        };
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        let path = format!("/{}", token());
        p.page = Some(format!("http://{ip}:{port}{path}"));
        eprintln!("mortar-ai pair: serving {}", p.page.as_deref().unwrap_or(""));
        if let Ok(f) = std::env::var("MORTAR_AI_PAIR_URL_FILE") {
            let _ = std::fs::write(f, p.page.as_deref().unwrap_or(""));
        }
        p.other_addrs = addrs.iter().skip(1).map(|(n, a)| format!("{n} {a}:{port}")).collect();
        let mut fds = [0; 2];
        unsafe {
            libc::pipe(fds.as_mut_ptr());
            libc::fcntl(fds[0], libc::F_SETFL, libc::O_NONBLOCK);
            libc::fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fds[1], libc::F_SETFD, libc::FD_CLOEXEC);
        }
        let (wr, ww) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        let (tx, rx) = channel();
        let url = p.login_url.clone();
        std::thread::spawn(move || serve(listener, path, url, tx, ww));
        p.rx = Some(rx);
        p.wake_r = Some(wr);
        p
    }

    /// For previews: pretend a page is served.
    pub fn demo(login_url: &str, page: &str, phone_seen: bool) -> Pair {
        let mut p = Pair::new(login_url, false);
        p.page = Some(page.to_string());
        p.direct = false;
        p.phone_seen = phone_seen;
        p
    }

    fn qr(&mut self, text: &str) -> (usize, Vec<bool>) {
        if let Some((t, w, m)) = &self.qr_cache
            && t == text
        {
            return (*w, m.clone());
        }
        let code = qrcode::QrCode::with_error_correction_level(text.as_bytes(), qrcode::EcLevel::L)
            .or_else(|_| qrcode::QrCode::new(text.as_bytes()));
        let (w, m) = match code {
            Ok(c) => (c.width(), c.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect()),
            Err(_) => (0, Vec::new()),
        };
        self.qr_cache = Some((text.to_string(), w, m.clone()));
        (w, m)
    }
}

impl Screen for Pair {
    fn title(&self) -> &str {
        "Sign in to Claude"
    }

    fn wake_fd(&self) -> Option<RawFd> {
        self.wake_r.as_ref().map(|f| f.as_raw_fd())
    }

    fn poll(&mut self) -> bool {
        let mut dirty = false;
        if let Some(fd) = &self.wake_r {
            let mut b = [0u8; 64];
            while unsafe { libc::read(fd.as_raw_fd(), b.as_mut_ptr() as *mut libc::c_void, b.len()) } > 0 {}
        }
        if let Some(rx) = &self.rx {
            while let Ok(ev) = rx.try_recv() {
                dirty = true;
                match ev {
                    Event::PageOpened => self.phone_seen = true,
                    Event::Code(c) => {
                        self.code = Some(c);
                        self.sent_at = Some(Instant::now());
                    }
                }
            }
        }
        dirty
    }

    fn key(&mut self, code: u16, down: bool) {
        if !down {
            return;
        }
        match code {
            keys::TAB if self.page.is_some() => self.direct = !self.direct,
            keys::BACKSPACE | keys::ESC => self.cancelled = true,
            _ => {}
        }
    }

    fn reset_keys(&mut self) {}

    fn draw(&mut self, ui: &mut Ui) {
        let t = theme::get();
        let mut hints = Vec::new();
        if self.page.is_some() {
            hints.push(hint("X", if self.direct { "Use the Brick's page" } else { "Link only" }));
        }
        hints.push(hint("B", "Close"));
        let status = toolkit::StatusInfo { left: "Sign in".into(), ..Default::default() };
        let body = toolkit::frame(ui, &status, &hints);

        let target = if self.direct { self.login_url.clone() } else { self.page.clone().unwrap_or_default() };
        let (n, mods) = self.qr(&target);
        // QR on the right: as big as fits, whole pixels per module, 4-module quiet zone
        let avail = body.h.min(body.w * 0.58);
        let px = if n > 0 { ((avail / (n + 8) as f32).floor()).max(1.0) } else { 1.0 };
        let side = px * (n + 8) as f32;
        let qx = body.right() - side;
        let qy = body.y + (body.h - side) / 2.0;
        ui.fill_block(Rect::new(qx, qy, side, side), mortar_ui::gfx::Color::hex(0xffffff));
        ui.outline(Rect::new(qx, qy, side, side), toolkit::hair(ui), t.line);
        for y in 0..n {
            for x in 0..n {
                if mods[y * n + x] {
                    ui.fill_rect(Rect::new(qx + px * (x + 4) as f32, qy + px * (y + 4) as f32, px, px), mortar_ui::gfx::Color::hex(0x000000));
                }
            }
        }

        // steps on the left
        let lx = body.x;
        let lw = qx - lx - ui.cq(4.0);
        let mut y = body.y + ui.cq(1.0);
        y += toolkit::title_h2(ui, lx, y, "Sign in to Claude") + ui.cq(2.0);
        let st = TextStyle::new(Face::Body400, ui.cq(2.3), t.line).line_height(1.35);
        let steps: Vec<String> = if let Some(c) = &self.code {
            vec![format!("Code received ({} characters). Sending it to Claude Code…", c.len())]
        } else if self.direct {
            vec![
                "Scan the code with your phone and sign in.".into(),
                "Claude then shows a code. Close this screen (B) and type that code into the terminal: START opens the keyboard.".into(),
                "Easier with a computer: ssh into the Brick and run `claude auth login` there; it signs in this Brick too.".into(),
            ]
        } else {
            vec![
                "Scan the code with your phone. It must be on the same Wi-Fi as the Brick.".into(),
                "On the page that opens, tap \"Sign in to Claude\" and log in.".into(),
                "Claude shows a code. Copy it, go back to the Brick's page, paste it and tap Send. The Brick types it in for you.".into(),
            ]
        };
        for (i, s) in steps.iter().enumerate() {
            let num = format!("{}", i + 1);
            if steps.len() > 1 {
                toolkit::button_cap(ui, lx, y + st.line_px() / 2.0, &num);
            }
            let ix = if steps.len() > 1 { ui.cq(4.6) } else { 0.0 };
            y += toolkit::paragraph(ui, lx + ix, y, lw - ix, s, &st) + ui.cq(1.6);
        }
        // state tags and the address
        let tag_y = body.bottom() - ui.cq(7.0);
        if self.code.is_some() {
            toolkit::tag(ui, lx + toolkit::tag_w(ui, "CODE RECEIVED"), tag_y, "CODE RECEIVED", PillKind::Ok);
        } else if self.phone_seen && !self.direct {
            toolkit::tag(ui, lx + toolkit::tag_w(ui, "PHONE CONNECTED"), tag_y, "PHONE CONNECTED", PillKind::Ok);
        }
        let mono = TextStyle::new(Face::Mono400, ui.cq(1.7), t.line_soft);
        let addr = if self.direct { "claude.ai sign-in link".to_string() } else { target.split('/').take(3).collect::<Vec<_>>().join("/") };
        ui.text_fit(lx, body.bottom() - ui.cq(4.0), &addr, &mono, lw);
        if !self.direct && !self.other_addrs.is_empty() {
            ui.text_fit(lx, body.bottom() - ui.cq(1.8), &format!("also {}", self.other_addrs.join(", ")), &mono, lw);
        }
    }

    fn done(&self) -> bool {
        self.cancelled || self.sent_at.is_some_and(|t| t.elapsed() > Duration::from_millis(800))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codes() {
        assert_eq!(clean_code("  abcDEF123_-#state-xyz \n").as_deref(), Some("abcDEF123_-#state-xyz"));
        assert_eq!(clean_code("short"), None);
        assert_eq!(clean_code("abcdefgh; rm -rf /"), None);
        assert_eq!(clean_code("abcdefgh\nwhoami"), None);
        assert_eq!(url_decode("a%23b+c%2"), "a#b c%2");
    }
    #[test]
    fn page_escapes_url() {
        let h = page_html("https://x/?a=1&b=\"2\"", None);
        assert!(h.contains("https://x/?a=1&amp;b=&quot;2&quot;"));
    }
}
