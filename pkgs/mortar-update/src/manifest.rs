// SPDX-License-Identifier: GPL-2.0-or-later
//! `channels/<channel>/<config>.json`: the newest system published for a
//! configuration on a channel.

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

pub const SCHEMA: u32 = 1;

/// What is signed: this tag, then the manifest as JSON without its signature.
/// (A publisher that adds a field bumps SCHEMA: an older Brick can't check it.)
const SIGNED_TAG: &[u8] = b"mortar-manifest-1\n";

/// Where a channel's manifest lives in the bucket.
pub fn key(channel: &str, config: &str) -> String {
    format!("channels/{channel}/{config}.json")
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
    /// `keyname:base64` Ed25519 over [`Manifest::signed_bytes`], made with the
    /// publisher's Nix signing key; the Brick refuses a manifest without a
    /// valid one (`mortar.update.trustedKeys`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// A Nix secret key: `name:base64(seed + public)` (`nix key generate-secret`).
pub struct SecretKey {
    name: String,
    key: SigningKey,
}

impl SecretKey {
    /// The public half, as `nix key convert-secret-to-public` writes it.
    pub fn public(&self) -> String {
        format!("{}:{}", self.name, B64.encode(self.key.verifying_key().to_bytes()))
    }

    pub fn parse(text: &str) -> Result<SecretKey, String> {
        let (name, b) = text.trim().split_once(':').ok_or("signing key: want name:base64")?;
        let raw = B64.decode(b).map_err(|e| format!("signing key: {e}"))?;
        let raw: [u8; 64] = raw.try_into().map_err(|_| "signing key: want 64 bytes (nix key generate-secret)")?;
        let key = SigningKey::from_keypair_bytes(&raw).map_err(|e| format!("signing key: {e}"))?;
        Ok(SecretKey { name: name.to_string(), key })
    }
}

fn parse_public(text: &str) -> Result<(&str, VerifyingKey), String> {
    let (name, b) = text.trim().split_once(':').ok_or_else(|| format!("key '{text}': want name:base64"))?;
    let raw = B64.decode(b).map_err(|e| format!("key {name}: {e}"))?;
    let raw: [u8; 32] = raw.try_into().map_err(|_| format!("key {name}: want 32 bytes"))?;
    Ok((name, VerifyingKey::from_bytes(&raw).map_err(|e| format!("key {name}: {e}"))?))
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

    /// The bytes the signature covers.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let mut bare = self.clone();
        bare.signature = None;
        let mut out = SIGNED_TAG.to_vec();
        out.extend(serde_json::to_vec(&bare).expect("manifest serializes"));
        out
    }

    pub fn sign(&mut self, key: &SecretKey) {
        let sig = key.key.sign(&self.signed_bytes());
        self.signature = Some(format!("{}:{}", key.name, B64.encode(sig.to_bytes())));
    }

    /// A valid signature by one of `trusted` (Nix public keys, `name:base64`).
    pub fn verify(&self, trusted: &[String]) -> Result<(), String> {
        let sig = self.signature.as_deref().ok_or("the manifest isn't signed")?;
        let (name, b) = sig.split_once(':').ok_or("malformed signature")?;
        let raw = B64.decode(b).map_err(|_| "malformed signature")?;
        let sig = Signature::from_slice(&raw).map_err(|_| "malformed signature")?;
        let msg = self.signed_bytes();
        for t in trusted {
            let (key_name, key) = parse_public(t)?;
            if key_name == name {
                return key.verify(&msg, &sig).map_err(|_| format!("bad signature for key {name}"));
            }
        }
        Err(format!("signed by {name}, which this Brick doesn't trust"))
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
            signature: None,
        }
    }

    // Throwaway keys for these tests only: `nix key generate-secret --key-name test-1`
    // and its public half.
    const SECRET: &str = "test-1:JMPmnQgjB/X0wg+yK5jOkfHTpB0SnK3WQnrptJSIo5UgRd4ydstkc+7FqUCsadPkKIelyJQW0f62jEacj+reRA==";
    const PUBLIC: &str = "test-1:IEXeMnbLZHPuxalArGnT5CiHpciUFtH+toxGnI/q3kQ=";
    // another key with the name of the first
    const OTHER_SECRET: &str = "test-1:+ELUiEGocXxNHU9eEKTaMrR9CqLsBrjEqhc5dNxvu9f4vfB4aFGzVIyFhC208mm4cj9TiFnqlLyxsqiGzj07lw==";

    fn signed() -> Manifest {
        let mut x = m();
        x.sign(&SecretKey::parse(SECRET).unwrap());
        x
    }

    #[test]
    fn signature_round_trip() {
        let x = signed();
        assert_eq!(x.signature.as_deref().unwrap().split(':').next(), Some("test-1"));
        // survives the bucket: JSON and back, unknown fields ignored
        let mut v: serde_json::Value = serde_json::from_str(&x.to_json()).unwrap();
        v["future"] = 1.into();
        let back = Manifest::parse(v.to_string().as_bytes()).unwrap();
        back.verify(&[PUBLIC.to_string()]).unwrap();
    }

    #[test]
    fn signature_refusals() {
        let keys = [PUBLIC.to_string()];
        assert!(m().verify(&keys).unwrap_err().contains("isn't signed"));
        // every signed field is covered
        for tamper in [
            (|x: &mut Manifest| x.toplevel = "/nix/store/0c7f8ms0x6r4n3pq7x2a9w1kz5lbv8yd-evil".into()) as fn(&mut Manifest),
            |x| x.published += 1,
            |x| x.channel = "testing".into(),
            |x| x.config = "brick-full".into(),
            |x| x.system = "x86_64-linux".into(),
        ] {
            let mut x = signed();
            tamper(&mut x);
            assert!(x.verify(&keys).unwrap_err().contains("bad signature"), "{x:?}");
        }
        // the right name, the wrong key
        let mut x = m();
        x.sign(&SecretKey::parse(OTHER_SECRET).unwrap());
        assert!(x.verify(&keys).unwrap_err().contains("bad signature"));
        // nothing trusted
        assert!(signed().verify(&[]).unwrap_err().contains("doesn't trust"));
        assert!(signed().verify(&["other:IEXeMnbLZHPuxalArGnT5CiHpciUFtH+toxGnI/q3kQ=".into()]).unwrap_err().contains("doesn't trust"));
        // garbage
        let mut x = m();
        x.signature = Some("test-1:notbase64!".into());
        assert!(x.verify(&keys).is_err());
        x.signature = Some("nocolon".into());
        assert!(x.verify(&keys).is_err());
        assert!(SecretKey::parse("test-1:AAAA").is_err());
        assert_eq!(SecretKey::parse(SECRET).unwrap().public(), PUBLIC);
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
