# SPDX-License-Identifier: GPL-2.0-or-later
# Built-in apps (greeter sessions in flake.nix) and what they need at
# runtime.
#
#   puzzlescript  PuzzleScript engine (C core + Rust/SDL3 shell)
#   m8c           Dirtywave M8 / M8 Headless client. Plug the M8 into the
#                 Brick's TOP USB-C port (USB host).
#   choochootracker  multi-engine tracker / groovebox ("ChooChoo"); see
#                 pkgs/choochootracker for controls, data dir and MIDI.
#   pico8         PICO-8 is commercial, so it is never part of the system
#                 closure (which CI builds and the update cache holds). It
#                 lives in its own Nix profile on the Brick,
#                 /nix/var/nix/profiles/pico8, put there from a computer
#                 with `mortar pico8` (built from your own zip, see
#                 pkgs/pico8). Over-the-air updates leave that profile
#                 alone; the PICO-8 entries hide while it is missing.
#   acgc          Animal Crossing (GameCube decomp port, pkgs/acgc). Same
#                 arrangement as PICO-8: its own profile,
#                 /nix/var/nix/profiles/acgc, and the user's own disc image
#                 in ~/.local/share/acgc/rom/GAFE01.iso, both put there by
#                 `mortar acgc --iso PATH`. The entry hides until both exist.
#
# Display: the sessions start both through `mortar-sdl-run` (brick-gpu.nix),
# which puts them on the PowerVR GPU (pkgs/sdl3-pvr). If that fails they
# run on SDL's KMSDRM backend with Mesa's software GL, described here.
# KMSDRM needs GBM and EGL. The PowerVR userspace has neither (NULL WSEGL only, see
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
# Audio (`mortar.apps.audio.pipewire`, on by default): PipeWire with
# WirePlumber in the session user's systemd manager (mortar-user.nix), with
# its ALSA plugin (ALSA `default`) and PulseAudio server, so SDL, ALSA and
# Pulse apps share the codec, USB audio (the M8) and Bluetooth headphones
# (A2DP through BlueZ). Low latency for the trackers: 256-frame quantum
# (5.8 ms at 44.1 kHz, the M8's rate; the codec runs 44.1 or 48 kHz S16
# only), apps may ask for less down to 128 or more up to 2048.
# WirePlumber does not drive the codec's mixer (no ALSA card profile for
# it): its levels are the boot defaults below, the volume is PipeWire's
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
    puzzlescript = {
      enable = lib.mkEnableOption "PuzzleScript" // { default = cfg.puzzlescript.package != null; };
      package = lib.mkOption {
        type = lib.types.nullOr lib.types.package;
        default = null;
        description = "PuzzleScript package (from the puzzlescript flake input).";
      };
    };
    pico8 = {
      enable = lib.mkEnableOption "PICO-8 entries, shown once `mortar pico8` has installed it (see pkgs/pico8)";
      package = lib.mkOption {
        type = lib.types.package;
        default = pkgs.callPackage ../pkgs/pico8 { };
        defaultText = lib.literalExpression "pkgs.callPackage ../pkgs/pico8 { }";
        description = ''
          PICO-8 built from the user-supplied Raspberry Pi zip. Not part of
          the system: `mortar pico8` builds this (flake output
          `packages.aarch64-linux.pico8`) and installs it into `profile`.
        '';
      };
      profile = lib.mkOption {
        type = lib.types.str;
        default = "/nix/var/nix/profiles/pico8";
        description = "The Nix profile PICO-8 is installed into (a GC root of its own, untouched by system updates).";
      };
      bin = lib.mkOption {
        type = lib.types.str;
        readOnly = true;
        default = "${cfg.pico8.profile}/bin/pico8";
        description = "PICO-8 inside the profile (for launcher entries; use it with `requires`).";
      };
    };
    acgc = {
      enable = lib.mkEnableOption "the Animal Crossing entry, shown once `mortar acgc` has installed the port and your disc image (see pkgs/acgc)";
      package = lib.mkOption {
        type = lib.types.package;
        default = pkgs.callPackage ../pkgs/acgc { };
        defaultText = lib.literalExpression "pkgs.callPackage ../pkgs/acgc { }";
        description = ''
          The Animal Crossing port (no game data). Not part of the system:
          `mortar acgc` builds this (flake output
          `packages.aarch64-linux.acgc`) and installs it into `profile`.
        '';
      };
      profile = lib.mkOption {
        type = lib.types.str;
        default = "/nix/var/nix/profiles/acgc";
        description = "The Nix profile the port is installed into (a GC root of its own, untouched by system updates).";
      };
      bin = lib.mkOption {
        type = lib.types.str;
        readOnly = true;
        default = "${cfg.acgc.profile}/bin/acgc";
        description = "The launcher inside the profile.";
      };
      rom = lib.mkOption {
        type = lib.types.str;
        default = "/home/brick/.local/share/acgc/rom/GAFE01.iso";
        description = "The disc image the entry waits for (`requires`); `mortar acgc --iso` puts it here.";
      };
    };
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
    choochootracker = {
      enable = lib.mkEnableOption "ChooChooTracker, a multi-engine music tracker" // { default = true; };
      package = lib.mkOption {
        type = lib.types.package;
        default = pkgs.callPackage ../pkgs/choochootracker { };
        defaultText = lib.literalExpression "pkgs.callPackage ../pkgs/choochootracker { }";
        description = "ChooChooTracker (Mortar build) with its launcher.";
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
      ++ lib.optional cfg.choochootracker.enable cfg.choochootracker.package
      ++ [ pkgs.alsa-utils ];

    # `pico8` on PATH once installed (the profile is not in the closure)
    environment.profiles = lib.mkMerge [
      (lib.mkIf cfg.pico8.enable (lib.mkAfter [ cfg.pico8.profile ]))
      (lib.mkIf cfg.acgc.enable (lib.mkAfter [ cfg.acgc.profile ]))
    ];

    # ALSA sequencer: /dev/snd/seq exists only once snd-seq is loaded; MIDI
    # apps (ChooChooTracker) list USB MIDI ports through it.
    boot.kernelModules = [ "snd-seq" "snd-seq-midi" ];

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
        wireplumber.extraConfig."90-mortar-codec" = {
          # The Brick codec (the only platform sound card; USB cards are
          # alsa_card.usb-*): plain PCM nodes, no ALSA card profile. ACP's
          # speaker/headphone paths would switch "Headphone" off for the
          # speaker, which is fed from HPOUT, and fight the boot levels.
          "monitor.alsa.rules" = [
            {
              matches = [ { "device.name" = "~alsa_card.platform-*"; } ];
              actions.update-props = {
                "api.alsa.use-acp" = false;
                "device.description" = "Brick";
              };
            }
            {
              matches = [ { "node.name" = "~alsa_output.platform-*"; } ];
              actions.update-props = {
                "node.description" = "Brick speaker / headphones";
                "priority.session" = 1500;
                "priority.driver" = 1500;
                "api.alsa.headroom" = 0;
                "api.alsa.period-size" = 256;
                "audio.format" = "S16LE";
                # Without ACP the PCM node takes the driver's maximum
                # (64 channels), which SDL rejects for the default sink:
                # m8c then opens no output and the M8 is silent.
                "audio.channels" = 2;
                "audio.position" = [ "FL" "FR" ];
              };
            }
            {
              # USB audio (the M8's interface) never becomes the default
              # output by itself: m8c plays the M8 to the default sink.
              matches = [ { "node.name" = "~alsa_output.usb-*"; } ];
              actions.update-props = {
                "priority.session" = 1000;
                "priority.driver" = 1000;
              };
            }
          ];
          # Bluetooth headphones take over while connected.
          "monitor.bluez.rules" = [
            {
              matches = [ { "node.name" = "~bluez_output.*"; } ];
              actions.update-props."priority.session" = 2000;
            }
          ];
        };
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

    # Codec levels at boot: speaker amp on (it defaults off), DAC at 0 dB,
    # DAC Digital 160 = 0 dB (0.75 dB steps, more is digital gain that
    # clips), Headphone 7/7 = 0 dB (the HPOUT attenuator; the speaker amp
    # is fed from HPOUT, so this sets the speaker level too, as stock
    # does; patches/kernel/0004 makes 7 the loud end). The codec's LINEOUT
    # is not wired to the speaker on the Brick, so it stays off. With
    # PipeWire nothing else touches these (the volume is PipeWire's);
    # without it the volume keys adjust DAC from here. Set with amixer;
    # nothing is persisted.
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
        amixer -q -c Codec sset DAC 63 || true
        amixer -q -c Codec sset 'DAC Digital' 160 || true
        amixer -q -c Codec sset Headphone 7 on || true
      '';
    };
  };
}
