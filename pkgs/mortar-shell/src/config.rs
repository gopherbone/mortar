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
    /// Tile illustration (default: guessed from id, name and icon).
    #[serde(default)]
    pub art: Option<crate::art::Art>,
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
    /// Set the time zone; "{zone}" is replaced by e.g. "Europe/Lisbon".
    #[serde(default = "d_timezone_set")]
    pub timezone_set: Vec<String>,
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
fn d_timezone_set() -> Vec<String> {
    argv(&["timedatectl", "set-timezone", "{zone}"])
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
            timezone_set: d_timezone_set(),
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
    /// The shell's own screens: renderer, motion, sounds, haptics.
    #[serde(default)]
    pub ui: UiConfig,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Renderer {
    /// GLES on the PowerVR when `glLibDir` is set and it initialises, else CPU.
    #[default]
    Auto,
    Gpu,
    Cpu,
}

/// Defaults for the user settings (the Settings tab overrides them, saved
/// in $XDG_STATE_HOME/mortar-shell/prefs.json) and the fixed UI options.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct UiConfig {
    pub renderer: Renderer,
    /// The stock PowerVR EGL/GLES libraries (libEGL.so.1, libGLESv2.so.2).
    pub gl_lib_dir: Option<std::path::PathBuf>,
    /// Multisampling for the GPU UI (EXT_multisampled_render_to_texture): 0 or 4.
    pub msaa: u32,
    pub motion: crate::motion::Preset,
    pub reduce_motion: bool,
    pub sounds: bool,
    pub sound_volume: u8,
    pub haptics: bool,
    pub haptic_ms: crate::haptics::Pulses,
    /// "day" | "night" (default: from `theme`).
    pub palette: Option<String>,
    pub boot_animation: bool,
    /// ALSA PCM for UI sounds.
    pub audio_device: String,
    /// Where screenshots go ("~" is the home directory).
    pub screenshot_dir: String,
    /// Frame callbacks per second for apps whose live card shows in the
    /// MENU drawer.
    pub card_fps: u32,
    /// After the screen-off sleep goes dark, also run `actions.sleep`
    /// (suspend-to-RAM). Off: untested on the Brick.
    pub suspend: bool,
    /// tzdata (zone1970.tab and the zone files) for Settings > Time zone.
    pub zoneinfo: String,
    /// The local-time link naming the current zone ("~" allowed).
    pub localtime: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig {
            renderer: Renderer::Auto,
            gl_lib_dir: None,
            msaa: 4,
            motion: crate::motion::Preset::Bouncy,
            reduce_motion: false,
            sounds: true,
            sound_volume: 60,
            haptics: true,
            haptic_ms: crate::haptics::Pulses::default(),
            palette: None,
            boot_animation: true,
            audio_device: "default".into(),
            screenshot_dir: "~/Pictures/Screenshots".into(),
            card_fps: 15,
            suspend: false,
            zoneinfo: "/etc/zoneinfo".into(),
            localtime: "/etc/localtime".into(),
        }
    }
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
    /// The settings before the user changed any.
    pub fn default_prefs(&self) -> crate::shellui::Prefs {
        let u = &self.ui;
        let night = match u.palette.as_deref() {
            Some(p) => p == "night" || p == "dark",
            None => self.theme.as_deref().is_some_and(|t| t == "dark" || t == "night"),
        };
        crate::shellui::Prefs {
            sounds: u.sounds,
            sound_volume: u.sound_volume.min(100),
            haptics: u.haptics,
            motion: u.motion,
            reduce_motion: u.reduce_motion,
            night,
        }
    }
}

/// $XDG_STATE_HOME/mortar-shell/prefs.json (else ~/.local/state/...).
pub fn prefs_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("mortar-shell/prefs.json")
}

/// The saved settings over the config's defaults.
pub fn load_prefs(defaults: crate::shellui::Prefs) -> crate::shellui::Prefs {
    let p = prefs_path();
    let Ok(s) = std::fs::read_to_string(&p) else { return defaults };
    // fields missing from the file keep the module's defaults
    let mut base = serde_json::to_value(&defaults).unwrap_or_default();
    match serde_json::from_str::<serde_json::Value>(&s) {
        Ok(serde_json::Value::Object(m)) => {
            if let serde_json::Value::Object(b) = &mut base {
                for (k, v) in m {
                    b.insert(k, v);
                }
            }
            serde_json::from_value(base).unwrap_or(defaults)
        }
        _ => {
            tracing::warn!("{}: not a settings object; ignored", p.display());
            defaults
        }
    }
}

pub fn save_prefs(p: &crate::shellui::Prefs) {
    let path = prefs_path();
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = path.with_extension("json.tmp");
    let r = serde_json::to_vec_pretty(p)
        .map_err(|e| e.to_string())
        .and_then(|b| std::fs::write(&tmp, b).map_err(|e| e.to_string()))
        .and_then(|_| std::fs::rename(&tmp, &path).map_err(|e| e.to_string()));
    if let Err(e) = r {
        tracing::warn!("{}: {e}", path.display());
    }
}

/// Expand a leading "~/".
pub fn expand_home(p: &str) -> std::path::PathBuf {
    match (p.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(h)) => std::path::PathBuf::from(h).join(rest),
        _ => std::path::PathBuf::from(p),
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
    fn ui_section() {
        let c: Config = serde_json::from_str(
            r#"{"theme":"dark","ui":{"renderer":"cpu","motion":"snappy","soundVolume":30,"hapticMs":{"tick":12,"double":18,"gap":72,"long":60},
                "audioDevice":"default","bootAnimation":true,"cardFps":15,"glLibDir":null,"haptics":true,"msaa":4,
                "palette":"night","reduceMotion":false,"screenshotDir":"~/Pictures/Screenshots","sounds":true},
                "apps":[{"id":"x","name":"X","command":["x"],"art":"claude"}]}"#,
        )
        .unwrap();
        assert_eq!(c.ui.renderer, Renderer::Cpu);
        assert_eq!(c.ui.haptic_ms.tick, 12);
        assert_eq!(c.ui.haptic_ms.long, 60);
        assert_eq!(c.apps[0].art, Some(crate::art::Art::Claude));
        let p = c.default_prefs();
        assert!(p.night && p.motion == crate::motion::Preset::Snappy && p.sound_volume == 30);
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
