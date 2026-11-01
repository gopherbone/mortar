#!/bin/bash
# Write a Mortar SD image to a card on macOS. Refuses unless the target is an
# external, removable disk of the expected size.
#   usage: flash-mac.sh <image.img|image.img.zst> <diskN> <expected-bytes>
set -euo pipefail
IMG="$1"; DISK="$2"; WANT="$3"
info=$(diskutil info "$DISK")
grep -q 'Device Location: *External' <<<"$info" || { echo "refusing: $DISK is not external"; exit 1; }
grep -q 'Removable Media: *Removable' <<<"$info" || { echo "refusing: $DISK is not removable"; exit 1; }
have=$(sed -nE 's/.*Disk Size:.*\(([0-9]+) Bytes\).*/\1/p' <<<"$info")
[ "$have" = "$WANT" ] || { echo "refusing: $DISK is $have bytes, expected $WANT"; exit 1; }
echo "Writing $IMG to /dev/r$DISK ($have bytes). Everything on it will be erased."
diskutil unmountDisk "/dev/$DISK"
case "$IMG" in
  *.zst) zstd -dc "$IMG" | sudo dd of="/dev/r$DISK" bs=4m ;;
  *)     sudo dd if="$IMG" of="/dev/r$DISK" bs=4m ;;
esac
sync
diskutil eject "/dev/$DISK" || true
echo "Done. Put the card in the Brick and power it on."
