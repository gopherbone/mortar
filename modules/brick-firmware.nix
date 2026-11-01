# SPDX-License-Identifier: GPL-2.0-or-later
#
# Mortar: proprietary TrimUI Brick firmware + the radios that need it.
#
#   - hardware.firmware += pkgs/trimui-brick-firmware.nix (main output only):
#       xr829/{boot,fw,sdd}_xr829.bin  Wi-Fi   (xradio_wlan, SDIO 0a9e:2282)
#       xr829/fw_xr829_bt.bin          BT      (hci_xradio on UART1)
#       rgx.{fw,sh}.22.102.54.38       GE8300  (pvrsrvkm)
#     The GPU userspace (DDK 1.19 libEGL/libGLESv2/...) lives in the
#     package's `gpu` output and is deliberately NOT installed here.
#   - Wi-Fi via iwd (station only; networkd does DHCP on wlan0 because
#     mortar-usb sets networking.useNetworkd and NixOS adds
#     99-wireless-client-dhcp for networking.useDHCP).
#   - Bluetooth (BlueZ) + a boot-time public-address service; see below.
#
# The firmware package is unfree; the flake's allowUnfreePredicate must allow
# "trimui-brick-firmware" (and "brick-bluetooth-identity" is GPL-2.0, fine).
#
# Wi-Fi limits (brick-port-survey.md, jkingston kernel drivers/net/wireless/
# xradio): 2.4 GHz only, station mode only (no AP/hotspot, no P2P), no WPA3/
# SAE, HT20 only (no HT40/SGI/STBC/GF) and HT needs the driver's `allow_ht`
# parameter. The driver has no MAC in the DT, so wlan0 gets a random MAC on
# every boot (eth_random_addr) unless something pins it.
#
# Bluetooth address
# -----------------
# hci_xradio sets HCI_QUIRK_INVALID_BDADDR: the controller registers
# *unconfigured* and bluetoothd ignores it until userspace programs a public
# address with MGMT_OP_SET_PUBLIC_ADDRESS (the driver then writes it with
# vendor command 0xfc0a, resets, and reads it back). Each Brick's real address
# is in the stock OS's xr_bt.conf; jkingston imports that once per device
# (tools/provision_bluetooth_identity.py) and applies it at boot with
# brick-bluetooth-identity (package/brick-bluetooth-identity), refusing to
# invent one.
#
# An address must never be baked into the image (every Brick flashed from it
# would share one identity), so the address is resolved on the device at boot:
#   1. `bluetooth.addressFile` (default /boot/bluetooth/xr_bt.conf): copy the
#      stock xr_bt.conf there. Either "AA:BB:CC:DD:EE:FF" or the stock
#      "AA BB CC DD EE FF" format. Preferred: it is the device's real
#      (IEEE-assigned) address, so it matches stock and existing pairings,
#      and FAT /boot is writable from any computer with a card reader. It
#      survives nixos-rebuild but not a full re-flash of the card.
#   2. Otherwise, if `bluetooth.deriveFromSoc` (default true): a stable
#      address derived from the SoC's fused SID (sha256 of
#      "mortar-bluetooth:" + SID, with the locally-administered bit set and
#      the multicast bit cleared, and steered away from the inquiry LAPs).
#      Unique per chip, identical across re-flashes, needs no provisioning;
#      but it is not the stock address and not IEEE-assigned.
#   3. Otherwise the controller stays unconfigured (Bluetooth off), exactly
#      like jkingston's image without provisioning.
# Applying uses jkingston's own brick-bluetooth-identity (GPL-2.0, built from
# the pinned firmware repo), which waits for the XR829, handles the
# registration power-on race and verifies the programmed address.
{ config, lib, pkgs, ... }:

let
  cfg = config.mortar.firmware;

  brick-bluetooth-identity = pkgs.stdenv.mkDerivation {
    pname = "brick-bluetooth-identity";
    version = "0-unstable-2026-10-30";
    src = pkgs.fetchFromGitHub {
      owner = "jkingston";
      repo = "trimui-brick-firmware";
      rev = "1161bc7dac1df2a4cd32fea525d6b14129301163";
      hash = "sha256-VPzcYRcfc6ErFXp+k4t8WxvJzCYg4KFa+fLUTxY24TU=";
    };
    buildPhase = ''
      runHook preBuild
      $CC -O2 -Wall -Wextra -Werror \
        package/brick-bluetooth-identity/brick-bluetooth-identity.c \
        -o brick-bluetooth-identity
      runHook postBuild
    '';
    installPhase = ''
      runHook preInstall
      install -Dm0755 brick-bluetooth-identity $out/bin/brick-bluetooth-identity
      runHook postInstall
    '';
    meta = {
      description = "Apply a provisioned XR829 public Bluetooth address via the mgmt API";
      homepage = "https://github.com/jkingston/trimui-brick-firmware";
      license = lib.licenses.gpl2Only;
      platforms = lib.platforms.linux;
      mainProgram = "brick-bluetooth-identity";
    };
  };

  addressScript = pkgs.writeShellScript "mortar-bluetooth-address" ''
    set -euo pipefail
    export PATH=${lib.makeBinPath [ pkgs.coreutils pkgs.gnugrep brick-bluetooth-identity ]}

    state=/run/mortar-bluetooth
    rm -rf "$state"
    mkdir -m 0700 "$state"
    candidate="$state/public-address"
    umask 077

    source=""
    file=${lib.escapeShellArg cfg.bluetooth.addressFile}
    if [ -r "$file" ]; then
      # strip CR (file may come from Windows/macOS editors) and keep it small
      head -c 128 "$file" | tr -d '\r' > "$candidate"
      if brick-bluetooth-identity check "$candidate" >/dev/null 2>&1; then
        source="$file"
      else
        echo "ignoring $file: not a usable Bluetooth address" >&2
      fi
    fi

    ${lib.optionalString cfg.bluetooth.deriveFromSoc ''
      if [ -z "$source" ]; then
        sid=""
        for f in /sys/bus/nvmem/devices/sunxi-sid*/nvmem; do
          [ -r "$f" ] || continue
          sid=$(od -An -tx1 -N16 "$f" 2>/dev/null | tr -d ' \n' || true)
          [ -n "$sid" ] && [ "$sid" != 00000000000000000000000000000000 ] && break
          sid=""
        done
        if [ -n "$sid" ]; then
          h=$(printf 'mortar-bluetooth:%s' "$sid" | sha256sum | cut -c1-12)
          b0=$(( (0x''${h:0:2} & 0xfc) | 0x02 ))   # locally administered, unicast
          b3=$(( 0x''${h:6:2} ))
          # don't land in the GIAC/LIAC inquiry LAP range 9E8B00..9E8B3F
          if [ "''${h:6:4}" = 9e8b ]; then b3=0x9f; fi
          printf '%02X:%s:%s:%02X:%s:%s\n' "$b0" "''${h:2:2}" "''${h:4:2}" \
            "$b3" "''${h:8:2}" "''${h:10:2}" | tr a-f A-F > "$candidate"
          source="SoC SID"
        fi
      fi
    ''}

    if [ -z "$source" ]; then
      echo "no Bluetooth address (put the stock xr_bt.conf at $file);" \
        "XR829 stays unconfigured" >&2
      rm -f "$candidate"
      exit 0
    fi

    addr=$(brick-bluetooth-identity apply "$candidate")
    echo "XR829 public address $addr (from $source)"
    printf '%s\n' "$addr" > "$state/address"
  '';
in
{
  options.mortar.firmware = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Install the TrimUI Brick's proprietary XR829 Wi-Fi/Bluetooth and
        GE8300 GPU firmware (extracted from TrimUI's stock firmware release;
        unfree, not redistributable), and enable Wi-Fi (iwd) and Bluetooth.
      '';
    };

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../pkgs/trimui-brick-firmware.nix { };
      defaultText = lib.literalExpression "pkgs.callPackage ../pkgs/trimui-brick-firmware.nix { }";
      description = ''
        Firmware package. Its main output goes into hardware.firmware; its
        `gpu` output (PowerVR DDK userspace) is not installed.
      '';
    };

    bluetooth = {
      enable = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Enable BlueZ and program the XR829 public address at boot.";
      };

      addressFile = lib.mkOption {
        type = lib.types.str;
        default = "/boot/bluetooth/xr_bt.conf";
        description = ''
          Per-device Bluetooth public address, read at boot: the stock OS's
          xr_bt.conf ("AA BB CC DD EE FF") or "AA:BB:CC:DD:EE:FF". Lives on
          the device, never in the Nix store.
        '';
      };

      deriveFromSoc = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = ''
          When addressFile is missing or invalid, derive a stable,
          locally-administered address from the SoC SID instead of leaving
          the controller unconfigured.
        '';
      };
    };
  };

  config = lib.mkIf cfg.enable (lib.mkMerge [
    {
      hardware.firmware = [ cfg.package ];

      # iwd replaces wpa_supplicant (modules/trimui-brick.nix still enables
      # the latter from the RTL8189FTV days; NixOS refuses both at once).
      networking.wireless.enable = lib.mkForce false;
      networking.wireless.iwd = {
        enable = true;
        # networkd (networking.useNetworkd) runs DHCP on wlan0; iwd only
        # associates. 2.4 GHz, WPA2-PSK/open, station mode (see header).
        settings.General.EnableNetworkConfiguration = lib.mkDefault false;
      };

      # The XR829 has no burned-in MAC, so the driver picks a random one on
      # every boot (and reports it as permanent, so a .link
      # MACAddressPolicy=persistent leaves it alone): new IPs each boot, and
      # Avahi then sees its own name as taken ("mortar-2.local"). Set a
      # stable, locally administered MAC derived from /etc/machine-id before
      # iwd starts.
      # Set the MAC once, before iwd starts. iwd normally deletes the
      # kernel's wlan0 and creates its own, which would bring back the
      # driver's random MAC (and changing the MAC under iwd leaves the
      # device powered off), so keep the kernel's interface.
      networking.wireless.iwd.settings.DriverQuirks.DefaultInterface = "*";
      systemd.services.iwd = {
        wants = [ "mortar-wlan-mac.service" ];
        after = [ "mortar-wlan-mac.service" ];
      };
      systemd.services.mortar-wlan-mac = {
        description = "Stable MAC address for the XR829 Wi-Fi";
        requires = [ "sys-subsystem-net-devices-wlan0.device" ];
        after = [ "sys-subsystem-net-devices-wlan0.device" ];
        before = [ "iwd.service" "network-pre.target" ];
        wants = [ "network-pre.target" ];
        path = [ pkgs.coreutils pkgs.iproute2 ];
        serviceConfig = { Type = "oneshot"; RemainAfterExit = true; };
        script = ''
          h=$(printf 'mortar-wlan:%s' "$(cat /etc/machine-id)" | sha256sum | cut -c1-10)
          # 02:... = locally administered, unicast
          mac="02:$(echo "$h" | sed 's/../&:/g; s/:$//')"
          if [ "$(cat /sys/class/net/wlan0/address)" != "$mac" ]; then
            ip link set dev wlan0 down
            ip link set dev wlan0 address "$mac"
          fi
          echo "wlan0 MAC is $mac"
        '';
      };
    }

    (lib.mkIf cfg.bluetooth.enable {
      hardware.bluetooth.enable = true;

      environment.systemPackages = [ brick-bluetooth-identity ];

      systemd.services.mortar-bluetooth-address = {
        description = "Program the XR829 Bluetooth public address";
        # hci0 appears (unconfigured) once hci_xradio has loaded the firmware;
        # systemd's udev rules then pull in bluetooth.target.
        wantedBy = [ "bluetooth.target" ];
        bindsTo = [ "sys-subsystem-bluetooth-devices-hci0.device" ];
        after = [ "sys-subsystem-bluetooth-devices-hci0.device" ];
        before = [ "bluetooth.service" ];
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          ExecStart = addressScript;
          TimeoutStartSec = 60;
        };
      };
    })
  ]);
}
