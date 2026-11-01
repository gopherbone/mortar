# SPDX-License-Identifier: GPL-2.0-or-later
# `mortar` CLI: build Mortar here and deploy it to a Brick over SSH.
{ lib, rustPlatform, makeWrapper, openssh }:

rustPlatform.buildRustPackage {
  pname = "mortar";
  version = "0.1.0";
  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./src ];
  };
  cargoLock.lockFile = ./Cargo.lock;

  nativeBuildInputs = [ makeWrapper ];
  # `nix` is taken from the user's PATH on purpose (their own install and
  # its builder configuration); ssh comes with the package.
  postInstall = ''
    wrapProgram $out/bin/mortar --suffix PATH : ${lib.makeBinPath [ openssh ]}
  '';

  meta = {
    description = "Build Mortar and deploy it to a TrimUI Brick over SSH";
    license = lib.licenses.gpl2Plus;
    mainProgram = "mortar";
  };
}
