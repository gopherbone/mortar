# SPDX-License-Identifier: GPL-2.0-or-later
#
# Mortar: talk to the Brick over its USB-C port.
#
#   - configfs USB gadget: CDC-ACM serial (ttyGS0, with a getty) + CDC-ECM
#     or CDC-NCM network (usb0), Linux Foundation multifunction composite IDs
#   - usb0 = 10.55.0.1/24 with networkd's DHCP server, mDNS as mortar.local
#   - "black box" boot logs copied to the FAT /boot partition
#   - root SSH key
#
# Needs a kernel with USB_CONFIGFS{,_ACM,_ECM,_NCM} and a UDC (MUSB on the Brick),
# i.e. hardware.trimui-brick.variant = "mainline".
{ config, lib, pkgs, ... }:

let
  cfg = config.mortar.usb;

  gadgetDir = "/sys/kernel/config/usb_gadget/mortar";

  gadgetUp = pkgs.writeShellScript "mortar-usb-gadget-up" ''
    set -euo pipefail
    export PATH=${lib.makeBinPath [ pkgs.coreutils pkgs.util-linux ]}

    G=${gadgetDir}

    if ! mountpoint -q /sys/kernel/config; then
      mount -t configfs configfs /sys/kernel/config
    fi

    if [ -d "$G" ] && [ -n "$(cat "$G/UDC" 2>/dev/null || true)" ]; then
      echo "gadget already bound to $(cat "$G/UDC")"
      exit 0
    fi

    # Serial number: SoC SID (stable across reflashes) > machine-id > fixed.
    serial=""
    for f in /sys/bus/nvmem/devices/sunxi-sid*/nvmem; do
      if [ -r "$f" ]; then
        serial=$(od -An -tx1 -N16 "$f" 2>/dev/null | tr -d ' \n' || true)
        [ -n "$serial" ] && [ "$serial" != "00000000000000000000000000000000" ] && break
        serial=""
      fi
    done
    if [ -z "$serial" ] && [ -s /etc/machine-id ]; then
      serial=$(tr -dc '0-9a-f' < /etc/machine-id | cut -c1-32)
    fi
    [ -n "$serial" ] || serial=mortar

    if [ ! -d "$G" ]; then
      mkdir "$G"
      cd "$G"

      echo 0x1d6b > idVendor   # Linux Foundation
      echo 0x0104 > idProduct  # Multifunction Composite Gadget
      echo 0x0100 > bcdDevice
      echo 0x0200 > bcdUSB
      # Miscellaneous / Common / Interface Association: lets hosts (macOS in
      # particular) bind ACM and ECM/NCM as separate functions.
      echo 0xEF > bDeviceClass
      echo 0x02 > bDeviceSubClass
      echo 0x01 > bDeviceProtocol

      mkdir -p strings/0x409
      echo "Mortar"       > strings/0x409/manufacturer
      echo "TrimUI Brick" > strings/0x409/product
      echo "$serial"      > strings/0x409/serialnumber

      mkdir -p configs/c.1/strings/0x409
      echo "ACM + ${lib.toUpper cfg.network}" > configs/c.1/strings/0x409/configuration
      echo 500 > configs/c.1/MaxPower

      # Serial: /dev/ttyGS0
      mkdir -p functions/acm.usb0

      # Network: fixed MACs so the Mac keeps the same interface/service.
      mkdir -p functions/${cfg.network}.usb0
      echo ${cfg.deviceMac} > functions/${cfg.network}.usb0/dev_addr
      echo ${cfg.hostMac}   > functions/${cfg.network}.usb0/host_addr
      # Pin the interface name (supported by u_ether configfs in this
      # kernel); the .link file below is the fallback.
      echo usb0 > functions/${cfg.network}.usb0/ifname 2>/dev/null || true

      ln -s functions/acm.usb0 configs/c.1/
      ln -s functions/${cfg.network}.usb0 configs/c.1/
    fi

    # The MUSB UDC is built in but may still be probing (deferred probe on
    # PHY/regulators). Wait up to 30 s for it.
    udc=""
    for _ in $(seq 1 60); do
      udc=$(ls /sys/class/udc 2>/dev/null | head -n1 || true)
      [ -n "$udc" ] && break
      sleep 0.5
    done
    if [ -z "$udc" ]; then
      echo "no UDC in /sys/class/udc; is the MUSB controller probed?" >&2
      exit 1
    fi

    echo "binding gadget (serial $serial) to UDC $udc"
    echo "$udc" > "$G/UDC"
  '';

  gadgetDown = pkgs.writeShellScript "mortar-usb-gadget-down" ''
    echo "" > ${gadgetDir}/UDC 2>/dev/null || true
  '';

  blackboxDump = pkgs.writeShellScript "mortar-blackbox" ''
    set -u
    export PATH=${lib.makeBinPath [ pkgs.coreutils pkgs.util-linux config.systemd.package ]}

    stage="''${1:-manual}"
    base=${cfg.blackbox.directory}
    keep=${toString cfg.blackbox.keepBoots}
    cap=${toString cfg.blackbox.maxBytes}
    state=/run/mortar-blackbox

    mkdir -p "$state"
    exec 9>"$state/lock"
    flock 9

    if ! mountpoint -q /boot; then
      echo "/boot is not mounted; skipping black box write ($stage)" >&2
      exit 0
    fi

    mkdir -p "$base"

    if [ -s "$state/dir" ]; then
      dir=$(cat "$state/dir")
    else
      # First write this boot: allocate boot-NNNNN from a counter on the
      # FAT partition (no RTC we can trust), then prune old boots.
      n=$(cat "$base/counter" 2>/dev/null || echo 0)
      case "$n" in ""|*[!0-9]*) n=0 ;; esac
      n=$((10#$n + 1))
      echo "$n" > "$base/counter"
      dir=$(printf '%s/boot-%05d' "$base" "$n")
      mkdir -p "$dir"
      echo "$dir" > "$state/dir"
      {
        echo "boot_id: $(cat /proc/sys/kernel/random/boot_id)"
        echo "uname:   $(uname -a)"
        echo "cmdline: $(cat /proc/cmdline)"
        echo "system:  $(readlink -f /run/current-system)"
      } > "$dir/info.txt"
      ls -1d "$base"/boot-* 2>/dev/null | sort | head -n -"$keep" | while read -r old; do
        rm -rf -- "$old"
      done
      sync -f "$base"
    fi

    # Each stage gets its own files: later ones are supersets, but if a
    # write is cut short by a crash the earlier stage survives.
    dmesg 2>&1 | tail -c "$cap" > "$dir/dmesg-$stage.txt"
    sync -f "$dir"
    journalctl -b --no-pager -o short-monotonic 2>&1 | tail -c "$cap" > "$dir/journal-$stage.txt"
    sync -f "$dir"
    systemctl --failed --no-pager --no-legend > "$dir/failed-$stage.txt" 2>&1 || true
    sync -f "$dir"
  '';

  blackboxStages = [ "15s" "45s" "120s" ];
in
{
  options.mortar.usb = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "USB gadget (serial + network), mDNS and black-box logs for Mortar.";
    };

    network = lib.mkOption {
      type = lib.types.enum [ "ecm" "ncm" ];
      # ECM first: it is what jkingston's firmware ships (ACM+ECM) and is
      # known to work with macOS out of the box. NCM (faster, also native
      # on recent macOS) is one option flip away; the kernel has both.
      #
      # Offering ACM+ECM+NCM at once is not possible: the Brick's MUSB
      # config has 5 IN endpoints and that combination needs 6 (ACM
      # notify+bulk, ECM notify+bulk, NCM notify+bulk). A second USB
      # configuration would fit, but macOS and Linux hosts select
      # configuration 1, so it would not act as a fallback.
      default = "ecm";
      description = ''
        USB network function paired with the ACM serial port: CDC-ECM or
        CDC-NCM. Both appear as interface usb0 on the Brick.
      '';
    };

    deviceMac = lib.mkOption {
      type = lib.types.str;
      default = "02:4d:4f:52:54:01";
      description = "MAC of usb0 on the Brick (locally administered).";
    };

    hostMac = lib.mkOption {
      type = lib.types.str;
      default = "02:4d:4f:52:54:02";
      description = "MAC the host sees for its end of the USB link.";
    };

    address = lib.mkOption {
      type = lib.types.str;
      default = "10.55.0.1/24";
      description = "Static address of usb0; the DHCP pool is taken from the same /24.";
    };

    blackbox = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Copy dmesg/journal to the FAT boot partition during boot and at shutdown.";
      };
      directory = lib.mkOption {
        type = lib.types.str;
        default = "/boot/mortar-logs";
        description = "Where the logs go (must be on /boot).";
      };
      keepBoots = lib.mkOption {
        type = lib.types.ints.positive;
        default = 5;
        description = "Number of boots to keep.";
      };
      maxBytes = lib.mkOption {
        type = lib.types.ints.positive;
        default = 262144;
        description = ''
          Per-file size cap (tail). Keeps each write small: the SD card has
          shown errors under sustained writes.
        '';
      };
    };
  };

  config = lib.mkIf cfg.enable (lib.mkMerge [
    {
      # ---------------------------------------------------------------
      # Gadget
      # ---------------------------------------------------------------
      systemd.services.mortar-usb-gadget = {
        description = "Mortar USB gadget (ACM serial + ${cfg.network} network)";
        # Runs in early boot, needs nothing but /sys and the read-only
        # /nix/store: no local-fs/remount-rw dependency.
        unitConfig.DefaultDependencies = false;
        wantedBy = [ "sysinit.target" ];
        wants = [ "network-pre.target" "serial-getty@ttyGS0.service" ];
        after = [ "sys-kernel-config.mount" "systemd-modules-load.service" ];
        before = [ "network-pre.target" "shutdown.target" ];
        conflicts = [ "shutdown.target" ];
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          ExecStart = gadgetUp;
          ExecStop = gadgetDown;
          TimeoutStartSec = "45s";
        };
      };

      # Login on the ACM port: shows up on the Mac as /dev/cu.usbmodem*.
      # Wanted from multi-user.target directly (the Liontron config forces
      # getty.target's wantedBy to []), and pulled in by the gadget.
      systemd.services."serial-getty@ttyGS0" = {
        wantedBy = [ "multi-user.target" ];
        after = [ "mortar-usb-gadget.service" ];
        # agetty exits when the host closes the port; keep respawning.
        serviceConfig.Restart = "always";
        serviceConfig.RestartSec = "1s";
      };

      # ---------------------------------------------------------------
      # Network on usb0
      # ---------------------------------------------------------------
      # Make sure the gadget netdev is called usb0 even if udev would
      # rename it (matched by the fixed device MAC).
      systemd.network.links."10-mortar-usb0" = {
        matchConfig.MACAddress = cfg.deviceMac;
        linkConfig.Name = "usb0";
      };

      # networkd manages usb0. Hand the rest (wlan0's DHCP from
      # networking.useDHCP) to networkd too, so dhcpcd and networkd don't
      # fight over interfaces; usb0's 10-* network sorts before NixOS'
      # 99-ethernet-default-dhcp catch-all.
      systemd.network.enable = true;
      networking.useNetworkd = lib.mkDefault true;
      systemd.network.networks."10-mortar-usb0" = {
        matchConfig.Name = "usb0";
        address = [ cfg.address ];
        networkConfig = {
          DHCPServer = true;
          ConfigureWithoutCarrier = true;
          LinkLocalAddressing = "ipv6";
          IPv6AcceptRA = false;
        };
        dhcpServerConfig = {
          # 10.55.0.10 .. 10.55.0.109
          PoolOffset = 10;
          PoolSize = 100;
          # Point-to-point link: don't steal the Mac's default route/DNS.
          EmitRouter = false;
          EmitDNS = false;
        };
        linkConfig.RequiredForOnline = "no";
      };

      # In case dhcpcd is turned back on (useNetworkd = false).
      networking.dhcpcd.denyInterfaces = [ "usb0" ];

      # The cable is physical access anyway; allow DHCP, mDNS, SSH, ...
      networking.firewall.trustedInterfaces = [ "usb0" ];

      services.avahi = {
        enable = true;
        hostName = "mortar";
        nssmdns4 = true;
        allowInterfaces = [ "usb0" "wlan0" ];
        publish = {
          enable = true;
          addresses = true;
          workstation = true;
        };
      };

      # ---------------------------------------------------------------
      # SSH
      # ---------------------------------------------------------------
      services.openssh.enable = true;
      # Keys only. Root is allowed, but never with a password: the base
      # config ships known passwords for the local console.
      services.openssh.settings.PermitRootLogin = lib.mkDefault "prohibit-password";
      services.openssh.settings.PasswordAuthentication = lib.mkForce false;
      services.openssh.settings.KbdInteractiveAuthentication = lib.mkForce false;
      # No keys live in this repo. Each user's keys are read from the FAT
      # boot partition, so they can be added with any card reader:
      #   <card>/ssh-keys/root   (one public key per line)
      # The partition is mounted with umask 0077 (root-owned 0700/0600),
      # which satisfies sshd's StrictModes checks. sshd reads these files
      # with the target user's rights, so this works for root only.
      services.openssh.authorizedKeysFiles = [ "/boot/ssh-keys/%u" ];
    }

    # -----------------------------------------------------------------
    # Black box: logs on the FAT partition, readable from a Mac
    # -----------------------------------------------------------------
    (lib.mkIf cfg.blackbox.enable {
      systemd.services."mortar-blackbox@" = {
        description = "Mortar black box: write boot logs to ${cfg.blackbox.directory} (%i)";
        unitConfig.RequiresMountsFor = [ "/boot" ];
        serviceConfig = {
          Type = "oneshot";
          ExecStart = "${blackboxDump} %i";
          Nice = 10;
          IOSchedulingClass = "idle";
        };
      };

      systemd.timers = lib.listToAttrs (map (stage: {
        name = "mortar-blackbox-${stage}";
        value = {
          wantedBy = [ "timers.target" ];
          timerConfig = {
            OnBootSec = stage;
            AccuracySec = "1s";
            Unit = "mortar-blackbox@${stage}.service";
          };
        };
      }) blackboxStages);

      # Started at boot, does its work in ExecStop. Ordered after the /boot
      # mount (via RequiresMountsFor) so it is stopped before /boot is
      # unmounted; journald is still up at that point.
      systemd.services.mortar-blackbox-shutdown = {
        description = "Mortar black box: write logs to ${cfg.blackbox.directory} at shutdown";
        wantedBy = [ "multi-user.target" ];
        unitConfig.RequiresMountsFor = [ "/boot" ];
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          ExecStart = "${pkgs.coreutils}/bin/true";
          ExecStop = "${blackboxDump} shutdown";
          TimeoutStopSec = "30s";
        };
      };
    })
  ]);
}
