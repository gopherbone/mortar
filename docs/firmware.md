# Brick proprietary firmware

`pkgs/trimui-brick-firmware.nix` extracts, at build time, the radio and GPU
files that the jkingston kernel's drivers request. Nothing is committed. The
files are proprietary and no licence grants redistribution; because TrimUI publishes
them, the project has decided they may be hosted in the public bare image and cache
([updates.md](updates.md#what-may-be-published)).

Source: TrimUI's `TG3040_Brick_firmware_v1.1.1.zip` (release
v1.1.1-20251126, sha256 `488db021…`). This is the same fixed-output fetch as
`firmware/mainline/scp.nix`, so the file is downloaded only once. The chain is
zip (LZMA) → `trimui_tg3040.awimg` (IMAGEWTY) → `rootfs.fex` (ext4, sha256
`023de6fe…`) → `debugfs dump` of each file. Every file is checked against the
sha256 in jkingston/trimui-brick-firmware `package/brick-{radio,gpu}/files.json`.

| Output path | Driver |
|---|---|
| `lib/firmware/xr829/boot_xr829.bin`, `fw_xr829.bin`, `sdd_xr829.bin` | `xradio_wlan` (Wi-Fi) |
| `lib/firmware/xr829/fw_xr829_bt.bin` | `hci_xradio` (Bluetooth; the driver also checks its sha256) |
| `lib/firmware/rgx.fw.22.102.54.38`, `rgx.sh.22.102.54.38` | `pvrsrvkm` (GE8300) |
| `gpu` output: `lib/lib{EGL,GLESv2,GLES_CM,IMGegl,glslcompiler,usc,srv_um,pvrNULL_WSEGL}.so*` | DDK 1.19 userspace. It is not installed; see its `share/doc/.../README` |

`devices/brick/firmware.nix` (`mortar.firmware.enable`, default true):

- Adds the firmware to `hardware.firmware`.
- Turns on iwd instead of wpa_supplicant. networkd runs DHCP on wlan0.
- Turns on BlueZ.
- Programs the XR829 Bluetooth address at boot.

## Wi-Fi

Wi-Fi works on 2.4 GHz in station mode only, with no AP, P2P or WPA3, and
HT20 only. HT needs the driver's `allow_ht` option. wlan0 gets a random MAC
on each boot.

```sh
iwctl station wlan0 connect <SSID>
```

## Bluetooth address

The XR829 comes up *unconfigured* (`HCI_QUIRK_INVALID_BDADDR`). Userspace has
to set its public address through mgmt `SET_PUBLIC_ADDRESS` before bluetoothd
will use it. `mortar-bluetooth-address.service` runs jkingston's
`brick-bluetooth-identity apply` before `bluetooth.service`, and takes the
address from the first source that works:

1. `/boot/bluetooth/xr_bt.conf` (`mortar.firmware.bluetooth.addressFile`).
   This is the stock OS's per-device file, copied onto the FAT partition.
   It takes either `AA BB CC DD EE FF` or `AA:BB:CC:DD:EE:FF`. This gives
   the real address, so it matches stock and pairings carry over.
2. If that file is missing, an address derived from the SoC SID
   (`bluetooth.deriveFromSoc`): sha256 of `mortar-bluetooth:<SID>`, with
   the locally-administered bit set. It is stable and unique per chip, but
   it is not the stock address.
3. Otherwise the controller stays unconfigured and Bluetooth stays off.

No address is ever in the image or the Nix store. To check the result:

```sh
journalctl -u mortar-bluetooth-address
cat /run/mortar-bluetooth/address
```
