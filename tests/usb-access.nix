# SPDX-License-Identifier: GPL-2.0-or-later
# What a USB host gets (docs/pairing.md), from the evaluated system: nothing is
# trusted and only the UART pads log in by themselves unless mortar.usb.trustHost
# is on. Fails at evaluation, naming what is wrong.
{ lib, runCommand, closed, open }:

let
  unit = c: n: (c.systemd.units."${n}.service".text or "");
  has = lib.hasInfix;
  checks = {
    "closed: usb0 not trusted" = !(lib.elem "usb0" closed.networking.firewall.trustedInterfaces);
    "closed: no global autologin" = closed.services.getty.autologinUser == null;
    "closed: no getty on ttyGS0" = !(closed.systemd.units ? "serial-getty@ttyGS0.service");
    "closed: the gadget does not pull one in" =
      !(lib.elem "serial-getty@ttyGS0.service" closed.systemd.services.mortar-usb-gadget.wants);
    "closed: ttyS0 logs in as root" = has "--autologin root" (unit closed "serial-getty@ttyS0");
    "closed: the template does not" = !(has "--autologin" (unit closed "serial-getty@"));
    "closed: VTs do not" = !(has "--autologin" (unit closed "getty@"));
    "closed: pairing reachable on usb0" =
      lib.elem closed.mortar.setup.pairing.port closed.networking.firewall.interfaces.usb0.allowedTCPPorts;
    "closed: DHCP reachable on usb0" = lib.elem 67 closed.networking.firewall.interfaces.usb0.allowedUDPPorts;
    "closed: SSH reachable" =
      closed.services.openssh.openFirewall && lib.elem 22 closed.services.openssh.ports;
    "closed: mDNS reachable" = closed.services.avahi.openFirewall;
    "closed: SSH never takes a root password" =
      closed.services.openssh.settings.PermitRootLogin == "prohibit-password"
      && !closed.services.openssh.settings.PasswordAuthentication;
    "open: usb0 trusted" = lib.elem "usb0" open.networking.firewall.trustedInterfaces;
    "open: getty on ttyGS0" = open.systemd.units ? "serial-getty@ttyGS0.service";
    "open: autologin" = open.services.getty.autologinUser == "root";
  };
  failed = lib.attrNames (lib.filterAttrs (_: ok: !ok) checks);
in
assert lib.assertMsg (failed == [ ]) "usb-access: ${lib.concatStringsSep "; " failed}";
runCommand "mortar-usb-access-check" { } ''
  echo PASS > $out
''
