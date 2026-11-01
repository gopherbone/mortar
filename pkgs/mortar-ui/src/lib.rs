// SPDX-License-Identifier: GPL-2.0-or-later
//! mortar-ui as a library: the software renderer, theme, toolkit, icons,
//! input mapping and status probes shared by the greeter (this package's
//! binary) and mortar-shell. Build with `default-features = false` to get
//! only these (no greetd, KMS or window backends); the `backend` feature
//! adds the presentation/input backends.

#[cfg(feature = "backend")]
pub mod backend;
pub mod gfx;
pub mod icons;
pub mod input;
pub mod status;
pub mod theme;
pub mod toolkit;
