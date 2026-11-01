#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-or-later
# Dev mode: run mortar-ui in a desktop window (macOS) with a fake greetd,
# demo status values and the built-in demo config.
#
#   ./dev.sh                    greeter
#   ./dev.sh quick-menu --session games
#   ./dev.sh greeter --config examples/mockup-config.json
#
# Keys: arrows = D-pad, Enter = A, Esc/Backspace = B, Q/E = L1/R1,
# Tab or M = MENU, S = START, digits type PINs (the "kid" profile's is 1234).
# Fonts come from fonts.nix, built with the nixpkgs pinned in flake.lock.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
fonts=$(nix build --no-link --print-out-paths --impure --expr "
  let
    lock = (builtins.fromJSON (builtins.readFile $here/../../flake.lock)).nodes.nixpkgs.locked;
    nixpkgs = builtins.getFlake \"github:\${lock.owner}/\${lock.repo}/\${lock.rev}\";
  in nixpkgs.legacyPackages.\${builtins.currentSystem}.callPackage $here/fonts.nix { }")
export MORTAR_UI_FONTS=$fonts/share/fonts/mortar-ui
cd "$here"
[ $# -eq 0 ] && set -- greeter
exec cargo run --release -- "$@" --dev
