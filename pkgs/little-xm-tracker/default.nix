# SPDX-License-Identifier: GPL-2.0-or-later
#
# Little XM Tracker (BSD-3-Clause): a FastTracker II .xm tracker written for
# composing with a gamepad on the TrimUI Brick (upstream targets KNULLI).
# C++17, SDL2 (here sdl2-compat on our sdl3-pvr), playback through unmodified
# MilkyPlay sources. It draws a fixed 1024x768 canvas, the Brick's panel,
# 1:1. No sample songs ship upstream; it opens on its own demo document,
# whose PCM data is original and under the same licence.
#
# Build: upstream's CMake with the two pinned source snapshots it would fetch
# itself. Only the BSD src/milkyplay files of MilkyTracker get compiled (none
# of its GPL GUI); from miniaudio (MIT-0) only WAV/FLAC/MP3 and stb_vorbis
# for sample import. mortar.patch documents the small changes (MORTAR_BUILD).
#
#   bin/little-xm-tracker   launcher: makes the data folders, runs fullscreen
#
# Data: $XDG_DATA_HOME/LittleXMTracker (the session user's
# ~/.local/share/LittleXMTracker): songs/, instruments/, samples/, themes/,
# editor-settings.txt, recovery slots, gamecontrollerdb.txt. The app finds
# its read-only assets (font, theme presets) beside its binary in the store
# and copies missing themes and the mapping file over; existing ones stay.
#
# Controls, by the Brick's labels (SDL gamepad; X, Y, L, R, F1, F2 are unused,
# MENU is the shell's; in the app, Actions > CONTROLS and the manual in
# share/doc/little-xm-tracker/manual.html):
#
#   D-pad                 move the cursor, change focus
#   A / A+D-pad           enter, confirm / change a value
#   A+B                   cut or clear the field or selection
#   B                     copy the selection; back or cancel in dialogs
#   B+D-pad               page through patterns, instruments and samples
#   SELECT+A              paste
#   SELECT+Left/Right     change screen (Song, Pattern, Instrument, Sample)
#   SELECT                cancel the selection, else open Actions
#   hold SELECT, tap B    start, expand or cancel a selection
#   START                 play/stop (pattern or song), audition in Instrument
#                         and Sample
#   SELECT+START          play the song from the top / stop
#   hold B, SELECT/START  mute / solo the channel
#
# Press and hold the first button of a chord before the second. Quit with
# Actions > EXIT on the Song screen (or close it from the MENU): work is kept
# in the recovery slots and reopened on the next start.
#
# Audio: SDL picks PipeWire; 44.1 kHz by default, 512 frames (11.6 ms). No
# MIDI in upstream (USB MIDI does nothing here). Recording uses a USB audio
# input if one is plugged in.
{
  lib,
  stdenv,
  fetchFromGitHub,
  cmake,
  pkg-config,
  SDL2,
  coreutils,
  writeShellApplication,
}:

let
  milkytracker-src = fetchFromGitHub {
    owner = "milkytracker";
    repo = "MilkyTracker";
    rev = "e3ceb05105e8c5bdc907d6451245bf5a1926b1f9"; # cmake/Dependencies.cmake
    hash = "sha256-pySdZtzOxp/51kbNJYuKH9SjdEJJbrGsLZnTLYYVGJY=";
  };
  miniaudio-src = fetchFromGitHub {
    owner = "mackron";
    repo = "miniaudio";
    rev = "f40cf03f80cdb7e741d43e53b7e706e8c1394bcf"; # 0.11.23
    hash = "sha256-ZrfKw5a3AtIER2btCKWhuvygasNaHNf9EURf1Kv96Vc=";
  };

  tracker = stdenv.mkDerivation (finalAttrs: {
    pname = "little-xm-tracker";
    version = "0.9.0";

    src = fetchFromGitHub {
      owner = "yoonsangmin";
      repo = "little-xm-tracker";
      rev = "9fd7d42acc93ed704696172cf0d58b77b95457b1";
      hash = "sha256-+tMoGRbAuQF7DrlFANYclWZL45iIcxxFLLI6z4TxhXo=";
    };

    patches = [ ./mortar.patch ];

    nativeBuildInputs = [ cmake pkg-config ];
    buildInputs = [ SDL2 ];

    cmakeFlags = [
      (lib.cmakeFeature "MILKYTRACKER_SOURCE_DIR" "${milkytracker-src}")
      (lib.cmakeFeature "MINIAUDIO_SOURCE_DIR" "${miniaudio-src}")
      (lib.cmakeFeature "CMAKE_CXX_FLAGS" "-DMORTAR_BUILD")
      (lib.cmakeBool "TRACKER_BUILD_TESTS" finalAttrs.doCheck)
    ];

    # Upstream's codec, audio and gamepad flow tests, on SDL's dummy drivers.
    doCheck = true;
    env.CTEST_OUTPUT_ON_FAILURE = 1;
    enableParallelBuilding = true;

    # SDL_GetBasePath() is where the binary really is: assets/ goes beside it.
    installPhase = ''
      runHook preInstall
      install -Dm0755 littlexm-tracker $out/libexec/little-xm-tracker/littlexm-tracker
      cp -r ../assets $out/libexec/little-xm-tracker/assets
      doc=$out/share/doc/little-xm-tracker
      mkdir -p $doc
      cp -r ../docs/manual.html ../LICENSE ../THIRD_PARTY_NOTICES.txt ../licenses $doc/
      runHook postInstall
    '';

    meta = {
      description = "FastTracker II XM music tracker for composing with a gamepad";
      homepage = "https://github.com/yoonsangmin/little-xm-tracker";
      license = with lib.licenses; [ bsd3 zlib mit mit0 ofl ];
      platforms = lib.platforms.linux;
    };
  });

  launcher = writeShellApplication {
    name = "little-xm-tracker";
    runtimeInputs = [ coreutils ];
    text = ''
      dir="''${XDG_DATA_HOME:-$HOME/.local/share}/LittleXMTracker"
      # The app never creates these; they are where its file dialogs open.
      mkdir -p "$dir/songs" "$dir/instruments" "$dir/samples"
      # Start at PipeWire's rate instead of upstream's 48 kHz, so songs aren't
      # resampled; only for a new install, Settings > Audio can change it.
      if [ ! -e "$dir/editor-settings.txt" ]; then
        echo "output_sample_rate 44100" > "$dir/editor-settings.txt"
      fi
      exec ${tracker}/libexec/little-xm-tracker/littlexm-tracker --fullscreen --data-dir "$dir" "$@"
    '';
  };
in
launcher.overrideAttrs (old: {
  passthru = (old.passthru or { }) // { inherit tracker; };
  meta = tracker.meta // { mainProgram = "little-xm-tracker"; };
})
