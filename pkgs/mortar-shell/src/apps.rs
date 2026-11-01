// SPDX-License-Identifier: GPL-2.0-or-later
//! Running apps: launch, map Wayland clients to the app that started them
//! (by process ancestry), focus order, close and reap.
//!
//! Generic over the window type so the bookkeeping is testable without a
//! compositor.

use crate::config::{AppEntry, GamepadMode};
use mortar_ui::icons::Icon;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// Virtual-gamepad slots (see pads.rs): one per running app.
pub const MAX_SLOTS: u8 = 16;

pub struct App<W> {
    /// Instance id, unique for the shell's lifetime.
    pub uid: u32,
    /// Launcher entry id, or None for a client started outside the shell.
    pub entry_id: Option<String>,
    pub name: String,
    pub icon: Icon,
    pub gamepad: GamepadMode,
    pub slot: u8,
    pub child: Option<Child>,
    pub pid: Option<u32>,
    pub windows: Vec<W>,
    /// Close requested at this time (escalates to SIGTERM / SIGKILL).
    pub closing: Option<Instant>,
}

impl<W> App<W> {
    pub fn alive(&self) -> bool {
        self.child.is_some() || !self.windows.is_empty()
    }
}

pub struct Apps<W> {
    pub list: Vec<App<W>>,
    /// Most recently focused first (uids).
    pub mru: Vec<u32>,
    next_uid: u32,
}

impl<W> Default for Apps<W> {
    fn default() -> Self {
        Apps { list: Vec::new(), mru: Vec::new(), next_uid: 1 }
    }
}

/// Environment every app gets on top of the shell's own.
pub struct LaunchEnv<'a> {
    pub wayland_display: &'a str,
    pub extra: Vec<(String, String)>,
}

impl<W> Apps<W> {
    pub fn get(&self, uid: u32) -> Option<&App<W>> {
        self.list.iter().find(|a| a.uid == uid)
    }
    pub fn get_mut(&mut self, uid: u32) -> Option<&mut App<W>> {
        self.list.iter_mut().find(|a| a.uid == uid)
    }
    pub fn by_entry(&self, id: &str) -> Option<&App<W>> {
        self.list.iter().find(|a| a.entry_id.as_deref() == Some(id))
    }
    /// The visible app.
    pub fn focused(&self) -> Option<u32> {
        self.mru.first().copied()
    }
    pub fn focus(&mut self, uid: u32) {
        self.mru.retain(|u| *u != uid);
        self.mru.insert(0, uid);
    }
    /// Apps in switcher order (most recent first, then the rest).
    pub fn ordered(&self) -> Vec<u32> {
        let mut v = self.mru.clone();
        for a in &self.list {
            if !v.contains(&a.uid) {
                v.push(a.uid);
            }
        }
        v
    }

    fn free_slot(&self) -> Option<u8> {
        (0..MAX_SLOTS).find(|s| !self.list.iter().any(|a| a.slot == *s))
    }

    /// Register an app (launched or external) and return its uid.
    pub fn add(&mut self, entry_id: Option<String>, name: String, icon: Icon, gamepad: GamepadMode) -> Option<u32> {
        let slot = self.free_slot()?;
        let uid = self.next_uid;
        self.next_uid += 1;
        self.list.push(App {
            uid,
            entry_id,
            name,
            icon,
            gamepad,
            slot,
            child: None,
            pid: None,
            windows: Vec::new(),
            closing: None,
        });
        Some(uid)
    }

    /// Start a launcher entry. `slot_env` gives per-slot environment (the
    /// gamepad blacklist). Returns the new uid.
    pub fn launch(
        &mut self,
        e: &AppEntry,
        env: &LaunchEnv,
        slot_env: impl Fn(u8) -> Vec<(String, String)>,
    ) -> Result<u32, String> {
        let uid = self.add(Some(e.id.clone()), e.name.clone(), e.icon, e.gamepad).ok_or("too many apps running")?;
        let slot = self.get(uid).unwrap().slot;
        let mut cmd = Command::new(&e.command[0]);
        cmd.args(&e.command[1..]);
        cmd.env("WAYLAND_DISPLAY", env.wayland_display);
        cmd.env("SDL_VIDEO_DRIVER", "wayland");
        cmd.env("XDG_SESSION_TYPE", "wayland");
        cmd.env("MORTAR_APP", &e.id);
        cmd.env_remove("DISPLAY");
        for (k, v) in env.extra.iter().cloned().chain(slot_env(slot)) {
            cmd.env(k, v);
        }
        for (k, v) in &e.env {
            cmd.env(k, v);
        }
        // own process group, so closing kills helpers too
        unsafe {
            cmd.pre_exec(|| {
                libc::setpgid(0, 0);
                Ok(())
            });
        }
        match cmd.spawn() {
            Ok(child) => {
                let a = self.get_mut(uid).unwrap();
                a.pid = Some(child.id());
                a.child = Some(child);
                Ok(uid)
            }
            Err(err) => {
                self.list.retain(|a| a.uid != uid);
                Err(format!("{}: {err}", e.command[0]))
            }
        }
    }

    /// Which app a client process belongs to: the app whose launched pid is
    /// the process itself or one of its ancestors.
    pub fn for_pid(&self, pid: u32, parent_of: impl Fn(u32) -> Option<u32>) -> Option<u32> {
        let mut p = pid;
        for _ in 0..64 {
            if let Some(a) = self.list.iter().find(|a| a.pid == Some(p)) {
                return Some(a.uid);
            }
            match parent_of(p) {
                Some(pp) if pp > 1 && pp != p => p = pp,
                _ => return None,
            }
        }
        None
    }

    /// Ask an app to close: `close_windows` sends xdg_toplevel.close; the
    /// process group gets SIGTERM after 3 s and SIGKILL after 6 s (see
    /// `tick`).
    pub fn close(&mut self, uid: u32, close_windows: impl Fn(&W)) {
        if let Some(a) = self.get_mut(uid) {
            for w in &a.windows {
                close_windows(w);
            }
            if a.windows.is_empty() {
                signal(a, libc::SIGTERM);
            }
            a.closing.get_or_insert(Instant::now());
        }
    }

    /// Reap exited children, escalate pending closes, drop apps that have
    /// neither a process nor a window. Returns the uids removed.
    pub fn tick(&mut self) -> Vec<u32> {
        for a in &mut self.list {
            if let Some(c) = &mut a.child
                && let Ok(Some(st)) = c.try_wait()
            {
                tracing::info!(app = %a.name, status = %st, "app exited");
                a.child = None;
            }
            if let Some(t) = a.closing {
                let el = t.elapsed();
                if el > Duration::from_secs(6) {
                    signal(a, libc::SIGKILL);
                } else if el > Duration::from_secs(3) {
                    signal(a, libc::SIGTERM);
                }
            }
        }
        let gone: Vec<u32> = self.list.iter().filter(|a| !a.alive()).map(|a| a.uid).collect();
        self.list.retain(|a| a.alive());
        self.mru.retain(|u| !gone.contains(u));
        gone
    }

    /// SIGTERM every app (shell exit).
    pub fn terminate_all(&mut self) {
        for a in &mut self.list {
            signal(a, libc::SIGTERM);
        }
    }
}

fn signal(a: &App<impl Sized>, sig: i32) {
    if let (Some(pid), Some(_)) = (a.pid, &a.child) {
        unsafe {
            // the process group (setpgid in launch); fall back to the pid
            if libc::kill(-(pid as i32), sig) != 0 {
                libc::kill(pid as i32, sig);
            }
        }
    }
}

/// Parent pid from /proc/<pid>/stat.
pub fn proc_parent(pid: u32) -> Option<u32> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_stat_ppid(&s)
}

/// The 4th field of /proc/<pid>/stat, after the parenthesised command
/// (which may itself contain spaces and parentheses).
pub fn parse_stat_ppid(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_ppid() {
        assert_eq!(parse_stat_ppid("1234 (foot (x) y) S 77 1234 1234 0"), Some(77));
        assert_eq!(parse_stat_ppid("garbage"), None);
    }

    #[test]
    fn ancestry_and_focus() {
        let mut apps: Apps<()> = Apps::default();
        let a = apps.add(Some("m8".into()), "M8".into(), Icon::Pad, GamepadMode::Forward).unwrap();
        let b = apps.add(Some("term".into()), "Terminal".into(), Icon::Term, GamepadMode::Keyboard).unwrap();
        apps.get_mut(a).unwrap().pid = Some(100);
        apps.get_mut(b).unwrap().pid = Some(200);
        // 300 -> 250 -> 200 (terminal's shell spawned a client)
        let parent = |p: u32| match p {
            300 => Some(250),
            250 => Some(200),
            _ => Some(1),
        };
        assert_eq!(apps.for_pid(300, parent), Some(b));
        assert_eq!(apps.for_pid(100, parent), Some(a));
        assert_eq!(apps.for_pid(999, parent), None);
        assert_ne!(apps.get(a).unwrap().slot, apps.get(b).unwrap().slot);

        apps.focus(a);
        apps.focus(b);
        assert_eq!(apps.focused(), Some(b));
        assert_eq!(apps.ordered(), vec![b, a]);
        apps.focus(a);
        assert_eq!(apps.ordered(), vec![a, b]);
    }

    #[test]
    fn dead_apps_are_dropped() {
        let mut apps: Apps<u8> = Apps::default();
        let a = apps.add(None, "ext".into(), Icon::Pad, GamepadMode::None).unwrap();
        apps.get_mut(a).unwrap().windows.push(1);
        apps.focus(a);
        assert!(apps.tick().is_empty());
        apps.get_mut(a).unwrap().windows.clear();
        assert_eq!(apps.tick(), vec![a]);
        assert_eq!(apps.focused(), None);
    }
}
