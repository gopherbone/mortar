// SPDX-License-Identifier: GPL-2.0-or-later
//! Time zones for Settings > Time zone: the list comes from the system's
//! tzdata (zone1970.tab, else zone.tab, else `timedatectl list-timezones`),
//! the current zone from /etc/localtime, and each zone's current UTC offset
//! from a helper process (`mortar-shell tz-offsets`), which sets TZ per zone:
//! setenv in the multi-threaded compositor itself would be unsafe.
//!
//! The zone is set with `timedatectl set-timezone` (config
//! `actions.timezoneSet`); NixOS leaves /etc/localtime to timedated when
//! `time.timeZone` is null (modules/core/base.nix).

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The zone /etc/localtime points at ("UTC" if it is missing).
pub fn current(localtime: &Path) -> String {
    match std::fs::read_link(localtime) {
        Ok(t) => {
            let s = t.to_string_lossy();
            match s.find("zoneinfo/") {
                Some(i) => s[i + 9..].to_string(),
                None => s.rsplit('/').next().unwrap_or("UTC").to_string(),
            }
        }
        Err(_) => "UTC".into(),
    }
}

/// Zone ids from a zone(1970).tab file (third column).
pub fn parse_tab(s: &str) -> Vec<String> {
    s.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split('\t').nth(2))
        .map(|z| z.trim().to_string())
        .filter(|z| !z.is_empty())
        .collect()
}

/// All canonical zones, sorted, with "UTC".
pub fn list(dir: &Path) -> Vec<String> {
    let mut v = ["zone1970.tab", "zone.tab"]
        .iter()
        .find_map(|f| std::fs::read_to_string(dir.join(f)).ok().map(|s| parse_tab(&s)).filter(|v| !v.is_empty()))
        .or_else(|| {
            let out = Command::new("timedatectl").arg("list-timezones").output().ok()?;
            Some(String::from_utf8_lossy(&out.stdout).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        })
        .unwrap_or_default();
    v.push("UTC".into());
    v.sort();
    v.dedup();
    v
}

/// Current UTC offsets (seconds) of `zones`, from the helper process.
pub fn offsets(dir: &Path, zones: &[String]) -> HashMap<String, i32> {
    let mut m = HashMap::new();
    let Ok(exe) = std::env::current_exe() else { return m };
    let child = Command::new(exe)
        .arg("tz-offsets")
        .arg(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else { return m };
    if let Some(mut stdin) = child.stdin.take() {
        let list = zones.join("\n") + "\n";
        // the helper reads everything before it answers: no deadlock
        let _ = stdin.write_all(list.as_bytes());
    }
    if let Some(out) = child.stdout.take() {
        for l in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            if let Some((z, o)) = l.split_once('\t')
                && let Ok(o) = o.parse()
            {
                m.insert(z.to_string(), o);
            }
        }
    }
    let _ = child.wait();
    m
}

unsafe extern "C" {
    fn tzset();
}

/// `mortar-shell tz-offsets DIR`: zone ids on stdin, "zone\toffset" out.
pub fn helper_main(dir: &str) -> Result<(), String> {
    let zones: Vec<String> = std::io::stdin().lock().lines().map_while(Result::ok).filter(|l| !l.is_empty()).collect();
    let mut out = std::io::stdout().lock();
    for z in zones {
        // SAFETY: this helper is single-threaded
        unsafe {
            std::env::set_var("TZ", format!(":{}", PathBuf::from(dir).join(&z).display()));
            tzset();
            let t = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            if !libc::localtime_r(&t, &mut tm).is_null() {
                let _ = writeln!(out, "{z}\t{}", tm.tm_gmtoff);
            }
        }
    }
    Ok(())
}

/// "America/Argentina/Buenos_Aires" -> ("Buenos Aires", Some("Argentina")).
pub fn pretty(zone: &str) -> (String, Option<String>) {
    let parts: Vec<&str> = zone.split('/').collect();
    let city = parts.last().unwrap_or(&zone).replace('_', " ");
    let sub = if parts.len() > 2 { Some(parts[1..parts.len() - 1].join(" / ").replace('_', " ")) } else { None };
    (city, sub)
}

/// The region of a zone ("America"), or the zone itself ("UTC").
pub fn region(zone: &str) -> &str {
    zone.split('/').next().unwrap_or(zone)
}

/// "UTC+01:00", "UTC-03:30", "UTC+00:00" (fixed width: the column lines up).
pub fn fmt_offset(secs: i32) -> String {
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.abs();
    format!("UTC{sign}{:02}:{:02}", a / 3600, a % 3600 / 60)
}

/// Local time "HH:MM" at an offset, now.
pub fn hhmm_at(offset: i32) -> String {
    // SAFETY: plain time()
    let t = unsafe { libc::time(std::ptr::null_mut()) } as i64 + offset as i64;
    let day = t.rem_euclid(86_400);
    format!("{:02}:{:02}", day / 3600, day % 3600 / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_and_names() {
        let tab = "# comment\nAD\t+4230+00131\tEurope/Andorra\nAR\t-3436-05827\tAmerica/Argentina/Buenos_Aires\n";
        assert_eq!(parse_tab(tab), vec!["Europe/Andorra", "America/Argentina/Buenos_Aires"]);
        assert_eq!(pretty("America/Argentina/Buenos_Aires"), ("Buenos Aires".into(), Some("Argentina".into())));
        assert_eq!(pretty("Europe/Isle_of_Man"), ("Isle of Man".into(), None));
        assert_eq!(region("Pacific/Auckland"), "Pacific");
        assert_eq!(fmt_offset(3600), "UTC+01:00");
        assert_eq!(fmt_offset(-12600), "UTC-03:30");
        assert_eq!(fmt_offset(0), "UTC+00:00");
    }

    #[test]
    fn current_from_symlink() {
        let d = std::env::temp_dir().join(format!("mortar-tz-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        let l = d.join("localtime");
        let _ = std::fs::remove_file(&l);
        std::os::unix::fs::symlink("../etc/zoneinfo/Europe/Berlin", &l).unwrap();
        assert_eq!(current(&l), "Europe/Berlin");
        assert_eq!(current(&d.join("missing")), "UTC");
        let _ = std::fs::remove_dir_all(&d);
    }
}
