// SPDX-License-Identifier: GPL-2.0-or-later
//! mortar-files: a file manager for the buttons. See DESIGN.md.

pub mod bg;
pub mod listing;
pub mod ops;
pub mod places;
pub mod preview;
pub mod storage;
#[cfg(test)]
pub mod ops_tests;
pub mod app;
pub mod demo;
pub mod keys;
pub mod mosaic;
pub mod preview_cli;
pub mod shell;
pub mod view;
#[cfg(target_os = "linux")]
pub mod wl;
#[cfg(test)]
mod app_tests;
