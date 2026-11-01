# SPDX-License-Identifier: GPL-2.0-or-later
# Built-in apps (greeter sessions in flake.nix) and what they need at
# runtime.
#
#   puzzlescript  PuzzleScript engine (C core + Rust/SDL3 shell)
#   m8c           Dirtywave M8 / M8 Headless client. Plug the M8 into the
#                 Brick's TOP USB-C port (USB host).
#
# Display: both are SDL3 apps on SDL's KMSDRM backend, which needs GBM and
# EGL. The PowerVR userspace has neither (NULL WSEGL only, see
# brick-gpu.nix), so Mesa provides them in software: GBM's kms_swrast on
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
# Audio: sessions run as root, and PipeWire refuses to run for root, so
# apps use ALSA directly. Its default device is the Brick codec through
# dmix (shared, resampled by plug), not whichever card enumerated first;
# USB cards such as the M8 stay reachable by name (hw:M8). The codec's
# mixer gets usable levels at boot (the speaker amp switch defaults off).
{ config, lib, pkgs, ... }:

let
  cfg = config.mortar.apps;
in
{
  options.mortar.apps = {
    puzzlescript = {
      enable = lib.mkEnableOption "PuzzleScript" // { default = cfg.puzzlescript.package != null; };
      package = lib.mkOption {
        type = lib.types.nullOr lib.types.package;
        default = null;
        description = "PuzzleScript package (from the puzzlescript flake input).";
      };
    };
    m8c = {
      enable = lib.mkEnableOption "m8c, the M8 tracker client" // { default = true; };
      package = lib.mkOption {
        type = lib.types.package;
        default = pkgs.callPackage ../pkgs/m8c { };
        defaultText = lib.literalExpression "pkgs.callPackage ../pkgs/m8c { }";
        description = "m8c with the Brick button config (m8c-brick).";
      };
    };
  };

  config = {
    environment.systemPackages =
      lib.optional (cfg.puzzlescript.enable && cfg.puzzlescript.package != null) cfg.puzzlescript.package
      ++ lib.optional cfg.m8c.enable cfg.m8c.package
      ++ [ pkgs.alsa-utils ];

    # Mesa: GBM + EGL + llvmpipe for SDL's KMSDRM backend (see above).
    hardware.graphics.enable = true;

    # services.graphical-desktop (pulled in by greetd) turns PipeWire on,
    # whose ALSA plugin then becomes the default device and fails with
    # "Host is down" for root.
    services.pipewire.enable = false;

    environment.etc."asound.conf".text = ''
      # Default: the Brick codec, shared through dmix. 44.1 kHz is the M8's
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
          pcm "hw:Codec,0"
          rate 44100
          format S16_LE
          channels 2
          period_size 512
          buffer_size 2048
        }
      }
      ctl.!default {
        type hw
        card Codec
      }
      # SDL 3.4 names ALSA devices "<prefix>CARD=<id>,DEV=<n>", taking the
      # prefix from the first hint that starts with hw:, sysdefault: or
      # default:. That is sysdefault:, whose stock definition takes no DEV
      # argument, so every named device (the M8's input) failed with
      # "Unknown PCM sysdefault:CARD=M8,DEV=0". Accept DEV, and convert
      # formats (the M8 is 24-bit).
      pcm.!sysdefault {
        @args [ CARD DEV ]
        @args.CARD { type string default "Codec" }
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

    # Codec levels at boot: speaker amp on (it defaults off), DAC about
    # -15 dB, headphones mid. Set with amixer; nothing is persisted.
    systemd.services.mortar-audio-defaults = {
      description = "Brick codec mixer defaults";
      wantedBy = [ "multi-user.target" ];
      after = [ "sound.target" ];
      wants = [ "sound.target" ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
      };
      path = [ pkgs.alsa-utils pkgs.coreutils ];
      script = ''
        # the codec probes early, but don't race it
        for _ in $(seq 50); do [ -e /proc/asound/Codec ] && break; sleep 0.2; done
        amixer -q -c Codec sset Speaker on || true
        amixer -q -c Codec sset DAC 50 || true
        amixer -q -c Codec sset 'DAC Digital' 160 || true
        amixer -q -c Codec sset Headphone 4 on || true
      '';
    };
  };
}
