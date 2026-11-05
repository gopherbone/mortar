// SPDX-License-Identifier: GPL-2.0-or-later
//! Mortar over-the-air updates: what the publisher (`mortar publish`, CI)
//! and the Brick (`mortar-update`) agree on.
//!
//! - An S3-compatible bucket (Cloudflare R2) is a Nix binary cache holding
//!   the system closure, signed with the publisher's Nix key.
//! - `channels/<channel>/<config>.json` in the same bucket names the newest system
//!   ([`Manifest`]). It is written last, after the closure is complete.
//! - The Brick reads the manifest and fetches the closure, either anonymously
//!   over HTTPS (a Worker in front of the bucket, with cache.nixos.org as an
//!   upstream) or from the bucket with read-only credentials. Either way Nix
//!   checks the signatures against the Brick's own list of keys, and the
//!   closure becomes the next boot entry.
//!
//! Plain objects go through curl (`--aws-sigv4` for the bucket), so no S3 SDK
//! is linked; credentials reach curl on stdin, never on the command line. See
//! docs/updates.md.

pub use mortar_index::{manifest, time};
pub mod s3;

pub use manifest::Manifest;
pub use s3::{get_many, get_public, Bucket, Credentials, FetchError};
