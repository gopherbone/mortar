# SPDX-License-Identifier: GPL-2.0-or-later
# mortar-setup: first-boot setup and remote access on the Brick.
#   mortar-setup apply    mortar.toml from the card (boot service)
#   mortar-setup daemon   pairing service on usb0, control socket, Wi-Fi
#   mortar-setup ui       first-boot screens (KMS, mortar-ui toolkit)
# Path dependencies on mortar-ui (toolkit) and mortar-pair (protocol).
# The checks include the pairing protocol end to end on localhost.
{
  lib,
  rustPlatform,
  callPackage,
}:

let
  fonts = callPackage ../mortar-ui/fonts.nix { };
in
rustPlatform.buildRustPackage {
  pname = "mortar-setup";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./mortar.toml
      ./src
      ./tests
      ../mortar-ui/Cargo.toml
      ../mortar-ui/src
      ../mortar-pair/Cargo.toml
      ../mortar-pair/src
    ];
  };
  sourceRoot = "source/mortar-setup";
  cargoLock.lockFile = ./Cargo.lock;

  env.MORTAR_UI_FONT_DIR = "${fonts}/share/fonts/mortar-ui";

  passthru = {
    inherit fonts;
    # the blank setup file put on new images (modules/core/setup.nix)
    template = ./mortar.toml;
  };

  meta = {
    description = "First-boot setup, pairing and remote-access settings for Mortar on the TrimUI Brick";
    license = lib.licenses.gpl2Plus;
    platforms = lib.platforms.linux ++ lib.platforms.darwin;
    mainProgram = "mortar-setup";
  };
}
