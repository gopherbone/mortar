# SPDX-License-Identifier: GPL-2.0-or-later
# Headless integration test: mortar-shell on its headless backend in the build
# sandbox finds Files by its manifest, as on the device, and the buttons drive
# it. Real files change on disk; each step leaves a screenshot in $out.
{
  lib,
  runCommand,
  writeShellScript,
  writeText,
  app,
  mortar-shell,
  jq,
}:

let
  # stands in for mortar-play: says what it was asked to start
  fakePlay = writeShellScript "fake-play" ''echo "$@" > "$HOME/game-started"'';
  playConfig = writeText "play.json" (builtins.toJSON {
    dataDir = "/build/play";
    romDirs = [ "/build/play/roms" ];
    systems = [{ id = "nes"; folders = [ "nes" ]; extensions = [ "nes" ]; }];
  });
  config = writeText "shell.json" (builtins.toJSON {
    appProfiles = "/build/apps";
    autostart = [ ];
    env.MORTAR_PLAY_CONFIG = "${playConfig}";
    games = { list = [ "echo" "{}" ]; run = [ "${fakePlay}" ]; };
    ui = { bootAnimation = false; sounds = false; haptics = false; };
    actions = { volumeGet = [ "echo" "[50%]" ]; volumeSet = [ "true" ]; };
  });
in
runCommand "mortar-files-headless-test" { nativeBuildInputs = [ mortar-shell jq ]; }
  ''
    set +e
    export HOME=/build/home XDG_RUNTIME_DIR=$TMPDIR/run
    mkdir -p -m 700 $XDG_RUNTIME_DIR
    mkdir -p $out /build/apps
    ln -s ${app} /build/apps/files

    # made-up files
    mkdir -p $HOME/Documents $HOME/Downloads /build/play/roms/nes /build/play/saves /build/play/bios
    echo "the first line" > $HOME/Documents/notes.txt
    echo keep > $HOME/Documents/keep.txt
    head -c 400000 /dev/zero > $HOME/Downloads/data.bin
    printf 'NES\x1a' > "/build/play/roms/nes/Moon Miner.nes"

    log=$out/shell.log
    failed=0
    fail() { echo "FAIL: $*" | tee -a $out/result; failed=1; }
    check() { if eval "$2"; then echo "ok: $1"; else fail "$1"; fi; }

    mortar-shell run --backend headless --config ${config} --socket wayland-test 2> $log &
    shell=$!
    trap 'kill $shell 2>/dev/null || true' EXIT
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
    wait_file() {
      for _ in $(seq 100); do [ -e "$1" ] && return 0; sleep 0.1; done
      fail "no file $1"
    }

    echo "== the manifest: found, keyboard mode"
    wait_for '[.launcher[].id] | index("files")'
    ctl launch files
    wait_for '[.apps[] | select(.id == "files" and .visible)] | length == 1'
    sleep 1
    shot 01-places

    echo "== Home, into Documents, a preview"
    press a                 # Home
    press a                 # Documents (folders first)
    sleep 0.3
    shot 02-documents

    echo "== Copy with the actions menu, put it down elsewhere"
    press x a               # Copy: keep.txt
    press b down a          # back to Home, down to Downloads, in
    press y
    wait_file $HOME/Downloads/keep.txt
    check "copied" 'cmp $HOME/Documents/keep.txt $HOME/Downloads/keep.txt'
    shot 03-copied

    echo "== Delete: A does nothing, Y goes to the Trash"
    # Downloads holds data.bin, keep.txt; the copy is selected
    press x down down down a  # Copy Move Rename Delete
    press a a
    check "A does nothing" 'test -e $HOME/Downloads/keep.txt'
    shot 04-confirm
    press y
    sleep 0.5
    check "trashed" 'test ! -e $HOME/Downloads/keep.txt -a -e $HOME/.local/share/Trash/files/keep.txt'
    check "trash info" 'grep -q "^Path=$HOME/Downloads/keep.txt$" $HOME/.local/share/Trash/info/keep.txt.trashinfo'

    echo "== Rename: the app opens the shell's keyboard, types, closes it"
    press x down down a     # Copy Move Rename, on data.bin
    wait_for '.osk == true'
    sleep 0.6
    shot 05-rename-keyboard
    ctl key 30 >/dev/null   # a
    sleep 0.1
    ctl press y >/dev/null  # the OSK's Enter
    wait_for '.osk == false'
    wait_file $HOME/Downloads/dataa.bin
    sleep 0.5
    shot 06-renamed

    echo "== A game goes to the shell's launcher"
    press l2
    # Places: Home, Games, ...
    press right a           # Games
    press a                 # nes
    press a                 # Moon Miner.nes
    wait_file $HOME/game-started
    check "game" 'grep -q "^nes /build/play/roms/nes/Moon Miner.nes$" $HOME/game-started'
    ctl focus files >/dev/null || true
    sleep 0.5

    echo "== MENU belongs to the shell, and comes back"
    press menu
    wait_for '.overlay == true'
    shot 07-menu
    press menu
    wait_for '.overlay == false'

    echo "== At rest: no wakeups"
    sleep 2
    pid=$(ctl state | jq -r '.apps[] | select(.id == "files") | .pid')
    echo "pid $pid: $(cat /proc/$pid/comm)"
    sw() { grep -h ctxt_switches /proc/$pid/task/*/status | awk '{n += $2} END {print n}'; }
    before=$(sw)
    sleep 5
    after=$(sw)
    echo "context switches in 5 s at rest: $((after - before))" | tee $out/idle.txt
    check "idle" '[ $((after - before)) -le 2 ]'

    echo "== It closes when asked"
    ctl close files >/dev/null
    wait_for '[.apps[] | select(.id == "files")] | length == 0'
    [ -e /proc/$pid ] && fail "still running"

    grep -i "panicked\|mortar-files:" $log && fail "errors in the log"
    if [ $failed = 0 ]; then echo PASS | tee -a $out/result; else tail -30 $log; fi
    kill $shell 2>/dev/null || true
    true
  ''
