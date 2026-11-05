// SPDX-License-Identifier: GPL-2.0-or-later
//! Repositories, and what a device refuses from one.
//!
//! A repository is a signed index plus a binary cache. Several from the start, in the
//! data model and the socket's requests: Mortar's own comes from `mortar.update`, and
//! it's the only one configured by default. Nothing adds one at runtime.

use crate::catalog::{Catalog, FORMAT, Package};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepoKind {
    /// An index and a Nix binary cache. Adding one is trusting it with the whole
    /// system, which is why it isn't offered on any socket.
    #[default]
    Nix,
    /// Bundles only: never touches the store. No code for these yet.
    Bundle,
}

/// Another binary cache, and the key its paths are signed with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Upstream {
    pub url: String,
    pub public_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Repository {
    /// What requests call it. Short and fixed.
    pub id: String,
    /// What the screens call it: "from Mortar".
    pub name: String,
    /// `https://host`: `<url>/channels/<channel>/catalog.json` and a binary cache.
    pub url: String,
    /// Sign the index, and the packages. More than one while moving to a new key.
    pub public_keys: Vec<String>,
    #[serde(default)]
    pub kind: RepoKind,
    pub channel: String,
    /// Where the rest of a closure comes from. Their keys count for store paths,
    /// never for an index.
    #[serde(default)]
    pub upstreams: Vec<Upstream>,
    /// Its packages update on the daily check when Auto-check is on. Others are only
    /// offered.
    #[serde(default)]
    pub official: bool,
}

impl Repository {
    pub fn index_url(&self) -> String {
        format!("{}/{}", self.url.trim_end_matches('/'), crate::catalog::key(&self.channel))
    }

    /// What may hold a package of this repository, and the keys its paths may carry.
    pub fn package_sources(&self) -> (Vec<String>, Vec<String>) {
        let mut subs = vec![self.url.trim_end_matches('/').to_string()];
        let mut keys = self.public_keys.clone();
        for u in &self.upstreams {
            subs.push(u.url.clone());
            keys.push(u.public_key.clone());
        }
        (subs, keys)
    }

    /// A tool from a large index: the repository vouches for the list, and the package
    /// still has to carry the upstream's own signature. A tampered list can only point
    /// at packages that are already genuine.
    pub fn tool_sources(&self) -> (Vec<String>, Vec<String>) {
        (self.upstreams.iter().map(|u| u.url.clone()).collect(), self.upstreams.iter().map(|u| u.public_key.clone()).collect())
    }

    /// Only this repository's cache and key: the large index itself.
    pub fn own_sources(&self) -> (Vec<String>, Vec<String>) {
        (vec![self.url.trim_end_matches('/').to_string()], self.public_keys.clone())
    }
}

/// Why an index or a package wasn't taken. What was installed stays as it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// It's in a format from a newer Mortar.
    NewerFormat(u32),
    Malformed(String),
    BadSignature(String),
    WrongChannel { found: String, want: String },
    /// Older than the last index this device saw from the repository.
    Older { published: u64, seen: u64 },
    WrongDevice { id: String, device: String },
}

impl Refusal {
    /// A word the socket and the tests can match on.
    pub fn code(&self) -> &'static str {
        match self {
            Refusal::NewerFormat(_) => "newer-format",
            Refusal::Malformed(_) => "malformed",
            Refusal::BadSignature(_) => "bad-signature",
            Refusal::WrongChannel { .. } => "wrong-channel",
            Refusal::Older { .. } => "older",
            Refusal::WrongDevice { .. } => "wrong-device",
        }
    }

    /// One short sentence, for a person.
    pub fn headline(&self) -> String {
        match self {
            Refusal::NewerFormat(_) => "Update Mortar to see what's new".into(),
            Refusal::Malformed(_) => "This list can't be read".into(),
            Refusal::BadSignature(_) => "This list isn't trusted".into(),
            Refusal::WrongChannel { .. } => "This list is for another channel".into(),
            Refusal::Older { .. } => "This list is older than the last one".into(),
            Refusal::WrongDevice { .. } => "Not made for this device".into(),
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Refusal::NewerFormat(n) => format!("It uses format {n}, and this system reads {FORMAT}. What's installed stays as it is."),
            Refusal::Malformed(e) => e.clone(),
            Refusal::BadSignature(e) => e.clone(),
            Refusal::WrongChannel { found, want } => format!("It's for '{found}', and this device follows '{want}'."),
            Refusal::Older { published, seen } => {
                format!("Someone may be steering this device back to old versions. It's from {}, and {} was already seen.", crate::time::date(*published), crate::time::date(*seen))
            }
            Refusal::WrongDevice { id, device } => format!("{id} has no build for {device}."),
        }
    }
}

/// Read an index a repository served, and refuse it if it isn't for this device to
/// take. `seen` is the `published` of the last one taken from this repository.
pub fn read_index(bytes: &[u8], repo: &Repository, seen: u64) -> Result<Catalog, Refusal> {
    // the format first: another format may well have other fields
    let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| Refusal::Malformed(format!("catalog: {e}")))?;
    match v.get("format").and_then(|f| f.as_u64()) {
        None => return Err(Refusal::Malformed("catalog: no format".into())),
        Some(n) if n > FORMAT as u64 => return Err(Refusal::NewerFormat(n.min(u32::MAX as u64) as u32)),
        Some(_) => {}
    }
    let c: Catalog = serde_json::from_value(v).map_err(|e| Refusal::Malformed(format!("catalog: {e}")))?;
    // Before anything in it is looked at further. Malformed entries come second, so an
    // unsigned file can't choose which refusal a person sees.
    c.verify(&repo.public_keys).map_err(Refusal::BadSignature)?;
    c.check().map_err(Refusal::Malformed)?;
    if c.channel != repo.channel {
        return Err(Refusal::WrongChannel { found: c.channel, want: repo.channel.clone() });
    }
    if c.published < seen {
        return Err(Refusal::Older { published: c.published, seen });
    }
    Ok(c)
}

/// The build of a package this device can use, or why not.
pub fn pick_build<'a>(p: &'a Package, device: &str, system: &str) -> Result<&'a crate::catalog::Build, Refusal> {
    p.build_for(device, system).ok_or_else(|| Refusal::WrongDevice { id: p.id.clone(), device: device.to_string() })
}

/// The last `published` taken from each repository, persisted so a restart doesn't
/// forget it. Kept apart from what's installed: this only ever goes up.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Seen(pub BTreeMap<String, u64>);

impl Seen {
    pub fn get(&self, repo: &str) -> u64 {
        self.0.get(repo).copied().unwrap_or(0)
    }

    /// Never lowered.
    pub fn raise(&mut self, repo: &str, published: u64) -> bool {
        let e = self.0.entry(repo.to_string()).or_insert(0);
        let up = published > *e;
        *e = (*e).max(published);
        up
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SecretKey;
    use crate::catalog::tests::*;
    use crate::keys::test_keys::*;

    fn repo() -> Repository {
        Repository {
            id: "mortar".into(),
            name: "Mortar".into(),
            url: "https://cache.example/".into(),
            public_keys: vec![PUBLIC.into()],
            kind: RepoKind::Nix,
            channel: "mortar".into(),
            upstreams: vec![Upstream { url: "https://up.example".into(), public_key: "up:BB==".into() }],
            official: true,
        }
    }

    fn sign(f: impl FnOnce(&mut Catalog)) -> Vec<u8> {
        let mut c = catalog();
        f(&mut c);
        c.sign(&SecretKey::parse(SECRET).unwrap());
        c.to_json().into_bytes()
    }

    #[test]
    fn takes_a_good_index() {
        let c = read_index(&sign(|_| {}), &repo(), 0).unwrap();
        assert_eq!(c.packages.len(), 1);
        // the same one again is no refusal: it's still the last one seen
        read_index(&sign(|_| {}), &repo(), c.published).unwrap();
    }

    #[test]
    fn a_future_format_is_polite_even_if_nothing_else_about_it_can_be_read() {
        let r = read_index(br#"{"format":2,"anything":[1,2,3],"packages":"who knows"}"#, &repo(), 0).unwrap_err();
        assert_eq!(r, Refusal::NewerFormat(2));
        assert_eq!(r.headline(), "Update Mortar to see what's new");
        assert!(r.detail().contains("What's installed stays"));
    }

    #[test]
    fn refusals() {
        let code = |b: &[u8], seen| read_index(b, &repo(), seen).unwrap_err().code();
        assert_eq!(code(b"not json", 0), "malformed");
        assert_eq!(code(b"{}", 0), "malformed");
        // unsigned
        assert_eq!(code(catalog().to_json().as_bytes(), 0), "bad-signature");
        // tampered
        let mut c = signed();
        c.published += 1;
        assert_eq!(code(c.to_json().as_bytes(), 0), "bad-signature");
        // another key under the trusted name
        let mut c = catalog();
        c.sign(&SecretKey::parse(OTHER_SECRET).unwrap());
        assert_eq!(code(c.to_json().as_bytes(), 0), "bad-signature");
        // signed, for another channel
        assert_eq!(code(&sign(|c| c.channel = "testing".into()), 0), "wrong-channel");
        // signed, and older than what was seen
        let p = catalog().published;
        assert_eq!(code(&sign(|_| {}), p + 1), "older");
        // signed, and nonsense inside: its signature is checked first
        assert_eq!(code(&sign(|c| c.packages[0].id = "../x".into()), 0), "malformed");
        let mut c = signed();
        c.packages[0].id = "../x".into();
        assert_eq!(code(c.to_json().as_bytes(), 0), "bad-signature");
    }

    #[test]
    fn an_upstream_key_counts_for_paths_never_for_an_index() {
        let mut r = repo();
        r.public_keys = vec!["other:IEXeMnbLZHPuxalArGnT5CiHpciUFtH+toxGnI/q3kQ=".into()];
        r.upstreams = vec![Upstream { url: "https://u".into(), public_key: PUBLIC.into() }];
        assert_eq!(read_index(&sign(|_| {}), &r, 0).unwrap_err().code(), "bad-signature");
    }

    #[test]
    fn wrong_device_is_a_refusal_of_that_package() {
        let c = catalog();
        let p = &c.packages[0];
        assert!(pick_build(p, "brick", "aarch64-linux").is_ok());
        let e = pick_build(p, "other", "aarch64-linux").unwrap_err();
        assert_eq!(e.code(), "wrong-device");
        assert!(e.detail().contains("other"));
        assert_eq!(pick_build(p, "brick", "x86_64-linux").unwrap_err().code(), "wrong-device");
    }

    #[test]
    fn the_floor_only_goes_up_and_survives_a_restart() {
        let mut s = Seen::default();
        assert!(s.raise("mortar", 100));
        assert!(!s.raise("mortar", 50));
        assert_eq!(s.get("mortar"), 100);
        assert_eq!(s.get("elsewhere"), 0);
        let back: Seen = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn sources() {
        let r = repo();
        assert_eq!(r.index_url(), "https://cache.example/channels/mortar/catalog.json");
        assert_eq!(r.package_sources(), (vec!["https://cache.example".to_string(), "https://up.example".to_string()], vec![PUBLIC.to_string(), "up:BB==".to_string()]));
        assert_eq!(r.tool_sources(), (vec!["https://up.example".to_string()], vec!["up:BB==".to_string()]));
        assert_eq!(r.own_sources(), (vec!["https://cache.example".to_string()], vec![PUBLIC.to_string()]));
    }

    #[test]
    fn several_repositories_in_the_data_model() {
        let j = r#"[{"id":"mortar","name":"Mortar","url":"https://a","publicKeys":["k:AA=="],"channel":"mortar","official":true},
                    {"id":"yours","name":"Yours","url":"https://b","publicKeys":["y:BB=="],"kind":"nix","channel":"main"}]"#;
        let v: Vec<Repository> = serde_json::from_str(j).unwrap();
        assert!(v[0].official && !v[1].official);
        assert_eq!(v[1].kind, RepoKind::Nix);
        assert!(serde_json::from_str::<Repository>(r#"{"id":"a","name":"A","url":"u","publicKeys":[],"channel":"c","secret":"no"}"#).is_err());
    }
}
