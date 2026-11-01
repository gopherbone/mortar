#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-or-later
# Dev mode on macOS (or a Linux desktop): mortar-play in an SDL window.
#
#   ./dev.sh                      library UI
#   ./dev.sh run nes ~/Games/nes/some.nes
#
# Fonts and SDL3 come from the nixpkgs pinned in flake.lock. nixpkgs'
# libretro cores don't build on darwin (they pull in retroarch), so a few
# small free cores (fceumm, gambatte, 2048) are built here from their nixpkgs
# sources with the cores' own makefiles, into ~/.cache/mortar-play-dev.
# Games: ~/Games/<system>/ (nes, gb, gbc); the built-in NES test program is
# put there if the folder is empty.
#
# Keys: arrows = D-pad, X = A, Z = B, S = X, A = Y, Q/W = L1/R1, 1/2 = L2/R2,
# Return = START, Right Shift = SELECT, Esc/Tab = MENU (in-game menu),
# F1 = menu, Space/F2 = fast-forward on F2 systems, 2 = R2 (fast-forward).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
cache=${XDG_CACHE_HOME:-$HOME/.cache}/mortar-play-dev
mkdir -p "$cache/cores"
lock=$here/../../flake.lock
expr() {
  cat <<EOF
let
  lock = (builtins.fromJSON (builtins.readFile $lock)).nodes.nixpkgs.locked;
  nixpkgs = builtins.getFlake "github:\${lock.owner}/\${lock.repo}/\${lock.rev}";
  pkgs = nixpkgs.legacyPackages.\${builtins.currentSystem};
in $1
EOF
}
nb() { nix build --no-link --print-out-paths --impure --expr "$(expr "$1")"; }

fonts=$(nb "pkgs.callPackage $here/../mortar-ui/fonts.nix { }")
sdl=$(nb "pkgs.symlinkJoin { name = \"sdl3-dev\"; paths = [ pkgs.sdl3 pkgs.sdl3.dev ]; }")
export MORTAR_UI_FONTS=$fonts/share/fonts/mortar-ui MORTAR_UI_FONT_DIR=$fonts/share/fonts/mortar-ui
export PKG_CONFIG_PATH=$sdl/lib/pkgconfig LIBRARY_PATH=$sdl/lib DYLD_FALLBACK_LIBRARY_PATH=$sdl/lib

ext=so; plat=unix
[ "$(uname)" = Darwin ] && { ext=dylib; plat=osx; }
build_core() { # attr, makefile, output name
  local out=$cache/cores/$3_libretro.$ext
  [ -e "$out" ] && return
  local src; src=$(nb "pkgs.libretro.$1.src")
  rm -rf "$cache/build/$1"; mkdir -p "$cache/build"; cp -r "$src" "$cache/build/$1"; chmod -R u+w "$cache/build/$1"
  local dir=$cache/build/$1
  (cd "$dir" && make -f "$2" platform=$plat -j"$(getconf _NPROCESSORS_ONLN)" >/dev/null 2>&1)
  cp "$dir/$3_libretro.$ext" "$out"
}
build_core fceumm Makefile.libretro fceumm
build_core twenty-fortyeight Makefile.libretro 2048
build_core gambatte Makefile gambatte || echo "gambatte: build failed, GB disabled"

roms=$HOME/Games
mkdir -p "$roms/nes" "$roms/gb" "$roms/gbc"
cd "$here"
if [ -z "$(ls -A "$roms/nes")" ]; then cargo run -q --release -- make-test-rom "$roms/nes/mortar-test.nes"; fi
cat > "$cache/config.json" <<EOF
{
  "romDirs": ["$roms"],
  "biosDir": "$cache/bios", "saveDir": "$cache/saves", "stateDir": "$cache/states", "dataDir": "$cache",
  "systems": [
    {"id": "nes", "name": "NES", "tag": "NES", "folders": ["nes"], "extensions": ["nes", "fds"],
     "core": "$cache/cores/fceumm_libretro.$ext", "options": {"fceumm_aspect": "4:3"}, "order": 10},
    {"id": "gb", "name": "Game Boy", "tag": "GB", "folders": ["gb"], "extensions": ["gb"], "scaling": "integer",
     "core": "$cache/cores/gambatte_libretro.$ext", "options": {"gambatte_gb_colorization": "auto"}, "order": 20},
    {"id": "gbc", "name": "Game Boy Color", "tag": "GBC", "folders": ["gbc"], "extensions": ["gbc"], "scaling": "integer",
     "core": "$cache/cores/gambatte_libretro.$ext", "order": 21},
    {"id": "2048", "name": "2048", "tag": "2048", "core": "$cache/cores/2048_libretro.$ext", "noContent": true, "order": 90}
  ]
}
EOF
exec cargo run --release -- --config "$cache/config.json" --display sdl "$@"
