# SPDX-License-Identifier: GPL-2.0-or-later
# Headless integration test: run mortar-shell on its headless backend in the
# build sandbox, start real Wayland clients through the launcher with
# injected button presses, switch between them, and screenshot the
# composited output after each step ($out/*.png, plus state.json and the
# shell's log). The UI is drawn by the CPU renderer here (no GPU in the
# sandbox); `ctl time manual` freezes UI time so transition frames
# (MENU, launch, toast, OSD, flash, boot) are captured exactly
# ($out/t-*.png).
{
  lib,
  runCommand,
  writeText,
  mortar-shell,
  weston,
  foot,
  bashInteractive,
  jq,
  makeFontsConf,
  dejavu_fonts,
  tzdata,
}:

let
  footIni = writeText "foot.ini" ''
    font=DejaVu Sans Mono:size=14
    pad=12x12
    [colors-dark]
    background=1d1d1d
    foreground=e1ebf7
  '';
  config = writeText "mortar-shell-test.json" (
    builtins.toJSON {
      apps = [
        {
          id = "shm";
          name = "Fullscreen";
          description = "weston-fullscreen (wl_shm)";
          icon = "pad";
          command = [ "${weston}/bin/weston-fullscreen" ];
        }
        {
          id = "terminal";
          name = "Terminal";
          description = "foot";
          icon = "term";
          gamepad = "keyboard";
          command = [ "${lib.getExe foot}" "--config" "${footIni}" "${bashInteractive}/bin/bash" "--norc" ];
        }
        {
          id = "claude";
          name = "Claude";
          description = "weston-flower stands in";
          icon = "claude";
          command = [ "${weston}/bin/weston-flower" ];
        }
        {
          id = "m8";
          name = "M8";
          description = "Tracker · top USB-C";
          icon = "usb";
          command = [ "${weston}/bin/weston-simple-shm" ];
        }
      ];
      actions = {
        volumeGet = [ "echo" "[50%]" ];
        volumeSet = [ "true" ];
        # stands in for timedatectl: point a local link at the zone
        timezoneSet = [ "sh" "-c" "ln -sfn ${tzdata}/share/zoneinfo/$1 $HOME/localtime" "sh" "{zone}" ];
      };
      ui = {
        zoneinfo = "${tzdata}/share/zoneinfo";
        localtime = "~/localtime";
      };
    }
  );
in
# Always succeeds so the screenshots survive a failure; `check` (the
# package's passthru.tests.headless) fails unless $out/result says PASS.
runCommand "mortar-shell-headless-test"
  {
    nativeBuildInputs = [ mortar-shell jq ];
    FONTCONFIG_FILE = makeFontsConf { fontDirectories = [ dejavu_fonts ]; };
  }
  ''
    export XDG_RUNTIME_DIR=$TMPDIR/run HOME=$TMPDIR/home PS1='$ '
    mkdir -p -m 700 $XDG_RUNTIME_DIR $HOME
    mkdir -p $out
    log=$out/shell.log

    MORTAR_SHELL_LOG=info,mortar_shell=debug mortar-shell run --backend headless --config ${config} --socket wayland-test 2> $log &
    shell=$!
    trap 'kill $shell 2>/dev/null || true' EXIT
    for _ in $(seq 100); do [ -S $XDG_RUNTIME_DIR/mortar-shell.sock ] && break; sleep 0.1; done

    ctl() { mortar-shell ctl "$@"; }
    shot() { ctl screenshot "$out/$1.png" >/dev/null; echo "screenshot $1"; }
    wait_for() {
      for _ in $(seq 150); do
        ctl state | jq -e "$1" >/dev/null && return 0
        sleep 0.1
      done
      echo "TIMEOUT waiting for: $1"; ctl state | jq .
      echo "FAIL: $1" >> $out/result
      return 0
    }
    visible() { echo "[.apps[] | select(.id == \"$1\" and .visible)] | length == 1"; }

    echo "== boot screen, then home (no apps)"
    wait_for '.ui.booting == false'
    wait_for '.home == true'
    sleep 0.3
    shot 01-home

    echo "== A on the first tile: weston-fullscreen grows out of its tile"
    ctl press a
    wait_for "$(visible shm)"
    wait_for '.ui.layer == false'
    shot 02-weston-fullscreen

    echo "== MENU: the cross over the app; Right to Apps, Down, A: start the terminal"
    ctl press menu
    wait_for '.overlay == true and .category == "Running"'
    sleep 0.8
    shot 03-menu-running
    ctl press right
    ctl press down
    sleep 0.5
    shot 04-menu-apps
    ctl press a
    wait_for "$(visible terminal)"
    wait_for '.ui.layer == false'
    sleep 0.5

    echo "== type into the terminal: keyboard keys, then pad keys"
    for k in 18 46 35 24 57 35 23; do ctl key $k >/dev/null; done   # "echo hi"
    ctl press a   # pad A = Enter in keyboard mode
    sleep 0.5
    shot 05-terminal

    echo "== START: on-screen keyboard; type 'ls' with the D-pad, Y = Enter"
    ctl press start
    wait_for '.osk == true'
    for _ in 1 2 3 4 5 6 7 8; do ctl press right >/dev/null; done
    ctl press a
    for _ in 1 2 3 4 5 6 7; do ctl press left >/dev/null; done
    ctl press a
    ctl press y
    sleep 0.5
    shot 06-terminal-osk
    ctl press start
    wait_for '.osk == false'

    echo "== MENU over the terminal (its text blurred behind), A: back to the previous app"
    ctl press menu
    wait_for '.overlay == true'
    sleep 0.8
    shot 06b-menu-over-terminal
    ctl press a
    wait_for "$(visible shm)"
    wait_for '.ui.layer == false'
    shot 07-switched-back
    ctl state | jq . > $out/state.json
    jq -e '.apps | length == 2' $out/state.json || echo "FAIL: two apps" >> $out/result

    echo "== MENU, Quick (R1: the last category, Power; Left: Quick)"
    ctl press menu
    ctl press r1
    ctl press left
    wait_for '.category == "Quick"'
    sleep 0.8
    shot 08-menu-quick
    ctl press b
    wait_for '.ui.layer == false'

    echo "== close the terminal from Running (X)"
    ctl press menu   # opens with the previous app (the terminal) selected
    sleep 0.5
    ctl press x
    wait_for '.apps | length == 1'
    sleep 0.6
    shot 09-app-closed
    ctl press b
    wait_for '.ui.layer == false'
    shot 10-after-close

    echo "== transition frames (UI time frozen, stepped by hand)"
    tshot() { ctl time +$1 >/dev/null; shot "t-$2"; }
    ctl home
    wait_for '.home == true and .ui.layer == true'
    ctl time manual
    tshot 1500 home-settled
    ctl press right
    tshot 60 home-move-060
    tshot 400 home-move-460
    ctl press menu
    tshot 50 menu-050
    tshot 100 menu-150
    tshot 600 menu-750
    ctl press b
    tshot 80 menu-close-080
    tshot 600 menu-close-680
    ctl press left
    tshot 600 home-focus-first
    ctl press a     # weston-fullscreen is running: it grows out of its tile
    tshot 60 launch-060
    tshot 60 launch-120
    tshot 100 launch-220
    tshot 600 launch-820
    wait_for "$(visible shm)"
    tshot 300 launch-done
    ctl press volup
    tshot 80 osd-080
    tshot 400 osd-480
    ctl toast '{"title":"First jump","sub":"Games · Game Boy","kind":"achievement"}'
    tshot 60 toast-060
    tshot 500 toast-560
    tshot 4000 toast-gone
    ctl shot
    tshot 30 shutter-030
    tshot 300 shutter-330
    ctl time real
    sleep 0.4
    ctl time manual
    tshot 400 shot-toast
    ctl time real
    sleep 3
    ls $HOME/Pictures/Screenshots/mortar-*.png > $out/screenshots.txt || echo "FAIL: screenshot file" >> $out/result

    echo "== MENU from the app, Home row: the app shrinks back into its tile"
    ctl time manual
    ctl press menu
    tshot 700 menu-over-app
    ctl press up
    tshot 300 menu-home-row
    ctl press a
    tshot 100 close-menu-fade
    tshot 40 close-040
    tshot 60 close-100
    tshot 800 close-done

    # MENU > Quick > Settings… (the last Quick row)
    settings() {
      ctl press menu
      ctl press r1
      ctl press left
      for _ in 1 2 3 4 5 6 7 8; do ctl press down >/dev/null; done
      ctl press a
    }

    echo "== night palette (MENU > Quick > Settings…), saved"
    settings
    tshot 800 settings
    jq -e '.settings == true' <(ctl state) >/dev/null || echo "FAIL: settings screen" >> $out/result
    for _ in 1 2 3 4; do ctl press down >/dev/null; done
    ctl press right
    tshot 800 night-settings
    ctl press menu
    tshot 800 night-home
    ctl prefs > $out/prefs.json
    jq -e '.night == true' $HOME/.local/state/mortar-shell/prefs.json >/dev/null || echo "FAIL: prefs saved" >> $out/result

    echo "== Settings > Time zone: region list, Europe, pick the first city"
    jq -e '.timezone == "UTC"' <(ctl state) >/dev/null || echo "FAIL: starts in UTC" >> $out/result
    settings
    for _ in 1 2 3 4 5; do ctl press down >/dev/null; done
    tshot 700 tz-settings-row
    ctl press a
    tshot 700 tz-regions
    for _ in 1 2 3; do ctl press up >/dev/null; done   # UTC -> Pacific -> Indian -> Europe
    tshot 300 tz-regions-europe
    ctl press a
    tshot 700 tz-europe
    ctl press r1
    tshot 400 tz-europe-page
    ctl press a
    tshot 120 tz-set-toast
    tshot 800 tz-back-in-settings
    ctl state | jq -r .timezone > $out/timezone.txt
    jq -e '.timezone | startswith("Europe/")' <(ctl state) >/dev/null || echo "FAIL: time zone set" >> $out/result
    ctl press menu
    tshot 800 tz-home

    echo "== power key: hold for the power menu, confirm, Sleep, wake"
    ctl power hold
    sleep 0.5
    tshot 16 power-hold
    sleep 0.6
    tshot 16 power-hold-done
    ctl power-up
    tshot 500 power-menu
    jq -e '.power_menu == true' <(ctl state) >/dev/null || echo "FAIL: power menu" >> $out/result
    ctl press down
    ctl press a
    tshot 200 power-confirm-restart
    ctl press b
    ctl press up
    ctl press a
    tshot 250 sleep-fading
    tshot 2000 sleep-dark
    jq -e '.sleep == "Dark"' <(ctl state) >/dev/null || echo "FAIL: asleep" >> $out/result
    ctl press a
    tshot 120 wake-120
    tshot 1500 wake-done
    jq -e '.sleep == "Awake" and .power_menu == false' <(ctl state) >/dev/null || echo "FAIL: awake" >> $out/result
    echo "== power key tap: sleep, tap again: wake"
    ctl power
    tshot 2000 tap-sleep
    jq -e '.sleep == "Dark"' <(ctl state) >/dev/null || echo "FAIL: tap sleeps" >> $out/result
    ctl power
    tshot 1500 tap-wake
    jq -e '.sleep == "Awake"' <(ctl state) >/dev/null || echo "FAIL: tap wakes" >> $out/result

    echo "== boot replay"
    ctl boot
    tshot 200 boot-200
    tshot 500 boot-700
    tshot 400 boot-1100
    tshot 1200 boot-2300
    tshot 600 boot-done
    ctl time real

    ctl quit
    wait $shell || true
    trap - EXIT
    [ -e $out/result ] || echo PASS > $out/result
    cat $out/result
  ''
