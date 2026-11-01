// SPDX-License-Identifier: GPL-2.0-or-later
//! The control socket between the setup daemon (root) and the screens that
//! show its state (the first-boot app, mortar-shell's Settings).
//!
//! One JSON request per line, one JSON response per line, over a Unix
//! socket (`/run/mortar-setup/control.sock`, mode 0660, group
//! `mortar-setup`). Membership in that group is root-equivalent: it can read
//! the pairing code. Only the people holding the Brick (the session user)
//! belong in it.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DEFAULT_SOCKET: &str = "/run/mortar-setup/control.sock";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Status,
    /// Show a fresh pairing code (button press on the Brick).
    NewCode,
    /// Stop accepting the current code.
    EndPairing,
    SetPasswordLogin { on: bool },
    SetUsbNetwork { on: bool },
    SetSshServer { on: bool },
    RemoveKey { fingerprint: String },
    WifiScan,
    WifiConnect { ssid: String, passphrase: Option<String> },
    /// WPS push button: press the router's WPS button within 2 minutes.
    WifiWps,
    /// Mark first-boot setup as finished.
    Finish,
    /// sshd's PAM helper: may `user` log in from `rhost` with `password`?
    CheckPassword { user: String, rhost: String, password: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "r", rename_all = "snake_case")]
pub enum Response {
    Ok,
    Status(Box<Status>),
    Allowed { allowed: bool },
    Error { message: String },
}

/// What mortar.toml contained, as the boot-time apply step saw it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SetupSummary {
    /// The file existed and set anything at all.
    pub has_content: bool,
    pub path: String,
    pub hostname: Option<String>,
    pub wifi_ssid: Option<String>,
    /// The Wi-Fi password was stored and removed from the file.
    pub wifi_password_scrubbed: bool,
    /// Comments (or fingerprints) of the keys it added.
    pub keys: Vec<String>,
    pub password_login: Option<bool>,
    pub usb_network: Option<bool>,
    /// Problems worth showing (bad key lines, TOML errors).
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PairingView {
    pub code: String,
    pub expires_in_s: u64,
    pub tries_left: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct KeyView {
    pub kind: String,
    pub comment: String,
    pub fingerprint: String,
    /// "card" (/boot/ssh-keys/root) or "home" (~root/.ssh/authorized_keys)
    pub source: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NetView {
    pub ssid: String,
    /// 0..=4 bars.
    pub bars: u8,
    /// "open", "psk", "8021x", "wep"
    pub security: String,
    pub known: bool,
    pub connected: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WifiView {
    /// iwd answers and has a station.
    pub available: bool,
    /// "connected", "connecting", "disconnected", "roaming", "off"
    pub state: String,
    pub ssid: Option<String>,
    pub address: Option<String>,
    pub scanning: bool,
    pub networks: Vec<NetView>,
    /// Outcome of the last connect/WPS attempt, for people.
    pub last_error: Option<String>,
    /// A WPS push-button session is running.
    pub wps: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub hostname: String,
    pub setup_done: bool,
    pub setup_file: Option<SetupSummary>,
    /// The code on screen, if pairing is open.
    pub pairing: Option<PairingView>,
    /// Pairing is closed because of too many wrong tries.
    pub pairing_locked: bool,
    /// Interfaces the pairing service accepts connections on.
    pub pairing_interfaces: Vec<String>,
    pub host_key_fingerprint: Option<String>,
    pub keys: Vec<KeyView>,
    /// The key added most recently while the daemon ran.
    pub last_added: Option<KeyView>,
    pub password_login: bool,
    pub usb_network: bool,
    pub ssh_server: bool,
    /// A computer is on the other end of the USB cable.
    pub usb_connected: bool,
    pub usb_address: String,
    pub wifi: WifiView,
}

impl Status {
    pub fn pair_over_wifi(&self) -> bool {
        self.pairing_interfaces.iter().any(|i| i.starts_with("wl"))
    }
}

/// A connection to the daemon (one request per call, reconnecting).
#[derive(Clone, Debug)]
pub struct Client {
    pub path: PathBuf,
}

impl Client {
    pub fn new(path: impl AsRef<Path>) -> Client {
        Client { path: path.as_ref().to_path_buf() }
    }

    pub fn call(&self, req: &Request) -> Result<Response, String> {
        let mut s = UnixStream::connect(&self.path).map_err(|e| format!("setup service: {e}"))?;
        s.set_read_timeout(Some(Duration::from_secs(5))).ok();
        s.set_write_timeout(Some(Duration::from_secs(5))).ok();
        let mut line = serde_json::to_string(req).map_err(|e| e.to_string())?;
        line.push('\n');
        s.write_all(line.as_bytes()).map_err(|e| e.to_string())?;
        let mut r = BufReader::new(s);
        let mut buf = String::new();
        r.read_line(&mut buf).map_err(|e| e.to_string())?;
        serde_json::from_str(&buf).map_err(|e| format!("setup service: {e}"))
    }

    pub fn status(&self) -> Result<Status, String> {
        match self.call(&Request::Status)? {
            Response::Status(s) => Ok(*s),
            Response::Error { message } => Err(message),
            r => Err(format!("unexpected reply {r:?}")),
        }
    }

    /// Send an action; Ok(()) on success.
    pub fn act(&self, req: Request) -> Result<(), String> {
        match self.call(&req)? {
            Response::Ok | Response::Status(_) | Response::Allowed { allowed: true } => Ok(()),
            Response::Allowed { allowed: false } => Err("not allowed".into()),
            Response::Error { message } => Err(message),
        }
    }
}
