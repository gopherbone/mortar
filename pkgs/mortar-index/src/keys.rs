// SPDX-License-Identifier: GPL-2.0-or-later
//! Nix-format Ed25519 keys (`name:base64`) and what they sign.
//!
//! One key signs several kinds of thing, so a signature covers a [`Domain`] tag
//! first: a catalog's signature can't pass as a system manifest's, or the
//! reverse. The same key also signs store paths, whose fingerprints start
//! `1;/nix/store/`, which no tag here does.

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

/// What a signature is for: the first bytes of what it covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Domain {
    /// `channels/<channel>/<config>.json`. Published systems are signed with this
    /// tag, so it can't change.
    Manifest,
    /// `channels/<channel>/catalog.json`.
    Catalog,
}

impl Domain {
    pub fn tag(self) -> &'static [u8] {
        match self {
            Domain::Manifest => b"mortar-manifest-1\n",
            Domain::Catalog => b"mortar-catalog-1\n",
        }
    }

    /// The bytes a signature covers: the tag, then the body.
    pub fn message(self, body: &[u8]) -> Vec<u8> {
        let mut out = self.tag().to_vec();
        out.extend_from_slice(body);
        out
    }
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

    /// `keyname:base64`, over the domain's tag and `body`.
    pub fn sign(&self, domain: Domain, body: &[u8]) -> String {
        let sig = self.key.sign(&domain.message(body));
        format!("{}:{}", self.name, B64.encode(sig.to_bytes()))
    }
}

fn parse_public(text: &str) -> Result<(&str, VerifyingKey), String> {
    let (name, b) = text.trim().split_once(':').ok_or_else(|| format!("key '{text}': want name:base64"))?;
    let raw = B64.decode(b).map_err(|e| format!("key {name}: {e}"))?;
    let raw: [u8; 32] = raw.try_into().map_err(|_| format!("key {name}: want 32 bytes"))?;
    Ok((name, VerifyingKey::from_bytes(&raw).map_err(|e| format!("key {name}: {e}"))?))
}

/// A valid `signature` over the domain's tag and `body`, by one of `trusted`
/// (Nix public keys, `name:base64`).
pub fn verify(domain: Domain, body: &[u8], signature: &str, trusted: &[String]) -> Result<(), String> {
    let (name, b) = signature.split_once(':').ok_or("malformed signature")?;
    let raw = B64.decode(b).map_err(|_| "malformed signature")?;
    let sig = Signature::from_slice(&raw).map_err(|_| "malformed signature")?;
    let msg = domain.message(body);
    for t in trusted {
        let (key_name, key) = parse_public(t)?;
        if key_name == name {
            return key.verify(&msg, &sig).map_err(|_| format!("bad signature for key {name}"));
        }
    }
    Err(format!("signed by {name}, which this Brick doesn't trust"))
}

#[cfg(test)]
pub(crate) mod test_keys {
    // Throwaway keys for tests only: `nix key generate-secret --key-name test-1`
    // and its public half.
    pub const SECRET: &str = "test-1:JMPmnQgjB/X0wg+yK5jOkfHTpB0SnK3WQnrptJSIo5UgRd4ydstkc+7FqUCsadPkKIelyJQW0f62jEacj+reRA==";
    pub const PUBLIC: &str = "test-1:IEXeMnbLZHPuxalArGnT5CiHpciUFtH+toxGnI/q3kQ=";
    // another key with the name of the first
    pub const OTHER_SECRET: &str = "test-1:+ELUiEGocXxNHU9eEKTaMrR9CqLsBrjEqhc5dNxvu9f4vfB4aFGzVIyFhC208mm4cj9TiFnqlLyxsqiGzj07lw==";
}

#[cfg(test)]
mod tests {
    use super::test_keys::*;
    use super::*;

    #[test]
    fn a_signature_counts_in_its_own_domain_only() {
        let k = SecretKey::parse(SECRET).unwrap();
        let keys = [PUBLIC.to_string()];
        let body = b"the very same bytes";
        for (signed_as, read_as) in [(Domain::Manifest, Domain::Catalog), (Domain::Catalog, Domain::Manifest)] {
            let sig = k.sign(signed_as, body);
            verify(signed_as, body, &sig, &keys).unwrap();
            assert!(verify(read_as, body, &sig, &keys).unwrap_err().contains("bad signature"));
        }
    }

    #[test]
    fn one_tag_is_not_the_start_of_another_or_of_a_store_path_fingerprint() {
        let (m, c) = (Domain::Manifest.tag(), Domain::Catalog.tag());
        assert!(!m.starts_with(c) && !c.starts_with(m));
        for t in [m, c] {
            assert!(!t.starts_with(b"1;"));
        }
    }

    #[test]
    fn the_manifest_tag_stays() {
        assert_eq!(Domain::Manifest.tag(), b"mortar-manifest-1\n");
    }
}
