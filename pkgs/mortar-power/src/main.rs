// SPDX-License-Identifier: GPL-2.0-or-later
//! mortar-power: battery policy for Mortar on the TrimUI Brick.
//!
//! - USB input current: the stock PC/adapter policy (500 mA until it is
//!   clear no USB host is on the cable, then 2.5 A).
//! - Low battery: toasts with sound and haptics in the shell at 15 % and
//!   5 % (the rumble motor alone when no shell is running).
//! - Empty battery: a clean `systemctl poweroff` at 3 % (confirmed by
//!   voltage) or below a loaded-voltage floor, so the SD card is never cut
//!   off mid-write by the PMIC's own under-voltage switch.
//! - Writes /run/mortar-power/state (key=value) for the status bars,
//!   with a voltage estimate when the fuel gauge has no capacity.
//!
//! It only reads the kernel's power_supply class (axp2202-battery,
//! axp2202-usb) and writes the USB input current limit; the charger itself
//! (voltage, currents, gauge model) is configured by the kernel.

mod estimate;
mod policy;

use estimate::{Estimator, Flow};
use policy::{Battery, Event, Level, Reading, Thresholds, UsbLimit, UsbPolicy};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SUPPLIES: &str = "/sys/class/power_supply";

struct Opts {
    thresholds: Thresholds,
    host_ua: u32,
    adapter_ua: u32,
    state: PathBuf,
    rumble: Option<PathBuf>,
    dry_run: bool,
    /// Seconds between warning and power-off.
    grace_s: u64,
}

fn usage() -> ! {
    eprintln!(
        "usage: mortar-power [--low PCT] [--critical PCT] [--off PCT] [--off-confirm-mv MV]\n\
         \x20                   [--floor-mv MV] [--host-current-ua UA] [--adapter-current-ua UA]\n\
         \x20                   [--state PATH] [--rumble PATH] [--grace S] [--dry-run]\n\
         \x20      mortar-power status\n\
         --adapter-current-ua 0 leaves the USB input limit alone."
    );
    std::process::exit(2)
}

fn parse_args() -> Opts {
    let mut o = Opts {
        thresholds: Thresholds::default(),
        host_ua: 500_000,
        adapter_ua: 2_500_000,
        state: PathBuf::from("/run/mortar-power/state"),
        rumble: None,
        dry_run: false,
        grace_s: 10,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| usage());
        fn num<T: std::str::FromStr>(s: String) -> T {
            s.parse().unwrap_or_else(|_| usage())
        }
        match a.as_str() {
            "status" => {
                status();
                std::process::exit(0)
            }
            "--low" => o.thresholds.low = num(val()),
            "--critical" => o.thresholds.critical = num(val()),
            "--off" => o.thresholds.off = num(val()),
            "--off-confirm-mv" => o.thresholds.off_confirm_mv = num(val()),
            "--floor-mv" => o.thresholds.floor_mv = num(val()),
            "--host-current-ua" => o.host_ua = num(val()),
            "--adapter-current-ua" => o.adapter_ua = num(val()),
            "--state" => o.state = PathBuf::from(val()),
            "--rumble" => o.rumble = Some(PathBuf::from(val())),
            "--grace" => o.grace_s = num(val()),
            "--dry-run" => o.dry_run = true,
            _ => usage(),
        }
    }
    o
}

// ---------------------------------------------------------------- sysfs ----

fn read(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn read_num<T: std::str::FromStr>(p: &Path) -> Option<T> {
    read(p)?.parse().ok()
}

/// The battery and USB supply directories (by type, so names may change).
fn supplies() -> (Option<PathBuf>, Option<PathBuf>) {
    let (mut bat, mut usb) = (None, None);
    if let Ok(rd) = fs::read_dir(SUPPLIES) {
        let mut v: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        v.sort();
        for p in v {
            match read(&p.join("type")).as_deref() {
                Some("Battery") if bat.is_none() => bat = Some(p),
                Some("USB") if usb.is_none() => usb = Some(p),
                _ => {}
            }
        }
    }
    (bat, usb)
}

/// A USB host has configured our gadget (it is a computer, not a charger).
fn gadget_configured() -> bool {
    fs::read_dir("/sys/class/udc")
        .map(|rd| rd.flatten().any(|e| read(&e.path().join("state")).as_deref() == Some("configured")))
        .unwrap_or(false)
}

#[derive(Debug, Default, Clone)]
struct Sample {
    present: bool,
    status: String,
    voltage_mv: Option<u32>,
    gauge: Option<u8>,
    tte_s: Option<u32>,
    ttf_s: Option<u32>,
    online: bool,
}

fn sample(bat: &Path, usb: Option<&Path>) -> Sample {
    Sample {
        present: read(&bat.join("present")).as_deref() == Some("1"),
        status: read(&bat.join("status")).unwrap_or_else(|| "Unknown".into()),
        voltage_mv: read_num::<u32>(&bat.join("voltage_now")).map(|uv| uv / 1000),
        gauge: read_num::<u8>(&bat.join("capacity")).filter(|c| *c <= 100),
        tte_s: read_num(&bat.join("time_to_empty_now")),
        ttf_s: read_num(&bat.join("time_to_full_now")),
        online: usb.is_some_and(|u| read(&u.join("online")).as_deref() == Some("1")),
    }
}

// ---------------------------------------------------------------- shell ----

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Toast in every running shell (/run/user/*/mortar-shell.sock). Returns
/// whether one took it.
fn toast(title: &str, sub: &str, sound: Option<&str>, haptic: &str, secs: f32) -> bool {
    let mut j = format!("{{\"title\":{},\"sub\":{},\"secs\":{secs},\"haptic\":{}", json_str(title), json_str(sub), json_str(haptic));
    if let Some(s) = sound {
        j.push_str(&format!(",\"sound\":{}", json_str(s)));
    }
    j.push('}');
    let mut ok = false;
    let Ok(rd) = fs::read_dir("/run/user") else { return false };
    for e in rd.flatten() {
        let sock = e.path().join("mortar-shell.sock");
        let Ok(mut s) = UnixStream::connect(&sock) else { continue };
        let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
        let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
        if s.write_all(format!("toast {j}\n").as_bytes()).is_err() {
            continue;
        }
        let mut reply = String::new();
        if BufReader::new(&s).read_line(&mut reply).is_ok() && reply.trim() == "ok" {
            ok = true;
        } else {
            eprintln!("{}: {}", sock.display(), reply.trim());
        }
    }
    ok
}

fn rumble(o: &Opts, ms: u32, times: u32) {
    if let Some(r) = &o.rumble {
        let _ = Command::new(r).arg(ms.to_string()).arg(times.to_string()).status();
    }
}

/// Toast, or buzz when nothing shows it (greeter, a console session).
fn notify(o: &Opts, title: &str, sub: &str, sound: Option<&str>, haptic: &str, secs: f32, buzz: (u32, u32)) {
    eprintln!("{title}: {sub}");
    if !toast(title, sub, sound, haptic, secs) {
        rumble(o, buzz.0, buzz.1);
    }
}

fn minutes(s: u32) -> String {
    let m = s.div_ceil(60);
    if m >= 60 { format!("{} h {:02} min", m / 60, m % 60) } else { format!("{m} min") }
}

// ---------------------------------------------------------------- state ----

#[derive(Debug, Default)]
struct State {
    capacity: Option<u8>,
    source: &'static str,
    status: String,
    charging: bool,
    plugged: bool,
    voltage_mv: Option<u32>,
    tte_s: Option<u32>,
    ttf_s: Option<u32>,
    level: &'static str,
    input_current_ua: Option<u32>,
}

fn opt<T: ToString>(v: Option<T>) -> String {
    v.map(|v| v.to_string()).unwrap_or_default()
}

fn write_state(path: &Path, s: &State) {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let text = format!(
        "capacity={}\nsource={}\nstatus={}\ncharging={}\nplugged={}\nvoltage_mv={}\ntime_to_empty_s={}\ntime_to_full_s={}\nlevel={}\ninput_current_ua={}\nupdated={now}\n",
        opt(s.capacity),
        s.source,
        s.status,
        s.charging as u8,
        s.plugged as u8,
        opt(s.voltage_mv),
        opt(s.tte_s),
        opt(s.ttf_s),
        s.level,
        opt(s.input_current_ua),
    );
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, text).and_then(|_| fs::rename(&tmp, path)).is_err() {
        eprintln!("{}: cannot write", path.display());
    }
}

fn status() {
    let p = Path::new("/run/mortar-power/state");
    match fs::read_to_string(p) {
        Ok(t) => print!("{t}"),
        Err(_) => {
            let (bat, usb) = supplies();
            match bat {
                Some(b) => println!("{:?}", sample(&b, usb.as_deref())),
                None => println!("no battery"),
            }
        }
    }
}

// ----------------------------------------------------------------- main ----

fn main() {
    let o = parse_args();
    let start = Instant::now();
    let mut usb_policy = UsbPolicy::default();
    let mut battery = Battery::new(o.thresholds);
    let mut est = Estimator::default();
    let mut written_limit: Option<u32> = None;
    let mut poweroff_at: Option<Instant> = None;
    let mut last_battery = Instant::now() - Duration::from_secs(3600);
    let mut last_online: Option<bool> = None;
    eprintln!(
        "mortar-power: warn {}%/{}%, power off at {}% (<= {} mV) or below {} mV{}",
        o.thresholds.low,
        o.thresholds.critical,
        o.thresholds.off,
        o.thresholds.off_confirm_mv,
        o.thresholds.floor_mv,
        if o.dry_run { ", dry run" } else { "" }
    );

    loop {
        let (bat, usb) = supplies();
        let now_ms = start.elapsed().as_millis() as u64;

        // USB input current (every 2 s: the policy has a 5 s window)
        let online = usb.as_ref().is_some_and(|u| read(&u.join("online")).as_deref() == Some("1"));
        let mut input_current_ua = None;
        if let Some(u) = &usb {
            let limit = usb_policy.update(online, gadget_configured(), now_ms);
            if o.adapter_ua > 0 {
                let want = if limit == UsbLimit::Adapter { o.adapter_ua } else { o.host_ua };
                let f = u.join("input_current_limit");
                let have: Option<u32> = read_num(&f);
                // the driver rounds down to its 50 mA steps
                if have.is_none_or(|h| h.abs_diff(want) >= 50_000) {
                    let r = fs::write(&f, want.to_string());
                    if written_limit != Some(want) {
                        match &r {
                            Ok(()) => eprintln!(
                                "usb input limit {} mA ({})",
                                want / 1000,
                                if limit == UsbLimit::Adapter { "charger" } else { "host, or deciding" }
                            ),
                            Err(e) => eprintln!("{}: {e}", f.display()),
                        }
                        written_limit = Some(want);
                    }
                }
                input_current_ua = read_num(&f);
            }
        }
        let plug_changed = last_online.is_some_and(|l| l != online);
        last_online = Some(online);

        // battery: every 10 s, and at once when the cable changes
        let due = last_battery.elapsed() >= Duration::from_secs(10) || plug_changed;
        if let (Some(b), true) = (&bat, due) {
            last_battery = Instant::now();
            let s = sample(b, usb.as_deref());
            let flow = match s.status.as_str() {
                "Charging" => Flow::Charging,
                "Discharging" => Flow::Discharging,
                _ if !s.online => Flow::Discharging,
                _ => Flow::Idle,
            };
            let (capacity, source) = match (s.gauge, s.voltage_mv) {
                (Some(c), _) => (Some(c), "gauge"),
                (None, Some(mv)) if s.present => (Some(est.update(mv, flow)), "voltage"),
                _ => (None, "none"),
            };
            let on_battery = s.present && flow == Flow::Discharging && !s.online;
            let events = if s.present {
                battery.update(Reading { capacity, voltage_mv: s.voltage_mv, on_battery })
            } else {
                Vec::new()
            };
            for e in events {
                match e {
                    Event::Low(c) => {
                        let left = s.tte_s.map(|t| format!(" · about {} left", minutes(t))).unwrap_or_default();
                        notify(&o, "Battery low", &format!("{c}%{left} · plug in soon"), Some("chime"), "double", 4.0, (250, 2));
                    }
                    Event::Critical(c) => {
                        let sub = if capacity.is_some() {
                            format!("{c}% · plug in now, Mortar powers off at {}%", o.thresholds.off)
                        } else {
                            "Plug in now, Mortar powers off soon".to_string()
                        };
                        notify(&o, "Battery critical", &sub, Some("deny"), "long", 6.0, (500, 3));
                    }
                    Event::PowerOff => {
                        notify(&o, "Battery empty", "Powering off to protect the SD card", Some("deny"), "long", o.grace_s as f32, (800, 1));
                        poweroff_at = Some(Instant::now() + Duration::from_secs(o.grace_s));
                    }
                    Event::Cancelled => {
                        poweroff_at = None;
                        notify(&o, "Charging", "Power-off cancelled", Some("tick"), "tick", 3.0, (150, 1));
                    }
                    Event::Plugged => {
                        let full = s.ttf_s.map(|t| format!(" · full in about {}", minutes(t))).unwrap_or_default();
                        let sub = capacity.map(|c| format!("{c}%{full}")).unwrap_or_default();
                        let _ = toast("Charging", &sub, None, "tick", 2.6);
                    }
                }
            }
            write_state(
                &o.state,
                &State {
                    capacity: if s.present { capacity } else { None },
                    source,
                    status: s.status.clone(),
                    charging: s.status == "Charging",
                    plugged: s.online || s.status == "Charging" || s.status == "Full",
                    voltage_mv: s.voltage_mv,
                    tte_s: s.tte_s,
                    ttf_s: s.ttf_s,
                    level: if s.present { battery.level.name() } else { Level::Ok.name() },
                    input_current_ua,
                },
            );
        }

        if let Some(at) = poweroff_at
            && Instant::now() >= at
        {
            poweroff_at = None;
            // last look: never power off on external power
            let s = bat.as_ref().map(|b| sample(b, usb.as_deref()));
            if s.is_some_and(|s| !s.online && s.status != "Charging" && s.status != "Full") {
                if o.dry_run {
                    eprintln!("dry run: would run systemctl poweroff");
                } else {
                    eprintln!("battery empty: systemctl poweroff");
                    match Command::new("systemctl").arg("poweroff").status() {
                        Ok(st) if st.success() => {}
                        r => {
                            eprintln!("systemctl poweroff: {r:?}; retrying in 30 s");
                            poweroff_at = Some(Instant::now() + Duration::from_secs(30));
                        }
                    }
                }
            }
        }

        std::thread::sleep(Duration::from_secs(2));
    }
}
