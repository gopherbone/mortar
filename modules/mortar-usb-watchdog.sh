#!/usr/bin/env bash
# mortar-usb-watchdog: detect a wedged USB gadget link on usb0 and recover.
#
# Failure being handled (see README.md): the MUSB/PHY on the Brick stops
# answering on the bus (the host logs xHCI "transaction error" on every
# endpoint including ep0) while the Brick still thinks it is configured:
# carrier up, no kernel message, nothing ever reaches usb0 or ttyGS0, and
# the host never sees a disconnect, so it never re-enumerates.
#
# Detection: once the host has been seen (usb0 RX counter moved while carrier
# was up), RX that stays flat for IDLE seconds triggers a probe that every host
# answers (neighbour flush -> ARP/NS, plus an all-nodes ping). If RX is still
# flat, the link is declared dead.
# Recovery ladder, each step followed by a wait for RX:
#   1. soft_connect toggle (pull-up off/on: the host sees a real disconnect)
#   2. UDC unbind/rebind of the configfs gadget (musb_stop/musb_start)
#   3. the same with a longer (5 s) detach
# Do NOT add an unbind/bind of the musb-sunxi glue (5100000.usb) here: on
# the current kernel that oopses in devm_usb_phy_release() and leaves the
# device lock held, which then also hangs reboot (see README / 0001 patch).
# If nothing brings RX back (e.g. cable unplugged: this board has no VBUS
# detection, so carrier stays up), the watchdog disarms until RX resumes.
set -u

IF=${MORTAR_USB_IF:-usb0}
G=${MORTAR_USB_GADGET:-/sys/kernel/config/usb_gadget/mortar}
INTERVAL=${MORTAR_USB_INTERVAL:-5}
IDLE=${MORTAR_USB_IDLE:-30}
SETTLE=${MORTAR_USB_SETTLE:-20}
STATE=/run/mortar-usb-watchdog
DRY=${MORTAR_USB_DRY_RUN:-0}

mkdir -p "$STATE"

log() {
  echo "$*"
  echo "mortar-usb-watchdog: $*" > /dev/kmsg 2>/dev/null || true
}
now() { cut -d. -f1 /proc/uptime; }
# MORTAR_USB_RX_FILE: test hook, read the RX counter from a file instead
# (a frozen file simulates a wedged link and exercises the recovery path).
RXF=${MORTAR_USB_RX_FILE:-/sys/class/net/$IF/statistics/rx_packets}
rx() { cat "$RXF" 2>/dev/null || echo 0; }
carrier() { [ "$(cat "/sys/class/net/$IF/carrier" 2>/dev/null)" = 1 ]; }
bound_udc() { cat "$G/UDC" 2>/dev/null; }
first_udc() { ls /sys/class/udc 2>/dev/null | head -n1; }
musb_irqs() { awk '/musb/{s=0; for(i=2;i<=NF;i++) if($i ~ /^[0-9]+$/) s+=$i; else break; print s}' /proc/interrupts; }

probe() {
  # Force address resolution (ARP/NS replies are never firewalled), then ping
  # all neighbours and the all-nodes group. Only the RX counter is judged.
  ip neigh flush dev "$IF" >/dev/null 2>&1 || true
  ping -c 2 -i 0.5 -W 1 -I "$IF" ff02::1 >/dev/null 2>&1 &
  # Re-ping every address that was a neighbour in the last snapshot.
  while read -r a; do
    case "$a" in fe80:*) a="$a%$IF" ;; esac
    [ -n "$a" ] && ping -c 2 -i 0.5 -W 1 -I "$IF" "$a" >/dev/null 2>&1 &
  done < "$STATE/neigh" 2>/dev/null
  wait
}

snapshot_neigh() {
  ip neigh show dev "$IF" 2>/dev/null | awk '$1 !~ /^ff/ {print $1}' | sort -u > "$STATE/neigh.new" &&
    [ -s "$STATE/neigh.new" ] && mv "$STATE/neigh.new" "$STATE/neigh"
}

wait_rx() {
  local base t
  base=$(rx)
  for t in $(seq 1 "$SETTLE"); do
    sleep 1
    [ "$(rx)" != "$base" ] && return 0
    # Nudge the host a few times while it re-enumerates.
    [ $((t % 5)) = 0 ] && probe
  done
  return 1
}

dump_state() {
  local f="$STATE/wedge-$(now).txt" i0 i1
  i0=$(musb_irqs); sleep 2; i1=$(musb_irqs)
  {
    echo "uptime: $(cat /proc/uptime)"
    echo "udc: $(bound_udc) state=$(cat /sys/class/udc/*/state 2>/dev/null)"
    echo "musb irqs over 2s while wedged: $((i1 - i0))"
    ip -s link show dev "$IF"
    ip neigh show dev "$IF"
    cat /sys/kernel/debug/usb/musb-hdrc.*/regdump 2>/dev/null
  } > "$f" 2>&1
  log "wedge diagnostics in $f (musb irqs/2s: $((i1 - i0)))"
  # Persist logs to the FAT black box if that service exists.
  systemctl start --no-block mortar-blackbox@usbwedge.service >/dev/null 2>&1 || true
}

# mortar-power soft-reconnects the gadget when the cable is pulled (so the
# UDC forgets a stale "configured" state); both take this lock around any
# UDC change so they never toggle it at the same time.
LOCK=${MORTAR_USB_UDC_LOCK:-/run/mortar-usb-udc.lock}
with_udc_lock() { ( flock -w 10 9 || exit 1; "$@" ) 9>"$LOCK"; }

step_softconnect() {
  local u; u=$(bound_udc); [ -n "$u" ] || return 1
  log "recovery 1: soft_connect toggle on $u"
  [ "$DRY" = 1 ] && return 0
  echo disconnect > "/sys/class/udc/$u/soft_connect" || return 1
  sleep 1
  echo connect > "/sys/class/udc/$u/soft_connect"
}

step_rebind() {
  local u; u=$(bound_udc); [ -n "$u" ] || u=$(first_udc)
  log "recovery 2: gadget UDC unbind/rebind ($u)"
  [ "$DRY" = 1 ] && return 0
  echo "" > "$G/UDC" 2>/dev/null
  sleep 1
  echo "$u" > "$G/UDC"
}

step_long_rebind() {
  local u; u=$(bound_udc); [ -n "$u" ] || u=$(first_udc)
  log "recovery 3: gadget UDC unbind, 5 s detach, rebind ($u)"
  [ "$DRY" = 1 ] && return 0
  echo "" > "$G/UDC" 2>/dev/null
  sleep 5
  echo "$u" > "$G/UDC"
}

armed=0
last=$(rx)
last_change=$(now)
log "started (if=$IF idle=${IDLE}s)"

while sleep "$INTERVAL"; do
  if ! carrier || [ -z "$(bound_udc)" ]; then
    last=$(rx); last_change=$(now)
    continue
  fi
  r=$(rx)
  if [ "$r" != "$last" ]; then
    [ "$armed" = 1 ] || log "armed: host traffic seen on $IF"
    armed=1; last=$r; last_change=$(now)
    snapshot_neigh
    continue
  fi
  [ "$armed" = 1 ] || continue
  idle=$(( $(now) - last_change ))
  [ "$idle" -ge "$IDLE" ] || continue

  alive=0
  for attempt in 1 2; do
    probe; sleep 2
    if [ "$(rx)" != "$last" ]; then alive=1; break; fi
  done
  if [ "$alive" = 1 ]; then
    last=$(rx); last_change=$(now)
    continue
  fi

  log "link dead: no RX on $IF for ${idle}s, probes unanswered (carrier up, UDC $(bound_udc))"
  dump_state
  ok=0
  for step in step_softconnect step_rebind step_long_rebind; do
    if with_udc_lock $step && wait_rx; then
      log "recovered by ${step#step_}"
      ok=1
      break
    fi
  done
  if [ "$ok" = 0 ]; then
    log "recovery failed (host gone?); disarmed until RX resumes"
    armed=0
  fi
  last=$(rx); last_change=$(now)
done
