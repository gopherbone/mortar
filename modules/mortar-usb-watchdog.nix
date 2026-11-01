# SPDX-License-Identifier: GPL-2.0-or-later
#
# Mortar: watchdog for a wedged USB gadget link (see README.md).
# Import next to modules/mortar-usb.nix (or merge into it).
{ config, lib, pkgs, ... }:

let
  cfg = config.mortar.usb.watchdog;
  watchdog = pkgs.writeShellScript "mortar-usb-watchdog"
    (builtins.readFile ./mortar-usb-watchdog.sh);
in
{
  options.mortar.usb.watchdog = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = config.mortar.usb.enable;
      defaultText = lib.literalExpression "config.mortar.usb.enable";
      description = ''
        Detect a dead USB gadget link (carrier up, no RX, host probes
        unanswered) and recover it by soft-disconnecting / rebinding the UDC.
      '';
    };
    idleSeconds = lib.mkOption {
      type = lib.types.ints.positive;
      default = 30;
      description = "Seconds without any RX on usb0 before the host is probed.";
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.services.mortar-usb-watchdog = {
      description = "Mortar USB gadget link watchdog";
      wantedBy = [ "multi-user.target" ];
      after = [ "mortar-usb-gadget.service" "systemd-networkd.service" ];
      wants = [ "mortar-usb-gadget.service" ];
      path = [ pkgs.coreutils pkgs.gawk pkgs.iproute2 pkgs.iputils pkgs.util-linux config.systemd.package ];
      environment = {
        MORTAR_USB_IF = "usb0";
        MORTAR_USB_GADGET = "/sys/kernel/config/usb_gadget/mortar";
        MORTAR_USB_IDLE = toString cfg.idleSeconds;
      };
      serviceConfig = {
        ExecStart = watchdog;
        Restart = "always";
        RestartSec = "5s";
        Nice = 5;
      };
    };
  };
}
