// SPDX-License-Identifier: GPL-2.0-or-later
//! What the shell shows: `/run/mortar-update/status.json` (world-readable,
//! rewritten at each step), and the small persistent state in
//! `/var/lib/mortar-update/state.json`.

use crate::fsutil::{write_atomic, write_atomic_in};
use mortar_update::Manifest;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    /// S3 mode without credentials yet (docs/updates.md).
    Unconfigured,
    /// Not checked since boot.
    Idle,
    Checking,
    Downloading,
    /// Making it the next boot entry.
    Installing,
    /// Staged: restart to apply.
    Ready,
    UpToDate,
    /// No network; try again later.
    Offline,
    /// Battery below the minimum and not charging.
    LowBattery,
    Error,
}

/// The status file. `message` is short (the Settings row), `detail` longer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub state: Phase,
    pub message: String,
    #[serde(default)]
    pub detail: String,
    pub auto_check: bool,
    #[serde(default)]
    pub checked_at: Option<u64>,
    /// The update being fetched or staged (Downloading .. Ready).
    #[serde(default)]
    pub available: Option<Manifest>,
    #[serde(default)]
    pub running: Option<String>,
}

impl Status {
    pub fn new(state: Phase, message: impl Into<String>, auto_check: bool) -> Status {
        Status { state, message: message.into(), detail: String::new(), auto_check, checked_at: None, available: None, running: None }
    }
    pub fn detail(mut self, d: impl Into<String>) -> Status {
        self.detail = d.into();
        self
    }
    pub fn load(p: &Path) -> Option<Status> {
        serde_json::from_slice(&std::fs::read(p).ok()?).ok()
    }
    pub fn save(&self, p: &Path) {
        let j = serde_json::to_vec_pretty(self).expect("status serializes");
        if let Err(e) = write_atomic_in(p, &j, 0o644, 0o755) {
            eprintln!("mortar-update: {}: {e}", p.display());
        }
    }
}

/// Persistent: the Brick's own choices and the last manifest taken.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct State {
    /// "Auto-check daily" (None: the module's default).
    pub auto_check: Option<bool>,
    /// `published` of the last manifest staged or found running here: equal
    /// ones are not taken again, older ones are refused as downgrades.
    pub last_published: u64,
    pub last_toplevel: Option<String>,
    pub last_check: Option<u64>,
}

impl State {
    pub fn load(p: &Path) -> State {
        std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }
    pub fn save(&self, p: &Path) -> std::io::Result<()> {
        write_atomic(p, &serde_json::to_vec_pretty(self).expect("state serializes"), 0o644)
    }
}

/// What a fresh manifest means for this Brick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// It is what runs now.
    Running,
    /// Already the next boot entry.
    Staged,
    /// Not newer than the last manifest taken (e.g. the Brick has since
    /// been deployed to from a computer).
    NotNewer,
    /// Older than what this Brick has already taken: an old manifest put back
    /// (or a mistake). Taken only when asked for by name.
    Downgrade,
    Update,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Allow {
    /// `--force`: take the manifest last taken again.
    pub same: bool,
    /// `--allow-downgrade`
    pub downgrade: bool,
    /// `--allow-config-change`
    pub config_change: bool,
}

pub fn plan(m: &Manifest, running: Option<&str>, staged: Option<&str>, st: &State, allow: Allow) -> Plan {
    if running == Some(m.toplevel.as_str()) {
        Plan::Running
    } else if staged == Some(m.toplevel.as_str()) {
        Plan::Staged
    } else if m.published < st.last_published && !allow.downgrade {
        Plan::Downgrade
    } else if m.published == st.last_published && !allow.same && !allow.downgrade {
        Plan::NotNewer
    } else {
        Plan::Update
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(top: &str, published: u64) -> Manifest {
        Manifest {
            schema: 1,
            channel: "mortar".into(),
            system: "aarch64-linux".into(),
            config: "brick-mainline".into(),
            toplevel: top.into(),
            published,
            date: String::new(),
            rev: None,
            version: None,
            label: None,
            changelog: vec![],
            publisher: String::new(),
            signature: None,
        }
    }

    #[test]
    fn plans() {
        let st = State { last_published: 100, ..Default::default() };
        let no = Allow::default();
        let same = Allow { same: true, ..no };
        let down = Allow { downgrade: true, ..no };
        assert_eq!(plan(&m("/a", 200), Some("/a"), Some("/a"), &st, no), Plan::Running);
        assert_eq!(plan(&m("/b", 200), Some("/a"), Some("/b"), &st, no), Plan::Staged);
        assert_eq!(plan(&m("/b", 100), Some("/local"), Some("/local"), &st, no), Plan::NotNewer);
        assert_eq!(plan(&m("/b", 100), Some("/local"), Some("/local"), &st, same), Plan::Update);
        assert_eq!(plan(&m("/c", 101), Some("/a"), Some("/a"), &st, no), Plan::Update);
        assert_eq!(plan(&m("/c", 1), None, None, &State::default(), no), Plan::Update);
        // older than what was taken: --force isn't enough
        assert_eq!(plan(&m("/old", 99), Some("/a"), Some("/a"), &st, no), Plan::Downgrade);
        assert_eq!(plan(&m("/old", 99), Some("/a"), Some("/a"), &st, same), Plan::Downgrade);
        assert_eq!(plan(&m("/old", 99), Some("/a"), Some("/a"), &st, down), Plan::Update);
        // already there: nothing to refuse
        assert_eq!(plan(&m("/old", 99), Some("/old"), Some("/a"), &st, no), Plan::Running);
        assert_eq!(plan(&m("/old", 99), Some("/a"), Some("/old"), &st, no), Plan::Staged);
    }

    #[test]
    fn status_json() {
        let s = Status::new(Phase::LowBattery, "Charge to 30% first", true);
        let j = serde_json::to_string(&s).unwrap();
        assert!(j.contains("\"state\":\"low-battery\"") && j.contains("\"autoCheck\":true"), "{j}");
        assert_eq!(serde_json::from_str::<Status>(&j).unwrap(), s);
    }
}
