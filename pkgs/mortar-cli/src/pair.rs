// SPDX-License-Identifier: GPL-2.0-or-later
//! `mortar pair`: put this computer's SSH key on a Brick using the code on
//! its screen (protocol: pkgs/mortar-pair), then add `Host brick` to
//! ~/.ssh/config and pin the Brick's host key in ~/.ssh/known_hosts.

use mortar_pair::client::{hello, pair, PairError};
use mortar_pair::{Code, ErrorCode, Msg, PublicKey, PORT, USB_ADDR};
use std::env;
use std::fs;
use std::io::{self, BufRead, Write};
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

pub struct Opts {
    pub host: Option<String>,
    pub key: Option<PathBuf>,
    pub name: String,
    pub code: Option<String>,
    pub ssh_config: bool,
}

const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const OFF: &str = "\x1b[0m";

fn ok(msg: &str) {
    println!("{GREEN}✓{OFF} {msg}");
}

fn fail(msg: &str) -> ! {
    eprintln!("{RED}✗{OFF} {msg}");
    std::process::exit(1)
}

fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| fail("HOME is not set"))
}

/// Resolve with a deadline (mDNS lookups can hang for seconds).
fn resolve(host: &str, timeout: Duration) -> Vec<SocketAddr> {
    let (tx, rx) = mpsc::channel();
    let h = format!("{host}:{PORT}");
    thread::spawn(move || {
        let _ = tx.send(h.to_socket_addrs().map(|a| a.filter(|a| a.is_ipv4()).collect::<Vec<_>>()).unwrap_or_default());
    });
    rx.recv_timeout(timeout).unwrap_or_default()
}

/// Find a Brick that answers the pairing service.
fn discover(host: &Option<String>) -> Option<(SocketAddr, String, Msg)> {
    let mut cands: Vec<(String, Vec<SocketAddr>)> = Vec::new();
    match host {
        Some(h) => cands.push((h.clone(), resolve(h, Duration::from_secs(5)))),
        None => {
            cands.push(("USB".into(), vec![format!("{USB_ADDR}:{PORT}").parse().unwrap()]));
            cands.push(("mortar.local".into(), resolve("mortar.local", Duration::from_secs(3))));
        }
    }
    for (label, addrs) in cands {
        for a in addrs {
            if let Ok(m) = hello(&a, Duration::from_millis(1500)) {
                return Some((a, label, m));
            }
        }
    }
    None
}

fn find_key(o: &Opts) -> (PathBuf, PublicKey) {
    let paths: Vec<PathBuf> = match &o.key {
        Some(k) => vec![k.clone()],
        None => ["id_ed25519.pub", "id_ecdsa.pub", "id_ed25519_sk.pub", "id_rsa.pub"].iter().map(|f| home().join(".ssh").join(f)).collect(),
    };
    for p in &paths {
        if let Ok(s) = fs::read_to_string(p) {
            match PublicKey::parse(s.lines().next().unwrap_or("")) {
                Ok(k) => return (p.clone(), k),
                Err(e) => fail(&format!("{}: {e}", p.display())),
            }
        }
    }
    if o.key.is_some() {
        fail(&format!("cannot read {}", paths[0].display()));
    }
    // no key yet: offer to make one
    let p = home().join(".ssh/id_ed25519");
    print!("No SSH key found. Create {} now? [Y/n] ", p.display());
    io::stdout().flush().ok();
    let mut ans = String::new();
    io::stdin().lock().read_line(&mut ans).ok();
    if ans.trim().to_lowercase().starts_with('n') {
        fail("pairing needs an SSH key (ssh-keygen -t ed25519)");
    }
    let st = Command::new("ssh-keygen").args(["-t", "ed25519", "-f"]).arg(&p).status();
    if !st.is_ok_and(|s| s.success()) {
        fail("ssh-keygen failed");
    }
    let pubp = p.with_extension("pub");
    let s = fs::read_to_string(&pubp).unwrap_or_default();
    match PublicKey::parse(s.trim()) {
        Ok(k) => (pubp, k),
        Err(e) => fail(&format!("{}: {e}", pubp.display())),
    }
}

fn ask_code() -> Code {
    loop {
        print!("Enter the code on the Brick's screen: ");
        io::stdout().flush().ok();
        let mut s = String::new();
        if io::stdin().lock().read_line(&mut s).unwrap_or(0) == 0 {
            fail("no code entered");
        }
        match Code::parse(&s) {
            Some(c) => return c,
            None => println!("{DIM}  A code looks like TIDE-4782: a word and four digits.{OFF}"),
        }
    }
}

fn tilde(p: &Path) -> String {
    let h = home();
    match p.strip_prefix(&h) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

pub fn run(o: &Opts) {
    println!("Looking for Bricks on USB and the local network…");
    let Some((addr, label, info)) = discover(&o.host) else {
        fail(
            "No Brick answered. Connect the Brick's bottom USB-C port to this computer and \
             wait for it to show up as a network adapter (the Brick shows the code screen), \
             or pass --host.",
        );
    };
    let Msg::Info { hostname, pairing, reason, .. } = info else {
        if let Msg::Error { message, .. } = info {
            fail(&message);
        }
        fail("unexpected answer from the Brick");
    };
    let via = if label == "USB" { format!("USB · {}", addr.ip()) } else { format!("{label} · {}", addr.ip()) };
    println!("  {GREEN}●{OFF} {BOLD}{hostname}.local{OFF}  {DIM}{via}{OFF}");
    if !pairing {
        fail(&reason.unwrap_or_else(|| "The Brick is not pairing right now.".into()));
    }
    let (key_path, key) = find_key(o);

    let mut code = match &o.code {
        Some(c) => Code::parse(c).unwrap_or_else(|| fail("that is not a pairing code (e.g. TIDE-4782)")),
        None => ask_code(),
    };
    let paired = loop {
        let r = pair(&addr, &code, &key, Duration::from_secs(5), |s| {
            let fp = s.host_keys.first().and_then(|k| mortar_pair::fingerprint_line(k)).unwrap_or_default();
            ok(&format!("Host key matches the screen  {DIM}{fp}{OFF}"));
        });
        match r {
            Ok(p) => break p,
            Err(PairError::WrongCode { tries_left }) if o.code.is_none() && tries_left > 0 => {
                eprintln!("{RED}✗{OFF} That code doesn't match the one on the Brick ({tries_left} tries left).");
                thread::sleep(Duration::from_millis(1100));
                code = ask_code();
            }
            Err(PairError::Refused { code: ErrorCode::Busy, .. }) => thread::sleep(Duration::from_millis(1200)),
            Err(e) => fail(&e.to_string()),
        }
    };
    ok(&format!("Added {} to the Brick", tilde(&key_path)));
    if !paired.password_login {
        ok("Turned off password login");
    }

    let host = format!("{}.local", paired.hostname);
    if o.ssh_config {
        let identity = key_path.with_extension("");
        match write_known_hosts(&home().join(".ssh/known_hosts"), &o.name, &paired.host_keys)
            .and_then(|_| write_ssh_config(&home().join(".ssh/config"), &o.name, &host, &identity))
        {
            Ok(true) => ok(&format!("Added \"Host {}\" to ~/.ssh/config", o.name)),
            Ok(false) => {
                println!("  {DIM}~/.ssh/config already has its own \"Host {}\"; left it alone (use --name).{OFF}", o.name);
            }
            Err(e) => eprintln!("{RED}✗{OFF} ~/.ssh/config: {e}"),
        }
        println!("Connect any time with: {BOLD}ssh {}{OFF}", o.name);
    } else {
        println!("Connect with: {BOLD}ssh root@{host}{OFF}");
    }
    if let Some(wifi) = paired.addresses.iter().find(|a| a.as_str() != USB_ADDR) {
        println!("{DIM}  (also on Wi-Fi at {wifi}; over the cable at {USB_ADDR}){OFF}");
    }
}

const BEGIN: &str = "# >>> mortar pair:";
const END: &str = "# <<< mortar pair:";

/// Add/replace our managed `Host` block at the top of ~/.ssh/config (top:
/// the first value ssh finds wins, so a later `Host *` can't override it).
/// Returns false if the user has an unmanaged block of that name.
pub fn update_ssh_config(text: &str, name: &str, hostname: &str, identity: &str) -> Option<String> {
    let begin = format!("{BEGIN} {name}");
    let end = format!("{END} {name}");
    let block = format!(
        "{begin} (managed; this block is rewritten by `mortar pair`)\n\
         Host {name}\n\
         \x20 HostName {hostname}\n\
         \x20 User root\n\
         \x20 HostKeyAlias {name}\n\
         \x20 IdentityFile {identity}\n\
         \x20 IdentitiesOnly yes\n\
         {end}\n"
    );
    if let (Some(b), Some(e)) = (text.find(&begin), text.find(&end)) {
        let e_line = text[e..].find('\n').map(|i| e + i + 1).unwrap_or(text.len());
        return Some(format!("{}{block}{}", &text[..b], &text[e_line..]));
    }
    let has_own = text.lines().any(|l| {
        let mut w = l.split_whitespace();
        w.next().is_some_and(|k| k.eq_ignore_ascii_case("host")) && w.any(|h| h == name)
    });
    if has_own {
        return None;
    }
    Some(if text.is_empty() { block } else { format!("{block}\n{text}") })
}

fn write_ssh_config(path: &Path, name: &str, host: &str, identity: &Path) -> io::Result<bool> {
    let text = fs::read_to_string(path).unwrap_or_default();
    match update_ssh_config(&text, name, host, &tilde(identity)) {
        Some(new) => {
            write_private(path, &new)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Replace the known_hosts entries for `alias` with the verified keys.
pub fn update_known_hosts(text: &str, alias: &str, keys: &[String]) -> String {
    let mut out: String = text
        .lines()
        .filter(|l| !l.split_whitespace().next().is_some_and(|h| h.split(',').any(|x| x == alias)))
        .map(|l| format!("{l}\n"))
        .collect();
    for k in keys {
        out.push_str(&format!("{alias} {k}\n"));
    }
    out
}

fn write_known_hosts(path: &Path, alias: &str, keys: &[String]) -> io::Result<()> {
    let text = fs::read_to_string(path).unwrap_or_default();
    write_private(path, &update_known_hosts(&text, alias, keys))
}

fn write_private(path: &Path, text: &str) -> io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if let Some(d) = path.parent() {
        if !d.exists() {
            fs::create_dir_all(d)?;
            fs::set_permissions(d, fs::Permissions::from_mode(0o700))?;
        }
    }
    let tmp = path.with_extension("mortar-tmp");
    {
        let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    // keep the original file's mode
    if let Ok(m) = fs::metadata(path) {
        fs::set_permissions(&tmp, m.permissions())?;
    }
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_config_block() {
        let a = update_ssh_config("", "brick", "mortar.local", "~/.ssh/id_ed25519").unwrap();
        assert!(a.starts_with("# >>> mortar pair: brick"));
        assert!(a.contains("Host brick\n  HostName mortar.local\n  User root\n"));
        let user = "Host *\n  User someone\n";
        let b = update_ssh_config(user, "brick", "mortar.local", "~/.ssh/id_ed25519").unwrap();
        assert!(b.starts_with("# >>> mortar pair: brick"), "ours goes first");
        assert!(b.ends_with(user));
        // re-pairing replaces our block in place
        let c = update_ssh_config(&b, "brick", "my-brick.local", "~/.ssh/id_ed25519").unwrap();
        assert_eq!(c.matches("Host brick").count(), 1);
        assert!(c.contains("HostName my-brick.local") && !c.contains("HostName mortar.local"));
        assert!(c.ends_with(user));
        // someone else's Host brick is left alone
        assert!(update_ssh_config("Host brick\n  HostName 1.2.3.4\n", "brick", "x", "y").is_none());
        assert!(update_ssh_config("Host brick2\n", "brick", "x", "y").is_some());
    }

    #[test]
    fn known_hosts_replaced() {
        let t = "github.com ssh-ed25519 AAAA1\nbrick ssh-ed25519 OLD\nbrick,10.0.0.1 ssh-rsa OLD2\n";
        let n = update_known_hosts(t, "brick", &["ssh-ed25519 NEW".into()]);
        assert_eq!(n, "github.com ssh-ed25519 AAAA1\nbrick ssh-ed25519 NEW\n");
    }
}
