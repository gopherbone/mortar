// SPDX-License-Identifier: GPL-2.0-or-later
// The libretro log callback is variadic (printf-style); stable Rust cannot
// define variadic functions, so a few lines of C format the message and
// hand it to Rust (csrc/log_shim.c).
fn main() {
    println!("cargo:rerun-if-changed=csrc/log_shim.c");
    cc::Build::new().file("csrc/log_shim.c").warnings(true).compile("mortar_play_shim");
}
