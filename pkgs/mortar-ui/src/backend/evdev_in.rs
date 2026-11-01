// SPDX-License-Identifier: GPL-2.0-or-later
//! evdev input: every /dev/input/event* device with keys or a hat (Brick
//! gamepad, volume keys, PMIC power key, USB/Bluetooth pads and keyboards),
//! read directly and mapped by `input::map_evdev_key`. New devices (a
//! Bluetooth keyboard connecting) are picked up by rescanning.

use crate::input::{map_evdev_char, map_evdev_key, Button, Event};
use evdev::{Device, EventType};
use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const ABS_HAT0X: u16 = 0x10;
const ABS_HAT0Y: u16 = 0x11;
const RESCAN: Duration = Duration::from_secs(2);

pub struct Inputs {
    devs: HashMap<PathBuf, Device>,
    last_scan: Instant,
    hat: HashMap<(PathBuf, u16), i32>,
}

impl Inputs {
    pub fn open() -> Inputs {
        let mut i = Inputs { devs: HashMap::new(), last_scan: Instant::now(), hat: HashMap::new() };
        i.rescan();
        i
    }

    fn rescan(&mut self) {
        self.last_scan = Instant::now();
        for (path, dev) in evdev::enumerate() {
            if self.devs.contains_key(&path) {
                continue;
            }
            let has_keys = dev.supported_keys().is_some_and(|k| k.iter().next().is_some());
            let has_hat = dev.supported_absolute_axes().is_some_and(|a| a.iter().any(|x| x.0 == ABS_HAT0X));
            if !(has_keys || has_hat) {
                continue;
            }
            if dev.set_nonblocking(true).is_err() {
                continue;
            }
            if std::env::var_os("MORTAR_UI_DEBUG").is_some() {
                eprintln!("input: {} {:?}", path.display(), dev.name());
            }
            self.devs.insert(path, dev);
        }
    }

    pub fn wait(&mut self, timeout: Option<Duration>, out: &mut Vec<Event>) {
        let t = timeout.unwrap_or(RESCAN).min(RESCAN);
        let paths: Vec<PathBuf> = self.devs.keys().cloned().collect();
        let mut fds: Vec<libc::pollfd> = paths
            .iter()
            .map(|p| libc::pollfd { fd: self.devs[p].as_raw_fd(), events: libc::POLLIN, revents: 0 })
            .collect();
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, t.as_millis() as i32) };
        if n > 0 {
            for (fd, path) in fds.iter().zip(&paths) {
                if fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                    self.devs.remove(path); // unplugged
                    continue;
                }
                if fd.revents & libc::POLLIN == 0 {
                    continue;
                }
                let Some(dev) = self.devs.get_mut(path) else { continue };
                let res = dev.fetch_events().map(|it| it.collect::<Vec<_>>()).map_err(|e| e.kind());
                let evs = match res {
                    Ok(v) => v,
                    Err(std::io::ErrorKind::WouldBlock) => continue,
                    Err(_) => {
                        self.devs.remove(path);
                        continue;
                    }
                };
                for ev in evs {
                    match ev.event_type() {
                        EventType::KEY => {
                            let (code, v) = (ev.code(), ev.value());
                            if v == 2 {
                                continue; // kernel repeat; we do our own
                            }
                            if v == 1 {
                                if let Some(c) = map_evdev_char(code) {
                                    out.push(Event::Char(c));
                                    continue;
                                }
                            }
                            if let Some(b) = map_evdev_key(code) {
                                out.push(if v == 1 { Event::Press { button: b, repeat: false } } else { Event::Release(b) });
                            }
                        }
                        EventType::ABSOLUTE if ev.code() == ABS_HAT0X || ev.code() == ABS_HAT0Y => {
                            let key = (path.clone(), ev.code());
                            let old = self.hat.insert(key, ev.value()).unwrap_or(0);
                            let (neg, pos) = if ev.code() == ABS_HAT0X {
                                (Button::Left, Button::Right)
                            } else {
                                (Button::Up, Button::Down)
                            };
                            let btn = |v: i32| if v < 0 { Some(neg) } else if v > 0 { Some(pos) } else { None };
                            if let Some(b) = btn(old) {
                                out.push(Event::Release(b));
                            }
                            if let Some(b) = btn(ev.value()) {
                                out.push(Event::Press { button: b, repeat: false });
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        if self.last_scan.elapsed() >= RESCAN {
            self.rescan();
        }
    }
}
