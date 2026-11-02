# SPDX-License-Identifier: GPL-2.0-or-later
# Animal Crossing (GameCube) as a native aarch64 app: the ac-decomp
# decompilation (ACreTeam, CC0) with the PC port layer (flyngmt/ACGC-PC-Port,
# MIT), built from digdat0/ACGC-Switch-Port, which carries zwaetschge's
# Android port (64-bit work by birabittoh and chasem-dev, GX on OpenGL ES 3)
# plus later 64-bit fixes (NPC/structure actor slots, audio pointers, town
# tune). See CMakeLists.txt for how the Mortar build maps onto it.
#
# This package contains NO game data. The game reads everything (models,
# textures, sound, text) from the user's own disc image at run time:
# Animal Crossing USA, GAFE01 revision 0 (plain ISO/GCM, CISO, or an NKit
# image whose files sit at their original offsets). `mortar acgc --iso PATH`
# checks the image and copies it to the Brick as
# ~brick/.local/share/acgc/rom/GAFE01.iso. The disc image must never be
# committed, fetched into the Nix store, or pushed to CI or a cache.
#
# The engine itself is free (CC0 + MIT), but it only exists to run a
# Nintendo game, so Mortar treats it like PICO-8: it is not part of the
# system closure (CI never builds it, the update cache never holds it);
# `mortar acgc` builds it and installs it into its own profile on the Brick,
# /nix/var/nix/profiles/acgc (modules/mortar-apps.nix).
#
#   bin/acgc                 launcher: data dir, shaders link, ROM check
#   libexec/acgc/AnimalCrossing
#   share/acgc/shaders/      default.vert/.frag (rewritten to GLSL ES at run time)
#
# Data dir ($XDG_DATA_HOME/acgc, i.e. ~/.local/share/acgc): rom/GAFE01.iso,
# save/card_a (Dolphin-compatible GCI saves), settings.ini, keybindings.ini.
#
# Controls on the Brick (no analog sticks), by label:
#   D-pad          main stick (walk; hold B to run)
#   L2 + D-pad     GameCube D-pad (letter / pattern editors, tool switching)
#   L2 + X/B/Y/A   C-stick up / down / left / right (by position; camera,
#                  diary pages)
#   A B X Y        A B X Y
#   L1 R1          L R
#   R2             Z
#   START          START
#   SELECT         the port's pause menu (settings, keybindings, quit)
#   MENU           the shell's
# A USB/Bluetooth pad with sticks gets them as the main and C-stick.
{ lib, stdenv, fetchFromGitHub, cmake, pkg-config, SDL2, libGL, makeWrapper }:

stdenv.mkDerivation {
  pname = "acgc";
  version = "1.0.1-unstable-2026-10-29";

  src = fetchFromGitHub {
    owner = "digdat0";
    repo = "ACGC-Switch-Port";
    rev = "4f85b86736969495a1ee40da3f0ad70732bf6297";
    hash = "sha256-s83ecJhqUAANz6TT1vX0PSEVmCO8fGRM8drm6wu+rsk=";
  };

  # The Brick build files go in as mortar/ next to the Switch build's switch/.
  postUnpack = ''
    mkdir -p $sourceRoot/mortar/compat/glad
    cp ${./CMakeLists.txt} $sourceRoot/mortar/CMakeLists.txt
    cp ${./mortar_glue.c} $sourceRoot/mortar/mortar_glue.c
    cp ${./mortar_decls.h} $sourceRoot/mortar/mortar_decls.h
    cp ${./cxx_launcher.sh} $sourceRoot/mortar/cxx_launcher.sh
    cp ${./compat/glad/gl.h} $sourceRoot/mortar/compat/glad/gl.h
  '';

  # pc_main: load GLES through SDL; pc_pad/pc_keybindings/pc_pause_menu: Brick
  # buttons; libultra.h: glibc declares bcmp/bcopy/bzero.
  patches = [ ./mortar.patch ];

  cmakeDir = "../mortar";
  nativeBuildInputs = [ cmake pkg-config makeWrapper ];
  # libGL: the GLES 3.2 headers only (nothing links GL; SDL loads it)
  buildInputs = [ SDL2 libGL ];

  env.OBJCOPY = "${stdenv.cc.targetPrefix}objcopy";
  enableParallelBuilding = true;

  postInstall = ''
    mkdir -p $out/bin
    cat > $out/bin/acgc <<EOF
    #!${stdenv.shell}
    # Animal Crossing: runs in \$XDG_DATA_HOME/acgc (rom/, save/, settings.ini).
    set -e
    data="\''${XDG_DATA_HOME:-\$HOME/.local/share}/acgc"
    mkdir -p "\$data/rom"
    cd "\$data"
    if [ ! -e rom/GAFE01.iso ]; then
      echo "acgc: no disc image: put your Animal Crossing (USA, GAFE01) ISO at \$data/rom/GAFE01.iso (mortar acgc --iso PATH)" >&2
      exit 1
    fi
    # the shaders this build expects (a store path, so relink every start)
    rm -rf shaders
    ln -s $out/share/acgc/shaders shaders
    # compiled shader programs belong to one build (and one GPU driver)
    if [ "\$(cat .build 2>/dev/null)" != "$out" ]; then
      rm -f shader_binaries.bin
      echo "$out" > .build
    fi
    # Without them the game compiles its shaders before the window appears
    # (~20 s): say so inside mortar-shell (\$MORTAR_APP: started by the shell).
    if [ ! -e shader_binaries.bin ] && [ -n "\''${MORTAR_APP:-}" ] && command -v mortar-shell >/dev/null; then
      mortar-shell toast --sound none --secs 20 "Animal Crossing" "Preparing graphics (about 20 s)" >/dev/null 2>&1 || true
    fi
    exec $out/libexec/acgc/AnimalCrossing "\$@"
    EOF
    chmod +x $out/bin/acgc
  '';

  meta = {
    description = "Animal Crossing (GameCube) native port; needs the user's own GAFE01 disc image";
    homepage = "https://github.com/digdat0/ACGC-Switch-Port";
    # CC0 decomp + MIT port layer, but it only runs a Nintendo game: keep it
    # off CI and caches like PICO-8 (flake.nix mortarUnfree).
    license = lib.licenses.unfree;
    platforms = [ "aarch64-linux" ];
    mainProgram = "acgc";
  };
}
