# SPDX-License-Identifier: GPL-2.0-or-later
# ChooChooTracker: multi-engine tracker / groovebox ("ChooChoo"); see
# pkgs/choochootracker for controls, data dir and MIDI.
{ config, lib, pkgs, ... }:

let
  cfg = config.mortar.apps.choochootracker;
in
{
  options.mortar.apps.choochootracker = {
    enable = lib.mkEnableOption "ChooChooTracker, a multi-engine music tracker" // { default = true; };
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../../../pkgs/choochootracker { };
      defaultText = lib.literalExpression "pkgs.callPackage ../../../pkgs/choochootracker { }";
      description = "ChooChooTracker (Mortar build) with its launcher.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];

    # ALSA sequencer: /dev/snd/seq exists only once snd-seq is loaded; MIDI
    # apps (ChooChooTracker) list USB MIDI ports through it.
    boot.kernelModules = [ "snd-seq" "snd-seq-midi" ];

    mortar.greeter.sessions.choochoo = {
      name = "ChooChoo";
      description = "Tracker · groovebox";
      icon = "pad";
      order = 22;
      command = "exec ${config.mortar.device.sdlRun}/bin/mortar-sdl-run ${cfg.package}/bin/choochootracker";
    };
  };
}
