# SPDX-License-Identifier: GPL-2.0-or-later
# mortar-ui: Mortar's on-device UI toolkit, greeter (greetd) and quick menu.
# Pure Rust, software rendered: KMS dumb buffers + evdev on Linux, a minifb
# window on macOS for development. Fonts are baked in by store path.
{
  lib,
  rustPlatform,
  callPackage,
}:

let
  fonts = callPackage ./fonts.nix { };
in
rustPlatform.buildRustPackage {
  pname = "mortar-ui";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./src
      ./examples
    ];
  };

  cargoLock.lockFile = ./Cargo.lock;

  env.MORTAR_UI_FONT_DIR = "${fonts}/share/fonts/mortar-ui";

  passthru = { inherit fonts; };

  meta = {
    description = "Controller-driven UI toolkit and greetd greeter for Mortar on the TrimUI Brick";
    license = lib.licenses.gpl2Plus;
    platforms = lib.platforms.linux ++ lib.platforms.darwin;
    mainProgram = "mortar-ui";
  };
}
