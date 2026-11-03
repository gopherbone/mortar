// SPDX-License-Identifier: GPL-2.0-or-later
//! /etc/mortar-update/config.json, written by modules/core/update.nix.
//! No secrets: the credentials live in `<stateDir>/credentials` (root, 0600).

use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    pub bucket: String,
    pub endpoint: String,
    #[serde(default = "d_region")]
    pub region: String,
    #[serde(default = "d_channel")]
    pub channel: String,
    /// Only take manifests for this Nix system.
    #[serde(default = "d_system")]
    pub system: String,
    /// Only take manifests of this configuration (`mortar.update.configName`).
    #[serde(default)]
    pub config_name: Option<String>,
    /// Nix public keys (`name:base64`): a manifest must be signed by one, and
    /// the closure too, whatever else nix.conf trusts. Empty: nothing is taken.
    #[serde(default)]
    pub trusted_keys: Vec<String>,
    /// Default for "Auto-check daily" until it is changed on the Brick.
    #[serde(default = "yes")]
    pub auto_check: bool,
    /// Don't download below this battery level unless charging (percent).
    #[serde(default = "d_min_battery")]
    pub min_battery: u8,
    /// Free space wanted in /nix/store before downloading (MiB); below it
    /// unreferenced store paths are collected first.
    #[serde(default = "d_min_free")]
    pub min_free_mib: u64,
    /// System generations to keep (older ones are deleted when an update is
    /// staged; 0 keeps all).
    #[serde(default = "d_keep")]
    pub keep_generations: u32,
    #[serde(default = "d_state_dir")]
    pub state_dir: PathBuf,
    #[serde(default = "d_run_dir")]
    pub run_dir: PathBuf,
    /// The setup file on the card's FAT partition (`[update]` section).
    #[serde(default = "d_card")]
    pub card_file: PathBuf,
    #[serde(default = "d_profile")]
    pub profile: PathBuf,
    #[serde(default = "d_current")]
    pub current_system: PathBuf,
    #[serde(default = "d_power")]
    pub power_supply_dir: PathBuf,
    #[serde(default = "d_curl")]
    pub curl: String,
    #[serde(default = "d_nix")]
    pub nix: String,
    #[serde(default = "d_nix_env")]
    pub nix_env: String,
    #[serde(default = "d_gc")]
    pub nix_collect_garbage: String,
}

fn yes() -> bool {
    true
}
fn d_region() -> String {
    "auto".into()
}
fn d_channel() -> String {
    "mortar".into()
}
fn d_system() -> String {
    "aarch64-linux".into()
}
fn d_min_battery() -> u8 {
    30
}
fn d_min_free() -> u64 {
    1024
}
fn d_keep() -> u32 {
    5
}
fn d_state_dir() -> PathBuf {
    "/var/lib/mortar-update".into()
}
fn d_run_dir() -> PathBuf {
    "/run/mortar-update".into()
}
fn d_card() -> PathBuf {
    "/boot/mortar.toml".into()
}
fn d_profile() -> PathBuf {
    "/nix/var/nix/profiles/system".into()
}
fn d_current() -> PathBuf {
    "/run/current-system".into()
}
fn d_power() -> PathBuf {
    "/sys/class/power_supply".into()
}
fn d_curl() -> String {
    "curl".into()
}
fn d_nix() -> String {
    "nix".into()
}
fn d_nix_env() -> String {
    "nix-env".into()
}
fn d_gc() -> String {
    "nix-collect-garbage".into()
}

pub const DEFAULT_PATH: &str = "/etc/mortar-update/config.json";

impl Config {
    pub fn load(p: &Path) -> Result<Config, String> {
        let s = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        serde_json::from_str(&s).map_err(|e| format!("{}: {e}", p.display()))
    }
    pub fn bucket(&self) -> Result<mortar_update::Bucket, String> {
        mortar_update::Bucket::new(&self.endpoint, &self.bucket, &self.region)
    }
    /// This Brick's manifest in the bucket.
    pub fn manifest_key(&self) -> String {
        mortar_update::manifest::key(&self.channel, self.config_name.as_deref().unwrap_or("unset"))
    }
    pub fn credentials_file(&self) -> PathBuf {
        self.state_dir.join("credentials")
    }
    pub fn state_file(&self) -> PathBuf {
        self.state_dir.join("state.json")
    }
    pub fn status_file(&self) -> PathBuf {
        self.run_dir.join("status.json")
    }
    pub fn lock_file(&self) -> PathBuf {
        self.run_dir.join("lock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal() {
        let c: Config = serde_json::from_str(r#"{"bucket":"b","endpoint":"https://x.r2.cloudflarestorage.com"}"#).unwrap();
        assert_eq!((c.region.as_str(), c.channel.as_str(), c.min_battery), ("auto", "mortar", 30));
        assert_eq!(c.credentials_file(), PathBuf::from("/var/lib/mortar-update/credentials"));
        assert!(c.bucket().is_ok());
        assert!(serde_json::from_str::<Config>(r#"{"bucket":"b","endpoint":"e","secret":"no"}"#).is_err());
    }
}
