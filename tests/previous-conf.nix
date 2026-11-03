# SPDX-License-Identifier: GPL-2.0-or-later
# What the rollback key would boot: extlinux/previous.conf from a real-format
# extlinux.conf, for the histories that matter (devices/brick/rollback.nix).
{ runCommand, previousConf }:

runCommand "mortar-previous-conf-check" { } ''
  entry() { # label, generation system
    printf '\nLABEL %s\n  MENU LABEL x\n  LINUX ../nixos/%s-Image\n  INITRD ../nixos/%s-initrd\n  APPEND init=/nix/store/%s/init root=/dev/mmcblk0p2\n  FDT ../nixos/%s-dtbs/x.dtb\n' "$1" "$2" "$2" "$2" "$2"
  }
  conf() { # default system, then "generation:system" newest first
    d=$1; shift; printf '# Generated\n\nDEFAULT nixos-default\n\nTIMEOUT 1\nMENU TITLE ---\n'
    entry nixos-default "$d"
    for g in "$@"; do entry "nixos-''${g%%:*}-default" "''${g##*:}"; done
  }
  prev() { sed -n 's/^DEFAULT //p' t/previous.conf; }

  mkdir t
  run() { "$@" > t/extlinux.conf; ${previousConf}/bin/mortar-previous-conf t; }

  # newest is the default: the one before it
  run conf C 7:C 6:B 5:A
  test "$(prev)" = nixos-6-default
  grep -q 'init=/nix/store/B/init' t/previous.conf
  grep -q 'LINUX ../nixos/B-Image' t/previous.conf
  grep -q 'FDT ../nixos/B-dtbs/x.dtb' t/previous.conf
  grep -q '^TIMEOUT 1$' t/previous.conf
  test "$(grep -c '^LABEL' t/previous.conf)" = 1
  ! grep -q 'init=/nix/store/C' t/previous.conf

  # the same system published twice in a row: a different one
  run conf C 8:C 7:C 6:B
  test "$(prev)" = nixos-6-default

  # rolled back from 7 to 6: 5, not the 7 that was left
  run conf B 7:C 6:B 5:A
  test "$(prev)" = nixos-5-default

  # the same system again later (5 and 7 are A): below the newest match
  run conf A 7:A 6:B 5:A
  test "$(prev)" = nixos-6-default

  # the default isn't among the generations (a deployed toplevel, pruned): newest
  run conf X 7:C 6:B
  test "$(prev)" = nixos-7-default

  # one generation: nothing to go back to, and an old file goes away
  run conf C 7:C
  test ! -e t/previous.conf

  # an empty or broken extlinux.conf leaves no file and no error
  run printf 'garbage\n'
  test ! -e t/previous.conf
  test "$(ls t | grep -c tmp || true)" = 0

  echo PASS > $out
''
