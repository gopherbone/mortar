# SPDX-License-Identifier: GPL-2.0-or-later
# Headless integration test: mortar-shell on its headless backend in the build sandbox
# finds Packages by its manifest, as on the device, and the buttons drive it, against
# the made-up service on a real socket. What it asks is logged and checked: all it can
# do to anything is send a repository and an ID. Each step leaves a screenshot in $out.
# tests/mortar-packages.nix runs the real service.
{
  lib,
  runCommand,
  writeText,
  app,
  mortar-shell,
  jq,
}:

let
  config = writeText "shell.json" (builtins.toJSON {
    appProfiles = "/build/apps";
    autostart = [ ];
    ui = { bootAnimation = false; sounds = false; haptics = false; };
    actions = { volumeGet = [ "echo" "[50%]" ]; volumeSet = [ "true" ]; };
  });
in
runCommand "mortar-packages-headless-test" { nativeBuildInputs = [ mortar-shell jq ]; }
  ''
    set +e
    export HOME=/build/home XDG_RUNTIME_DIR=$TMPDIR/run
    export MORTAR_PACKAGES_SOCKET=$TMPDIR/run/packages.sock MORTAR_PACKAGES_JOB_MS=3000
    mkdir -p -m 700 $XDG_RUNTIME_DIR
    mkdir -p $out /build/apps $HOME
    ln -s ${app} /build/apps/packages

    log=$out/shell.log
    asked=$out/asked.log
    failed=0
    fail() { echo "FAIL: $*" | tee -a $out/result; failed=1; }
    check() { if eval "$2"; then echo "ok: $1"; else fail "$1"; fi; }

    ${app}/bin/mortar-packages fake-service --socket $MORTAR_PACKAGES_SOCKET --log $asked &
    service=$!
    for _ in $(seq 100); do [ -S $MORTAR_PACKAGES_SOCKET ] && break; sleep 0.1; done

    mortar-shell run --backend headless --config ${config} --socket wayland-test 2> $log &
    shell=$!
    trap 'kill $shell $service 2>/dev/null || true' EXIT
    for _ in $(seq 100); do [ -S $XDG_RUNTIME_DIR/mortar-shell.sock ] && break; sleep 0.1; done

    ctl() { mortar-shell ctl "$@"; }
    shot() { ctl screenshot "$out/$1.png" >/dev/null; }
    press() { for b in "$@"; do ctl press "$b" >/dev/null; sleep 0.35; done; }
    wait_for() {
      for _ in $(seq 150); do
        ctl state | jq -e "$1" >/dev/null && return 0
        sleep 0.1
      done
      fail "timeout: $1"
    }
    wait_asked() {
      for _ in $(seq 100); do grep -q "$1" $asked && return 0; sleep 0.1; done
      fail "never asked: $1"
    }

    echo "== the manifest: found, keyboard mode"
    wait_for '[.launcher[].id] | index("packages")'
    ctl launch packages
    wait_for '[.apps[] | select(.id == "packages" and .visible)] | length == 1'
    wait_asked '"op":"overview"'
    sleep 1
    shot 01-list

    echo "== the list rolls under the band"
    press down
    shot 02-browser

    echo "== details: asks for the size of what's missing"
    press a
    wait_asked '"op":"details","repo":"mortar","id":"harbour-browser"'
    sleep 0.5
    shot 03-details

    echo "== install: a repository and an ID, then progress"
    press a
    wait_asked '"op":"install","repo":"mortar","id":"harbour-browser"'
    wait_asked '"op":"watch"'
    sleep 1
    shot 04-progress

    echo "== leaving doesn't stop it: back at the list, and it carries on"
    press b
    sleep 0.3
    shot 05-carries-on
    check "leaving isn't cancelling" '! grep -q "\"op\":\"cancel\"" $asked'

    echo "== it comes back installed"
    sleep 3.5
    wait_asked '"op":"overview"'
    shot 06-installed
    check "installed once" '[ $(grep -c "\"op\":\"install\"" $asked) = 1 ]'

    echo "== cancel"
    press down a            # Cave Runner II: needs your files
    sleep 0.5
    shot 07-needs-files
    press a
    wait_asked '"id":"cave-runner"'
    sleep 0.5
    shot 08-progress-needy
    press x
    wait_asked '"op":"cancel"'
    sleep 1
    shot 09-cancelled

    echo "== remove takes Y: A does nothing"
    press b down            # Tracker Nine, installed
    press a x
    press a a
    check "A does nothing" '! grep -q "\"op\":\"remove\"" $asked'
    shot 10-confirm
    press y
    wait_asked '"op":"remove","repo":"mortar","id":"tracker-nine"'
    sleep 0.5
    shot 11-removed

    echo "== Terminal tools, and search on the shell's keyboard"
    press down down down
    press a
    sleep 0.3
    shot 12-tools
    press start
    wait_for '.osk == true'
    sleep 0.6
    ctl key 38 >/dev/null   # l
    ctl key 23 >/dev/null   # i
    sleep 0.6
    wait_asked '"op":"search"'
    sleep 0.3
    shot 13-search
    ctl press y >/dev/null  # the OSK's Enter: done, and the app closes the keyboard
    wait_for '.osk == false'
    sleep 0.6
    shot 14-results

    echo "== MENU belongs to the shell, and comes back"
    press menu
    wait_for '.overlay == true'
    press menu
    wait_for '.overlay == false'

    echo "== At rest: no wakeups, no frames, and nothing asked"
    sleep 2
    pid=$(ctl state | jq -r '.apps[] | select(.id == "packages") | .pid')
    echo "pid $pid: $(cat /proc/$pid/comm)"
    sw() { grep -h ctxt_switches /proc/$pid/task/*/status | awk '{n += $2} END {print n}'; }
    cpu() { awk '{print $14 + $15}' /proc/$pid/stat; }
    before=$(sw); cpu_before=$(cpu); asked_before=$(wc -l < $asked)
    sleep 5
    after=$(sw); cpu_after=$(cpu); asked_after=$(wc -l < $asked)
    echo "context switches in 5 s at rest: $((after - before)); CPU ticks: $((cpu_after - cpu_before)); requests: $((asked_after - asked_before))" | tee $out/idle.txt
    check "idle: no wakeups" '[ $((after - before)) -le 2 ]'
    check "idle: no CPU" '[ $((cpu_after - cpu_before)) -le 1 ]'
    check "idle: no requests" '[ $asked_after = $asked_before ]'

    echo "== All it asked for"
    jq -c '.op' $asked | sort | uniq -c
    check "only a repository, an ID and a query, ever" '[ -z "$(jq -c "keys - [\"op\", \"repo\", \"id\", \"query\", \"refresh\"] | select(length > 0)" $asked)" ]'

    echo "== It closes when asked"
    ctl close packages >/dev/null
    wait_for '[.apps[] | select(.id == "packages")] | length == 0'
    [ -e /proc/$pid ] && fail "still running"

    grep -i "panicked\|mortar-packages:" $log && fail "errors in the log"
    if [ $failed = 0 ]; then echo PASS | tee -a $out/result; else tail -30 $log; fi
    kill $shell $service 2>/dev/null || true
    true
  ''
