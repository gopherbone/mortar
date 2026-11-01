// SPDX-License-Identifier: GPL-2.0-or-later
//! mortar-setup: Mortar's first-boot setup and remote access.
//!
//! - `apply`: the boot-time step that applies `mortar.toml` from the card.
//! - `daemon`: pairing service on usb0 (`mortar pair` talks to it), the
//!   pairing code, root's SSH keys, Wi-Fi via iwd, Bluetooth via BlueZ,
//!   and a control socket.
//! - `ui`: the setup and Settings > Remote access screens, drawn with the
//!   mortar-ui toolkit; used by this crate's binary (first boot) and
//!   embedded in mortar-shell.
//!
//! With `default-features = false` only `control` and `ui` are built (what
//! mortar-shell needs).

pub mod control;
pub mod ui;

#[cfg(feature = "daemon")]
pub mod apply;
#[cfg(feature = "daemon")]
pub mod bluez;
#[cfg(feature = "daemon")]
pub mod daemon;
#[cfg(feature = "daemon")]
pub mod iwd;
#[cfg(feature = "daemon")]
pub mod keys;
#[cfg(feature = "daemon")]
pub mod pairing;
#[cfg(feature = "daemon")]
pub mod setupfile;
#[cfg(feature = "daemon")]
pub mod state;
#[cfg(feature = "daemon")]
pub mod sys;
