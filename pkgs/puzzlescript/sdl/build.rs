// SPDX-License-Identifier: MIT
// Compiles the platform-independent C engine core (../src) plus the layout probe.
use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let core = root.join("../src");
    let files = [
        "ps_util.c", "ps_rng.c", "ps_sfx.c", "ps_font.c", "ps_engine.c", "ps_parse.c", "ps_compile.c",
    ];
    let mut b = cc::Build::new();
    b.include(&core)
        .flag_if_supported("-std=c99")
        // glibc hides clock_gettime (used by ps_compile.c's timing) under strict -std=c99
        .define("_DEFAULT_SOURCE", None)
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-sign-compare")
        .flag_if_supported("-Wno-unused-function")
        .opt_level(2)
        .warnings(false);
    for f in files {
        let p = core.join(f);
        println!("cargo:rerun-if-changed={}", p.display());
        b.file(p);
    }
    b.file(root.join("csrc/layout.c"));
    println!("cargo:rerun-if-changed=csrc/layout.c");
    println!("cargo:rerun-if-changed={}", core.join("ps.h").display());
    println!("cargo:rerun-if-changed={}", core.join("ps_compile_internal.h").display());
    b.compile("pscore");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        println!("cargo:rustc-link-lib=m");
    }
}
