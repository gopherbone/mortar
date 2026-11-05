// SPDX-License-Identifier: GPL-2.0-or-later
//! Packages: installing software from the Brick, with the pad and no computer. The app
//! and the service behind it. See DESIGN.md.

pub mod app;
pub mod backend;
pub mod demo;
pub mod preview_cli;
pub mod protocol;
pub mod service;
pub mod view;
#[cfg(test)]
mod app_tests;
