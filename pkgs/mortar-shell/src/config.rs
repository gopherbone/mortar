// SPDX-License-Identifier: GPL-2.0-or-later
//! Shell configuration: the launcher entries and system actions, as JSON
//! written by the NixOS module (modules/mortar-shell.nix) to
//! /etc/mortar-shell/config.json.

use mortar_ui::icons::Icon;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// How an app gets the Brick's buttons while it is the visible app.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GamepadMode {
    /// The app reads a gamepad itself (SDL): it gets its own virtual copy of
    /// every physical pad, fed only while it is visible.
    #[default]
    Forward,
    /// The app only reads Wayland keyboard input (a terminal): the pad
    /// drives arrow/enter/... keys and the on-screen keyboard.
    Keyboard,
    /// Buttons go nowhere (MENU still opens the shell).
    None,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_icon")]
    pub icon: Icon,
    /// argv; run directly (no shell), in its own process group.
    pub command: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub gamepad: GamepadMode,
    /// Launching it again switches to the running instance.
    #[serde(default = "yes")]
    pub single_instance: bool,
}

fn default_icon() -> Icon {
    Icon::Pad
}
fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Actions {
    #[serde(default = "d_sleep")]
    pub sleep: Vec<String>,
    #[serde(default = "d_poweroff")]
    pub poweroff: Vec<String>,
    #[serde(default = "d_reboot")]
    pub reboot: Vec<String>,
    /// Change the volume; "{step}" is replaced by e.g. "5%+" or "5%-".
    #[serde(default = "d_volume_set")]
    pub volume_set: Vec<String>,
    /// Print the mixer state; the first "[NN%]" in the output is the volume.
    #[serde(default = "d_volume_get")]
    pub volume_get: Vec<String>,
}

fn argv(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}
fn d_sleep() -> Vec<String> {
    argv(&["systemctl", "suspend"])
}
fn d_poweroff() -> Vec<String> {
    argv(&["systemctl", "poweroff"])
}
fn d_reboot() -> Vec<String> {
    argv(&["systemctl", "reboot"])
}
fn d_volume_set() -> Vec<String> {
    argv(&["amixer", "-M", "-q", "-c", "Codec", "sset", "DAC", "{step}"])
}
fn d_volume_get() -> Vec<String> {
    argv(&["amixer", "-M", "-c", "Codec", "sget", "DAC"])
}

impl Default for Actions {
    fn default() -> Self {
        Actions {
            sleep: d_sleep(),
            poweroff: d_poweroff(),
            reboot: d_reboot(),
            volume_set: d_volume_set(),
            volume_get: d_volume_get(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub apps: Vec<AppEntry>,
    #[serde(default)]
    pub actions: Actions,
    /// Frame callbacks per second for apps that are running but not shown.
    #[serde(default = "d_bg_fps")]
    pub background_fps: u32,
    /// "light" | "dark" (mortar-ui theme).
    #[serde(default)]
    pub theme: Option<String>,
    /// xkb layout for keyboards and the on-screen keyboard.
    #[serde(default = "d_layout")]
    pub keyboard_layout: String,
    /// App ids to start at login (the first one ends up visible).
    #[serde(default)]
    pub autostart: Vec<String>,
    /// Extra environment for every app.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// mortar-setup's control socket: Settings > Wi-Fi and Remote access
    /// open its screens (unset: those rows only show the state).
    #[serde(default)]
    pub setup_socket: Option<std::path::PathBuf>,
}

fn d_bg_fps() -> u32 {
    2
}
fn d_layout() -> String {
    "us".into()
}

impl Default for Config {
    fn default() -> Self {
        serde_json::from_str("{}").expect("empty config")
    }
}

impl Config {
    pub fn load(p: &Path) -> Result<Config, String> {
        let s = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let c: Config = serde_json::from_str(&s).map_err(|e| format!("{}: {e}", p.display()))?;
        c.check()?;
        Ok(c)
    }
    pub fn check(&self) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for a in &self.apps {
            if a.command.is_empty() {
                return Err(format!("app '{}': empty command", a.id));
            }
            if !seen.insert(&a.id) {
                return Err(format!("app '{}' defined twice", a.id));
            }
        }
        for id in &self.autostart {
            if self.app(id).is_none() {
                return Err(format!("autostart: unknown app '{id}'"));
            }
        }
        Ok(())
    }
    pub fn app(&self, id: &str) -> Option<&AppEntry> {
        self.apps.iter().find(|a| a.id == id)
    }
}

/// The first "[NN%]" in amixer's output.
pub fn parse_volume(out: &str) -> Option<u8> {
    let i = out.find('[')?;
    let rest = &out[i + 1..];
    let j = rest.find("%]")?;
    rest[..j].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_module_output() {
        let c: Config = serde_json::from_str(
            r#"{"apps":[{"id":"term","name":"Terminal","icon":"term","command":["foot"],"gamepad":"keyboard"},
                        {"id":"m8","name":"M8","command":["m8c"],"env":{"A":"b"}}],
                "autostart":["m8"],"backgroundFps":5}"#,
        )
        .unwrap();
        c.check().unwrap();
        assert_eq!(c.apps[0].gamepad, GamepadMode::Keyboard);
        assert_eq!(c.apps[1].gamepad, GamepadMode::Forward);
        assert!(c.apps[1].single_instance);
        assert_eq!(c.background_fps, 5);
        assert_eq!(c.actions.sleep, vec!["systemctl", "suspend"]);
    }

    #[test]
    fn rejects_unknown_autostart() {
        let c: Config = serde_json::from_str(r#"{"autostart":["nope"]}"#).unwrap();
        assert!(c.check().is_err());
    }

    #[test]
    fn volume() {
        let out = "Simple mixer control 'DAC',0\n  Front Left: Playback 50 [79%] [-15.00dB]\n";
        assert_eq!(parse_volume(out), Some(79));
        assert_eq!(parse_volume("nothing"), None);
    }
}
