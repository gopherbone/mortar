# shellcheck shell=bash
# mortar: build Mortar on this computer and put it on a Brick over SSH.
#
#   mortar deploy [--boot] [--reboot] [--host HOST] [--config NAME]
#       Build the system, copy it to the Brick, make it the default boot
#       entry and (unless --boot) switch to it now. --reboot reboots into
#       it and waits for the Brick to come back.
#   mortar rollback [--host HOST]
#       Switch back to the previous generation (also the boot default).
#   mortar generations [--host HOST]
#       List the Brick's system generations.
#   mortar reboot [--host HOST]
#       Reboot and wait until SSH answers again.
#   mortar ssh [--host HOST] [-- CMD...]
#
# Defaults: HOST=mortar.local (USB or Wi-Fi), CONFIG=brick-mainline,
# flake = the current directory (override with MORTAR_FLAKE).
set -euo pipefail

host="${MORTAR_HOST:-mortar.local}"
config="brick-mainline"
flake="${MORTAR_FLAKE:-.}"
mode="switch"
reboot=0

die() { echo "mortar: $*" >&2; exit 1; }
say() { printf '\033[1m==>\033[0m %s\n' "$*"; }

cmd="${1:-help}"; shift || true
while [ $# -gt 0 ]; do
  case "$1" in
    --host) host="$2"; shift 2 ;;
    --config) config="$2"; shift 2 ;;
    --boot) mode="boot"; shift ;;
    --reboot) reboot=1; mode="boot"; shift ;;
    --) shift; break ;;
    *) die "unknown option: $1" ;;
  esac
done

remote() { ssh -o ConnectTimeout=10 -o BatchMode=yes "root@$host" "$@"; }

wait_for_brick() {
  say "Waiting for $host"
  sleep 5
  for _ in $(seq 1 90); do
    if remote true 2>/dev/null; then say "$host is back"; return 0; fi
    sleep 2
  done
  die "$host did not come back within 3 minutes"
}

do_reboot() {
  say "Rebooting $host"
  remote 'systemctl reboot' || true
  wait_for_brick
  remote 'echo "running: $(readlink /run/current-system)"; systemctl is-system-running || true'
}

case "$cmd" in
  deploy)
    say "Building $config"
    out=$(nix build --no-link --print-out-paths \
      "$flake#nixosConfigurations.$config.config.system.build.toplevel")
    say "Copying to $host"
    # The Brick doesn't trust this computer's signing key; the closure is
    # built here from source we control, so skip the signature check.
    nix copy --no-check-sigs --to "ssh-ng://root@$host" "$out"
    say "Registering generation"
    remote "nix-env -p /nix/var/nix/profiles/system --set '$out' && '$out/bin/switch-to-configuration' $mode"
    if [ "$reboot" = 1 ]; then do_reboot; fi
    remote 'nix-env -p /nix/var/nix/profiles/system --list-generations | tail -3'
    ;;
  rollback)
    say "Rolling back $host"
    remote 'nix-env -p /nix/var/nix/profiles/system --rollback && /nix/var/nix/profiles/system/bin/switch-to-configuration switch'
    ;;
  generations)
    remote 'nix-env -p /nix/var/nix/profiles/system --list-generations'
    ;;
  reboot)
    do_reboot
    ;;
  ssh)
    exec ssh "root@$host" "$@"
    ;;
  help|-h|--help)
    cat <<'USAGE'
mortar deploy [--boot] [--reboot] [--host HOST] [--config NAME]
mortar rollback | generations | reboot [--host HOST]
mortar ssh [--host HOST] [-- CMD...]
Defaults: HOST=mortar.local, CONFIG=brick-mainline, MORTAR_FLAKE=.
USAGE
    ;;
  *) die "unknown command: $cmd (try: mortar help)" ;;
esac
