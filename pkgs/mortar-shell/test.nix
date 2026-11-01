# SPDX-License-Identifier: GPL-2.0-or-later
# Headless integration test: run mortar-shell on its headless backend in the
# build sandbox, start real Wayland clients through the launcher with
# injected button presses, switch between them, and screenshot the
# composited output after each step ($out/*.png, plus state.json and the
# shell's log).
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
      ];
      actions = {
        volumeGet = [ "echo" "[50%]" ];
        volumeSet = [ "true" ];
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

    echo "== home screen (no apps)"
    wait_for '.home == true'
    shot 01-home

    echo "== A on the first tile starts weston-fullscreen"
    ctl press a
    wait_for "$(visible shm)"
    sleep 0.5
    shot 02-weston-fullscreen

    echo "== MENU, R1 to Launch, Right, A: start the terminal"
    ctl press menu
    wait_for '.overlay == true'
    shot 03-menu-apps
    ctl press r1
    ctl press right
    shot 04-menu-launch
    ctl press a
    wait_for "$(visible terminal)"
    sleep 1

    echo "== type into the terminal: keyboard keys, then pad keys"
    # e c h o space (evdev codes) then the pad: Y = space? no: type letters
    for k in 18 46 35 24 57 35 23; do ctl key $k >/dev/null; done   # "echo hi"
    ctl press a   # pad A = Enter in keyboard mode
    sleep 0.5
    shot 05-terminal

    echo "== START: on-screen keyboard; type 'ls' with the D-pad, Y = Enter"
    ctl press start
    wait_for '.osk == true'
    # OSK opens on 'a' (row 3); 'l' is 8 to the right, 's' 1 to the right
    for _ in 1 2 3 4 5 6 7 8; do ctl press right >/dev/null; done
    ctl press a
    for _ in 1 2 3 4 5 6 7; do ctl press left >/dev/null; done
    ctl press a
    ctl press y
    sleep 0.5
    shot 06-terminal-osk
    ctl press start
    wait_for '.osk == false'

    echo "== MENU, A: back to the previous app (weston-fullscreen kept running)"
    ctl press menu
    wait_for '.overlay == true'
    ctl press a
    wait_for "$(visible shm)"
    sleep 0.3
    shot 07-switched-back
    ctl state | jq . > $out/state.json
    jq -e '.apps | length == 2' $out/state.json || echo "FAIL: two apps" >> $out/result

    echo "== MENU, settings tab"
    ctl press menu
    ctl press l1
    shot 08-menu-settings
    ctl press b

    echo "== close the terminal from the switcher (X twice)"
    ctl press menu   # opens with the previous app (the terminal) selected
    ctl press x
    ctl press x
    wait_for '.apps | length == 1'
    ctl press b
    shot 09-after-close

    ctl quit
    wait $shell || true
    trap - EXIT
    [ -e $out/result ] || echo PASS > $out/result
    cat $out/result
  ''
