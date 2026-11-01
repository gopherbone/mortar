// SPDX-License-Identifier: GPL-2.0-or-later
//! The Mortar pairing protocol (`mortar-pair/1`): put your SSH public key on
//! a Brick using the short code on its screen, and learn the Brick's SSH host
//! key in a way you can trust.
//!
//! Shared by the Brick side (`mortar-setup daemon`, pkgs/mortar-setup) and
//! the computer side (`mortar pair`, pkgs/mortar-cli).
//!
//! # Wire format
//!
//! One TCP connection, newline-delimited JSON messages (`Msg`), at most
//! `MAX_LINE` bytes each. The Brick listens on port `PORT`, only on usb0.
//!
//! ```text
//! C -> S  hello                         (optional: is pairing open?)
//! S -> C  info {hostname, pairing, tries_left, reason}
//! C -> S  start {spake: A}
//! S -> C  challenge {spake: B, hostname, host_keys, confirm}
//! C -> S  add_key {key, mac}
//! S -> C  done {key_fingerprint, password_login, addresses, mac}
//! ```
//!
//! # Why it is safe with a short code
//!
//! The code (e.g. `TIDE-4782`, about 21 bits) is the password of a SPAKE2
//! exchange (Ed25519 group). SPAKE2 gives both ends the same 256-bit key
//! only if they used the same code, and an eavesdropper learns nothing it
//! could test offline. An active attacker gets exactly one guess per
//! connection, which the Brick counts: after `MAX_TRIES` connections that
//! did not end in a correct `add_key`, the code is dead and only a button
//! press on the Brick makes a new one. Codes also expire after `CODE_TTL`
//! and as soon as one key has been added.
//!
//! All later messages are bound to the full transcript (both SPAKE2
//! messages, the hostname and the host keys) through HKDF:
//!
//! - `challenge.confirm` proves to the computer that the Brick showing the
//!   code is the one sending these host keys. That is the check a human
//!   would otherwise do by comparing fingerprints, so `mortar pair` can say
//!   "host key matches the screen" and pin the key in known_hosts.
//! - `add_key.mac` proves the key comes from whoever read the code.
//! - `done.mac` proves the Brick really stored it.

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::io::{self, BufRead, Write};
use std::time::Duration;

mod words;
pub use words::WORDS;

pub const PROTOCOL: &str = "mortar-pair/1";
/// TCP port of the pairing service ("MP" on a phone keypad is 67; 7330 is
/// unassigned and easy to remember next to SSH's 22).
pub const PORT: u16 = 7330;
/// The Brick's fixed address on the USB link.
pub const USB_ADDR: &str = "10.55.0.1";
/// Longest accepted message line.
pub const MAX_LINE: usize = 16 * 1024;
/// Connections per code that may fail before the code is dead.
pub const MAX_TRIES: u32 = 10;
/// How long a code lives.
pub const CODE_TTL: Duration = Duration::from_secs(15 * 60);

const ID_CLIENT: &[u8] = b"mortar-pair/1 computer";
const ID_SERVER: &[u8] = b"mortar-pair/1 brick";

type HmacSha256 = Hmac<Sha256>;

// ------------------------------------------------------------- messages ----

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Msg {
    Hello {
        protocol: String,
    },
    Info {
        protocol: String,
        hostname: String,
        /// A code is on screen and accepting tries.
        pairing: bool,
        tries_left: u32,
        /// Why pairing is closed, for people.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Start {
        protocol: String,
        spake: String,
    },
    Challenge {
        spake: String,
        hostname: String,
        /// OpenSSH public host keys, `type base64`, preferred first.
        host_keys: Vec<String>,
        confirm: String,
    },
    AddKey {
        key: String,
        mac: String,
    },
    Done {
        key_fingerprint: String,
        /// Whether the code still works as root's SSH password over USB.
        password_login: bool,
        addresses: Vec<String>,
        mac: String,
    },
    Error {
        code: ErrorCode,
        message: String,
        #[serde(default)]
        tries_left: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// No code is active (expired, used, or never shown).
    NotPairing,
    /// Too many failed tries; a new code needs a button press on the Brick.
    Locked,
    /// The code did not match (the MAC over the key was wrong).
    WrongCode,
    /// The public key was not acceptable.
    BadKey,
    /// Not allowed from this network.
    Forbidden,
    /// Too many connections too quickly.
    Busy,
    Protocol,
    Internal,
}

pub fn write_msg(w: &mut impl Write, m: &Msg) -> io::Result<()> {
    let mut s = serde_json::to_string(m).map_err(io::Error::other)?;
    s.push('\n');
    w.write_all(s.as_bytes())?;
    w.flush()
}

/// Read one message line (at most `MAX_LINE` bytes).
pub fn read_msg<R: BufRead>(r: &mut R) -> io::Result<Msg> {
    let mut buf = Vec::new();
    let n = io::Read::take(&mut *r, MAX_LINE as u64 + 1).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed"));
    }
    if buf.len() > MAX_LINE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too long"));
    }
    serde_json::from_slice(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

// ----------------------------------------------------------------- code ----

/// A pairing code: one of 256 words and four digits, e.g. `TIDE-4782`
/// (8 + 13.3 bits).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Code(String);

impl Code {
    pub fn random() -> Code {
        let mut b = [0u8; 4];
        getrandom::getrandom(&mut b).expect("getrandom");
        let word = WORDS[b[0] as usize];
        let n = u16::from_le_bytes([b[1], b[2]]) as u32 * 256 + b[3] as u32;
        Code(format!("{word}-{:04}", n % 10000))
    }

    /// Accept what people type: any case, spaces or dashes or nothing
    /// between word and digits ("tide 4782", "Tide4782").
    pub fn parse(s: &str) -> Option<Code> {
        let s: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_uppercase();
        let split = s.find(|c: char| c.is_ascii_digit())?;
        let (word, digits) = s.split_at(split);
        if !WORDS.contains(&word) || digits.len() != 4 || !digits.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        Some(Code(format!("{word}-{digits}")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Constant-time comparison (for the SSH password path).
    pub fn matches(&self, typed: &str) -> bool {
        use subtle::ConstantTimeEq;
        match Code::parse(typed) {
            Some(c) if c.0.len() == self.0.len() => c.0.as_bytes().ct_eq(self.0.as_bytes()).into(),
            _ => false,
        }
    }
}

impl std::fmt::Display for Code {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ------------------------------------------------------------ ssh keys ----

/// Key types accepted for upload.
const KEY_TYPES: &[&str] = &[
    "ssh-ed25519",
    "sk-ssh-ed25519@openssh.com",
    "ecdsa-sha2-nistp256",
    "ecdsa-sha2-nistp384",
    "ecdsa-sha2-nistp521",
    "sk-ecdsa-sha2-nistp256@openssh.com",
    "ssh-rsa",
];

/// A parsed OpenSSH public key line: `type base64 [comment]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicKey {
    pub kind: String,
    pub blob: Vec<u8>,
    pub comment: String,
}

impl PublicKey {
    /// Parse one key. No authorized_keys options (`command=`, …) are
    /// accepted: a paired key is a plain key.
    pub fn parse(line: &str) -> Result<PublicKey, String> {
        let line = line.trim();
        if line.contains('\n') || line.contains('\r') || line.contains('\0') {
            return Err("a key is one line".into());
        }
        let mut it = line.splitn(3, |c: char| c == ' ' || c == '\t');
        let kind = it.next().unwrap_or("");
        if !KEY_TYPES.contains(&kind) {
            return Err(format!("unsupported key type '{}'", kind.chars().take(40).collect::<String>()));
        }
        let b64 = it.next().ok_or("missing key data")?.trim();
        let blob = STANDARD.decode(b64).map_err(|_| "key data is not base64")?;
        // the blob starts with string(kind)
        if blob.len() < 4 {
            return Err("key data too short".into());
        }
        let n = u32::from_be_bytes([blob[0], blob[1], blob[2], blob[3]]) as usize;
        if blob.len() < 4 + n || &blob[4..4 + n] != kind.as_bytes() {
            return Err("key data does not match its type".into());
        }
        if kind == "ssh-rsa" && blob.len() < 256 {
            return Err("RSA key too small".into());
        }
        let comment: String = it
            .next()
            .unwrap_or("")
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(200)
            .collect();
        Ok(PublicKey { kind: kind.into(), blob, comment })
    }

    /// `SHA256:…` as printed by `ssh-keygen -lf`.
    pub fn fingerprint(&self) -> String {
        fingerprint_blob(&self.blob)
    }

    /// The canonical one-line form.
    pub fn line(&self) -> String {
        let b = STANDARD.encode(&self.blob);
        if self.comment.is_empty() {
            format!("{} {}", self.kind, b)
        } else {
            format!("{} {} {}", self.kind, b, self.comment)
        }
    }

    /// Short type name for people: ed25519, ecdsa, rsa, …
    pub fn short_kind(&self) -> &str {
        match self.kind.as_str() {
            "ssh-ed25519" => "ed25519",
            "sk-ssh-ed25519@openssh.com" => "ed25519-sk",
            "ssh-rsa" => "rsa",
            k if k.starts_with("sk-ecdsa") => "ecdsa-sk",
            k if k.starts_with("ecdsa") => "ecdsa",
            k => k,
        }
    }

    /// Same key material (comments may differ).
    pub fn same_key(&self, other: &PublicKey) -> bool {
        self.kind == other.kind && self.blob == other.blob
    }
}

pub fn fingerprint_blob(blob: &[u8]) -> String {
    format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(blob)))
}

/// Fingerprint of a `type base64 [comment]` line, without type checks
/// (host keys).
pub fn fingerprint_line(line: &str) -> Option<String> {
    let b64 = line.split_whitespace().nth(1)?;
    STANDARD.decode(b64).ok().map(|b| fingerprint_blob(&b))
}

// ---------------------------------------------------------------- crypto ----

fn b64(b: &[u8]) -> String {
    STANDARD.encode(b)
}

fn unb64(s: &str) -> Result<Vec<u8>, String> {
    STANDARD.decode(s).map_err(|_| "bad base64".to_string())
}

fn transcript(msg_a: &[u8], msg_b: &[u8], hostname: &str, host_keys: &[String]) -> [u8; 32] {
    let mut h = Sha256::new();
    let mut put = |b: &[u8]| {
        h.update((b.len() as u32).to_be_bytes());
        h.update(b);
    };
    put(PROTOCOL.as_bytes());
    put(msg_a);
    put(msg_b);
    put(hostname.as_bytes());
    put(&(host_keys.len() as u32).to_be_bytes());
    for k in host_keys {
        put(k.as_bytes());
    }
    h.finalize().into()
}

/// Keys derived from the SPAKE2 secret and the transcript.
struct Keys {
    server_confirm: [u8; 32],
    client_mac: [u8; 32],
    done_mac: [u8; 32],
}

fn derive(shared: &[u8], transcript: &[u8; 32]) -> Keys {
    let hk = Hkdf::<Sha256>::new(Some(transcript), shared);
    let mut k = Keys { server_confirm: [0; 32], client_mac: [0; 32], done_mac: [0; 32] };
    hk.expand(b"mortar-pair/1 server confirm", &mut k.server_confirm).unwrap();
    hk.expand(b"mortar-pair/1 client key", &mut k.client_mac).unwrap();
    hk.expand(b"mortar-pair/1 done", &mut k.done_mac).unwrap();
    k
}

fn mac(key: &[u8; 32], parts: &[&[u8]]) -> Vec<u8> {
    let mut m = <HmacSha256 as Mac>::new_from_slice(key).unwrap();
    for p in parts {
        m.update(&(p.len() as u32).to_be_bytes());
        m.update(p);
    }
    m.finalize().into_bytes().to_vec()
}

fn mac_ok(key: &[u8; 32], parts: &[&[u8]], tag: &[u8]) -> bool {
    let mut m = <HmacSha256 as Mac>::new_from_slice(key).unwrap();
    for p in parts {
        m.update(&(p.len() as u32).to_be_bytes());
        m.update(p);
    }
    m.verify_slice(tag).is_ok()
}

fn done_parts(fp: &str, password_login: bool, addresses: &[String]) -> Vec<Vec<u8>> {
    let mut v = vec![b"done".to_vec(), fp.as_bytes().to_vec(), vec![password_login as u8]];
    v.extend(addresses.iter().map(|a| a.as_bytes().to_vec()));
    v
}

/// The computer's side.
pub struct ClientStart {
    spake: Spake2<Ed25519Group>,
    msg_a: Vec<u8>,
}

/// The computer's side after a verified challenge.
pub struct ClientSession {
    keys: Keys,
    pub hostname: String,
    pub host_keys: Vec<String>,
}

impl ClientStart {
    /// Returns the state and the `start` message to send.
    pub fn new(code: &Code) -> (ClientStart, Msg) {
        let (spake, msg_a) = Spake2::<Ed25519Group>::start_a(
            &Password::new(code.as_str().as_bytes()),
            &Identity::new(ID_CLIENT),
            &Identity::new(ID_SERVER),
        );
        let m = Msg::Start { protocol: PROTOCOL.into(), spake: b64(&msg_a) };
        (ClientStart { spake, msg_a }, m)
    }

    /// Check the Brick's challenge. An error means a wrong code (or someone
    /// in the middle): either way, nothing it sent can be trusted.
    pub fn finish(self, challenge: &Msg) -> Result<ClientSession, String> {
        let Msg::Challenge { spake, hostname, host_keys, confirm } = challenge else {
            return Err("expected a challenge".into());
        };
        let msg_b = unb64(spake)?;
        let shared = self.spake.finish(&msg_b).map_err(|_| "bad SPAKE2 message".to_string())?;
        let t = transcript(&self.msg_a, &msg_b, hostname, host_keys);
        let keys = derive(&shared, &t);
        if !mac_ok(&keys.server_confirm, &[b"challenge"], &unb64(confirm)?) {
            return Err("wrong code".into());
        }
        Ok(ClientSession { keys, hostname: hostname.clone(), host_keys: host_keys.clone() })
    }
}

impl ClientSession {
    pub fn add_key(&self, key: &PublicKey) -> Msg {
        let line = key.line();
        Msg::AddKey { mac: b64(&mac(&self.keys.client_mac, &[b"add_key", line.as_bytes()])), key: line }
    }

    pub fn verify_done(&self, done: &Msg) -> Result<(), String> {
        let Msg::Done { key_fingerprint, password_login, addresses, mac: tag } = done else {
            return Err("expected done".into());
        };
        let parts = done_parts(key_fingerprint, *password_login, addresses);
        let refs: Vec<&[u8]> = parts.iter().map(|p| p.as_slice()).collect();
        if mac_ok(&self.keys.done_mac, &refs, &unb64(tag)?) {
            Ok(())
        } else {
            Err("the Brick's reply was not authentic".into())
        }
    }
}

/// The Brick's side of one connection.
pub struct ServerSession {
    keys: Keys,
}

impl ServerSession {
    /// Answer a `start`: returns the session and the `challenge` to send.
    pub fn respond(code: &Code, start_spake_b64: &str, hostname: &str, host_keys: &[String]) -> Result<(ServerSession, Msg), String> {
        let msg_a = unb64(start_spake_b64)?;
        let (spake, msg_b) = Spake2::<Ed25519Group>::start_b(
            &Password::new(code.as_str().as_bytes()),
            &Identity::new(ID_CLIENT),
            &Identity::new(ID_SERVER),
        );
        let shared = spake.finish(&msg_a).map_err(|_| "bad SPAKE2 message".to_string())?;
        let t = transcript(&msg_a, &msg_b, hostname, host_keys);
        let keys = derive(&shared, &t);
        let confirm = b64(&mac(&keys.server_confirm, &[b"challenge"]));
        let m = Msg::Challenge { spake: b64(&msg_b), hostname: hostname.into(), host_keys: host_keys.to_vec(), confirm };
        Ok((ServerSession { keys }, m))
    }

    /// Check `add_key`. Ok(key) only if the computer used the right code.
    pub fn check_key(&self, m: &Msg) -> Result<Result<PublicKey, String>, ()> {
        let Msg::AddKey { key, mac: tag } = m else { return Err(()) };
        let Ok(tag) = unb64(tag) else { return Err(()) };
        if !mac_ok(&self.keys.client_mac, &[b"add_key", key.as_bytes()], &tag) {
            return Err(());
        }
        Ok(PublicKey::parse(key))
    }

    pub fn done(&self, key_fingerprint: &str, password_login: bool, addresses: Vec<String>) -> Msg {
        let parts = done_parts(key_fingerprint, password_login, &addresses);
        let refs: Vec<&[u8]> = parts.iter().map(|p| p.as_slice()).collect();
        Msg::Done {
            key_fingerprint: key_fingerprint.into(),
            password_login,
            addresses,
            mac: b64(&mac(&self.keys.done_mac, &refs)),
        }
    }
}

// ---------------------------------------------------------------- client ----

/// The computer side over TCP (used by `mortar pair` and the tests).
pub mod client {
    use super::*;
    use std::io::BufReader;
    use std::net::{SocketAddr, TcpStream};

    #[derive(Debug)]
    pub enum PairError {
        Io(io::Error),
        /// The code was wrong (or someone is in the middle).
        WrongCode { tries_left: u32 },
        /// The Brick said no.
        Refused { code: ErrorCode, message: String, tries_left: u32 },
        Protocol(String),
    }

    impl std::fmt::Display for PairError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                PairError::Io(e) => write!(f, "{e}"),
                PairError::WrongCode { .. } => write!(f, "that code does not match the one on the Brick"),
                PairError::Refused { message, .. } => write!(f, "{message}"),
                PairError::Protocol(m) => write!(f, "protocol error: {m}"),
            }
        }
    }

    impl From<io::Error> for PairError {
        fn from(e: io::Error) -> Self {
            PairError::Io(e)
        }
    }

    #[derive(Clone, Debug)]
    pub struct Paired {
        pub hostname: String,
        /// Verified host keys (`type base64`), preferred first.
        pub host_keys: Vec<String>,
        pub key_fingerprint: String,
        pub password_login: bool,
        pub addresses: Vec<String>,
    }

    fn connect(addr: &SocketAddr, timeout: Duration) -> io::Result<(BufReader<TcpStream>, TcpStream)> {
        let s = TcpStream::connect_timeout(addr, timeout)?;
        s.set_read_timeout(Some(Duration::from_secs(30)))?;
        s.set_write_timeout(Some(Duration::from_secs(30)))?;
        Ok((BufReader::new(s.try_clone()?), s))
    }

    /// Ask whether pairing is open (no code needed). Returns an `Info`.
    pub fn hello(addr: &SocketAddr, timeout: Duration) -> io::Result<Msg> {
        let (mut r, mut w) = connect(addr, timeout)?;
        write_msg(&mut w, &Msg::Hello { protocol: PROTOCOL.into() })?;
        read_msg(&mut r)
    }

    fn refused(m: Msg) -> PairError {
        match m {
            Msg::Error { code: ErrorCode::WrongCode, tries_left, .. } => PairError::WrongCode { tries_left },
            Msg::Error { code, message, tries_left } => PairError::Refused { code, message, tries_left },
            other => PairError::Protocol(format!("unexpected {other:?}")),
        }
    }

    /// Run the whole exchange. `on_verified` is called with the verified
    /// session before the key is sent (to show the fingerprint).
    pub fn pair(
        addr: &SocketAddr,
        code: &Code,
        key: &PublicKey,
        timeout: Duration,
        on_verified: impl FnOnce(&ClientSession),
    ) -> Result<Paired, PairError> {
        let (mut r, mut w) = connect(addr, timeout)?;
        let (start, msg) = ClientStart::new(code);
        write_msg(&mut w, &msg)?;
        let ch = read_msg(&mut r)?;
        if !matches!(ch, Msg::Challenge { .. }) {
            return Err(refused(ch));
        }
        let session = match start.finish(&ch) {
            Ok(s) => s,
            Err(_) => {
                // Tell the Brick (it counts the try either way) and learn
                // how many are left: an add_key with a MAC that can't verify.
                let _ = write_msg(&mut w, &Msg::AddKey { key: key.line(), mac: STANDARD.encode([0u8; 32]) });
                return Err(match read_msg(&mut r) {
                    Ok(m) => refused(m),
                    Err(_) => PairError::WrongCode { tries_left: 0 },
                });
            }
        };
        on_verified(&session);
        write_msg(&mut w, &session.add_key(key))?;
        let done = read_msg(&mut r)?;
        if !matches!(done, Msg::Done { .. }) {
            return Err(refused(done));
        }
        session.verify_done(&done).map_err(PairError::Protocol)?;
        let Msg::Done { key_fingerprint, password_login, addresses, .. } = done else { unreachable!() };
        Ok(Paired { hostname: session.hostname.clone(), host_keys: session.host_keys.clone(), key_fingerprint, password_login, addresses })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ED: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl you@laptop";

    #[test]
    fn code_parse_and_format() {
        let c = Code::parse("tide 4782").unwrap();
        assert_eq!(c.as_str(), "TIDE-4782");
        assert_eq!(Code::parse("TIDE-4782"), Some(c.clone()));
        assert_eq!(Code::parse("Tide4782"), Some(c.clone()));
        assert!(Code::parse("TIDE-478").is_none());
        assert!(Code::parse("NOPE-4782").is_none());
        assert!(Code::parse("4782").is_none());
        assert!(c.matches("tide-4782"));
        assert!(!c.matches("tide-4783"));
        for _ in 0..200 {
            let r = Code::random();
            assert_eq!(Code::parse(r.as_str()), Some(r));
        }
    }

    #[test]
    fn words_unique() {
        let mut w = WORDS.to_vec();
        w.sort();
        w.dedup();
        assert_eq!(w.len(), 256);
        assert!(WORDS.iter().all(|w| w.chars().all(|c| c.is_ascii_uppercase())));
    }

    #[test]
    fn key_parse() {
        let k = PublicKey::parse(ED).unwrap();
        assert_eq!(k.kind, "ssh-ed25519");
        assert_eq!(k.comment, "you@laptop");
        assert_eq!(k.short_kind(), "ed25519");
        assert_eq!(k.line(), ED);
        assert!(k.fingerprint().starts_with("SHA256:"));
        assert_eq!(k.fingerprint().len(), 7 + 43);
        assert!(PublicKey::parse("command=\"x\" ssh-ed25519 AAAA").is_err());
        assert!(PublicKey::parse("ssh-ed25519 AAAAB3NzaC1yc2EAAAADAQABAAABAQ").is_err());
        assert!(PublicKey::parse("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMq\nssh-rsa x").is_err());
    }

    fn run(client_code: &str, server_code: &str) -> Result<(), String> {
        let host_keys = vec!["ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA2SX5sCWSkqrIKgZy8HTRtXwBDQd9vvOyR0x8wbGG4T".to_string()];
        let (cs, start) = ClientStart::new(&Code::parse(client_code).unwrap());
        let Msg::Start { spake, .. } = &start else { panic!() };
        let (ss, challenge) = ServerSession::respond(&Code::parse(server_code).unwrap(), spake, "mortar", &host_keys).unwrap();
        let session = cs.finish(&challenge)?;
        assert_eq!(session.host_keys, host_keys);
        let key = PublicKey::parse(ED).unwrap();
        let add = session.add_key(&key);
        let got = ss.check_key(&add).map_err(|_| "server: wrong code")?.unwrap();
        assert!(got.same_key(&key));
        let done = ss.done(&got.fingerprint(), false, vec!["10.55.0.1".into()]);
        session.verify_done(&done)
    }

    #[test]
    fn handshake() {
        run("TIDE-4782", "TIDE-4782").unwrap();
        assert_eq!(run("TIDE-4782", "TIDE-4783").unwrap_err(), "wrong code");
    }

    #[test]
    fn tampering_detected() {
        let host_keys = vec!["ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA2SX5sCWSkqrIKgZy8HTRtXwBDQd9vvOyR0x8wbGG4T".to_string()];
        let code = Code::parse("MOSS-0001").unwrap();
        let (cs, start) = ClientStart::new(&code);
        let Msg::Start { spake, .. } = &start else { panic!() };
        let (ss, mut challenge) = ServerSession::respond(&code, spake, "mortar", &host_keys).unwrap();
        // a man in the middle swapping the host key is caught
        if let Msg::Challenge { host_keys, .. } = &mut challenge {
            host_keys[0] = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl".into();
        }
        assert!(ClientStart::finish(cs, &challenge).is_err());
        // a key swapped in transit is caught
        let (cs, start) = ClientStart::new(&code);
        let Msg::Start { spake, .. } = &start else { panic!() };
        let (ss2, challenge) = ServerSession::respond(&code, spake, "mortar", &host_keys).unwrap();
        let session = cs.finish(&challenge).unwrap();
        let mut add = session.add_key(&PublicKey::parse(ED).unwrap());
        if let Msg::AddKey { key, .. } = &mut add {
            key.push_str("x");
        }
        assert!(ss2.check_key(&add).is_err());
        let _ = ss;
    }

    #[test]
    fn wire_roundtrip() {
        let m = Msg::Error { code: ErrorCode::WrongCode, message: "no".into(), tries_left: 3 };
        let mut buf = Vec::new();
        write_msg(&mut buf, &m).unwrap();
        assert_eq!(read_msg(&mut &buf[..]).unwrap(), m);
        let long = vec![b'x'; MAX_LINE + 10];
        assert!(read_msg(&mut &long[..]).is_err());
    }
}
