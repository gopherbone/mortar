# SPDX-License-Identifier: GPL-2.0-or-later
#
# OpenXcom (GPLv3), the open-source engine for UFO: Enemy Unknown, for the
# Brick. SDL 1.2 API through sdl12-compat, sdl2-compat and our sdl3-pvr; no
# desktop GL (the GPU is GLES only), so the engine's OpenGL path is left out
# and SDL scales the 8-bit picture on the GPU.
#
# Data: the user's own copy of the game in
# $XDG_DATA_HOME/openxcom/UFO, put there by `mortar xcom --data PATH`. It is
# never in the Nix store, a cache or a repository.
#
# Picture: the game renders 342x256 (SDL 1.2 compat refuses odd widths) and
# SDL scales it to the 1024x768 panel, about 3x. 48% more battle map than 320x200; popups stay 320x200.
# Controls, by the Brick's labels; every screen keeps the original UI and
# OPTIONS > CONTROLS lists them in the game. MENU is the shell's.
#
#   Everywhere   D-pad moves focus. A press, B back, START OK, SELECT next group,
#                L1/R1 previous/next, L2/R2 page, F1 quicksave, hold F2 quickload.
#   Battlescape  D-pad tile cursor (two directions: one step along the grid).
#                A select/walk (A again confirms the previewed path)/fire,
#                B cancel or recentre, X kneel, Y turn, L1/R1 hands, L2/R2
#                soldiers (aiming: aliens), START inventory, hold START end
#                turn, SELECT the bottom panel. SELECT+Up/Down level,
#                +Left/Right aliens, +X reload, +A run or strafe.
#   Geoscape     D-pad turns the globe under a reticle, A opens what's under it,
#                L1/R1 contacts, L2/R2 zoom, X faster, Y 5 seconds, START
#                intercept, SELECT the buttons.
#   Inventory    A pick up/place, B put back, X unload, Y auto-place, L1/R1
#                soldiers, L2/R2 scroll ground, START done.
#   Base         D-pad facility grid, SELECT buttons, X the facility's other
#                action, L1/R1 bases.
#   Lists        Left/Right less/more, X all or none, X+Up/Down reorder.
#
# Leaving saves the game (Load Game, the autosave). Time stops while another
# app is on screen.
{
  lib,
  stdenv,
  fetchFromGitHub,
  cmake,
  pkg-config,
  sdl12-compat,
  SDL_gfx,
  SDL_image,
  SDL_mixer,
  yaml-cpp,
  zlib,
  coreutils,
  writeShellApplication,
}:

let
  engine = stdenv.mkDerivation {
    pname = "openxcom";
    version = "1.0.0-unstable-2026-09-28";

    src = fetchFromGitHub {
      owner = "OpenXcom";
      repo = "OpenXcom";
      rev = "4a4bae5fa2e5a09282928e84943fc84b093e34ff";
      hash = "sha256-hgW0n1BzzQe0guXJWF9KlDcpWz1JpFFJ6IGPP1nZgTA=";
    };

    # Mortar's gamepad layer: src/Engine/PadNav.* and a few small hooks.
    patches = [ ./mortar.patch ];

    nativeBuildInputs = [ cmake pkg-config ];
    buildInputs = [ sdl12-compat SDL_gfx SDL_image SDL_mixer yaml-cpp zlib ];

    cmakeFlags = [
      (lib.cmakeBool "CMAKE_DISABLE_FIND_PACKAGE_OpenGL" true)
      (lib.cmakeBool "DEV_BUILD" false)
      (lib.cmakeBool "BUILD_PACKAGE" false)
      (lib.cmakeFeature "CMAKE_BUILD_TYPE" "Release")
    ];
    enableParallelBuilding = true;

    meta = {
      description = "Open-source engine for UFO: Enemy Unknown, Brick controls";
      homepage = "https://openxcom.org";
      license = lib.licenses.gpl3Plus;
      platforms = lib.platforms.linux;
      mainProgram = "openxcom";
    };
  };

  # First-run settings only; the in-game Options can change them afterwards.
  defaults = builtins.toFile "options.cfg" ''
    options:
      displayWidth: 342
      displayHeight: 256
      fullscreen: true
      allowResize: false
      geoscapeScale: 5
      battlescapeScale: 5
      useOpenGL: false
      keepAspectRatio: true
      nonSquarePixelRatio: false
      captureMouse: 0
      battleEdgeScroll: 0
      battleNewPreviewPath: 3
      pauseMode: 3
      FPS: 30
      FPSInactive: 15
      keyboardMode: 0
  '';

  launcher = writeShellApplication {
    name = "openxcom";
    runtimeInputs = [ coreutils ];
    text = ''
      cfg="''${XDG_CONFIG_HOME:-$HOME/.config}/openxcom"
      mkdir -p "$cfg"
      if [ ! -e "$cfg/options.cfg" ]; then
        install -m 0644 ${defaults} "$cfg/options.cfg"
      fi
      export SDL12COMPAT_SCALE_METHOD=nearest
      exec ${engine}/bin/openxcom "$@"
    '';
  };
in
launcher.overrideAttrs (old: {
  passthru = (old.passthru or { }) // { inherit engine; };
  meta = engine.meta;
})
