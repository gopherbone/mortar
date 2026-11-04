// SPDX-License-Identifier: GPL-2.0-or-later
//! `mortar-setup wifi`: join a network from a terminal, for someone who would rather
//! type a password on their computer's keyboard (`mortar wifi`, or `ssh -t` by hand).
//! The password goes over SSH to the control socket, never on a command line.

use crate::control::{Client, NetView, Request, Status};
use std::io::{BufRead, Write};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq)]
pub enum Pick {
    Skip,
    Net(usize),
    Bad,
}

/// What was typed at the prompt: a number from the list, or a network's name.
pub fn pick(input: &str, nets: &[NetView]) -> Pick {
    let t = input.trim();
    if t.is_empty() {
        return Pick::Skip;
    }
    if let Ok(n) = t.parse::<usize>()
        && (1..=nets.len()).contains(&n)
    {
        return Pick::Net(n - 1);
    }
    match nets.iter().position(|n| n.ssid == t) {
        Some(i) => Pick::Net(i),
        None => Pick::Bad,
    }
}

/// None: this kind of network can't be joined here.
pub fn needs_password(n: &NetView) -> Option<bool> {
    match n.security.as_str() {
        "open" => Some(false),
        "psk" => Some(!n.known),
        _ => None,
    }
}

fn line() -> Result<String, String> {
    let mut s = String::new();
    std::io::stdin().lock().read_line(&mut s).map_err(|e| e.to_string())?;
    Ok(s)
}

fn hidden_line(prompt: &str) -> Result<String, String> {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let stty = |arg: &str| Command::new("stty").arg(arg).stdin(Stdio::inherit()).stderr(Stdio::null()).status();
    let _ = stty("-echo");
    let r = line();
    let _ = stty("echo");
    println!();
    r
}

fn connected(st: &Status) -> bool {
    st.wifi.state == "connected"
}

fn say_connected(st: &Status) {
    let name = st.wifi.ssid.as_deref().unwrap_or("Wi-Fi");
    match st.wifi.address.as_deref() {
        Some(a) => println!("Connected to {name} ({a})."),
        None => println!("Connected to {name}."),
    }
}

/// `if_needed`: says nothing and does nothing when already connected or there's no Wi-Fi
/// (`mortar pair` runs it this way, right after pairing).
pub fn run(c: &Client, if_needed: bool) -> Result<i32, String> {
    let st = c.status()?;
    if !st.wifi.available {
        if !if_needed {
            println!("This Brick has no Wi-Fi available.");
        }
        return Ok(0);
    }
    if connected(&st) {
        if if_needed {
            return Ok(0);
        }
        say_connected(&st);
    }
    println!("Looking for networks...");
    c.act(Request::WifiScan)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    sleep(Duration::from_secs(1));
    let mut st = c.status()?;
    while st.wifi.scanning && Instant::now() < deadline {
        sleep(Duration::from_millis(400));
        st = c.status()?;
    }
    let nets = st.wifi.networks.clone();
    if nets.is_empty() {
        println!("No networks found.");
        return Ok(if if_needed { 0 } else { 1 });
    }
    for (i, n) in nets.iter().enumerate() {
        let note = if n.connected {
            "connected"
        } else if n.known {
            "saved"
        } else if n.security == "open" {
            "open"
        } else {
            ""
        };
        println!("{:>3}  {} {}{}", i + 1, "▮".repeat(n.bars as usize), n.ssid, if note.is_empty() { String::new() } else { format!("  ({note})") });
    }
    let net = loop {
        print!("Network (number or name, Enter to skip): ");
        let _ = std::io::stdout().flush();
        match pick(&line()?, &nets) {
            Pick::Skip => return Ok(0),
            Pick::Net(i) => break &nets[i],
            Pick::Bad => println!("That isn't one of these."),
        }
    };
    let Some(ask) = needs_password(net) else {
        return Err(format!("{} needs a kind of login that isn't supported here.", net.ssid));
    };
    let passphrase = if ask {
        let p = hidden_line(&format!("Password for {}: ", net.ssid))?;
        Some(p.trim_end_matches(['\r', '\n']).to_string())
    } else {
        None
    };
    c.act(Request::WifiConnect { ssid: net.ssid.clone(), passphrase })?;
    println!("Joining {}...", net.ssid);
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(45) {
        sleep(Duration::from_millis(500));
        let st = c.status()?;
        if connected(&st) && st.wifi.ssid.as_deref() == Some(net.ssid.as_str()) && st.wifi.last_error.is_none() {
            say_connected(&st);
            return Ok(0);
        }
        if st.wifi.state != "connecting" && start.elapsed() > Duration::from_secs(2)
            && let Some(e) = st.wifi.last_error
        {
            return Err(format!("Couldn't join {}: {e}", net.ssid));
        }
    }
    Err(format!("Couldn't join {}: it took too long.", net.ssid))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(ssid: &str, security: &str, known: bool) -> NetView {
        NetView { ssid: ssid.into(), security: security.into(), known, ..Default::default() }
    }

    #[test]
    fn picks_by_number_or_name() {
        let nets = [net("Home", "psk", false), net("Cafe 2", "open", false)];
        assert_eq!(pick("\n", &nets), Pick::Skip);
        assert_eq!(pick(" 2\n", &nets), Pick::Net(1));
        assert_eq!(pick("Home\n", &nets), Pick::Net(0));
        assert_eq!(pick("Cafe 2", &nets), Pick::Net(1));
        assert_eq!(pick("3", &nets), Pick::Bad);
        assert_eq!(pick("0", &nets), Pick::Bad);
        assert_eq!(pick("home", &nets), Pick::Bad);
    }

    #[test]
    fn asks_for_a_password_only_when_it_needs_one() {
        assert_eq!(needs_password(&net("a", "open", false)), Some(false));
        assert_eq!(needs_password(&net("a", "psk", false)), Some(true));
        assert_eq!(needs_password(&net("a", "psk", true)), Some(false));
        assert_eq!(needs_password(&net("a", "8021x", false)), None);
        assert_eq!(needs_password(&net("a", "wep", false)), None);
    }
}
