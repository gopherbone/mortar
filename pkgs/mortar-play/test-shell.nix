# SPDX-License-Identifier: GPL-2.0-or-later
# mortar-play inside mortar-shell (headless backend, in the sandbox): the
# wl_shm presenter, pause while the shell's overlay is up, keys through
# wl_keyboard, and a clean quit with the resume state when the shell closes
# the app. Screenshots of the composited output in $out. (No /dev/uinput in
# the sandbox, so the gamepad path is covered by the device test.)
{
  lib,
  runCommand,
  writeText,
  jq,
  libretro,
  mortar-play,
  mortar-shell,
}:

let
  core = pkg: "${pkg}/lib/retroarch/cores/${lib.replaceStrings [ "-" ] [ "_" ] pkg.core}_libretro.so";
  playConfig = writeText "mortar-play.json" (builtins.toJSON {
    romDirs = [ "@ROOT@/roms" ];
    biosDir = "@ROOT@/bios";
    saveDir = "@ROOT@/saves";
    stateDir = "@ROOT@/states";
    dataDir = "@ROOT@";
    systems = [
      {
        id = "nes";
        name = "NES";
        tag = "NES";
        folders = [ "nes" ];
        extensions = [ "nes" ];
        core = core libretro.fceumm;
        options = { fceumm_aspect = "4:3"; };
      }
    ];
  });
  shellConfig = writeText "mortar-shell-test.json" (builtins.toJSON {
    apps = [
      {
        id = "play";
        name = "Games";
        description = "mortar-play";
        icon = "pad";
        command = [ "${lib.getExe mortar-play}" "--config" "@ROOT@/play.json" ];
        env = { MORTAR_PLAY_LOG = "warn"; };
      }
    ];
    actions = {
      volumeGet = [ "echo" "[50%]" ];
      volumeSet = [ "true" ];
    };
  });
in
runCommand "mortar-play-shell-test"
  {
    nativeBuildInputs = [ mortar-shell mortar-play jq ];
  }
  ''
    export XDG_RUNTIME_DIR=$TMPDIR/run HOME=$TMPDIR/home
    R=$TMPDIR/play
    mkdir -p -m 700 $XDG_RUNTIME_DIR
    mkdir -p $HOME $out $R/roms/nes
    sed "s|@ROOT@|$R|g" ${playConfig} > $R/play.json
    sed "s|@ROOT@|$R|g" ${shellConfig} > $R/shell.json
    mortar-play make-test-rom $R/roms/nes/mortar-test.nes
    fail() { echo "FAIL: $*" | tee -a $out/result; }

    MORTAR_SHELL_LOG=info mortar-shell run --backend headless --config $R/shell.json --socket wayland-test 2> $out/shell.log &
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
      fail "timeout: $1"; ctl state | jq .
    }
    log_has() {
      for _ in $(seq 50); do grep -q "$1" $out/shell.log && return 0; sleep 0.1; done
      fail "log lacks: $1"
    }

    echo "== launch: the library inside the shell"
    ctl launch play
    wait_for '[.apps[] | select(.id == "play" and .visible)] | length == 1'
    sleep 1.5
    shot 01-library
    log_has "display wayland-shm"

    echo "== Enter (A) twice: game list, then the game"
    ctl key 28; sleep 0.5
    shot 02-games
    ctl key 28
    log_has "FCEUmm"
    sleep 2
    shot 03-game

    echo "== MENU: shell overlay, the game pauses"
    ctl press menu
    wait_for '.overlay == true'
    log_has "hidden, paused"
    shot 04-shell-overlay
    ctl press b
    wait_for '.overlay == false'
    log_has "visible again, resuming"
    sleep 1
    shot 05-game-again

    echo "== F1 (keyboard): the game's own menu, Esc resumes"
    ctl key 59; sleep 0.7
    shot 06-game-menu
    ctl key 1; sleep 0.5

    echo "== close from the shell: quits cleanly with the resume state"
    ctl close play
    wait_for '[.apps[] | select(.id == "play")] | length == 0'
    log_has "bye"
    test -s $R/states/nes/mortar-test.state.auto || fail "no resume state after close"

    ctl quit
    wait $shell || true
    trap - EXIT
    cp $out/shell.log $out/log.txt
    [ -e $out/result ] || echo PASS > $out/result
    cat $out/result
  ''
