# SPDX-License-Identifier: GPL-2.0-or-later
# The Brick's codec: the levels it boots with, and how PipeWire sees it.
{ config, lib, pkgs, ... }:

{
  mortar.device.audio = {
    card = "Codec";
    wireplumber = {
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
  };

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
}
