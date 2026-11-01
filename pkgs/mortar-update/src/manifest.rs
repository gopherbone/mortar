// SPDX-License-Identifier: GPL-2.0-or-later
//! `channels/<channel>.json`: the newest published system.

use serde::{Deserialize, Serialize};

pub const SCHEMA: u32 = 1;

/// Where a channel's manifest lives in the bucket.
pub fn key(channel: &str) -> String {
    format!("channels/{channel}.json")
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub schema: u32,
    pub channel: String,
    /// The Nix system the closure is for ("aarch64-linux").
    pub system: String,
    /// The flake's nixosConfigurations attribute ("brick-mainline").
    pub config: String,
    /// `config.system.build.toplevel`.
    pub toplevel: String,
    /// Unix time of publication. Also the serial: a Brick only takes a
    /// manifest newer than the last one it took, so a newer local
    /// `mortar deploy` is not replaced by an older published build.
    pub published: u64,
    /// `published` as RFC 3339 (for people).
    pub date: String,
    /// Git commit the system was built from (None: dirty tree).
    #[serde(default)]
    pub rev: Option<String>,
    /// NixOS label (`nixos-version`).
    #[serde(default)]
    pub label: Option<String>,
    /// Commit subjects since the previous manifest, newest first.
    #[serde(default)]
    pub changelog: Vec<String>,
    /// "github-actions", "mortar publish", ...
    #[serde(default)]
    pub publisher: String,
}

/// `/nix/store/<32 base32 chars>-<name>` and nothing more.
pub fn valid_store_path(p: &str) -> bool {
    let Some(rest) = p.strip_prefix("/nix/store/") else { return false };
    let (hash, name) = match rest.split_at_checked(32) {
        Some((h, n)) => (h, n),
        None => return false,
    };
    hash.chars().all(|c| c.is_ascii_digit() || (c.is_ascii_lowercase() && !matches!(c, 'e' | 'o' | 'u' | 't')))
        && name.len() > 1
        && name.starts_with('-')
        && name[1..].chars().all(|c| c.is_ascii_alphanumeric() || "+-._?=".contains(c))
}

impl Manifest {
    pub fn parse(bytes: &[u8]) -> Result<Manifest, String> {
        let m: Manifest = serde_json::from_slice(bytes).map_err(|e| format!("manifest: {e}"))?;
        m.check()?;
        Ok(m)
    }

    pub fn check(&self) -> Result<(), String> {
        if self.schema != SCHEMA {
            return Err(format!("manifest schema {} (this mortar-update reads {SCHEMA})", self.schema));
        }
        if !valid_store_path(&self.toplevel) {
            return Err(format!("manifest: '{}' is not a store path", self.toplevel));
        }
        if self.published == 0 {
            return Err("manifest: no publication time".into());
        }
        Ok(())
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("manifest serializes") + "\n"
    }

    /// "a1b2c3d · 2026-11-01"
    pub fn short(&self) -> String {
        let rev = self.rev.as_deref().map(|r| &r[..r.len().min(7)]).unwrap_or("local");
        format!("{rev} · {}", &self.date[..self.date.len().min(10)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: &str = "/nix/store/0c7f8ms0x6r4n3pq7x2a9w1kz5lbv8yd-nixos-system-mortar-26.05";

    fn m() -> Manifest {
        Manifest {
            schema: 1,
            channel: "mortar".into(),
            system: "aarch64-linux".into(),
            config: "brick-mainline".into(),
            toplevel: P.into(),
            published: 1_793_525_400,
            date: crate::time::rfc3339(1_793_525_400),
            rev: Some("fb4f6aa0123456789".into()),
            label: None,
            changelog: vec!["USB MIDI".into()],
            publisher: "test".into(),
        }
    }

    #[test]
    fn round_trip() {
        let j = m().to_json();
        assert!(j.contains("\"toplevel\""));
        assert_eq!(Manifest::parse(j.as_bytes()).unwrap(), m());
        assert_eq!(m().short(), "fb4f6aa · 2026-11-01");
    }

    #[test]
    fn rejects_bad_paths() {
        assert!(valid_store_path(P));
        for bad in [
            "/nix/store/0c7f8ms0x6r4n3pq7x2a9w1kz5lbv8yd",
            "/nix/store/0c7f8ms0x6r4n3pq7x2a9w1kz5lbv8yd-a/../../etc",
            "/nix/store/0c7f8ms0x6r4n3pq7x2a9w1kz5lbv8yd-a b",
            "/nix/store/0C7F8MS0X6R4N3PQ7X2A9W1KZ5LBV8YD-x",
            "/tmp/x",
            "",
        ] {
            assert!(!valid_store_path(bad), "{bad}");
        }
        let mut x = m();
        x.toplevel = "/etc/passwd".into();
        assert!(Manifest::parse(x.to_json().as_bytes()).is_err());
        let mut y = m();
        y.schema = 2;
        assert!(y.check().is_err());
    }

    #[test]
    fn tolerates_missing_optional_fields() {
        let j = format!(r#"{{"schema":1,"channel":"mortar","system":"aarch64-linux","config":"brick-mainline","toplevel":"{P}","published":5,"date":"x","extra":1}}"#);
        let m = Manifest::parse(j.as_bytes()).unwrap();
        assert!(m.changelog.is_empty() && m.rev.is_none());
    }
}
