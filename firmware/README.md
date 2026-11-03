# Firmware

The boot chain lives in `mainline/`: mainline U-Boot SPL, TF-A and U-Boot, plus
the SCP coprocessor firmware. The SCP firmware is proprietary, so it is fetched
from TrimUI's published firmware at build time with pinned hashes and never kept
in this repository. See [mainline/README.md](mainline/README.md).

`trimui-brick_defconfig` here is an old copy and the build doesn't read it: the
defconfig it uses comes from `mainline/patches/u-boot/0001-*`, with Mortar's
changes in `mainline/u-boot.nix`.

An older boot path used vendor `boot0` and `boot_package` images. It's gone; the
binaries and its build scripts are in the git history.
