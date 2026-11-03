# Bring-up notes

How the base system talks to a computer and what to check when bringing it up on
a card. For the layers see the [README](../README.md); for the boot chain,
[firmware/mainline/README.md](../firmware/mainline/README.md).

## Where things are

| Path | What |
|---|---|
| `pkgs/linux-trimui-brick.nix` | Kernel. `buildLinux` from `trimui_brick_defconfig` at jkingston `91f15effb15f3752f50d46ac70c8d296c8a9c179` (the rev the Brick firmware qualifies). `enableCommonConfig = false`, `autoModules = false`, `ignoreConfigErrors = false`. Adds the NixOS/systemd options, nftables, zstd firmware loading, VT/fbcon and the USB configfs functions. |
| `devices/brick/hardware.nix` | Hardware module: jkingston's kernel and native Brick DTB, consoles, journald in RAM, suspend. |
| `devices/brick/fastboot.nix` | No initrd, the activation shortcut, quiet consoles. |
| `modules/core/usb.nix` | `mortar.usb.*`: configfs gadget, `usb0` with a DHCP server, Avahi `mortar.local`, black-box logs on `/boot`, SSH from the card's keys. |

## USB gadget

- **IDs:** VID/PID `1d6b:0104`. Manufacturer "Mortar", product "TrimUI Brick". The serial is the SoC SID (from `/sys/bus/nvmem/devices/sunxi-sid*`), falling back to `/etc/machine-id`.
- **Functions:** `acm.usb0` (`/dev/ttyGS0` on the Brick) plus **ECM** by default. Set `mortar.usb.network = "ncm"` to use NCM instead.
  - ECM is the default because it's what jkingston ships and is known to work on macOS.
  - ACM+ECM+NCM together doesn't fit: the MUSB has 5 IN endpoints and that combination needs 6.
- **MACs:** fixed. Brick `02:4d:4f:52:54:01`, host `02:4d:4f:52:54:02`.
- **Interface name:** forced to `usb0` via the function's `ifname` attribute, plus a `.link` file as fallback.
- **Service:** `mortar-usb-gadget.service` has `DefaultDependencies=no`, is wanted by `sysinit.target` and runs before `network-pre.target`. It needs only `/sys` and the read-only store, and waits up to 30 s for a UDC.
- **Network:** `usb0` is `10.55.0.1/24`. The networkd DHCP server hands out `10.55.0.10`–`.109`, with no router or DNS so it doesn't hijack the computer's routing.
- **mDNS:** Avahi publishes `mortar.local` on `usb0` and `wlan0`.

Who can do what over the cable: [pairing.md](pairing.md#what-a-usb-host-can-do).

## Black box

`mortar-blackbox@{15s,45s,120s}` (timers) and `mortar-blackbox-shutdown` (in ExecStop) write to `/boot/mortar-logs/boot-NNNNN/`:

- `info.txt`
- `dmesg-<stage>.txt`
- `journal-<stage>.txt`
- `failed-<stage>.txt`

Each file is tail-capped at 256 KiB and followed by `sync -f`. Only the last 5 boots are kept. The boot number comes from `/boot/mortar-logs/counter`.

## Expected bring-up signals, in order

1. **UART, if attached:** U-Boot, then kernel output on ttyS0 at 115200 and a root shell (the pads are the unauthenticated recovery path).
2. **The computer sees a USB device:** "TrimUI Brick" by "Mortar", `0x1d6b/0x0104`. On a Mac: `ioreg -p IOUSB -l | grep -i mortar`.
3. **Network:** a new "TrimUI Brick" network service appears and gets `10.55.0.x` by DHCP. `ping 10.55.0.1`, then `ping mortar.local`, then `mortar pair`.
4. **Logs:** put the SD card in a computer. `NIXOS_BOOT/mortar-logs/boot-NNNNN/` holds the 15s/45s/120s/shutdown snapshots. If step 2 never happens, this is where to look: check the musb/UDC lines in dmesg and `failed-*.txt`.

## What to look at on a device

- `/sys/class/udc` lists `musb-hdrc.*`; the gadget binds; `usb0` gets `10.55.0.1`; `systemd-networkd` serves DHCP; Avahi answers on `usb0`.
- macOS: ECM enumerates, and the IAD/`0xEF` device class makes ACM and ECM show up separately.
- Watchdog: no reset at ~30 s, and `systemctl show -p RuntimeWatchdogUSec` reads 15 s. `reboot` actually reboots.
- Black box: files land on FAT and pruning keeps 5 boots. The shutdown snapshot is written before `/boot` is unmounted.
- SD write stability: if "Card stuck being busy" shows up, add an overlay capping mmc0 at 50 MHz.
