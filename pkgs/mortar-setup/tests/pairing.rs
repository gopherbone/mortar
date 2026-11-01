// SPDX-License-Identifier: GPL-2.0-or-later
//! The pairing protocol end to end on localhost: the real daemon (pairing
//! listener + control socket) in a scratch root, and the real client code
//! `mortar pair` uses.

use mortar_pair::client::{hello, pair, PairError};
use mortar_pair::{Code, ErrorCode, Msg, PublicKey};
use mortar_setup::control::{Client, Request, Response};
use mortar_setup::daemon::{Config, Daemon};
use mortar_setup::state::Paths;
use std::fs;
use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

const HOST_KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIA2SX5sCWSkqrIKgZy8HTRtXwBDQd9vvOyR0x8wbGG4T root@mortar";
const LAPTOP: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl you@laptop";
const T: Duration = Duration::from_secs(5);

fn root(name: &str) -> PathBuf {
    // short path: Unix socket paths are limited to ~100 bytes
    let tmp = std::env::temp_dir();
    let base = if tmp.as_os_str().len() < 40 { tmp } else { PathBuf::from("/tmp") };
    let d = base.join(format!("msp-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    d
}

fn wrong(code: &Code) -> Code {
    let s = code.as_str();
    let (w, n) = s.split_once('-').unwrap();
    let n: u32 = n.parse().unwrap();
    Code::parse(&format!("{w}-{:04}", (n + 1) % 10000)).unwrap()
}

#[test]
fn pair_over_tcp() {
    let r = root("pair");
    let paths = Paths::under(&r);
    fs::create_dir_all(&paths.host_key_dir).unwrap();
    fs::write(paths.host_key_dir.join("ssh_host_ed25519_key.pub"), HOST_KEY).unwrap();
    let d = Daemon::new(Config::test(paths.clone()));
    d.serve_control().unwrap();
    let addr = d.serve_pairing().unwrap()[0];
    let ctl = Client::new(paths.socket());

    // first boot: no keys, so a code is on offer and the code works as
    // root's SSH password (sshd drop-in present)
    let st = ctl.status().unwrap();
    let code = Code::parse(&st.pairing.as_ref().expect("a code without keys").code).unwrap();
    assert!(st.password_login);
    let dropin = paths.sshd_dir().join("password-login.conf");
    assert!(fs::read_to_string(&dropin).unwrap().contains("Match LocalAddress 10.55.0.1"));

    let Msg::Info { pairing, tries_left, .. } = hello(&addr, T).unwrap() else { panic!() };
    assert!(pairing);
    assert_eq!(tries_left, 10);

    // SSH password path (PAM helper -> control socket)
    let check = |user: &str, rhost: &str, pw: &str| match ctl.call(&Request::CheckPassword { user: user.into(), rhost: rhost.into(), password: pw.into() }) {
        Ok(Response::Allowed { allowed }) => allowed,
        r => panic!("{r:?}"),
    };
    assert!(check("root", "127.0.0.1", &code.as_str().to_lowercase()));
    assert!(!check("root", "192.168.1.20", code.as_str()), "only from the USB subnet");
    assert!(!check("alex", "127.0.0.1", code.as_str()));

    let key = PublicKey::parse(LAPTOP).unwrap();

    // a wrong code: refused, and it costs a try (the wrong SSH password
    // above did not happen: only wrong passwords from allowed peers count)
    sleep(Duration::from_millis(1100));
    match pair(&addr, &wrong(&code), &key, T, |_| panic!("must not verify")) {
        Err(PairError::WrongCode { tries_left }) => assert_eq!(tries_left, 9),
        r => panic!("{r:?}"),
    }
    assert!(fs::read_to_string(&paths.card_keys).is_err(), "nothing stored");

    // the right code
    sleep(Duration::from_millis(1100));
    let mut seen_fp = None;
    let p = pair(&addr, &code, &key, T, |s| seen_fp = mortar_pair::fingerprint_line(&s.host_keys[0])).unwrap();
    assert_eq!(seen_fp, mortar_pair::fingerprint_line(HOST_KEY));
    assert_eq!(p.host_keys, vec![HOST_KEY.rsplit_once(' ').unwrap().0.to_string()]);
    assert_eq!(p.key_fingerprint, key.fingerprint());
    assert!(!p.password_login, "password login turns off once a key exists");
    assert_eq!(fs::read_to_string(&paths.card_keys).unwrap().trim(), LAPTOP);

    // the code is used up; the screen learns about the key
    let st = ctl.status().unwrap();
    assert!(st.pairing.is_none());
    assert_eq!(st.keys.len(), 1);
    assert_eq!(st.last_added.as_ref().unwrap().comment, "you@laptop");
    assert!(!dropin.exists(), "SSH password switched off");
    assert!(!check("root", "127.0.0.1", code.as_str()));
    sleep(Duration::from_millis(1100));
    match pair(&addr, &code, &key, T, |_| {}) {
        Err(PairError::Refused { code: ErrorCode::NotPairing, .. }) => {}
        r => panic!("{r:?}"),
    }

    // "Pair a new computer" on the Brick: a fresh code
    ctl.act(Request::NewCode).unwrap();
    let code2 = Code::parse(&ctl.status().unwrap().pairing.unwrap().code).unwrap();
    sleep(Duration::from_millis(1100));
    let desk = PublicKey::parse("ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBEmKSENjQEezOmxkZMy7opKgwFB9nkt5YRrYMjNuG5N87uRgg6CLrbo5wAdT/y6v0mKV0U2w0WZ2YB/++Tpockg= you@desktop").unwrap();
    pair(&addr, &code2, &desk, T, |_| {}).unwrap();
    assert_eq!(ctl.status().unwrap().keys.len(), 2);

    // remove one from the Brick
    ctl.act(Request::RemoveKey { fingerprint: key.fingerprint() }).unwrap();
    let st = ctl.status().unwrap();
    assert_eq!(st.keys.len(), 1);
    assert_eq!(st.keys[0].comment, "you@desktop");

    fs::remove_dir_all(&r).unwrap();
}

#[test]
fn tries_run_out() {
    let r = root("tries");
    let paths = Paths::under(&r);
    fs::create_dir_all(&paths.host_key_dir).unwrap();
    fs::write(paths.host_key_dir.join("ssh_host_ed25519_key.pub"), HOST_KEY).unwrap();
    let d = Daemon::new(Config::test(paths.clone()));
    let addr = d.serve_pairing().unwrap()[0];
    let code = Code::parse(&d.status().pairing.unwrap().code).unwrap();
    let key = PublicKey::parse(LAPTOP).unwrap();
    let bad = wrong(&code);
    for i in 0..mortar_pair::MAX_TRIES {
        match pair(&addr, &bad, &key, T, |_| {}) {
            Err(PairError::WrongCode { tries_left }) => assert_eq!(tries_left, mortar_pair::MAX_TRIES - i - 1),
            r => panic!("{r:?}"),
        }
        // too fast is refused without costing a try
        match pair(&addr, &bad, &key, T, |_| {}) {
            Err(PairError::Refused { code: ErrorCode::Busy | ErrorCode::Locked, .. }) => {}
            r => panic!("{r:?}"),
        }
        sleep(Duration::from_millis(1050));
    }
    // even the right code is refused now
    match pair(&addr, &code, &key, T, |_| {}) {
        Err(PairError::Refused { code: ErrorCode::Locked, .. }) => {}
        r => panic!("{r:?}"),
    }
    let st = d.status();
    assert!(st.pairing_locked && st.pairing.is_none());
    // a button press on the Brick unlocks with a new code
    d.handle(Request::NewCode);
    let code2 = Code::parse(&d.status().pairing.unwrap().code).unwrap();
    sleep(Duration::from_millis(1050));
    pair(&addr, &code2, &key, T, |_| {}).unwrap();
    fs::remove_dir_all(&r).unwrap();
}

#[test]
fn forbidden_peer() {
    let r = root("peer");
    let paths = Paths::under(&r);
    let mut cfg = Config::test(paths);
    cfg.allow = vec![("10.55.0.0".parse().unwrap(), 24)];
    let d = Daemon::new(cfg);
    let addr = d.serve_pairing().unwrap()[0];
    match hello(&addr, T).unwrap() {
        Msg::Error { code: ErrorCode::Forbidden, .. } => {}
        m => panic!("{m:?}"),
    }
    let _ = fs::remove_dir_all(&r);
}
