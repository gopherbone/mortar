# SPDX-License-Identifier: GPL-2.0-or-later
# What apps need at runtime: software GL and audio.
#
# Display: app sessions start through `mortar-sdl-run` (devices/brick/gpu.nix),
# which puts them on the PowerVR GPU (pkgs/sdl3-pvr). If that fails they
# run on SDL's KMSDRM backend with Mesa's software GL, described here.
# KMSDRM needs GBM and EGL. The PowerVR userspace has neither (NULL WSEGL only, see
# devices/brick/gpu.nix), so Mesa provides them in software: GBM's kms_swrast on
# the sun4i display, llvmpipe for GL/GLES on the 4 A53 cores. SDL picks
# card0 (the only card with a connector) on its own; no env is needed.
# The PowerVR libs stay off the default library paths (only the GPU tests
# link them by RPATH), so libglvnd only ever sees Mesa.
#   Cost: llvmpipe runs kmscube at 60 fps on ~1 core; m8c redrawing
#   continuously reaches ~30 fps using ~2 cores. Mesa adds ~1 GiB to the
#   closure (disk only); an app's RSS is ~160 MB, most of it shared libs.
#   Opaque full-screen blits matter: blending reads the write-combined
#   scanout buffer back (see pkgs/m8c and pkgs/puzzlescript).
#
# Audio (`mortar.apps.audio.pipewire`, on by default): PipeWire with
# WirePlumber in the session user's systemd manager (user.nix), with
# its ALSA plugin (ALSA `default`) and PulseAudio server, so SDL, ALSA and
# Pulse apps share the codec, USB audio (the M8) and Bluetooth headphones
# (A2DP through BlueZ). Low latency for the trackers: 256-frame quantum
# (5.8 ms at 44.1 kHz, the M8's rate; the codec runs 44.1 or 48 kHz S16
# only), apps may ask for less down to 128 or more up to 2048.
# WirePlumber does not drive the codec's mixer (no ALSA card profile for
# it): its levels are the device's boot defaults, the volume is PipeWire's
# (the shell's volume keys use wpctl on the default sink, so they follow
# Bluetooth headphones too). MIDI stays on the ALSA sequencer.
#
# With `pipewire = false` apps use ALSA directly: its default is the codec
# through dmix (shared, resampled by plug), USB cards such as the M8 stay
# reachable by name (hw:M8), and the volume keys move the codec's DAC.
{ config, lib, pkgs, ... }:

let
  cfg = config.mortar.apps;
in
{
  options.mortar.apps = {
    audio.pipewire = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "PipeWire (+ WirePlumber, ALSA and Pulse compatibility) for the session; off: plain ALSA with dmix on the codec.";
    };
    audio.quantum = lib.mkOption {
      type = lib.types.ints.positive;
      default = 256;
      description = "PipeWire's default quantum (frames per cycle) at 44.1 kHz.";
    };
  };

  config = {
    environment.systemPackages = [ pkgs.alsa-utils ];

    # Mesa: GBM + EGL + llvmpipe for SDL's KMSDRM backend (see above).
    hardware.graphics.enable = true;

    # services.graphical-desktop (pulled in by greetd) turns PipeWire on
    # by default; this decides.
    services.pipewire = lib.mkMerge [
      { enable = lib.mkForce cfg.audio.pipewire; }
      (lib.mkIf cfg.audio.pipewire {
        audio.enable = true;
        alsa.enable = true;
        pulse.enable = true;
        wireplumber.enable = true;
        extraConfig.pipewire."90-mortar" = {
          "context.properties" = {
            "default.clock.rate" = 44100;
            "default.clock.allowed-rates" = [ 44100 48000 ];
            "default.clock.quantum" = cfg.audio.quantum;
            "default.clock.min-quantum" = 128;
            "default.clock.max-quantum" = 2048;
          };
        };
        extraConfig.pipewire-pulse."90-mortar" = {
          "pulse.properties" = {
            "pulse.min.req" = "128/44100";
            "pulse.default.req" = "${toString cfg.audio.quantum}/44100";
            "pulse.min.quantum" = "128/44100";
          };
        };
        # No cameras on the Brick: WirePlumber would still start libcamera
        # (its camera manager, IPA search, dma-buf probing) and the V4L2
        # monitor in every session.
        wireplumber.extraConfig."90-mortar-no-cameras" = {
          "wireplumber.profiles".main = {
            "monitor.libcamera" = "disabled";
            "monitor.v4l2" = "disabled";
          };
        };
        wireplumber.extraConfig."90-mortar-codec" = config.mortar.device.audio.wireplumber;
      })
    ];

    # PipeWire only for people: the greeter (a system user) never starts it,
    # so it never holds the codec when the session starts; for the session
    # user it starts with the login (not on the first sound).
    systemd.user = lib.mkIf cfg.audio.pipewire {
      services.pipewire = {
        wantedBy = [ "default.target" ];
        unitConfig.ConditionUser = "!@system";
      };
      services.wireplumber.unitConfig.ConditionUser = "!@system";
      services.pipewire-pulse.unitConfig.ConditionUser = "!@system";
      sockets.pipewire.unitConfig.ConditionUser = "!@system";
      sockets.pipewire-pulse.unitConfig.ConditionUser = "!@system";
    };

    environment.etc."asound.conf".text =
      lib.optionalString (!cfg.audio.pipewire) ''
        # Default: the built-in card, shared through dmix. 44.1 kHz is the M8's
        # rate; plug converts anything else.
        pcm.!default {
          type plug
          slave.pcm "codec"
        }
        pcm.codec {
          type dmix
          ipc_key 2026
          ipc_perm 0666
          slave {
            pcm "hw:${config.mortar.device.audio.card},0"
            rate 44100
            format S16_LE
            channels 2
            period_size 512
            buffer_size 2048
          }
        }
        ctl.!default {
          type hw
          card ${config.mortar.device.audio.card}
        }
      ''
      + ''
        # SDL 3.4 names ALSA devices "<prefix>CARD=<id>,DEV=<n>", taking the
        # prefix from the first hint that starts with hw:, sysdefault: or
        # default:. That is sysdefault:, whose stock definition takes no DEV
        # argument, so every named device (the M8's input) failed with
        # "Unknown PCM sysdefault:CARD=M8,DEV=0". Accept DEV, and convert
        # formats (the M8 is 24-bit).
        pcm.!sysdefault {
          @args [ CARD DEV ]
          @args.CARD { type string default "${config.mortar.device.audio.card}" }
          @args.DEV { type integer default 0 }
          type plug
          slave.pcm {
            type hw
            card $CARD
            device $DEV
          }
          hint {
            show on
            description "Card default, converted"
          }
        }
      '';

    # Mesa would probe zink (GL on Vulkan) first and log an error each time.
    environment.variables.GALLIUM_DRIVER = "llvmpipe";
  };
}
