// SPDX-License-Identifier: GPL-2.0-or-later
//! The on-demand root service: writing to the Nix store takes root, and the app runs
//! as the session user. Socket-activated and exits when idle, so it uses no memory when
//! it isn't working. See DESIGN.md.

pub mod config;
pub mod daemon;
pub mod nix;
pub mod server;
pub mod state;
