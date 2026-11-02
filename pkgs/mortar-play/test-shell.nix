# SPDX-License-Identifier: GPL-2.0-or-later
# mortar-play inside mortar-shell (headless backend, in the sandbox): the
# wl_shm presenter, pause while the shell's overlay is up, keys through
# wl_keyboard, and a clean quit with the resume state when the shell closes
# the app. Then games as apps: MENU > Games (the library from `list
# --json`), a system's page, a game started as its own app, its game panel
# (save to a slot, load it), a second game putting the first to sleep
# (maxGames = 1), waking it (it resumes), starting a running game again
# (switches), the save before suspend, the screenshot combo, quitting with
# the "Saved" toast. Only the built-in test ROM (`make-test-rom`), twice.
# Screenshots of the composited output in $out, the games' memory in
# $out/rss.txt. (No /dev/uinput in the sandbox, so the gamepad path is
# covered by the device test.)
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
    games = {
      list = [ "${lib.getExe mortar-play}" "--config" "@ROOT@/play.json" "list" "--json" ];
      run = [ "${lib.getExe mortar-play}" "--config" "@ROOT@/play.json" "run" ];
      library = "play";
      maxGames = 1;
      hibernateBelowMb = 0;
    };
    ui = { screenshotDir = "@ROOT@/shots"; bootAnimation = false; };
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
    mortar-play make-test-rom "$R/roms/nes/Second Test (World).nes"
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

    echo "== games as apps: the library in MENU > Games"
    mortar-play --config $R/play.json list --json > $out/list.json
    jq -e '.systems[0].count == 2 and .recent[0].resume == true' $out/list.json >/dev/null || fail "list --json"
    ctl press menu
    wait_for '.overlay == true'
    ctl press right; ctl press right
    wait_for '.category == "Games" and (.games.systems | length) == 1'
    sleep 0.5
    shot 10-games
    echo "== Continue, then the NES row: its page"
    ctl press down
    ctl press a
    wait_for '.game_page == "nes"'
    sleep 0.6
    shot 11-games-nes
    echo "== A: the first game starts as its own app, titled from the library"
    ctl press a
    wait_for '[.apps[] | select(.game and .visible and .windows == 1)] | length == 1'
    A=$(ctl state | jq '[.apps[] | select(.game)][0].uid')
    ctl state | jq -e "[.apps[] | select(.uid == $A)][0].name == \"mortar-test · NES\"" >/dev/null || fail "game title"
    log_has "control socket"
    sleep 1.5
    shot 12-game

    echo "== Y in Running: the game panel; save to slot 1, load it"
    ctl press menu
    wait_for '.category == "Running"'
    ctl press y
    wait_for ".game_panel == $A and .games.status[\"$A\"] != null"
    sleep 0.5
    shot 13-game-panel
    ctl press a
    for _ in $(seq 50); do [ -s $R/states/nes/mortar-test.state.0 ] && break; sleep 0.1; done
    test -s $R/states/nes/mortar-test.state.0 || fail "slot 1 not written"
    test -s $R/states/nes/mortar-test.state.0.png || fail "slot 1 picture"
    wait_for ".games.status[\"$A\"].slots[0] == true"
    sleep 0.5
    shot 14-saved
    echo "== Display in the game panel: step to the next choice, keep it for the game"
    ctl press down
    ctl press down
    wait_for ".games.status[\"$A\"].display.scope == \"default\""
    ctl press right
    wait_for ".games.status[\"$A\"].display.scope == \"session\""
    ctl press a
    wait_for ".games.status[\"$A\"].display.scope == \"game\""
    jq -e '.games["nes/mortar-test"]' $R/states/display.json >/dev/null || fail "display not kept for the game"
    shot 14b-display
    ctl press up
    ctl press a
    log_has "state loaded"
    wait_for '.overlay == false'
    wait_for "[.apps[] | select(.uid == $A and .visible)] | length == 1"

    echo "== the save before suspend"
    before=$(stat -c %Y $R/states/nes/mortar-test.state.auto)
    sleep 1.1
    ctl save-games
    log_has "saved games before suspend"
    [ "$(stat -c %Y $R/states/nes/mortar-test.state.auto)" != "$before" ] || fail "auto state not written before suspend"

    echo "== the screenshot combo with a game in front: just its frame"
    ctl shot
    for _ in $(seq 30); do ls $R/shots/*.png >/dev/null 2>&1 && break; sleep 0.1; done
    ls $R/shots/*.png >/dev/null 2>&1 || fail "no screenshot"
    cp $R/shots/*.png $out/15-combo.png 2>/dev/null || true

    echo "== memory per game"
    for p in $(ctl state | jq '.apps[] | select(.game) | .pid'); do
      echo "game pid $p: $(grep VmRSS /proc/$p/status)" >> $out/rss.txt
    done

    echo "== a second game (maxGames = 1): the first goes to sleep"
    sleep 8  # past "starting"
    ctl game nes "$R/roms/nes/Second Test (World).nes"
    wait_for '[.apps[] | select(.game and .visible and .windows == 1)] | length == 1'
    wait_for "[.apps[] | select(.uid == $A and .sleeping)] | length == 1"
    log_has "putting a game to sleep"
    ctl press menu
    wait_for '.category == "Running"'
    sleep 0.6
    shot 16-asleep
    for p in $(ctl state | jq '.apps[] | select(.game and .pid != null and (.sleeping | not)) | .pid'); do
      echo "second game pid $p: $(grep VmRSS /proc/$p/status)" >> $out/rss.txt
    done

    echo "== A on the sleeping game: it wakes and resumes"
    ctl press a
    wait_for "[.apps[] | select(.uid == $A and .visible and .windows == 1)] | length == 1"
    log_has "game woken"
    sleep 1
    shot 17-woken

    echo "== starting a running game again switches to it"
    n=$(ctl game nes $R/roms/nes/mortar-test.nes)
    [ "$n" = "$A" ] || fail "a second copy of a running game ($n, $A)"

    echo "== X in Running: it quits, saved"
    ctl press menu
    wait_for '.category == "Running"'
    ctl press up  # the game in front (MENU opens on the one before)
    ctl press x
    log_has "toast from an app.*Saved"
    sleep 0.3
    shot 18-saved-toast
    wait_for "[.apps[] | select(.uid == $A)] | length == 0"

    ctl quit
    wait $shell || true
    trap - EXIT
    cp $out/shell.log $out/log.txt
    [ -e $out/result ] || echo PASS > $out/result
    cat $out/result
  ''
