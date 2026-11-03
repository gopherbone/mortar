# SPDX-License-Identifier: GPL-2.0-or-later
#
# m8c (Dirtywave M8 / M8 Headless remote display client) for Mortar.
#
# The client itself comes from nixpkgs (pkgs/by-name/m8/m8c: v2.2.3, the
# latest upstream release, SDL3 + libserialport backend; prebuilt for
# aarch64-linux on cache.nixos.org). This file only adds the Brick bits:
#
#   bin/m8c-brick   launcher: seeds the Brick config on first run, then
#                   execs m8c
#   share/m8c/config.ini
#
# Usage (flake / configuration):
#   environment.systemPackages = [ (pkgs.callPackage ./pkgs/m8c { }) ];
#   users.users.<user>.extraGroups = [ "dialout" "audio" "input" ];
#
# Controls. The Brick's buttons are the gpio-keys device "TrimUI Brick
# Gamepad", which emits BTN_* gamepad codes only (no KEY_* codes), so SDL
# never sees them as keyboard keys; udev tags the device ID_INPUT_JOYSTICK
# and SDL3 builds a gamepad mapping from the evdev codes itself
# (BTN_SOUTH -> south, BTN_DPAD_* -> dpad, BTN_TL2/TR2 -> triggers, ...).
# config.ini therefore binds m8c's [gamepad] section:
#
#   M8        Brick                    SDL gamepad id
#   --------  -----------------------  --------------
#   up/down/  D-pad                    11 12 13 14
#   left/right
#   SHIFT     SELECT, and also L2      button 4, axis 4
#   PLAY      START,  and also R2      button 6, axis 5
#   OPTION    B (bottom face button)   0 (south)
#   EDIT      A (right face button)    1 (east)
#   quit      SELECT + MENU            5 (guide) while select is held
#   reset     SELECT+START+B+A         m8c's built-in combo (redraws screen)
#
# OPTION left of EDIT mirrors the M8's own button order. m8c allows one
# button per action in [gamepad]; L2/R2 come in through its trigger-axis
# bindings. X, Y, L, R, F1, F2 are unbound.
#
# m8c re-reads and then re-writes config.ini on every start (dropping
# anything it does not know), so the file must be a writable copy, never a
# store symlink; the launcher copies it only when none exists. To return to
# the Brick defaults, delete ~/.local/share/m8c/config.ini. `--config NAME`
# names a file in that same directory, not a path.
#
# The pad is only found through udev: SDL's fallback classifier (no
# libudev) calls a device a joystick only if it has ABS_X/ABS_Y, which
# gpio-keys lacks. nixpkgs' SDL3 dlopen()s libudev.so.1 without systemd in
# its RUNPATH, but libusb (linked by SDL3) already pulls libudev in, so the
# dlopen succeeds (checked with LD_DEBUG=libs on aarch64-linux). Keep that
# in mind if SDL3 is ever built without libusb.
#
# Runtime needs on the Brick (not handled here): the M8 on the top USB-C
# (host) port, CONFIG_USB_ACM (/dev/ttyACM*, group dialout) and
# CONFIG_SND_USB_AUDIO for the M8's audio interface.
#
# Audio: with audio_enabled=true m8c opens the SDL capture device whose
# name contains "M8" and plays it to SDL's default playback device (plain
# ALSA here; SDL converts rate/format, the Brick codec takes 44.1/48 kHz
# S16 only). Make ALSA's default the Brick codec, not whichever card
# enumerated first (asound.conf `defaults.pcm.card`/`defaults.ctl.card`
# by card id, or fixed snd_usb_audio index=1), and turn the codec's
# "Speaker" switch on for the speaker (it defaults off; headphones share
# HPOUT). audio_buffer_size=0 lets SDL choose; set 256..1024 frames if
# there are dropouts or too much lag. F12 / key_toggle_audio toggles
# routing. Alternative without m8c in the loop: `alsaloop -C plughw:M8
# -P default -t 20000` (alsa-utils) as a service, with audio_enabled=false.
#
# m8c also creates its window with SDL_WINDOW_OPENGL, so it needs a working
# GL/GLES path (EGL on KMSDRM or a compositor); with no GL it exits "Failed
# to initialize renderer" (SDL_RENDER_DRIVER=software does not avoid that
# flag). On Mortar that is Mesa's llvmpipe (modules/modules/core/audio.nix).
{
  lib,
  m8c,
  coreutils,
  writeShellApplication,
  symlinkJoin,
}:

let
  config = ./config.ini;

  # The M8 screen texture is opaque, but SDL gives ARGB textures alpha
  # blending by default. Blending makes the final full-screen draw read the
  # scanout buffer back, and with Mesa's software rasteriser (llvmpipe on
  # kms_swrast) that buffer is uncached write-combined CMA: reads from it
  # cost several times the whole frame. Draw it opaque instead.
  m8c' = m8c.overrideAttrs (old: {
    # Ask the M8 for a full screen after a resize or when shown again (it
    # only sends changed cells, so content was missing until selected),
    # and stop presenting while the compositor has us hidden.
    patches = (old.patches or [ ]) ++ [ ./redraw-and-occlusion.patch ];
    patchFlags = [ "-p1" ];
    postPatch = (old.postPatch or "") + ''
      substituteInPlace src/render.c --replace-fail \
        'SDL_SetTextureScaleMode(main_texture, texture_scaling_mode);' \
        'SDL_SetTextureScaleMode(main_texture, texture_scaling_mode); SDL_SetTextureBlendMode(main_texture, SDL_BLENDMODE_NONE);'
    '';
  });

  launcher = writeShellApplication {
    name = "m8c-brick";
    runtimeInputs = [ coreutils ];
    text = ''
      # Same directory SDL_GetPrefPath("", "m8c") resolves to on Linux.
      dir="''${XDG_DATA_HOME:-$HOME/.local/share}/m8c"
      mkdir -p "$dir"
      if [ ! -e "$dir/config.ini" ]; then
        install -m 0644 ${config} "$dir/config.ini"
      fi
      exec ${lib.getExe m8c'} "$@"
    '';
  };
in
symlinkJoin {
  name = "m8c-brick-${m8c.version}";
  paths = [ launcher m8c' ];
  postBuild = ''
    install -Dm0644 ${config} $out/share/m8c/config.ini
  '';
  passthru = {
    inherit config;
    m8c = m8c';
  };
  meta = {
    description = "m8c with TrimUI Brick defaults (gamepad mapping, fullscreen, audio)";
    inherit (m8c.meta) homepage license;
    platforms = lib.platforms.linux;
    mainProgram = "m8c-brick";
  };
}
