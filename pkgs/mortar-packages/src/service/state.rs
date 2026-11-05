// SPDX-License-Identifier: GPL-2.0-or-later
//! `/var/lib/mortar-packages/state.json`: what this service has installed, and the
//! newest index seen from each repository, which only ever goes up.

use mortar_index::fsutil::write_atomic_in;
use mortar_index::repo::Seen;
use mortar_index::Kind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Previous {
    pub version: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    pub repo: String,
    pub kind: Kind,
    pub name: String,
    pub version: String,
    pub path: String,
    #[serde(default)]
    pub previous: Option<Previous>,
    /// What a rollback went back from. The daily check leaves it alone from then on;
    /// it's still offered.
    #[serde(default)]
    pub rolled_back_from: Option<String>,
    /// A tool's links on Terminal's path.
    #[serde(default)]
    pub links: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct State {
    pub seen: Seen,
    /// By id: a profile holds one package, from one repository.
    pub installed: BTreeMap<String, Installed>,
    /// When each repository's index was last read.
    pub checked: BTreeMap<String, u64>,
}

impl State {
    /// A file that can't be read gives up the floor, so it's not silently forgiven:
    /// a damaged state is kept beside it to look at.
    pub fn load(p: &Path) -> State {
        match std::fs::read(p) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|e| {
                eprintln!("mortar-packages: {}: {e}; starting again", p.display());
                let _ = std::fs::rename(p, p.with_extension("json.damaged"));
                State::default()
            }),
            Err(_) => State::default(),
        }
    }

    pub fn save(&self, p: &Path) -> std::io::Result<()> {
        write_atomic_in(p, &serde_json::to_vec_pretty(self).expect("state serializes"), 0o644, 0o755)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_damage() {
        let d = std::env::temp_dir().join(format!("mortar-packages-state-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("state.json");
        assert_eq!(State::load(&p), State::default());
        let mut s = State::default();
        s.seen.raise("mortar", 5);
        s.installed.insert(
            "a".into(),
            Installed { repo: "mortar".into(), kind: Kind::Tool, name: "A".into(), version: "1".into(), path: "/nix/store/x".into(), previous: None, rolled_back_from: None, links: vec!["a".into()] },
        );
        s.save(&p).unwrap();
        assert_eq!(State::load(&p), s);
        std::fs::write(&p, "{ not json").unwrap();
        assert_eq!(State::load(&p), State::default());
        assert!(p.with_extension("json.damaged").exists());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
