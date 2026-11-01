# SPDX-License-Identifier: GPL-2.0-or-later
# Showcase footage: run the real mortar-shell on its headless backend (as
# pkgs/mortar-shell/test.nix does) with real clients (foot, mortar-ai
# against a scripted stand-in for `claude -p`), freeze UI time and step it
# 1/30 s at a time while injecting buttons, screenshotting every frame.
#
#   $out/clips/<name>/NNNN.png   1024x768 frames at 30 fps
#   $out/clips/<name>/events.tsv frame, kind (press|sfx|haptic|note), value
#   $out/sfx/<name>.wav          the shell's own UI sounds (mortar-shell sfx)
#   $out/fonts/                  the UI fonts (for the title cards)
#   $out/shell.log
#
# Sounds and haptics come from the shell's debug log ("sfx Move",
# "haptic Tick"), read after every control command, so they land on the
# frame where the shell played them. tools/showcase/compose.py turns this
# into the video.
{
  lib,
  runCommand,
  writeText,
  writeShellScript,
  callPackage,
  weston,
  foot,
  bashInteractive,
  ncurses,
  util-linux,
  inetutils,
  jq,
  makeFontsConf,
  dejavu_fonts,
}:

let
  mortar-shell = callPackage ../../pkgs/mortar-shell { };
  mortar-ai = callPackage ../../pkgs/mortar-ai { inherit mortar-shell; };

  # Stand-in for `claude -p` (stream-json), after pkgs/mortar-ai/test.nix,
  # with a scripted, streamed reply.
  fakeClaude = writeShellScript "claude" ''
    emit() { printf '%s\n' "$1"; }
    say() { # stream TEXT in small chunks, like the API does
      local t="$1" i=0
      while [ $i -lt ''${#t} ]; do
        emit '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"'"''${t:$i:6}"'"}}}'
        i=$((i + 6))
        sleep 0.06
      done
    }
    emit '{"type":"system","subtype":"init","model":"claude-sonnet-5","cwd":"'"$PWD"'","permissionMode":"default"}'
    while IFS= read -r line; do
      case "$line" in
        *'"subtype":"initialize"'*)
          emit '{"type":"control_response","response":{"subtype":"success","request_id":"x","response":{}}}' ;;
        *'"type":"user"'*)
          emit '{"type":"system","subtype":"status","status":"requesting"}'
          sleep 0.8
          emit '{"type":"stream_event","event":{"type":"message_start","message":{"id":"m1"}}}'
          say "I'll read the SoC's thermal sensors."
          sleep 0.3
          emit '{"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cat /sys/class/thermal/thermal_zone*/temp","description":"Read the CPU and GPU temperatures"}}]}}'
          emit '{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"t1","input":{"command":"cat /sys/class/thermal/thermal_zone*/temp","description":"Read the CPU and GPU temperatures"},"description":"Read the CPU and GPU temperatures","permission_suggestions":[{"type":"addRules","behavior":"allow","destination":"localSettings","rules":[{"toolName":"Bash","ruleContent":"cat *"}]}]}}'
          ;;
        *'"behavior":"allow"'*)
          sleep 0.3
          emit '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"41250\n40875","is_error":false}]}}'
          sleep 0.4
          emit '{"type":"stream_event","event":{"type":"message_start","message":{"id":"m2"}}}'
          say "About 41 °C on both the CPU and the GPU: barely warm, even with the M8 playing in the background."
          emit '{"type":"result","subtype":"success","duration_ms":3100,"is_error":false}'
          ;;
      esac
    done
  '';

  # amixer stand-in: a level in a file, "{step}" is e.g. 10%+
  volume = writeShellScript "volume" ''
    f=$HOME/.volume
    v=$(cat $f 2>/dev/null || echo 60)
    case "$1" in
      *%+) v=$((v + ''${1%\%+})) ;;
      *%-) v=$((v - ''${1%\%-})) ;;
    esac
    [ $v -gt 100 ] && v=100
    [ $v -lt 0 ] && v=0
    echo $v > $f
    echo "[$v%]"
  '';

  footIni = writeText "foot.ini" ''
    font=DejaVu Sans Mono:size=15
    pad=16x16
    [colors-dark]
    background=1d1d1d
    foreground=e1ebf7
    regular2=cef058
    regular3=ffd330
    regular5=cba6f7
  '';
  bashrc = writeText "bashrc" ''
    PS1='\[\e[38;5;149m\]\h\[\e[0m\] \w $ '
  '';
  standIn = [ "${weston}/bin/weston-simple-shm" ];
  # The launcher as on the Brick (modules/mortar-*.nix), minus the
  # second terminal and Remote Control. Apps that need the GPU or hardware
  # are filmed on the device; here they only have tiles.
  config = writeText "mortar-shell-showcase.json" (
    builtins.toJSON {
      apps = [
        { id = "terminal"; name = "Terminal"; description = "foot · START for keyboard"; icon = "term"; art = "term"; category = "apps"; gamepad = "keyboard";
          command = [ "${lib.getExe foot}" "--config" "${footIni}" "${bashInteractive}/bin/bash" "--rcfile" "${bashrc}" ]; }
        { id = "claude"; name = "Claude Code"; description = "Terminal · keeps running"; icon = "claude"; gamepad = "keyboard"; command = standIn; }
        { id = "claude-pad"; name = "Claude"; description = "A allows · B denies"; icon = "claude"; gamepad = "keyboard";
          command = [ "${lib.getExe mortar-ai}" "run" "--cwd" "/build/home/ai" ];
          env = { MORTAR_AI_CLAUDE = "${fakeClaude}"; MORTAR_AI_RUMBLE = "0"; }; }
        { id = "play"; name = "Games"; description = "Emulators · libretro"; icon = "pad"; category = "games"; command = standIn; }
        { id = "games"; name = "PuzzleScript"; description = "Puzzle games, controller"; icon = "pad"; command = standIn; }
        { id = "m8"; name = "M8"; description = "Tracker on the top USB-C port"; icon = "usb"; command = standIn; }
        { id = "choochoo"; name = "ChooChoo"; description = "Tracker · groovebox"; icon = "pad"; command = standIn; }
        { id = "pico8"; name = "PICO-8"; description = "Fantasy console, Splore"; icon = "pad"; command = standIn; }
      ];
      actions = {
        volumeGet = [ "${volume}" ];
        volumeSet = [ "${volume}" "{step}" ];
      };
    }
  );

  # The filming script: shell, clips, frames.
  run = writeShellScript "showcase-run" ''
    log=$out/shell.log
    MORTAR_SHELL_LOG=info,mortar_shell=debug NO_COLOR=1 \
      mortar-shell run --backend headless --config ${config} --socket wayland-0 2> $log &
    shell=$!
    trap 'kill $shell 2>/dev/null || true' EXIT
    for _ in $(seq 100); do [ -S $XDG_RUNTIME_DIR/mortar-shell.sock ] && break; sleep 0.1; done

    ctl() { mortar-shell ctl "$@" >/dev/null; }
    wait_for() {
      for _ in $(seq 200); do
        mortar-shell ctl state | jq -e "$1" >/dev/null && return 0
        sleep 0.1
      done
      echo "TIMEOUT waiting for: $1" | tee -a $out/warnings
    }
    visible() { echo "[.apps[] | select(.id == \"$1\" and .visible)] | length == 1"; }

    # -- frame capture ---------------------------------------------------
    clipdir=/dev/null F=0 logpos=0
    events() { # sounds and haptics the shell logged since the last call
      local size
      size=$(stat -c %s $log)
      if [ $size -gt $logpos ]; then
        tail -c +$((logpos + 1)) $log | head -c $((size - logpos)) \
          | grep -oE '(sfx|haptic) [A-Za-z]+(\([0-9]+\))?' \
          | while read -r k v; do printf '%s\t%s\t%s\n' $F $k "$v" >> $clipdir/events.tsv; done || true
        logpos=$size
      fi
    }
    clip() { clipdir=$out/clips/$1 F=0; mkdir -p $clipdir; : > $clipdir/events.tsv; events; echo "== clip $1"; }
    note() { printf '%s\tnote\t%s\n' $F "$*" >> $clipdir/events.tsv; }
    # UI time steps 1/30 s a frame (33, 33, 34 ms); real time is paced to
    # match, so the clients (which run on the real clock) keep up.
    next=0
    frame() {
      ctl time +$((F % 3 == 2 ? 34 : 33))
      events
      ctl screenshot $clipdir/$(printf %04d $F).png
      F=$((F + 1))
      local now=''${EPOCHREALTIME/./}
      if [ $next -gt $now ]; then
        sleep "$(printf '0.%06d' $((next - now)))"
      else
        next=$now
      fi
      next=$((next + 33333))
    }
    frames() { for _ in $(seq $1); do frame; done; }
    press() { printf '%s\tpress\t%s\n' $F $1 >> $clipdir/events.tsv; ctl press $1; events; }
    tap() { press $1; frames $2; } # press, then N frames
    skip() { ctl time +$1; events; } # move UI time on, uncaptured
    key() { ctl key $1; events; }
    type_text() { # lowercase letters and spaces, $2 frames a character
      local t="$1" c code i
      for ((i = 0; i < ''${#t}; i++)); do
        c=''${t:i:1}
        case $c in
          q) code=16;; w) code=17;; e) code=18;; r) code=19;; t) code=20;; y) code=21;; u) code=22;; i) code=23;; o) code=24;; p) code=25;;
          a) code=30;; s) code=31;; d) code=32;; f) code=33;; g) code=34;; h) code=35;; j) code=36;; k) code=37;; l) code=38;;
          z) code=44;; x) code=45;; c) code=46;; v) code=47;; b) code=48;; n) code=49;; m) code=50;; " ") code=57;; -) code=12;; .) code=52;;
          *) echo "type_text: no key for '$c'"; exit 1;;
        esac
        printf '%s\tkey\t%s\n' $F "$c" >> $clipdir/events.tsv
        key $code
        frames $2
      done
    }

    wait_for '.ui.booting == false'
    wait_for '.home == true'
    ctl time manual
    skip 1500

    clip boot
    ctl boot
    events
    frames 100

    clip home
    frames 12
    tap right 9
    tap right 9
    tap right 9
    tap right 12
    tap left 9
    tap left 16

    clip launch-ai   # A on the Claude tile (the third)
    press a
    wait_for "$(visible claude-pad)"
    frames 45

    clip ai-ask
    frames 6
    type_text "how warm is the cpu right now" 2
    frames 8
    tap a 4
    # the reply streams in real time; keep filming until the card is up
    for _ in $(seq 70); do frame; done
    note "permission card"
    tap a 4
    for _ in $(seq 90); do frame; done

    clip menu   # the cross menu over Claude: every category
    tap menu 24
    tap right 13
    tap right 13
    tap right 13
    tap right 13
    tap right 13
    tap left 13
    tap b 18

    clip launch-term   # MENU > Apps > Terminal: it grows out of its row
    tap menu 14
    tap right 14
    press a
    wait_for "$(visible terminal)"
    frames 36
    type_text "uname -m" 2
    tap a 20

    clip osk   # START: the on-screen keyboard
    tap start 18
    tap right 4
    tap right 4
    tap right 4
    tap down 4
    tap down 6
    tap left 4
    tap a 8
    tap start 16

    clip switch   # MENU opens on the previous app, with a live preview; A flips
    tap menu 26
    tap a 30
    tap menu 22
    tap a 30

    clip quick   # MENU > Quick: volume in adjust mode
    tap menu 12
    tap r1 8
    tap left 14
    tap down 10
    tap a 10
    tap right 7
    tap right 7
    tap right 7
    tap left 7
    tap a 12
    tap b 16

    clip shot   # VOL+ and VOL-: flash, shutter, toast
    frames 6
    ctl shot; events
    frames 75

    clip achievement
    ctl toast '{"title":"Treasure hunter","sub":"Homebrew · Game Boy","kind":"achievement"}'
    events
    frames 90
    skip 4000

    clip close-app   # MENU, X closes the previous app (Claude)
    tap menu 22
    tap x 30
    tap b 14

    clip close-home   # MENU, Up to Home, A: the app shrinks into its tile
    tap menu 18
    tap up 10
    tap a 40

    clip night   # Settings > Palette: Night, then home
    tap menu 8
    tap r1 4
    tap left 4
    for _ in 1 2 3 4 5 6 7 8; do press down; done
    frames 4
    tap a 22
    tap down 4
    tap down 4
    tap down 4
    tap down 10
    tap right 22
    tap menu 36
    # back to day, off camera
    press menu; press r1; press left
    for _ in 1 2 3 4 5 6 7 8; do press down; done
    press a; skip 800
    for _ in 1 2 3 4; do press down; done
    press left; press menu; skip 1500

    clip sleep   # power tap: fade out, dark; tap: wake
    frames 8
    ctl power; events
    frames 50
    ctl power; events
    frames 40

    clip power-menu   # hold power
    ctl power hold
    events
    for _ in $(seq 26); do frame; done
    ctl power-up
    events
    frames 24
    tap b 12

    ctl time real
    mortar-shell ctl state | jq . > $out/state.json
    ctl quit || true
    wait $shell || true
    trap - EXIT
    for d in $out/clips/*/; do echo "$(basename $d): $(ls $d | grep -c png) frames"; done | tee $out/clips.txt
  '';
in
runCommand "mortar-showcase-capture"
  {
    nativeBuildInputs = [ mortar-shell jq util-linux inetutils ];
    FONTCONFIG_FILE = makeFontsConf { fontDirectories = [ dejavu_fonts ]; };
    passthru = { inherit mortar-shell mortar-ai fakeClaude config run; };
  }
  ''
    export HOME=/build/home LANG=C.UTF-8
    export TERMINFO_DIRS=${foot.terminfo}/share/terminfo:${ncurses}/share/terminfo
    export XDG_RUNTIME_DIR=$TMPDIR/run
    mkdir -p -m 700 $XDG_RUNTIME_DIR $HOME $HOME/ai
    mkdir -p $out/clips $out/sfx $out/fonts
    cp ${mortar-shell.fonts}/share/fonts/mortar-ui/* $out/fonts/

    echo "== the UI sounds"
    for s in tick move tab open close drawer drawerup chime shutter unlock deny; do
      mortar-shell sfx $s --wav $out/sfx/$s.wav
    done
    for n in $(seq 0 100); do mortar-shell sfx vol$n --wav $out/sfx/vol$n.wav; done

    # The status bar shows the hostname: the Brick's, in a UTS namespace
    if unshare --user --map-root-user --uts true 2>/dev/null; then
      unshare --user --map-root-user --uts sh -c 'hostname mortar || echo "hostname: not set"; exec "$0"' ${run}
    else
      echo "no UTS namespace: the hostname stays $(cat /proc/sys/kernel/hostname)"
      ${run}
    fi
  ''
