// SPDX-License-Identifier: GPL-2.0-or-later
//! Input devices, read directly with evdev (no libinput: the Brick has
//! buttons, not pointers, and libinput ignores gamepads anyway).
//!
//! Devices are opened through the session (logind/seatd via libseat, which
//! revokes them on VT switch) or directly (headless with --input direct),
//! found by scanning /dev/input every 2 s (no udev dependency), and sorted:
//!   pad       any gamepad buttons: grabbed (EVIOCGRAB) and routed by the
//!             shell (router.rs), cloned per app (pads.rs)
//!   keyboard  letter keys: sent to the visible app over wl_keyboard
//!   keys      anything else with keys (volume, power): volume handled by
//!             the shell, the rest ignored (logind handles power)

use super::state::Shell;
use evdev::{Device, EventType, KeyCode};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, Mode, PostAction, RegistrationToken};
use std::collections::{HashMap, HashSet};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};

pub trait Opener {
    fn open(&mut self, path: &Path) -> io::Result<OwnedFd>;
    fn close(&mut self, fd: OwnedFd) {
        drop(fd);
    }
}

pub struct DirectOpener;
impl Opener for DirectOpener {
    fn open(&mut self, path: &Path) -> io::Result<OwnedFd> {
        use std::os::unix::fs::OpenOptionsExt;
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)?;
        Ok(f.into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Pad(u16),
    Keyboard,
    Keys,
}

struct Open {
    kind: Kind,
    /// The session's fd (closed through the opener); the Device holds a dup.
    fd: OwnedFd,
    token: RegistrationToken,
}

pub struct Inputs {
    pub opener: Box<dyn Opener>,
    open: HashMap<PathBuf, Open>,
    ignored: HashSet<PathBuf>,
    /// Devices are not read (session paused, or headless without input).
    pub enabled: bool,
}

pub fn classify(dev: &Device) -> Option<Kind> {
    let keys = dev.supported_keys()?;
    let has = |c: u16| keys.contains(KeyCode::new(c));
    let pad = (0x120..0x140).any(has) || (0x220..0x224).any(has) || (0x2c0..0x2c8).any(has);
    if pad {
        return Some(Kind::Pad(0));
    }
    if has(30) && has(44) && has(28) {
        return Some(Kind::Keyboard);
    }
    if keys.iter().next().is_some() {
        return Some(Kind::Keys);
    }
    None
}

impl Inputs {
    pub fn new(opener: Box<dyn Opener>, enabled: bool) -> Inputs {
        Inputs { opener, open: HashMap::new(), ignored: HashSet::new(), enabled }
    }

    pub fn summary(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> =
            self.open.iter().map(|(p, o)| (p.display().to_string(), format!("{:?}", o.kind))).collect();
        v.sort();
        v
    }
}

/// Open any new /dev/input/event* devices and drop vanished ones.
pub fn scan(shell: &mut Shell) {
    if !shell.inputs.enabled {
        return;
    }
    let Ok(rd) = std::fs::read_dir("/dev/input") else { return };
    let mut present = HashSet::new();
    for e in rd.flatten() {
        let path = e.path();
        if !path.file_name().is_some_and(|n| n.to_string_lossy().starts_with("event")) {
            continue;
        }
        present.insert(path.clone());
        if shell.inputs.open.contains_key(&path) || shell.inputs.ignored.contains(&path) {
            continue;
        }
        if let Err(err) = open_device(shell, &path) {
            tracing::debug!(path = %path.display(), "input: {err}");
            shell.inputs.ignored.insert(path);
        }
    }
    shell.inputs.ignored.retain(|p| present.contains(p));
    let gone: Vec<PathBuf> = shell.inputs.open.keys().filter(|p| !present.contains(*p)).cloned().collect();
    for p in gone {
        close_device(shell, &p);
    }
}

fn open_device(shell: &mut Shell, path: &Path) -> io::Result<()> {
    let fd = shell.inputs.opener.open(path)?;
    let dup = fd.try_clone()?;
    let mut dev = match Device::from_fd(dup) {
        Ok(d) => d,
        Err(e) => {
            shell.inputs.opener.close(fd);
            return Err(e);
        }
    };
    let Some(mut kind) = classify(&dev) else {
        drop(dev);
        shell.inputs.opener.close(fd);
        return Err(io::Error::other("not a key device"));
    };
    // Our own virtual pads must not be picked up as physical ones.
    if dev.input_id().vendor() == super::pads::VENDOR && dev.input_id().bus_type() == evdev::BusType::BUS_VIRTUAL {
        drop(dev);
        shell.inputs.opener.close(fd);
        return Err(io::Error::other("mortar virtual pad"));
    }
    let _ = dev.set_nonblocking(true);
    if let Kind::Pad(_) = kind {
        if let Err(e) = dev.grab() {
            tracing::warn!(path = %path.display(), "EVIOCGRAB failed: {e}; apps may see the pad directly");
        }
        kind = Kind::Pad(shell.pads.add_phys(&dev));
    }
    tracing::info!(path = %path.display(), name = dev.name().unwrap_or("?"), ?kind, "input device");
    let p = path.to_path_buf();
    let token = shell
        .loop_handle
        .insert_source(Generic::new(dev, Interest::READ, Mode::Level), move |_, dev, shell: &mut Shell| {
            // SAFETY: the device is not dropped while borrowed here
            let dev = unsafe { dev.get_mut() };
            let evs: Vec<evdev::InputEvent> = match dev.fetch_events() {
                Ok(it) => it.collect(),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(PostAction::Continue),
                Err(e) => {
                    tracing::info!(path = %p.display(), "input device gone: {e}");
                    shell.inputs_removed.push(p.clone());
                    return Ok(PostAction::Disable);
                }
            };
            for ev in evs {
                match ev.event_type() {
                    EventType::KEY => shell.on_key(kind, ev.code(), ev.value()),
                    EventType::ABSOLUTE => shell.on_abs(kind, ev.code(), ev.value()),
                    _ => {}
                }
            }
            Ok(PostAction::Continue)
        })
        .map_err(|e| io::Error::other(e.to_string()))?;
    let raw = fd.as_raw_fd();
    tracing::trace!(raw, "opened");
    shell.inputs.open.insert(path.to_path_buf(), Open { kind, fd, token });
    Ok(())
}

pub fn close_device(shell: &mut Shell, path: &Path) {
    if let Some(o) = shell.inputs.open.remove(path) {
        shell.loop_handle.remove(o.token);
        if let Kind::Pad(id) = o.kind {
            shell.pads.remove_phys(id);
        }
        shell.inputs.opener.close(o.fd);
    }
}

/// Close everything (session paused: logind revoked the fds anyway).
pub fn close_all(shell: &mut Shell) {
    let paths: Vec<PathBuf> = shell.inputs.open.keys().cloned().collect();
    for p in paths {
        close_device(shell, &p);
    }
    shell.inputs.ignored.clear();
}
