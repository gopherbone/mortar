// SPDX-License-Identifier: GPL-2.0-or-later
//! Says when to look at the app profiles again (discover.rs): one inotify
//! watch on the apps directory, in the event loop, so no timers and no
//! wakeups while nothing is being installed.
//!
//! One directory is enough: a profile is a symlink swapped by rename, its
//! target an immutable store path, so every change is an entry coming or going
//! in that directory. While the directory does not exist (nothing installed
//! yet) the nearest parent that does is watched instead.

use super::state::Shell;
use crate::discover::is_profile_name;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, LoopHandle, Mode, PostAction};
use std::cell::Cell;
use std::ffi::CString;
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

const TARGET_MASK: u32 = libc::IN_CREATE | libc::IN_MOVED_TO | libc::IN_MOVED_FROM | libc::IN_DELETE | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_ONLYDIR;
const PARENT_MASK: u32 = libc::IN_CREATE | libc::IN_MOVED_TO | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_ONLYDIR;

pub struct Watch {
    fd: OwnedFd,
    target: PathBuf,
    wd: Cell<i32>,
    /// `wd` is on the target itself, not a parent.
    on_target: Cell<bool>,
}

impl AsFd for Watch {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

impl Watch {
    pub fn new(target: &Path) -> io::Result<Watch> {
        // SAFETY: plain syscall; CLOEXEC so apps do not inherit it
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a new fd, ours alone
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let w = Watch { fd, target: target.to_path_buf(), wd: Cell::new(-1), on_target: Cell::new(false) };
        w.arm()?;
        Ok(w)
    }

    fn add(&self, path: &Path, mask: u32) -> io::Result<i32> {
        let c = CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)?;
        // SAFETY: valid fd and C string
        let wd = unsafe { libc::inotify_add_watch(self.fd.as_raw_fd(), c.as_ptr(), mask) };
        if wd < 0 { Err(io::Error::last_os_error()) } else { Ok(wd) }
    }

    /// Watch the target, or the nearest parent that exists.
    fn arm(&self) -> io::Result<()> {
        loop {
            let mut placed = None;
            for dir in self.target.ancestors().filter(|d| !d.as_os_str().is_empty()) {
                let on_target = dir == self.target;
                match self.add(dir, if on_target { TARGET_MASK } else { PARENT_MASK }) {
                    Ok(wd) => {
                        placed = Some((wd, dir, on_target));
                        break;
                    }
                    Err(e) if matches!(e.raw_os_error(), Some(libc::ENOENT | libc::ENOTDIR)) => continue,
                    Err(e) => return Err(e),
                }
            }
            let Some((wd, dir, on_target)) = placed else {
                return Err(io::Error::from(io::ErrorKind::NotFound));
            };
            let old = self.wd.replace(wd);
            self.on_target.set(on_target);
            if old >= 0 && old != wd {
                // SAFETY: valid fd; the old wd may be gone already, which is harmless
                unsafe { libc::inotify_rm_watch(self.fd.as_raw_fd(), old) };
            }
            // Something below appeared while this was being placed? Then no
            // event will come for it: go on down.
            let deeper = !on_target && self.target.ancestors().take_while(|d| *d != dir).any(|d| d.is_dir());
            if !deeper {
                return Ok(());
            }
        }
    }

    /// Read what is pending; true if the profiles may have changed.
    pub fn drain(&self) -> bool {
        let mut changed = false;
        let mut rearm = false;
        // aligned for inotify_event
        let mut buf = [0u64; 512];
        loop {
            // SAFETY: the buffer is valid for its size
            let n = unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), std::mem::size_of_val(&buf)) };
            if n <= 0 {
                break;
            }
            // SAFETY: the kernel wrote n initialized bytes; u64 to u8 is fine
            let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), n as usize) };
            for (wd, mask, name) in events(bytes) {
                if mask & libc::IN_Q_OVERFLOW != 0 {
                    changed = true;
                } else if wd != self.wd.get() {
                    // the watch this replaced
                } else if mask & (libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_IGNORED) != 0 {
                    rearm = true;
                } else if !self.on_target.get() {
                    rearm = true;
                } else if name.is_some_and(is_profile_name) {
                    changed = true;
                }
            }
        }
        if rearm {
            if let Err(e) = self.arm() {
                tracing::warn!("app profiles: {}: {e}; no more live updates", self.target.display());
            }
            changed = true;
        }
        changed
    }
}

/// (watch, mask, name) of each event in what read() returned.
fn events(mut b: &[u8]) -> Vec<(i32, u32, Option<&str>)> {
    const HEADER: usize = std::mem::size_of::<libc::inotify_event>();
    let mut v = Vec::new();
    while b.len() >= HEADER {
        let word = |at: usize| u32::from_ne_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
        let (wd, mask, len) = (word(0) as i32, word(4), word(12) as usize);
        let Some(raw) = b.get(HEADER..HEADER + len) else { break };
        let end = raw.iter().position(|c| *c == 0).unwrap_or(raw.len());
        v.push((wd, mask, if end > 0 { std::str::from_utf8(&raw[..end]).ok() } else { None }));
        b = &b[HEADER + len..];
    }
    v
}

/// Scan once and keep watching, if the config has an apps directory. Nothing
/// here can stop the shell: without a watch it has the first scan's apps.
pub fn start(shell: &mut Shell, handle: &LoopHandle<'static, Shell>) {
    let Some(dir) = shell.cfg.app_profiles.clone() else { return };
    shell.scanner = Some(crate::discover::Scanner::new(dir.clone(), shell.cfg.sdl_run.clone()));
    // the watch first, so nothing installed during the scan is missed
    let source = Watch::new(&dir).map_err(|e| e.to_string()).and_then(|w| {
        handle
            .insert_source(Generic::new(w, Interest::READ, Mode::Level), |_, w, shell: &mut Shell| {
                if w.drain() {
                    shell.rediscover();
                }
                Ok(PostAction::Continue)
            })
            .map_err(|e| e.to_string())
    });
    if let Err(e) = source {
        tracing::warn!("app profiles: {}: {e}; apps found now stay until the shell restarts", dir.display());
    }
    shell.rediscover();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mortar-appwatch-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn profile_coming_and_going() {
        let d = scratch("direct");
        let w = Watch::new(&d).unwrap();
        assert!(w.on_target.get());
        assert!(!w.drain(), "nothing yet");

        // what nix-env does: a generation link, then the profile by rename
        symlink("/nonexistent", d.join("app-1-link")).unwrap();
        assert!(!w.drain(), "a generation is not a change");
        symlink("app-1-link", d.join("app.tmp-1")).unwrap();
        assert!(!w.drain(), "nor the temporary name");
        fs::rename(d.join("app.tmp-1"), d.join("app")).unwrap();
        assert!(w.drain(), "the swap is");
        // swapping over an existing link
        symlink("app-1-link", d.join("app.tmp-2")).unwrap();
        fs::rename(d.join("app.tmp-2"), d.join("app")).unwrap();
        assert!(w.drain());
        fs::remove_file(d.join("app")).unwrap();
        assert!(w.drain(), "removal");
        assert!(!w.drain(), "and quiet again");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_directory_may_come_later_and_go_again() {
        let d = scratch("later");
        let target = d.join("profiles/apps");
        let w = Watch::new(&target).unwrap();
        assert!(!w.on_target.get(), "on a parent");

        fs::create_dir_all(&target).unwrap();
        assert!(w.drain(), "the new directory may already hold profiles");
        assert!(w.on_target.get(), "down to the target, through both levels");
        symlink("/nonexistent", target.join("app")).unwrap();
        assert!(w.drain(), "and it sees what happens there");

        fs::remove_dir_all(d.join("profiles")).unwrap();
        assert!(w.drain());
        assert!(!w.on_target.get(), "back on a parent");
        fs::create_dir_all(&target).unwrap();
        assert!(w.drain());
        assert!(w.on_target.get());
        symlink("/nonexistent", target.join("app")).unwrap();
        assert!(w.drain());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn nothing_is_read_while_quiet() {
        let d = scratch("quiet");
        let w = Watch::new(&d).unwrap();
        // files inside a profile are store paths: never watched, never a wakeup
        fs::create_dir_all(d.join("app/share")).unwrap();
        assert!(w.drain());
        fs::write(d.join("app/share/x"), "x").unwrap();
        assert!(!w.drain());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn event_parsing() {
        let mut b = Vec::new();
        for (wd, mask, name) in [(1i32, libc::IN_CREATE, &b"app\0\0\0\0\0"[..]), (2, libc::IN_IGNORED, &b""[..])] {
            b.extend(wd.to_ne_bytes());
            b.extend(mask.to_ne_bytes());
            b.extend(0u32.to_ne_bytes());
            b.extend((name.len() as u32).to_ne_bytes());
            b.extend(name);
        }
        assert_eq!(events(&b), [(1, libc::IN_CREATE, Some("app")), (2, libc::IN_IGNORED, None)]);
        assert!(events(&b[..10]).is_empty(), "short: no panic");
    }
}
