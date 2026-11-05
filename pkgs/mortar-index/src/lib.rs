// SPDX-License-Identifier: GPL-2.0-or-later
//! What a Mortar publisher and a Brick agree on, shared by `mortar-update`, the
//! `mortar` CLI and `mortar-packages`:
//!
//! - [`keys`]: Nix-format Ed25519 keys, and signatures that say what they sign
//! - [`manifest`]: `channels/<channel>/<config>.json`, the newest system
//! - [`catalog`]: `channels/<channel>/catalog.json`, the signed package index
//! - [`compact`]: a large index (nixpkgs) as a file searched without loading it
//! - [`repo`]: repositories, and the refusals that apply to any of their indexes
//! - [`nixargs`]: the options every download from a binary cache runs with
//!
//! No I/O beyond reading what it's handed, except under the `device` feature
//! ([`fsutil`], [`power`]).

pub mod catalog;
pub mod compact;
pub mod keys;
pub mod manifest;
pub mod nixargs;
pub mod repo;
pub mod time;

#[cfg(feature = "device")]
pub mod fsutil;
#[cfg(feature = "device")]
pub mod power;

pub use catalog::{Build, Catalog, Kind, Package};
pub use keys::{Domain, SecretKey};
pub use manifest::Manifest;
pub use repo::{Refusal, RepoKind, Repository};
