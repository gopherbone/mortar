# SPDX-License-Identifier: GPL-2.0-or-later
# mortar-index: signed manifests and package indexes, what the publisher and a Brick
# agree on. A library for pkgs/mortar-update, mortar-cli and mortar-packages. Built on
# its own for its tests (checks.*.mortar-index), which include searching an index
# the size of nixpkgs and counting every allocation.
{ lib, rustPlatform }:

rustPlatform.buildRustPackage {
  pname = "mortar-index";
  version = "0.1.0";
  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./src ./tests ];
  };
  cargoLock.lockFile = ./Cargo.lock;
  buildFeatures = [ "device" ];
  checkFeatures = [ "device" ];
  doCheck = true;

  meta = {
    description = "What a Mortar publisher and a Brick agree on: signed manifests and indexes";
    license = lib.licenses.gpl2Plus;
    platforms = lib.platforms.linux ++ lib.platforms.darwin;
  };
}
