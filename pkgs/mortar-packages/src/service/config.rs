// SPDX-License-Identifier: GPL-2.0-or-later
//! /etc/mortar-packages/config.json, written by modules/core/packages.nix. No secrets:
//! URLs and public keys. Nothing on the socket can change it.

use mortar_index::Repository;
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const DEFAULT_PATH: &str = "/etc/mortar-packages/config.json";

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    /// `mortar.device.id`: which builds are for this device.
    #[serde(default)]
    pub device: Option<String>,
    pub system: String,
    #[serde(default)]
    pub repositories: Vec<Repository>,
    /// The same rules as a system update.
    #[serde(default = "d_min_battery")]
    pub min_battery: u8,
    #[serde(default = "d_min_free")]
    pub min_free_mib: u64,
    /// The default of Auto-check daily, and where the Brick's own choice is kept.
    #[serde(default)]
    pub auto_check: bool,
    #[serde(default = "d_update_state")]
    pub update_state: PathBuf,
    /// The shell lists every profile in here.
    #[serde(default = "d_apps")]
    pub app_profiles: PathBuf,
    #[serde(default = "d_tools")]
    pub tool_profiles: PathBuf,
    /// On Terminal's path: a link per command of each tool.
    #[serde(default = "d_tools_bin")]
    pub tools_bin: PathBuf,
    #[serde(default = "d_roots")]
    pub gc_roots: PathBuf,
    #[serde(default = "d_state_dir")]
    pub state_dir: PathBuf,
    #[serde(default = "d_power")]
    pub power_supply_dir: PathBuf,
    /// Exit after this long with nothing to do.
    #[serde(default = "d_idle")]
    pub idle_seconds: u64,
    #[serde(default = "d_curl")]
    pub curl: String,
    #[serde(default = "d_nix_env")]
    pub nix_env: String,
    #[serde(default = "d_nix_store")]
    pub nix_store: String,
    #[serde(default = "d_gc")]
    pub nix_collect_garbage: String,
}

fn d_min_battery() -> u8 {
    30
}
fn d_min_free() -> u64 {
    1024
}
fn d_update_state() -> PathBuf {
    "/var/lib/mortar-update/state.json".into()
}
fn d_apps() -> PathBuf {
    "/nix/var/nix/profiles/apps".into()
}
fn d_tools() -> PathBuf {
    "/nix/var/nix/profiles/tools".into()
}
fn d_tools_bin() -> PathBuf {
    "/var/lib/mortar-packages/tools/bin".into()
}
fn d_roots() -> PathBuf {
    "/nix/var/nix/gcroots/mortar-packages".into()
}
fn d_state_dir() -> PathBuf {
    "/var/lib/mortar-packages".into()
}
fn d_power() -> PathBuf {
    "/sys/class/power_supply".into()
}
fn d_idle() -> u64 {
    20
}
fn d_curl() -> String {
    "curl".into()
}
fn d_nix_env() -> String {
    "nix-env".into()
}
fn d_nix_store() -> String {
    "nix-store".into()
}
fn d_gc() -> String {
    "nix-collect-garbage".into()
}

impl Config {
    pub fn load(p: &Path) -> Result<Config, String> {
        let s = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        serde_json::from_str(&s).map_err(|e| format!("{}: {e}", p.display()))
    }
    pub fn repo(&self, id: &str) -> Option<&Repository> {
        self.repositories.iter().find(|r| r.id == id)
    }
    pub fn state_file(&self) -> PathBuf {
        self.state_dir.join("state.json")
    }
    pub fn cached_index(&self, repo: &str) -> PathBuf {
        self.state_dir.join("repos").join(repo).join("catalog.json")
    }
    /// Keeps a repository's large index through garbage collection.
    pub fn tools_root(&self, repo: &str) -> PathBuf {
        self.gc_roots.join(format!("{repo}-tools"))
    }
    /// Keeps a download through a collection until its profile exists.
    pub fn install_root(&self, id: &str) -> PathBuf {
        self.gc_roots.join(format!("installing-{id}"))
    }
    /// Is Auto-check daily on? The Brick's own choice, else the default.
    pub fn auto_check_now(&self) -> bool {
        std::fs::read(&self.update_state)
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v.get("autoCheck").and_then(|a| a.as_bool()))
            .unwrap_or(self.auto_check)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_and_strict() {
        let c: Config = serde_json::from_str(r#"{"system":"aarch64-linux"}"#).unwrap();
        assert_eq!((c.min_battery, c.min_free_mib, c.repositories.len()), (30, 1024, 0));
        assert!(serde_json::from_str::<Config>(r#"{"system":"s","secret":"no"}"#).is_err());
    }

    #[test]
    fn auto_check_the_brick_says_wins() {
        let d = std::env::temp_dir().join(format!("mortar-packages-auto-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let mut c: Config = serde_json::from_str(r#"{"system":"s","autoCheck":true}"#).unwrap();
        c.update_state = d.join("state.json");
        assert!(c.auto_check_now(), "nothing chosen yet");
        std::fs::write(&c.update_state, r#"{"autoCheck":false}"#).unwrap();
        assert!(!c.auto_check_now());
        std::fs::write(&c.update_state, r#"{"lastPublished":1}"#).unwrap();
        assert!(c.auto_check_now());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
