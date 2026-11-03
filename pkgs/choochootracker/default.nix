# SPDX-License-Identifier: GPL-2.0-or-later
#
# ChooChooTracker (MIT): a multi-engine music tracker / groovebox forked
# from ChipNomad (AY, OPL, OPN, SID, DX7, Plaits, Braids, 303, samples and
# soundfonts ...). Upstream ships it for PortMaster handhelds, Linux,
# Android and the web.
#
# Build: the Linux desktop build (DESKTOP_BUILD: SDL2 gamepad, ALSA-seq
# MIDI via RtMidi, Ableton Link, audio export) plus MORTAR_BUILD
# (mortar.patch: native-size fullscreen, Brick button labels, 44.1 kHz,
# read-only install). Upstream's Makefile.linux compiles everything in one
# compiler call; Makefile.portmaster has per-object rules, so it drives the
# build here with the Linux defines and native g++ instead of its own
# PortMaster ones. Ableton Link and asio are git submodules (headers only).
#
#   bin/choochootracker   launcher: seeds the data directory, cds there,
#                         execs the tracker
#
# Data: $XDG_DATA_HOME/ChooChooTracker (the session user's
# ~/.local/share/ChooChooTracker): settings.txt, autosave.cct,
# projects/, samples/, instruments/ (incl. USER/soundfont), themes/,
# fonts/, wavetables, pitch tables, exports. The bundled content
# (share/choochootracker/common) is copied in once per top-level folder,
# only if missing, so it is the user's to edit; title/ and licenses/ are
# read-only and stay symlinks into the store. Delete a folder there to get
# the bundled copy back on the next start; delete settings.txt to reset
# settings (including the button mapping).
#
# Controls (SDL gamepad; the Brick's gpio-keys pad, or inside mortar-shell
# its per-app virtual clone, gets SDL's automatic evdev mapping):
#
#   ChooChoo  Brick    evdev / SDL gamepad
#   --------  -------  ---------------------------------
#   arrows    D-pad    BTN_DPAD_* / dpad
#   EDIT      A        BTN_EAST   / b      (right face button)
#   OPT       B        BTN_SOUTH  / a      (bottom face button)
#   PLAY      START    BTN_START  / start
#   SHIFT     SELECT   BTN_SELECT / back
#   Live      L1       BTN_TL     / leftshoulder
#   Record    L2       BTN_TL2    / left trigger
#   Erase     R2       BTN_TR2    / right trigger
#
# (the Live Stick motion keys; X, Y, R1, F1, F2 are free). Settings >
# Controls remaps them (three slots per action); the key-mapping screen
# shows the Brick's labels. MENU is the shell's (alone: overlay); quit with
# Settings > "Quit ChooChooTracker".
#
# Screen: 40x20 text cells; at 1024x768 the 24x36 font gives a 960x720 grid
# centred with a 32/24 px border, integer pixels, no scaling.
#
# Audio: SDL picks PipeWire (the session's sound server, modules/core/audio.nix);
# 44.1 kHz, 512 frames (11.6 ms) by default, both changeable in Settings >
# Audio. USB audio interfaces and Bluetooth headphones appear as output
# devices there too. SDL_AUDIO_DRIVER=alsa still works (through
# pipewire-alsa, or the codec's dmix when PipeWire is off).
#
# MIDI: RtMidi on the ALSA sequencer only (in/out, clock in/out). The
# Brick kernel has SND_USB_AUDIO and SND_RAWMIDI but not SND_SEQUENCER, so
# the MIDI port lists are empty (the tracker runs fine without). USB MIDI
# needs SND_SEQUENCER (snd-seq, /dev/snd/seq) and SND_SEQ_MIDI (snd-seq-midi,
# the rawmidi -> sequencer bridge, selected with SEQUENCER + RAWMIDI).
{
  lib,
  stdenv,
  fetchFromGitHub,
  pkg-config,
  SDL2,
  alsa-lib,
  coreutils,
  writeShellApplication,
}:

let
  tracker = stdenv.mkDerivation (finalAttrs: {
    pname = "choochootracker";
    version = "1.0.21-alpha-unstable-2026-10-30";

    src = fetchFromGitHub {
      owner = "paiheulevrai";
      repo = "Choochootracker";
      rev = "72bedf66808444df4b9318563500104bad1a0405";
      fetchSubmodules = true; # ableton_link, asio (Link on Linux)
      hash = "sha256-QhyzTDVRpQZWYKCs1OeQ9536I4GrH0eXVWgLPEyTFFw=";
    };

    patches = [ ./mortar.patch ];

    nativeBuildInputs = [ pkg-config ];
    buildInputs = [ SDL2 alsa-lib ];

    enableParallelBuilding = true;

    # Makefile.portmaster as a per-object build driver, with the Linux
    # (Makefile.linux) defines; the single-quoted $(...) are make's.
    # -O3 for the whole app as upstream's ARM handheld build does (the UI
    # and DSP are CPU-bound on the A53s); no -g (stripped anyway).
    preBuild = ''
      cd tracker
      makeFlagsArray+=(
        -f Makefile.portmaster
        "CXX=$CXX"
        STRIP=true
        'COMMON_CFLAGS=-std=c++17 -Wall -Os -flto=auto -DTEST'
        'CFLAGS=$(PORTMASTER_CFLAGS) $(INCLUDES) $(SDL_CFLAGS) -DDESKTOP_BUILD -DMORTAR_BUILD -DGAMEPAD_SUPPORT -D__LINUX_ALSA__ -D__LINUX_ALSASEQ__ -DLINK_PLATFORM_LINUX=1 -DASIO_STANDALONE=1 -DASIO_NO_TYPEID=1 -DASIO_VERSION_NAMESPACE=link_asio_1_38_2 -Wno-multichar'
        "SDL_CFLAGS=$(pkg-config --cflags sdl2)"
        "SDL_LIBS=$(pkg-config --libs sdl2)"
      )
    '';
    buildFlags = [ "PortMaster" ];

    installPhase = ''
      runHook preInstall
      install -Dm0755 build/portmaster/choochootracker.aarch64 $out/libexec/choochootracker/choochootracker
      mkdir -p $out/share/choochootracker $out/share/doc/choochootracker/licenses
      cp -r packaging/common $out/share/choochootracker/common
      cp ../docs/USER_MANUAL.md $out/share/doc/choochootracker/
      cp ../LICENSE $out/share/doc/choochootracker/licenses/LICENSE.choochootracker.md
      cp ../chipnomad_lib/external/insert_fx/LICENSE-JM-Formant packaging/portmaster/license/* \
        $out/share/doc/choochootracker/licenses/
      runHook postInstall
    '';

    meta = {
      description = "Multi-engine chiptune/groovebox music tracker (ChipNomad fork)";
      homepage = "https://github.com/paiheulevrai/Choochootracker";
      license = lib.licenses.mit;
      platforms = lib.platforms.linux;
    };
  });

  launcher = writeShellApplication {
    name = "choochootracker";
    runtimeInputs = [ coreutils ];
    text = ''
      dir="''${XDG_DATA_HOME:-$HOME/.local/share}/ChooChooTracker"
      common=${tracker}/share/choochootracker/common
      mkdir -p "$dir"
      for item in "$common"/*; do
        name=$(basename "$item")
        case "$name" in
          title|licenses)
            # read-only: follow the current store path
            if [ -L "$dir/$name" ] || [ ! -e "$dir/$name" ]; then
              ln -sfn "$item" "$dir/$name"
            fi
            ;;
          *)
            if [ ! -e "$dir/$name" ]; then
              cp -r "$item" "$dir/$name.tmp.$$"
              chmod -R u+w "$dir/$name.tmp.$$"
              mv "$dir/$name.tmp.$$" "$dir/$name"
            fi
            ;;
        esac
      done
      # The title screen and relative paths in settings.txt resolve from here.
      cd "$dir"
      exec ${tracker}/libexec/choochootracker/choochootracker "$@"
    '';
  };
in
launcher.overrideAttrs (old: {
  passthru = (old.passthru or { }) // { inherit tracker; };
  meta = tracker.meta // { mainProgram = "choochootracker"; };
})
