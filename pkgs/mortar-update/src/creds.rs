// SPDX-License-Identifier: GPL-2.0-or-later
//! The Brick's read-only bucket credentials: `<stateDir>/credentials`
//! (root, 0600, on the ext4 root file system; never in the Nix store or the
//! repository). They arrive through the `[update]` section of `mortar.toml`
//! on the card (moved here at boot, then blanked in the file) or through
//! `mortar update-creds` from a computer (`mortar-update provision --stdin`).

use crate::fsutil::write_atomic;
use mortar_update::Credentials;
use std::path::Path;
use toml_edit::{DocumentMut, Item, Value};

fn get<'a>(t: &'a dyn toml_edit::TableLike, k: &str) -> Option<&'a str> {
    t.get(k)?.as_str().map(str::trim).filter(|s| !s.is_empty())
}

/// `access_key_id = "…"` / `secret_access_key = "…"` at the top level.
pub fn parse(text: &str) -> Result<Credentials, String> {
    let doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    let t = doc.as_table();
    match (get(t, "access_key_id"), get(t, "secret_access_key")) {
        (Some(id), Some(secret)) => Ok(Credentials { access_key_id: id.into(), secret_access_key: secret.into() }),
        _ => Err("want access_key_id and secret_access_key".into()),
    }
}

pub fn to_text(c: &Credentials) -> String {
    let mut doc = DocumentMut::new();
    doc["access_key_id"] = toml_edit::value(c.access_key_id.clone());
    doc["secret_access_key"] = toml_edit::value(c.secret_access_key.clone());
    format!("# mortar-update: read-only bucket credentials (docs/updates.md)\n{doc}")
}

pub fn load(path: &Path) -> Option<Credentials> {
    parse(&std::fs::read_to_string(path).ok()?).ok()
}

pub fn save(path: &Path, c: &Credentials) -> std::io::Result<()> {
    write_atomic(path, to_text(c).as_bytes(), 0o600)
}

/// What the card's `[update]` section asks for.
#[derive(Debug, Default, PartialEq)]
pub struct CardUpdate {
    pub credentials: Option<Credentials>,
    pub auto_check: Option<bool>,
    pub warnings: Vec<String>,
}

pub fn parse_card(text: &str) -> Result<CardUpdate, String> {
    let doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    let mut out = CardUpdate::default();
    let Some(u) = doc.get("update").and_then(|u| u.as_table_like()) else { return Ok(out) };
    match (get(u, "access_key_id"), get(u, "secret_access_key")) {
        (Some(id), Some(s)) => out.credentials = Some(Credentials { access_key_id: id.into(), secret_access_key: s.into() }),
        (None, Some(_)) => out.warnings.push("[update] secret_access_key without access_key_id".into()),
        _ => {}
    }
    if let Some(v) = u.get("auto_check") {
        match v.as_bool() {
            Some(b) => out.auto_check = Some(b),
            None => out.warnings.push("[update] auto_check must be true or false".into()),
        }
    }
    Ok(out)
}

/// Blank `[update] secret_access_key` in the file text, keeping the rest.
/// None if there was nothing to blank.
pub fn scrub_card(text: &str) -> Option<String> {
    let mut doc: DocumentMut = text.parse().ok()?;
    let s = doc.get_mut("update")?.get_mut("secret_access_key")?;
    if s.as_str().is_none_or(|s| s.trim().is_empty()) {
        return None;
    }
    let mut v = Value::from("");
    v.decor_mut().set_suffix("  # saved on the Brick and removed from this file");
    *s = Item::Value(v);
    Some(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARD: &str = r#"hostname = "my-brick"

[wifi]
ssid = "x"

[update]
# read-only R2 token for over-the-air updates
access_key_id = "0123abcd"
secret_access_key = "s3cr3t-value" # removed after boot
auto_check = false
"#;

    #[test]
    fn card_section() {
        let u = parse_card(CARD).unwrap();
        assert_eq!(u.credentials.unwrap().secret_access_key, "s3cr3t-value");
        assert_eq!(u.auto_check, Some(false));
        let scrubbed = scrub_card(CARD).unwrap();
        assert!(!scrubbed.contains("s3cr3t"));
        assert!(scrubbed.contains("access_key_id = \"0123abcd\""));
        assert!(scrubbed.contains("# read-only R2 token"));
        assert!(scrubbed.contains("ssid = \"x\""));
        assert!(parse_card(&scrubbed).unwrap().credentials.is_none());
        assert!(scrub_card(&scrubbed).is_none());
        assert_eq!(parse_card("hostname = \"x\"\n").unwrap(), CardUpdate::default());
        assert!(parse_card("[update]\nsecret_access_key = \"x\"\n").unwrap().warnings.len() == 1);
    }

    #[test]
    fn file_round_trip() {
        let c = Credentials { access_key_id: "id".into(), secret_access_key: "a\"b".into() };
        assert_eq!(parse(&to_text(&c)).unwrap(), c);
        assert!(parse("access_key_id = \"x\"\n").is_err());
    }
}
