// SPDX-License-Identifier: GPL-2.0-or-later
//! Greeter configuration (JSON, written by the mortar-greeter NixOS module)
//! and the small persistent state (last profile, last session per profile).

use crate::icons::Icon;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub profiles: Vec<Profile>,
    pub sessions: Vec<Session>,
    #[serde(default = "default_state_dir")]
    pub state_dir: PathBuf,
    /// Where `mortar-ui quick-menu` drops "switch to session X" requests.
    #[serde(default = "default_switch_dir")]
    pub switch_dir: PathBuf,
    #[serde(default)]
    pub actions: Actions,
}

fn default_state_dir() -> PathBuf {
    "/var/lib/mortar-greeter".into()
}
fn default_switch_dir() -> PathBuf {
    "/run/mortar-greeter".into()
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Profile {
    /// Unix user name passed to greetd.
    pub user: String,
    /// Shown name (defaults to the user name).
    #[serde(default)]
    pub name: Option<String>,
    /// Session ids this profile may start; None = all.
    #[serde(default)]
    pub sessions: Option<Vec<String>>,
    /// Display hint for how this profile logs in. PAM decides for real; the
    /// greeter answers whatever greetd asks. "pin" shows a lock and makes the
    /// dev-mode fake greetd ask for a PIN (1234).
    #[serde(default)]
    pub auth: Auth,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Auth {
    #[default]
    None,
    Pin,
}

impl Profile {
    pub fn display(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.user)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Session {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_icon")]
    pub icon: Icon,
    /// argv for greetd's start_session.
    pub command: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub needs_wifi: bool,
    /// Only offered while this path exists (an app installed outside the
    /// system, e.g. PICO-8 in its own Nix profile).
    #[serde(default)]
    pub requires: Option<std::path::PathBuf>,
}

impl Session {
    pub fn available(&self) -> bool {
        self.requires.as_ref().is_none_or(|p| p.exists())
    }
}

fn default_icon() -> Icon {
    Icon::Term
}

/// Commands behind quick-menu entries.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Actions {
    pub sleep: Vec<String>,
    pub reboot: Vec<String>,
    pub poweroff: Vec<String>,
}

impl Default for Actions {
    fn default() -> Self {
        let sc = |a: &str| vec!["systemctl".to_string(), a.to_string()];
        Actions { sleep: sc("suspend"), reboot: sc("reboot"), poweroff: sc("poweroff") }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Config, String> {
        let s = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&s).map_err(|e| format!("{}: {e}", path.display()))
    }
    pub fn parse(s: &str) -> Result<Config, String> {
        let c: Config = serde_json::from_str(s).map_err(|e| e.to_string())?;
        if c.profiles.is_empty() {
            return Err("no profiles configured".into());
        }
        Ok(c)
    }
    pub fn demo() -> Config {
        Self::parse(include_str!("../examples/demo-config.json")).expect("demo config")
    }
    pub fn session(&self, id: &str) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }
    /// Sessions a profile may start, in config order.
    pub fn sessions_for(&self, p: &Profile) -> Vec<&Session> {
        self.sessions
            .iter()
            .filter(|s| s.available() && p.sessions.as_ref().is_none_or(|ids| ids.contains(&s.id)))
            .collect()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    #[serde(default)]
    pub last_user: Option<String>,
    #[serde(default)]
    pub last_session: BTreeMap<String, String>,
}

impl State {
    fn path(dir: &Path) -> PathBuf {
        dir.join("state.json")
    }
    pub fn load(dir: &Path) -> State {
        std::fs::read_to_string(Self::path(dir)).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    /// Atomic write (temp file + rename) so a power cut can't truncate it.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let tmp = dir.join(".state.json.tmp");
        let data = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, data).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, Self::path(dir)).map_err(|e| e.to_string())
    }
}
