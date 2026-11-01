# SPDX-License-Identifier: GPL-2.0-or-later
# mortar-update: over-the-air updates on the Brick (modules/mortar-update.nix,
# docs/updates.md). The library part (manifest, bucket access) is shared
# with the `mortar` CLI's `publish`.
{ lib, rustPlatform }:

rustPlatform.buildRustPackage {
  pname = "mortar-update";
  version = "0.1.0";
  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./src ];
  };
  cargoLock.lockFile = ./Cargo.lock;

  meta = {
    description = "Over-the-air updates for Mortar from a private Nix binary cache";
    license = lib.licenses.gpl2Plus;
    platforms = lib.platforms.linux ++ lib.platforms.darwin;
    mainProgram = "mortar-update";
  };
}
