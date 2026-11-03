# SPDX-License-Identifier: GPL-2.0-or-later
# The bare configuration: the device and Mortar core, nothing else (boot, setup,
# Wi-Fi, updates, the shell with its Terminal). No packs, no ports.
{
  imports = [
    ../devices/brick
    ../modules/core
  ];
}
