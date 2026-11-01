// SPDX-License-Identifier: GPL-2.0-or-later
//! `mortar-setup daemon`: the Brick side of pairing and remote access.
//!
//! - Pairing service (mortar-pair/1, TCP `PORT`) bound to usb0 only
//!   (SO_BINDTODEVICE) and accepting peers from the USB subnet only.
//! - Control socket for the screens (`control.rs`).
//! - The pairing code (`pairing.rs`), root's keys (`keys.rs`), Wi-Fi
//!   (`iwd.rs`), and the switches in Settings > Remote access.
//! - sshd: while "password login" is on and a code is showing, a Match
//!   block in /run/mortar-setup/sshd/ lets root log in over USB with the
//!   code as password (checked by `mortar-setup check-password` from PAM).

use crate::apply;
use crate::control::{KeyView, Request, Response, Status};
use crate::iwd::Wifi;
use crate::keys;
use crate::pairing::Pairing;
use crate::state::{Paths, State};
use crate::sys;
use mortar_pair::{read_msg, write_msg, ErrorCode, Msg, ServerSession, PROTOCOL};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Config {
    pub paths: Paths,
    pub port: u16,
    /// Interfaces the pairing service listens on (SO_BINDTODEVICE).
    pub interfaces: Vec<String>,
    /// Listen here instead (tests: 127.0.0.1:0). No device binding.
    pub listen: Option<SocketAddr>,
    /// Peers allowed to pair / use the code as SSH password.
    pub allow: Vec<(Ipv4Addr, u8)>,
    pub usb_iface: String,
    pub usb_address: String,
    pub socket_group: Option<String>,
    /// Command to make sshd re-read its config.
    pub sshd_reload: Vec<String>,
    /// Manage /run/mortar-setup/sshd (code as SSH password over USB).
    pub ssh_password: bool,
    /// Touch the running system (systemctl, networkctl).
    pub system: bool,
    pub iwd: bool,
}

impl Config {
    pub fn test(paths: Paths) -> Config {
        Config {
            paths,
            port: 0,
            interfaces: vec![],
            listen: Some("127.0.0.1:0".parse().unwrap()),
            allow: vec![("127.0.0.0".parse().unwrap(), 8)],
            usb_iface: "usb0".into(),
            usb_address: mortar_pair::USB_ADDR.into(),
            socket_group: None,
            sshd_reload: vec![],
            ssh_password: true,
            system: false,
            iwd: false,
        }
    }
}

struct Inner {
    state: State,
    pairing: Pairing,
    last_added: Option<KeyView>,
    key_fps: Vec<String>,
    sshd_conf: Option<bool>,
}

pub struct Daemon {
    pub cfg: Config,
    inner: Mutex<Inner>,
    wifi: Option<Wifi>,
    conns: AtomicUsize,
}

const MAX_CONNS: usize = 4;

impl Daemon {
    pub fn new(cfg: Config) -> Arc<Daemon> {
        let state = State::load(&cfg.paths.state_file());
        let key_fps = keys::list(&cfg.paths).into_iter().map(|k| k.fingerprint).collect();
        let wifi = cfg.iwd.then(Wifi::start);
        let d = Arc::new(Daemon {
            inner: Mutex::new(Inner { state, pairing: Pairing::default(), last_added: None, key_fps, sshd_conf: None }),
            wifi,
            conns: AtomicUsize::new(0),
            cfg,
        });
        d.tick();
        d
    }

    fn save(&self, inner: &Inner) {
        if let Err(e) = inner.state.save(&self.cfg.paths.state_file()) {
            eprintln!("saving state: {e}");
        }
    }

    // ----------------------------------------------------------- status ----

    pub fn status(&self) -> Status {
        let now = Instant::now();
        let paths = &self.cfg.paths;
        let inner = self.inner.lock().unwrap();
        let host_keys = keys::host_keys(&paths.host_key_dir);
        let summary = fs::read_to_string(paths.summary_file()).ok().and_then(|s| serde_json::from_str(&s).ok());
        let mut ifaces = self.cfg.interfaces.clone();
        if self.cfg.listen.is_some() {
            ifaces.push("test".into());
        }
        Status {
            hostname: sys::hostname(),
            setup_done: inner.state.done,
            setup_file: summary,
            pairing: inner.pairing.view(now),
            pairing_locked: inner.pairing.locked(),
            pairing_interfaces: ifaces,
            host_key_fingerprint: host_keys.first().and_then(|k| mortar_pair::fingerprint_line(k)),
            keys: keys::list(paths),
            last_added: inner.last_added.clone(),
            password_login: inner.state.password_login,
            usb_network: inner.state.usb_network,
            ssh_server: inner.state.ssh_server,
            usb_connected: sys::carrier(&self.cfg.usb_iface),
            usb_address: self.cfg.usb_address.clone(),
            wifi: self.wifi.as_ref().map(|w| w.view()).unwrap_or_default(),
        }
    }

    // ------------------------------------------------------------- tick ----

    /// Housekeeping, every second: notice keys added behind our back,
    /// expire/renew the code, keep sshd's password switch in line.
    pub fn tick(&self) {
        let now = Instant::now();
        let list = keys::list(&self.cfg.paths);
        let mut inner = self.inner.lock().unwrap();
        let new: Vec<&KeyView> = list.iter().filter(|k| !inner.key_fps.contains(&k.fingerprint)).collect();
        if !new.is_empty() {
            inner.last_added = new.last().map(|k| (*k).clone());
            inner.pairing.consumed();
            if inner.state.password_login {
                inner.state.password_login = false;
                self.save(&inner);
            }
        }
        inner.key_fps = list.iter().map(|k| k.fingerprint.clone()).collect();
        inner.pairing.tick(now, !list.is_empty());
        self.sync_sshd(&mut inner, now);
    }

    fn sync_sshd(&self, inner: &mut Inner, now: Instant) {
        if !self.cfg.ssh_password {
            return;
        }
        let want = inner.state.password_login && inner.pairing.active(now).is_some();
        if inner.sshd_conf == Some(want) {
            return;
        }
        let dir = self.cfg.paths.sshd_dir();
        let f = dir.join("password-login.conf");
        let existed = f.exists();
        let r = if want {
            let local = &self.cfg.usb_address;
            fs::create_dir_all(&dir).and_then(|_| {
                fs::write(
                    &f,
                    format!(
                        "# mortar-setup: the pairing code on the Brick's screen is root's\n\
                         # password over USB until a key is added (checked through PAM).\n\
                         Match LocalAddress {local}\n\
                         \tPasswordAuthentication yes\n\
                         \tKbdInteractiveAuthentication yes\n\
                         \tPermitRootLogin yes\n"
                    ),
                )
            })
        } else {
            match fs::remove_file(&f) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        };
        if let Err(e) = r {
            eprintln!("{}: {e}", f.display());
            return;
        }
        inner.sshd_conf = Some(want);
        // sshd reads the include when it (re)loads its config
        if !self.cfg.sshd_reload.is_empty() && existed != want {
            let argv: Vec<&str> = self.cfg.sshd_reload.iter().map(String::as_str).collect();
            sys::spawn(&argv);
        }
    }

    // ---------------------------------------------------------- control ----

    pub fn handle(&self, req: Request) -> Response {
        let now = Instant::now();
        let ok = Response::Ok;
        match req {
            Request::Status => return Response::Status(Box::new(self.status())),
            Request::NewCode => {
                let mut i = self.inner.lock().unwrap();
                i.pairing.new_code(now);
                self.sync_sshd(&mut i, now);
            }
            Request::EndPairing => {
                let mut i = self.inner.lock().unwrap();
                if !i.key_fps.is_empty() {
                    i.pairing.end();
                }
                self.sync_sshd(&mut i, now);
            }
            Request::SetPasswordLogin { on } => {
                let mut i = self.inner.lock().unwrap();
                i.state.password_login = on;
                self.save(&i);
                self.sync_sshd(&mut i, now);
            }
            Request::SetUsbNetwork { on } => {
                let mut i = self.inner.lock().unwrap();
                i.state.usb_network = on;
                self.save(&i);
                apply::set_usb_network(&self.cfg.paths, on, self.cfg.system);
            }
            Request::SetSshServer { on } => {
                let mut i = self.inner.lock().unwrap();
                i.state.ssh_server = on;
                self.save(&i);
                if self.cfg.system {
                    sys::spawn(&["systemctl", if on { "start" } else { "stop" }, "sshd.service"]);
                }
            }
            Request::RemoveKey { fingerprint } => {
                let mut i = self.inner.lock().unwrap();
                match keys::remove(&self.cfg.paths, &fingerprint) {
                    Ok(0) => return Response::Error { message: "no such key".into() },
                    Ok(_) => {
                        i.key_fps.retain(|f| f != &fingerprint);
                        if i.last_added.as_ref().is_some_and(|k| k.fingerprint == fingerprint) {
                            i.last_added = None;
                        }
                    }
                    Err(e) => return Response::Error { message: e.to_string() },
                }
            }
            Request::WifiScan => match &self.wifi {
                Some(w) => w.scan(),
                None => return Response::Error { message: "Wi-Fi is not available".into() },
            },
            Request::WifiConnect { ssid, passphrase } => match &self.wifi {
                Some(w) => w.connect(ssid, passphrase),
                None => return Response::Error { message: "Wi-Fi is not available".into() },
            },
            Request::WifiWps => match &self.wifi {
                Some(w) => w.wps(),
                None => return Response::Error { message: "Wi-Fi is not available".into() },
            },
            Request::Finish => {
                let mut i = self.inner.lock().unwrap();
                i.state.done = true;
                self.save(&i);
            }
            Request::CheckPassword { user, rhost, password } => {
                return Response::Allowed { allowed: self.check_password(&user, &rhost, &password) };
            }
        }
        ok
    }

    fn allowed_peer(&self, ip: IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => self.cfg.allow.iter().any(|(n, p)| sys::in_subnet(v4, *n, *p)),
            IpAddr::V6(v6) => v6.to_ipv4_mapped().is_some_and(|v4| self.allowed_peer(IpAddr::V4(v4))),
        }
    }

    pub fn check_password(&self, user: &str, rhost: &str, password: &str) -> bool {
        let now = Instant::now();
        if user != "root" || !rhost.parse::<IpAddr>().is_ok_and(|ip| self.allowed_peer(ip)) {
            return false;
        }
        let mut i = self.inner.lock().unwrap();
        if !i.state.password_login {
            return false;
        }
        match i.pairing.active(now) {
            Some(code) if code.matches(password) => true,
            Some(_) => {
                // a wrong password costs a try, like a wrong pairing code
                let _ = i.pairing.begin_try(now);
                false
            }
            None => false,
        }
    }

    pub fn serve_control(self: &Arc<Self>) -> std::io::Result<()> {
        let path = self.cfg.paths.socket();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let _ = fs::remove_file(&path);
        let l = UnixListener::bind(&path)?;
        set_socket_perms(&path, self.cfg.socket_group.as_deref());
        let me = self.clone();
        thread::Builder::new().name("control".into()).spawn(move || {
            for s in l.incoming().flatten() {
                let me = me.clone();
                thread::spawn(move || me.control_conn(s));
            }
        })?;
        Ok(())
    }

    fn control_conn(&self, s: UnixStream) {
        s.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let mut w = match s.try_clone() {
            Ok(w) => w,
            Err(_) => return,
        };
        let mut line = String::new();
        if BufReader::new(std::io::Read::take(s, 64 * 1024)).read_line(&mut line).is_err() {
            return;
        }
        let resp = match serde_json::from_str::<Request>(&line) {
            Ok(req) => self.handle(req),
            Err(e) => Response::Error { message: format!("bad request: {e}") },
        };
        let mut out = serde_json::to_string(&resp).unwrap_or_default();
        out.push('\n');
        let _ = w.write_all(out.as_bytes());
    }

    // ---------------------------------------------------------- pairing ----

    /// Bind the pairing listeners; returns their addresses.
    pub fn serve_pairing(self: &Arc<Self>) -> std::io::Result<Vec<SocketAddr>> {
        let mut out = Vec::new();
        if let Some(a) = self.cfg.listen {
            let l = TcpListener::bind(a)?;
            out.push(l.local_addr()?);
            self.spawn_accept(l);
            return Ok(out);
        }
        for iface in self.cfg.interfaces.clone() {
            let me = self.clone();
            let port = self.cfg.port;
            // the interface may not exist yet (gadget still binding): retry
            thread::Builder::new().name(format!("pair-{iface}")).spawn(move || loop {
                match listen_on_device(&iface, port) {
                    Ok(l) => {
                        eprintln!("pairing: listening on {iface} port {port}");
                        me.accept_loop(l);
                        return;
                    }
                    Err(e) => {
                        eprintln!("pairing: {iface}: {e}; retrying");
                        thread::sleep(Duration::from_secs(3));
                    }
                }
            })?;
        }
        Ok(out)
    }

    fn spawn_accept(self: &Arc<Self>, l: TcpListener) {
        let me = self.clone();
        thread::spawn(move || me.accept_loop(l));
    }

    fn accept_loop(self: Arc<Self>, l: TcpListener) {
        for s in l.incoming().flatten() {
            if self.conns.fetch_add(1, Ordering::SeqCst) >= MAX_CONNS {
                self.conns.fetch_sub(1, Ordering::SeqCst);
                let mut s = s;
                let _ = write_msg(&mut s, &err(ErrorCode::Busy, "busy", 0));
                continue;
            }
            let me = self.clone();
            thread::spawn(move || {
                // a closed connection after `hello` is normal
                if let Err(e) = me.pair_conn(s)
                    && e.kind() != std::io::ErrorKind::UnexpectedEof
                {
                    eprintln!("pairing: {e}");
                }
                me.conns.fetch_sub(1, Ordering::SeqCst);
            });
        }
    }

    fn pair_conn(&self, s: TcpStream) -> std::io::Result<()> {
        s.set_read_timeout(Some(Duration::from_secs(30)))?;
        s.set_write_timeout(Some(Duration::from_secs(30)))?;
        let peer = s.peer_addr()?;
        let mut w = s.try_clone()?;
        let mut r = BufReader::new(s);
        if !self.allowed_peer(peer.ip()) {
            return write_msg(&mut w, &err(ErrorCode::Forbidden, "Pairing works over the USB cable only.", 0));
        }
        loop {
            let m = read_msg(&mut r)?;
            match m {
                Msg::Hello { .. } => {
                    let now = Instant::now();
                    let i = self.inner.lock().unwrap();
                    let pairing = i.pairing.active(now).is_some();
                    let reason = if i.pairing.locked() {
                        Some("Too many wrong codes. Press Y on the Brick for a new one.".into())
                    } else if !pairing {
                        Some("The Brick is not showing a pairing code. On the Brick: Settings > Remote access > Pair a new computer.".into())
                    } else {
                        None
                    };
                    write_msg(
                        &mut w,
                        &Msg::Info { protocol: PROTOCOL.into(), hostname: sys::hostname(), pairing, tries_left: i.pairing.tries_left(), reason },
                    )?;
                }
                Msg::Start { protocol, spake } => {
                    if protocol != PROTOCOL {
                        return write_msg(&mut w, &err(ErrorCode::Protocol, "Please update mortar on your computer.", 0));
                    }
                    return self.pair_session(&mut r, &mut w, &spake, peer);
                }
                _ => return write_msg(&mut w, &err(ErrorCode::Protocol, "unexpected message", 0)),
            }
        }
    }

    fn pair_session(&self, r: &mut BufReader<TcpStream>, w: &mut TcpStream, spake: &str, peer: SocketAddr) -> std::io::Result<()> {
        let now = Instant::now();
        let host_keys = keys::host_keys(&self.cfg.paths.host_key_dir);
        if host_keys.is_empty() {
            return write_msg(w, &err(ErrorCode::Internal, "The Brick has no SSH host key yet; try again in a minute.", 0));
        }
        let code = {
            let mut i = self.inner.lock().unwrap();
            match i.pairing.begin_try(now) {
                Ok(c) => c,
                Err((code, msg)) => {
                    let left = i.pairing.tries_left();
                    return write_msg(w, &err(code, msg, left));
                }
            }
        };
        let hostname = sys::hostname();
        let (session, challenge) = match ServerSession::respond(&code, spake, &hostname, &host_keys) {
            Ok(x) => x,
            Err(e) => return write_msg(w, &err(ErrorCode::Protocol, &e, 0)),
        };
        write_msg(w, &challenge)?;
        let m = read_msg(r)?;
        let key = match session.check_key(&m) {
            Err(()) => {
                let left = self.inner.lock().unwrap().pairing.tries_left();
                eprintln!("pairing: wrong code from {peer} ({left} tries left)");
                return write_msg(w, &err(ErrorCode::WrongCode, "That code does not match the one on the Brick.", left));
            }
            Ok(Err(e)) => return write_msg(w, &err(ErrorCode::BadKey, &e, 0)),
            Ok(Ok(k)) => k,
        };
        let mut i = self.inner.lock().unwrap();
        if let Err(e) = keys::add(&self.cfg.paths, &key) {
            return write_msg(w, &err(ErrorCode::Internal, &format!("could not store the key: {e}"), 0));
        }
        eprintln!("pairing: added {} {} from {peer}", key.short_kind(), key.fingerprint());
        let view = keys::view(&key, "card");
        i.last_added = Some(view.clone());
        if !i.key_fps.contains(&view.fingerprint) {
            i.key_fps.push(view.fingerprint.clone());
        }
        i.pairing.succeeded();
        // "Once a key exists, password login turns off."
        if i.state.password_login {
            i.state.password_login = false;
            self.save(&i);
        }
        self.sync_sshd(&mut i, now);
        let mut addrs = vec![self.cfg.usb_address.clone()];
        addrs.extend(sys::addr_of("wlan0").map(|a| a.to_string()));
        write_msg(w, &session.done(&key.fingerprint(), i.state.password_login, addrs))
    }

    /// Run forever: listeners plus the once-a-second tick.
    pub fn run(self: Arc<Self>) -> Result<(), String> {
        if self.cfg.system && !self.inner.lock().unwrap().state.ssh_server {
            sys::spawn(&["systemctl", "stop", "sshd.service"]);
        }
        self.serve_control().map_err(|e| format!("control socket: {e}"))?;
        self.serve_pairing().map_err(|e| format!("pairing: {e}"))?;
        loop {
            thread::sleep(Duration::from_secs(1));
            self.tick();
        }
    }
}

fn err(code: ErrorCode, message: &str, tries_left: u32) -> Msg {
    Msg::Error { code, message: message.into(), tries_left }
}

fn set_socket_perms(path: &std::path::Path, group: Option<&str>) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o660));
    if let Some(g) = group {
        let Ok(cg) = std::ffi::CString::new(g) else { return };
        // SAFETY: getgrnam returns a pointer into static storage or null.
        let gr = unsafe { libc::getgrnam(cg.as_ptr()) };
        if gr.is_null() {
            eprintln!("group {g} does not exist; control socket stays root-only");
            return;
        }
        let gid = unsafe { (*gr).gr_gid };
        let Ok(cp) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else { return };
        // SAFETY: valid C string path.
        if unsafe { libc::chown(cp.as_ptr(), u32::MAX as libc::uid_t, gid) } != 0 {
            eprintln!("chown {}: {}", path.display(), std::io::Error::last_os_error());
        }
    }
}

/// A listening TCP socket that only sees traffic from one interface.
#[cfg(target_os = "linux")]
fn listen_on_device(iface: &str, port: u16) -> std::io::Result<TcpListener> {
    use std::os::fd::FromRawFd;
    // SAFETY: plain socket syscalls on a fresh fd we own; on error it is
    // closed before returning.
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let fail = |fd| {
            let e = std::io::Error::last_os_error();
            libc::close(fd);
            Err(e)
        };
        let one: libc::c_int = 1;
        libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, &one as *const _ as *const libc::c_void, 4);
        let name = iface.as_bytes();
        if libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_BINDTODEVICE, name.as_ptr() as *const libc::c_void, name.len() as u32) != 0 {
            return fail(fd);
        }
        let mut sa: libc::sockaddr_in = std::mem::zeroed();
        sa.sin_family = libc::AF_INET as libc::sa_family_t;
        sa.sin_port = port.to_be();
        sa.sin_addr.s_addr = 0;
        if libc::bind(fd, &sa as *const _ as *const libc::sockaddr, std::mem::size_of::<libc::sockaddr_in>() as u32) != 0 {
            return fail(fd);
        }
        if libc::listen(fd, 8) != 0 {
            return fail(fd);
        }
        Ok(TcpListener::from_raw_fd(fd))
    }
}

#[cfg(not(target_os = "linux"))]
fn listen_on_device(_iface: &str, _port: u16) -> std::io::Result<TcpListener> {
    Err(std::io::Error::other("binding to an interface needs Linux"))
}
