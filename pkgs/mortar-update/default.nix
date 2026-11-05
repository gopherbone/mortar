# SPDX-License-Identifier: GPL-2.0-or-later
# mortar-update: over-the-air updates on the Brick (modules/core/update.nix,
# docs/updates.md). The library part (bucket access, and the manifest from
# ../mortar-index) is shared with the `mortar` CLI's `publish`.
{ lib, rustPlatform }:

rustPlatform.buildRustPackage {
  pname = "mortar-update";
  version = "0.1.0";
  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./src ../mortar-index/Cargo.toml ../mortar-index/src ];
  };
  sourceRoot = "source/mortar-update";
  cargoLock.lockFile = ./Cargo.lock;

  meta = {
    description = "Over-the-air updates for Mortar from a Nix binary cache";
    license = lib.licenses.gpl2Plus;
    platforms = lib.platforms.linux ++ lib.platforms.darwin;
    mainProgram = "mortar-update";
  };
}
