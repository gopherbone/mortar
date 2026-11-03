# SPDX-License-Identifier: GPL-2.0-or-later
# Battery policy (pkgs/mortar-power), on top of the kernel's AXP2202 driver
# (patches/kernel/0005: stock charge voltage/currents, NTC windows, the
# stock fuel-gauge battery model, Charging/Full/Discharging status).
#
#  - USB input current: TrimUI's stock policy. Every plug-in starts at the
#    USB 2.0 PC limit (500 mA); when no USB host configures the gadget within
#    5 s (20 s at boot) it is a charger and the limit goes to 2.5 A; never
#    while the gadget is configured. On unplug it soft-reconnects the USB
#    gadget (sharing mortar-usb-watchdog's lock), since the MUSB otherwise
#    keeps reporting "configured" and a charger plugged in next would be
#    held at 500 mA until the watchdog notices.
#  - Warnings at 15 % and 5 %: a toast with sound and haptics in the shell
#    (the rumble motor alone when no shell runs).
#  - A clean `systemctl poweroff` at 3 % (only if the battery also reads
#    below 3.65 V, so a confused gauge cannot switch the Brick off) or when
#    the loaded voltage stays below 3.3 V, so the SD card is never cut off
#    mid-write by the PMIC's own under-voltage switch.
#  - /run/mortar-power/state for the status bars (`mortar-power status`).
{
  config,
  lib,
  pkgs,
  ...
}:

let
  inherit (lib) mkOption mkEnableOption mkIf types;
  cfg = config.mortar.power;
  pkg = pkgs.callPackage ../../pkgs/mortar-power { };
  rumble = pkgs.callPackage ../../pkgs/mortar-rumble { };
  pct = types.ints.between 0 100;
in
{
  options.mortar.power = {
    enable = mkEnableOption "the Mortar battery policy service (mortar-power)";

    lowPercent = mkOption {
      type = pct;
      default = 15;
      description = "First low-battery warning.";
    };
    criticalPercent = mkOption {
      type = pct;
      default = 5;
      description = "Second, urgent low-battery warning.";
    };
    powerOffPercent = mkOption {
      type = pct;
      default = 3;
      description = "Power off cleanly at or below this capacity (on battery).";
    };
    powerOffConfirmMillivolts = mkOption {
      type = types.ints.positive;
      default = 3650;
      description = ''
        The capacity rule only powers off if the battery also reads at or
        below this voltage, so a gauge that is wrong cannot switch the Brick
        off at a healthy charge.
      '';
    };
    floorMillivolts = mkOption {
      type = types.ints.positive;
      default = 3300;
      description = ''
        Power off when the loaded battery voltage (median of the last three
        10-second readings) stays at or below this, whatever the gauge says.
      '';
    };
    usbHostCurrent = mkOption {
      type = types.ints.unsigned;
      default = 500000;
      description = "USB input current limit (uA) while a computer is on the cable (stock: 500 mA).";
    };
    usbAdapterCurrent = mkOption {
      type = types.ints.unsigned;
      default = 2500000;
      description = ''
        USB input current limit (uA) on a charger (stock: 2.5 A, the driver's
        maximum). 0 leaves the input limit alone.
      '';
    };
    dryRun = mkOption {
      type = types.bool;
      default = false;
      description = "Log instead of powering off (for testing the thresholds).";
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = cfg.powerOffPercent < cfg.criticalPercent && cfg.criticalPercent < cfg.lowPercent;
        message = "mortar.power: need powerOffPercent < criticalPercent < lowPercent";
      }
    ];

    environment.systemPackages = [ pkg ];

    systemd.services.mortar-power = {
      description = "Mortar battery policy (USB input current, low-battery warnings, power-off when empty)";
      wantedBy = [ "multi-user.target" ];
      path = [ config.systemd.package ];
      serviceConfig = {
        ExecStart = lib.escapeShellArgs (
          [
            "${pkg}/bin/mortar-power"
            "--low" (toString cfg.lowPercent)
            "--critical" (toString cfg.criticalPercent)
            "--off" (toString cfg.powerOffPercent)
            "--off-confirm-mv" (toString cfg.powerOffConfirmMillivolts)
            "--floor-mv" (toString cfg.floorMillivolts)
            "--host-current-ua" (toString cfg.usbHostCurrent)
            "--adapter-current-ua" (toString cfg.usbAdapterCurrent)
            "--rumble" "${rumble}/bin/mortar-rumble"
          ]
          ++ lib.optional cfg.dryRun "--dry-run"
        );
        Restart = "always";
        RestartSec = 5;
        RuntimeDirectory = "mortar-power";
        RuntimeDirectoryMode = "0755";
      };
    };
  };
}
