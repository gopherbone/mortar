// SPDX-License-Identifier: GPL-2.0-or-later
//! The Mixer's view of PipeWire: the audio output streams in `pw-dump`'s
//! JSON (node id, the client's pid, volume, mute). The compositor maps a
//! stream to an app by its pid's ancestry (apps.rs) and sets levels with
//! wpctl (config `actions.streamVolume` / `streamMute`).
//!
//! Levels are on wpctl's cubic scale (what `wpctl get-volume` and the
//! volume keys show): PipeWire stores linear channel volumes, so the
//! percentage is 100 x cbrt(linear).

use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Stream {
    pub id: u32,
    pub pid: Option<u32>,
    pub name: String,
    /// Percent (cubic).
    pub level: u8,
    pub muted: bool,
    /// The node is running (sound is flowing), not idle or suspended.
    pub running: bool,
}

fn num(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// The output streams (media.class Stream/Output/Audio) in pw-dump output.
pub fn parse_streams(json: &str) -> Vec<Stream> {
    let Ok(Value::Array(objs)) = serde_json::from_str::<Value>(json) else { return Vec::new() };
    let mut out = Vec::new();
    for o in &objs {
        if o.get("type").and_then(Value::as_str) != Some("PipeWire:Interface:Node") {
            continue;
        }
        let Some(info) = o.get("info") else { continue };
        let props = info.get("props");
        let prop = |k: &str| props.and_then(|p| p.get(k));
        if prop("media.class").and_then(Value::as_str) != Some("Stream/Output/Audio") {
            continue;
        }
        let Some(id) = o.get("id").and_then(num) else { continue };
        let pid = prop("application.process.id").and_then(num).map(|p| p as u32);
        let name = prop("application.name").or_else(|| prop("node.name")).and_then(Value::as_str).unwrap_or("").to_string();
        // the Props param with the channel volumes
        let p = info.get("params").and_then(|p| p.get("Props")).and_then(Value::as_array).and_then(|a| a.iter().find(|x| x.get("channelVolumes").is_some()));
        let (level, muted) = match p {
            Some(p) => {
                let vols: Vec<f64> = p.get("channelVolumes").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
                let lin = if vols.is_empty() { 1.0 } else { vols.iter().cloned().fold(0.0, f64::max) };
                let lin = lin * p.get("volume").and_then(Value::as_f64).unwrap_or(1.0);
                ((lin.max(0.0).cbrt() * 100.0).round().clamp(0.0, 100.0) as u8, p.get("mute").and_then(Value::as_bool).unwrap_or(false))
            }
            None => (100, false),
        };
        let running = info.get("state").and_then(Value::as_str) == Some("running");
        out.push(Stream { id: id as u32, pid, name, level, muted, running });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_output_streams() {
        let j = r#"[
          {"id": 31, "type": "PipeWire:Interface:Node", "info": {"props": {"media.class": "Audio/Sink", "node.name": "codec"}}},
          {"id": 77, "type": "PipeWire:Interface:Node", "info": {"state": "running",
             "props": {"media.class": "Stream/Output/Audio", "application.name": "m8c", "application.process.id": 4242},
             "params": {"Props": [{"volume": 1.0, "mute": false, "channelVolumes": [0.343, 0.343]}]}}},
          {"id": 78, "type": "PipeWire:Interface:Node", "info": {
             "props": {"media.class": "Stream/Output/Audio", "application.name": "pico8", "application.process.id": "99"},
             "params": {"Props": [{"mute": true, "channelVolumes": [1.0]}]}}},
          {"id": 79, "type": "PipeWire:Interface:Node", "info": {"props": {"media.class": "Stream/Input/Audio"}}}
        ]"#;
        let s = parse_streams(j);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], Stream { id: 77, pid: Some(4242), name: "m8c".into(), level: 70, muted: false, running: true });
        assert!(!s[1].running, "no state: not playing");
        assert_eq!((s[1].pid, s[1].level, s[1].muted), (Some(99), 100, true));
        assert!(parse_streams("not json").is_empty());
    }
}
