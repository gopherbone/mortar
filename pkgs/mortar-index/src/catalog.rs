// SPDX-License-Identifier: GPL-2.0-or-later
//! `channels/<channel>/catalog.json`: what a repository offers. One signed file per
//! channel. Docs: docs/packages.md.
//!
//! Curated packages are listed here in full. A large index (nixpkgs) would be too
//! big to read as JSON, so this file only names it: `tools` points at a store path
//! holding a TSV ([`crate::compact`]), which arrives the way any package does, with
//! Nix checking its signature. That keeps one signed file per channel, one rollback
//! check, and nothing new for the Worker to serve.

use crate::keys::{self, Domain, SecretKey};
use crate::manifest::valid_store_path;
use serde::{Deserialize, Serialize};

pub const FORMAT: u32 = 1;

/// Where a channel's catalog lives in the bucket.
pub fn key(channel: &str) -> String {
    format!("channels/{channel}/catalog.json")
}

/// How a package gets onto a device, and where it shows up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// From the binary cache into `apps/<id>`: on the Apps page.
    #[default]
    Nix,
    /// A folder and a manifest in your home, sandboxed. Listed; not installable yet.
    Bundle,
    /// From the binary cache into `tools/<id>`: on Terminal's path, not the Apps page.
    Tool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Build {
    /// `mortar.device.id` ("brick"). Apps reach the GPU through device libraries, so a
    /// build for one handheld is no use on another.
    pub device: String,
    /// The Nix system ("aarch64-linux").
    pub system: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub version: String,
    /// apps | games | music | tools, as on the shell's pages.
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub licence: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub kind: Kind,
    /// What the app's manifest requires: the person's own files. Absolute or under
    /// `~/`. Left out when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    /// A terminal tool's main command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    pub builds: Vec<Build>,
}

impl Package {
    pub fn build_for(&self, device: &str, system: &str) -> Option<&Build> {
        self.builds.iter().find(|b| b.device == device && b.system == system)
    }
}

/// A large index of terminal tools, as a store path. See [`crate::compact`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolsIndex {
    /// The Nix system its packages are for.
    pub system: String,
    /// Holds `tools.tsv`. Signed by this repository like any of its paths.
    pub path: String,
    /// How many lines it lists, for the screen.
    pub count: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    pub format: u32,
    pub channel: String,
    /// Unix time of publication, and the serial: a Brick refuses a catalog older
    /// than the last one it saw.
    pub published: u64,
    #[serde(default)]
    pub packages: Vec<Package>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolsIndex>,
    /// `keyname:base64` Ed25519 over [`Catalog::signed_bytes`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// An app's id names its profile and the shell shows it: as `valid_id` in
/// pkgs/mortar-shell/src/discover.rs. A profile's generations are `<id>-<N>-link`.
pub fn valid_app_id(id: &str) -> bool {
    let mut c = id.chars();
    id.len() <= 64
        && c.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && c.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        && !looks_like_generation(id)
}

/// A tool's id can be a nixpkgs attribute (`python3Packages.yt-dlp`), and names its
/// profile too: no slashes, nothing hidden.
pub fn valid_tool_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        && id.chars().all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c))
        && !looks_like_generation(id)
}

fn looks_like_generation(id: &str) -> bool {
    id.strip_suffix("-link").and_then(|n| n.rsplit_once('-')).is_some_and(|(_, g)| !g.is_empty() && g.bytes().all(|b| b.is_ascii_digit()))
}

impl Catalog {
    /// A catalog's fields are checked, not its signature: that's
    /// [`crate::repo::read_index`], which is what a device uses.
    pub fn parse(bytes: &[u8]) -> Result<Catalog, String> {
        let c: Catalog = serde_json::from_slice(bytes).map_err(|e| format!("catalog: {e}"))?;
        c.check()?;
        Ok(c)
    }

    pub fn check(&self) -> Result<(), String> {
        if self.format != FORMAT {
            return Err(format!("catalog format {} (this reads {FORMAT})", self.format));
        }
        if self.published == 0 {
            return Err("catalog: no publication time".into());
        }
        let mut seen = std::collections::HashSet::new();
        for p in &self.packages {
            let ok = match p.kind {
                Kind::Tool => valid_tool_id(&p.id),
                Kind::Nix | Kind::Bundle => valid_app_id(&p.id),
            };
            if !ok {
                return Err(format!("catalog: bad id '{}'", p.id));
            }
            if !seen.insert(p.id.as_str()) {
                return Err(format!("catalog: '{}' is listed twice", p.id));
            }
            if p.name.trim().is_empty() {
                return Err(format!("catalog: '{}' has no name", p.id));
            }
            for r in &p.requires {
                if !(r.starts_with('/') || r.starts_with("~/")) {
                    return Err(format!("catalog: '{}' requires '{r}': absolute or under ~/", p.id));
                }
            }
            // a bundle's builds are for a format that doesn't exist yet
            if p.kind != Kind::Bundle
                && let Some(b) = p.builds.iter().find(|b| !valid_store_path(&b.path))
            {
                return Err(format!("catalog: '{}' is at '{}', which is not a store path", p.id, b.path));
            }
        }
        if let Some(t) = self.tools.iter().find(|t| !valid_store_path(&t.path)) {
            return Err(format!("catalog: tools index at '{}', which is not a store path", t.path));
        }
        Ok(())
    }

    /// The bytes the signature covers, tag included.
    pub fn signed_bytes(&self) -> Vec<u8> {
        Domain::Catalog.message(&self.body())
    }

    fn body(&self) -> Vec<u8> {
        let mut bare = self.clone();
        bare.signature = None;
        serde_json::to_vec(&bare).expect("catalog serializes")
    }

    pub fn sign(&mut self, key: &SecretKey) {
        self.signature = Some(key.sign(Domain::Catalog, &self.body()));
    }

    pub fn verify(&self, trusted: &[String]) -> Result<(), String> {
        let sig = self.signature.as_deref().ok_or("the catalog isn't signed")?;
        keys::verify(Domain::Catalog, &self.body(), sig, trusted)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("catalog serializes") + "\n"
    }

    pub fn package(&self, id: &str) -> Option<&Package> {
        self.packages.iter().find(|p| p.id == id)
    }

    pub fn tools_for(&self, system: &str) -> Option<&ToolsIndex> {
        self.tools.iter().find(|t| t.system == system)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::keys::test_keys::*;
    use crate::manifest::Manifest;

    pub const P: &str = "/nix/store/0c7f8ms0x6r4n3pq7x2a9w1kz5lbv8yd-wpe-browser-app";

    pub fn package(id: &str, kind: Kind) -> Package {
        Package {
            id: id.into(),
            name: "WPE Browser".into(),
            description: "Web browser on WPE WebKit".into(),
            version: "2.54.1".into(),
            category: "apps".into(),
            icon: "link".into(),
            licence: "MIT, LGPL-2.1, others".into(),
            source: "https://example.org/wpe".into(),
            kind,
            requires: vec![],
            program: None,
            builds: vec![Build { device: "brick".into(), system: "aarch64-linux".into(), path: P.into() }],
        }
    }

    pub fn catalog() -> Catalog {
        Catalog { format: 1, channel: "mortar".into(), published: 1_793_750_400, packages: vec![package("wpe-browser", Kind::Nix)], tools: vec![], signature: None }
    }

    pub fn signed() -> Catalog {
        let mut c = catalog();
        c.sign(&SecretKey::parse(SECRET).unwrap());
        c
    }

    fn manifest() -> Manifest {
        Manifest {
            schema: 1,
            channel: "mortar".into(),
            system: "aarch64-linux".into(),
            config: "brick-bare".into(),
            toplevel: "/nix/store/0c7f8ms0x6r4n3pq7x2a9w1kz5lbv8yd-nixos-system-mortar".into(),
            published: 1_793_750_400,
            date: "x".into(),
            rev: None,
            version: None,
            label: None,
            changelog: vec![],
            publisher: String::new(),
            signature: None,
        }
    }

    #[test]
    fn round_trip_as_in_the_design() {
        let j = signed().to_json();
        for field in ["\"format\": 1", "\"channel\": \"mortar\"", "\"published\"", "\"licence\"", "\"source\"", "\"builds\"", "\"device\": \"brick\"", "\"kind\": \"nix\""] {
            assert!(j.contains(field), "{field} in {j}");
        }
        let back = Catalog::parse(j.as_bytes()).unwrap();
        back.verify(&[PUBLIC.to_string()]).unwrap();
        assert_eq!(back.package("wpe-browser").unwrap().build_for("brick", "aarch64-linux").unwrap().path, P);
        assert!(back.package("wpe-browser").unwrap().build_for("other", "aarch64-linux").is_none());
    }

    #[test]
    fn kind_defaults_to_nix_and_bundles_parse() {
        let j = format!(r#"{{"format":1,"channel":"c","published":5,"packages":[
            {{"id":"a","name":"A","version":"1","builds":[{{"device":"brick","system":"s","path":"{P}"}}]}},
            {{"id":"b","name":"B","version":"1","kind":"bundle","builds":[]}},
            {{"id":"c","name":"C","version":"1","kind":"tool","program":"c","builds":[]}}
        ]}}"#);
        let c = Catalog::parse(j.as_bytes()).unwrap();
        assert_eq!(c.packages.iter().map(|p| p.kind).collect::<Vec<_>>(), [Kind::Nix, Kind::Bundle, Kind::Tool]);
    }

    #[test]
    fn every_signed_field_is_covered() {
        let keys = [PUBLIC.to_string()];
        for tamper in [
            (|c: &mut Catalog| c.published += 1) as fn(&mut Catalog),
            |c| c.channel = "testing".into(),
            |c| c.packages[0].builds[0].path = "/nix/store/0c7f8ms0x6r4n3pq7x2a9w1kz5lbv8yd-evil".into(),
            |c| c.packages[0].builds[0].device = "other".into(),
            |c| c.packages[0].version = "9".into(),
            |c| c.packages[0].requires = vec!["/x".into()],
            |c| c.packages.clear(),
            |c| c.tools.push(ToolsIndex { system: "s".into(), path: P.into(), count: 1 }),
        ] {
            let mut c = signed();
            tamper(&mut c);
            assert!(c.verify(&keys).unwrap_err().contains("bad signature"), "{c:?}");
        }
        assert!(catalog().verify(&keys).unwrap_err().contains("isn't signed"));
        let mut other = catalog();
        other.sign(&SecretKey::parse(OTHER_SECRET).unwrap());
        assert!(other.verify(&keys).unwrap_err().contains("bad signature"));
        assert!(signed().verify(&[]).unwrap_err().contains("doesn't trust"));
    }

    #[test]
    fn a_catalog_signature_never_verifies_as_a_system_manifest() {
        let keys = [PUBLIC.to_string()];
        // the signature moved onto a manifest
        let mut m = manifest();
        m.signature = signed().signature;
        assert!(m.verify(&keys).unwrap_err().contains("bad signature"));

        // one document that parses as both, with the catalog's signature on it
        let mut v = serde_json::to_value(signed()).unwrap();
        let mv = serde_json::to_value(manifest()).unwrap();
        for (k, x) in mv.as_object().unwrap() {
            if k != "signature" && k != "version" {
                v[k] = x.clone();
            }
        }
        let both = v.to_string();
        Catalog::parse(both.as_bytes()).unwrap().verify(&keys).unwrap();
        let as_manifest = Manifest::parse(both.as_bytes()).unwrap();
        assert!(as_manifest.verify(&keys).unwrap_err().contains("bad signature"));

        // and at the level of the bytes: what a catalog signs, offered as a manifest's body
        let k = SecretKey::parse(SECRET).unwrap();
        let body = serde_json::to_vec(&catalog()).unwrap();
        let sig = k.sign(Domain::Catalog, &body);
        assert!(keys::verify(Domain::Manifest, &body, &sig, &keys).unwrap_err().contains("bad signature"));
    }

    #[test]
    fn a_manifest_signature_never_verifies_as_a_catalog() {
        let keys = [PUBLIC.to_string()];
        let mut m = manifest();
        m.sign(&SecretKey::parse(SECRET).unwrap());
        m.verify(&keys).unwrap();

        let mut c = catalog();
        c.signature = m.signature.clone();
        assert!(c.verify(&keys).unwrap_err().contains("bad signature"));

        let mut v = serde_json::to_value(&m).unwrap();
        let cv = serde_json::to_value(catalog()).unwrap();
        for (k, x) in cv.as_object().unwrap() {
            if k != "signature" {
                v[k] = x.clone();
            }
        }
        let both = v.to_string();
        Manifest::parse(both.as_bytes()).unwrap().verify(&keys).unwrap();
        assert!(Catalog::parse(both.as_bytes()).unwrap().verify(&keys).unwrap_err().contains("bad signature"));

        let k = SecretKey::parse(SECRET).unwrap();
        let body = serde_json::to_vec(&catalog()).unwrap();
        let sig = k.sign(Domain::Manifest, &body);
        assert!(keys::verify(Domain::Catalog, &body, &sig, &keys).unwrap_err().contains("bad signature"));
    }

    #[test]
    fn rejects_what_could_do_harm() {
        let bad_id = |id: &'static str| {
            let mut c = catalog();
            c.packages[0].id = id.into();
            c.check().is_err()
        };
        for id in ["", "../etc", "a/b", "UPPER", ".hidden", "firefox-2-link", "a b", "-x"] {
            assert!(bad_id(id), "{id}");
        }
        assert!(!valid_app_id(&"x".repeat(65)));
        assert!(valid_app_id("wpe-browser") && valid_app_id("a_1"));
        assert!(valid_tool_id("python3Packages.yt-dlp") && valid_tool_id("gcc_14") && valid_tool_id("g++"));
        for id in ["", "../x", "a/b", ".a", "-a", "a b", "x-3-link", "a\nb"] {
            assert!(!valid_tool_id(id), "{id:?}");
        }
        let mut c = catalog();
        c.packages[0].builds[0].path = "/etc/passwd".into();
        assert!(c.check().is_err());
        let mut c = catalog();
        c.packages.push(package("wpe-browser", Kind::Nix));
        assert!(c.check().unwrap_err().contains("twice"));
        let mut c = catalog();
        c.packages[0].requires = vec!["relative/path".into()];
        assert!(c.check().is_err());
        let mut c = catalog();
        c.published = 0;
        assert!(c.check().is_err());
    }
}
