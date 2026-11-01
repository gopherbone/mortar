// SPDX-License-Identifier: GPL-2.0-or-later
//! Small system helpers: host name, interface addresses, link state,
//! running helper commands.

use std::ffi::CStr;
use std::fs;
use std::net::Ipv4Addr;
use std::process::{Command, Stdio};

pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: buf is valid for its length; gethostname NUL-terminates
    // within it on success.
    let r = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if r != 0 {
        return "mortar".into();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let s = String::from_utf8_lossy(&buf[..end]).to_string();
    s.split('.').next().unwrap_or("mortar").to_string()
}

#[cfg(target_os = "linux")]
pub fn set_hostname(h: &str) -> Result<(), String> {
    // SAFETY: pointer/length describe the bytes of `h`.
    let r = unsafe { libc::sethostname(h.as_ptr() as *const libc::c_char, h.len()) };
    if r == 0 { Ok(()) } else { Err(std::io::Error::last_os_error().to_string()) }
}

#[cfg(not(target_os = "linux"))]
pub fn set_hostname(_h: &str) -> Result<(), String> {
    Err("not supported here".into())
}

/// IPv4 addresses per interface.
pub fn ipv4_addrs() -> Vec<(String, Ipv4Addr)> {
    let mut out = Vec::new();
    let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs allocates a list we walk read-only and free once.
    unsafe {
        if libc::getifaddrs(&mut ifap) != 0 {
            return out;
        }
        let mut p = ifap;
        while !p.is_null() {
            let ifa = &*p;
            if !ifa.ifa_addr.is_null() && (*ifa.ifa_addr).sa_family as i32 == libc::AF_INET {
                let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                let name = CStr::from_ptr(ifa.ifa_name).to_string_lossy().into_owned();
                out.push((name, ip));
            }
            p = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
    }
    out
}

pub fn addr_of(iface: &str) -> Option<Ipv4Addr> {
    ipv4_addrs().into_iter().find(|(n, _)| n == iface).map(|(_, a)| a)
}

/// The interface carrying `ip`, if it is one of ours.
pub fn iface_of(ip: Ipv4Addr) -> Option<String> {
    ipv4_addrs().into_iter().find(|(_, a)| *a == ip).map(|(n, _)| n)
}

/// A link with carrier (for usb0: a computer has configured the gadget).
pub fn carrier(iface: &str) -> bool {
    fs::read_to_string(format!("/sys/class/net/{iface}/carrier")).is_ok_and(|s| s.trim() == "1")
        && fs::read_to_string(format!("/sys/class/net/{iface}/operstate")).is_ok_and(|s| s.trim() != "down")
}

/// Run a helper, ignoring its output; returns success. Missing helpers
/// (e.g. off-device) are not an error worth more than a log line.
pub fn run(argv: &[&str]) -> bool {
    let Some((cmd, args)) = argv.split_first() else { return false };
    match Command::new(cmd).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output() {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            eprintln!("{}: {}", argv.join(" "), String::from_utf8_lossy(&o.stderr).trim());
            false
        }
        Err(e) => {
            eprintln!("{cmd}: {e}");
            false
        }
    }
}

/// Start a helper in the background (reloads that may take a while).
pub fn spawn(argv: &[&str]) {
    let Some((cmd, args)) = argv.split_first() else { return };
    if let Err(e) = Command::new(cmd).args(args).stdin(Stdio::null()).stdout(Stdio::null()).spawn() {
        eprintln!("{cmd}: {e}");
    }
}

/// Is `ip` inside `net/prefix`?
pub fn in_subnet(ip: Ipv4Addr, net: Ipv4Addr, prefix: u8) -> bool {
    let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix as u32) };
    (u32::from(ip) & mask) == (u32::from(net) & mask)
}

/// Parse "10.55.0.0/24".
pub fn parse_cidr(s: &str) -> Option<(Ipv4Addr, u8)> {
    let (a, p) = s.split_once('/')?;
    let p: u8 = p.parse().ok()?;
    (p <= 32).then_some((a.parse().ok()?, p))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subnets() {
        let (n, p) = parse_cidr("10.55.0.0/24").unwrap();
        assert!(in_subnet("10.55.0.17".parse().unwrap(), n, p));
        assert!(!in_subnet("10.55.1.17".parse().unwrap(), n, p));
        assert!(in_subnet("1.2.3.4".parse().unwrap(), n, 0));
        assert!(parse_cidr("10.0.0.0/33").is_none());
        assert!(ipv4_addrs().iter().any(|(_, a)| a.is_loopback()));
        assert!(!hostname().is_empty());
    }
}
