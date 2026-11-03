# Mortar

Mortar is NixOS for the TrimUI Brick, built on jkingston's mainline port
(`github.com/jkingston/trimui-brick-kernel`, Linux 7.3) instead of the
Liontron H-A133L DTB. brittonr's original `trimui-brick` configuration still
evaluates to exactly the same system derivation as before.

## What's here

| Path | What |
|---|---|
| `pkgs/linux-trimui-brick.nix` | Kernel. `buildLinux` from `trimui_brick_defconfig` at jkingston `91f15effb15f3752f50d46ac70c8d296c8a9c179` (the rev the Brick firmware qualifies). `enableCommonConfig = false`, `autoModules = false`, `ignoreConfigErrors = false`. Adds the NixOS/systemd options (AUTOFS, BPF/CGROUP_BPF, AUDIT, KEYS, CRYPTO_USER_API_HASH, TMPFS_XATTR, ...), nftables, zstd firmware loading, VT/fbcon, and the USB configfs functions ACM/ECM/NCM/RNDIS plus `U_SERIAL_CONSOLE`. The source hash is `lib.fakeHash` until the first build. |
| `devices/brick/hardware.nix` | Hardware module: jkingston's kernel and native Brick DTB, consoles, journald on RAM, suspend. |
| `modules/mortar-usb.nix` | `mortar.usb.*`: configfs gadget, getty on `ttyGS0`, networkd + DHCP server on `usb0`, Avahi `mortar.local`, black-box logs on `/boot`, root SSH key. |
| `flake.nix` | `nixosConfigurations.brick-full` (hostname `mortar`; old name `brick-mainline`) and `brick-bare`, `packages.<sys>.linux-trimui-brick`, `mortar-toplevel`, `sd-image` (bare) and `sd-image-full`. |

### Mainline variant vs. the Liontron workarounds

- **Dropped:** the AXP803 disable, fixed vmmc, pinctrl supply redirects, eMMC/EMAC disables and SD 25 MHz cap. These were for the Liontron DTB only; the Brick DTS describes the real AXP2202 and SD supplies.
- **Dropped:** the PSCI disable. The mainline TF-A chain provides PSCI, so all 4 cores come up. Booting this kernel through brittonr's vendor-ATF chain would need PSCI disabled again.
- **Dropped:** the watchdog disable. U-Boot arms the watchdog and the kernel has `WATCHDOG_OPEN_TIMEOUT=30`. To stop a reset ~30 s in, systemd takes it over (`RuntimeWatchdogSec=15s`) and `watchdog.open_timeout=0` is set on the cmdline.
- **Dropped:** VT getty masking. `getty@tty1` runs on the panel. `serial-getty@ttyS0` and `ttyGS0` are wanted from `multi-user.target` directly.
- **Kept, shared by both variants:** `regulator_ignore_unused`, `clk_ignore_unused`, volatile journald, tmpfs for `/tmp`, `/var/tmp` and `/var/log`, and ext4 `commit=120`.
- **Added:** `console=tty0` before `console=ttyS0,115200n8`, so serial stays `/dev/console`. Also nftables firewall (the kernel has no legacy iptables), `boot.initrd.includeDefaultModules = false` and `allowMissingModules = true`.

### USB gadget

- **IDs:** VID/PID `1d6b:0104`. Manufacturer "Mortar", product "TrimUI Brick". The serial is the SoC SID (from `/sys/bus/nvmem/devices/sunxi-sid*`), falling back to `/etc/machine-id`.
- **Functions:** `acm.usb0` (`/dev/ttyGS0` on the Brick) plus **ECM** by default. Set `mortar.usb.network = "ncm"` to use NCM instead.
  - ECM is the default because it's what jkingston ships and is known to work on macOS.
  - ACM+ECM+NCM together doesn't fit: the MUSB has 5 IN endpoints and that combination needs 6.
- **MACs:** fixed. Brick `02:4d:4f:52:54:01`, host `02:4d:4f:52:54:02`.
- **Interface name:** forced to `usb0` via the function's `ifname` attribute, plus a `.link` file as fallback.
- **Service:** `mortar-usb-gadget.service` has `DefaultDependencies=no`, is wanted by `sysinit.target` and runs before `network-pre.target`. It needs only `/sys` and the read-only store, and waits up to 30 s for a UDC.
- **Network:** `usb0` is `10.55.0.1/24`. The networkd DHCP server hands out `10.55.0.10`–`.109`, with no router or DNS so it doesn't hijack the Mac's routing. `usb0` is a trusted firewall interface.
- **mDNS:** Avahi publishes `mortar.local` on `usb0` and `wlan0`.

### Black box

`mortar-blackbox@{15s,45s,120s}` (timers) and `mortar-blackbox-shutdown` (in ExecStop) write to `/boot/mortar-logs/boot-NNNNN/`:

- `info.txt`
- `dmesg-<stage>.txt`
- `journal-<stage>.txt`
- `failed-<stage>.txt`

Each file is tail-capped at 256 KiB and followed by `sync -f`. Only the last 5 boots are kept. The boot number comes from `/boot/mortar-logs/counter`.

## Expected bring-up signals, in order

1. **Serial (UART, if attached):** U-Boot, then kernel output on ttyS0 at 115200 and a `mortar login:` prompt. The panel shows the console (tty0) and a getty.
2. **Mac sees a USB device:** System Information → USB shows "TrimUI Brick" by "Mortar", `0x1d6b/0x0104`. Run `ioreg -p IOUSB -l | grep -i mortar`.
3. **Serial over USB:** `/dev/cu.usbmodem*` appears. Run `screen /dev/cu.usbmodem* 115200` to get a login.
4. **Network:** a new "TrimUI Brick" network service appears and gets `10.55.0.x` by DHCP. `ping 10.55.0.1`, then `ping mortar.local`, then `ssh root@mortar.local` (key auth, or password `trimui`).
5. **Logs:** put the SD card in the Mac. `NIXOS_BOOT/mortar-logs/boot-NNNNN/` holds the 15s/45s/120s/shutdown snapshots. If step 2 never happens, this is where to look: check the musb/UDC lines in dmesg and `failed-*.txt`.

## To verify once Nix and a builder are available

Already checked here by evaluation only (no builds):

- `nix flake check --no-build --all-systems` passes.
- `brick-full` evaluates with no warnings.
- `trimui-brick`'s toplevel `.drv` is unchanged from before these commits.
- The pinned nixpkgs `generic.nix` accepts every argument used.
- NixOS skips `requiredKernelConfig` assertions for `buildLinux` kernels.
- The generated gadget and black-box scripts pass `bash -n`.

Still open:

1. **Kernel source hash:** build `.#linux-trimui-brick` once and paste the real hash in place of `lib.fakeHash`.
2. **Config generation:** generate-config.pl must not report `unused option` or `option not set correctly`. `ignoreConfigErrors = false` makes this fatal. The options most likely to trip are the hidden/selected ones (`USB_LIBCOMPOSITE`, `TMPFS_XATTR`, `NF_TABLES_IPV4/6`) and the EXPERT-gated ones (`EPOLL`, `SIGNALFD`, `TIMERFD`, `FHANDLE`). Drop or mark any offender with `lib.kernel.option`.
3. **modDirVersion `7.3.0-brick-dev`:** must match `make kernelrelease`.
4. **DMIID:** it is intentionally off, because it would need EFI. Confirm nothing in NixOS asserts on it.
5. **DTB output:** `hardware.deviceTree.package` must contain `allwinner/sun50i-a133-trimui-brick.dtb`, and any `extraDtOverlays` (the TF-A `/reserved-memory` node at `0x40000000`, size `0x40000`) must apply. Overlays need root `compatible = "trimui,brick"`.
6. **Initrd:** it must contain what's needed to mount the ext4 root on mmc0. `allowMissingModules` hides missing modules, so check the boot itself.
7. **On device:**
   - `/sys/class/udc` lists `musb-hdrc.*`.
   - The gadget binds and `ttyGS0` appears.
   - `usb0` gets `10.55.0.1`.
   - `systemd-networkd` serves DHCP.
   - Avahi answers on `usb0`.
8. **macOS:**
   - ECM enumerates.
   - The IAD/`0xEF` device class makes ACM and ECM show up separately.
   - Then try `mortar.usb.network = "ncm"`.
9. **Watchdog:** no reset at ~30 s, and `systemctl show -p RuntimeWatchdogUSec` reads 15 s. `reboot` actually reboots.
10. **Black box:** files land on FAT and pruning keeps 5 boots. The shutdown snapshot is written before `/boot` is unmounted.
11. **SD write stability:** root-on-SD runs at SDR104/150 MHz on this kernel, which is untested. If "Card stuck being busy" shows up, add an overlay capping mmc0 at 50 MHz.
12. **WiFi interface name:** confirm it is `wlan0`, which Avahi's `allowInterfaces` assumes. XR829 firmware isn't packaged yet.
13. **`lsm=landlock,yama,bpf`:** it is in the shared params, but the Brick defconfig doesn't build those LSMs. Harmless, but noisy.
