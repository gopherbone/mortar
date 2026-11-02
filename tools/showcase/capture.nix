# SPDX-License-Identifier: GPL-2.0-or-later
# Showcase footage: run the real mortar-shell on its headless backend (as
# pkgs/mortar-shell/test.nix does) with real clients, freeze UI time and
# step it 1/30 s at a time while injecting buttons, screenshotting every
# frame. One session, filmed as a few long takes:
#
#   boot      the boot replay, landing on Home (Continue, Needs you, Now
#             playing), the D-pad over its tiles
#   running   MENU: the Running page, scroll, A switches, MENU A flips back
#   cross     ▶ into the cross: Apps, Games, Music, Settings and their mosaics
#   games     Games: a system's page, a game starts, MENU Y: the game
#             panel, save to a slot
#   settings  Settings: Display, Sound > Mixer; the volume pill over a game
#   terminal  zsh, the keyboard sliding up, suggestion chips, R1
#   saver     the cross left alone: its pattern becomes the screensaver
#
# Apps: zsh in foot, mortar-ai (the Claude controller, against a scripted
# stand-in for `claude -p`), ChooChooTracker and PICO-8's Splore (your own
# licensed copy, pkgs/pico8) on Mesa's software GL, and games through
# mortar-play: its built-in test ROM and Libbet and the Magic Floor (zlib,
# Damian Yerrick). There is no PipeWire in the sandbox: `actions.pwDump`
# is a stand-in that reports ChooChoo's stream as playing (Now playing,
# the Mixer). The wall clock is faked to 10:30, the hostname is "mortar".
#
#   $out/clips/<name>/NNNN.png   1024x768 frames at 30 fps
#   $out/clips/<name>/events.tsv frame, kind (press|key|sfx|haptic), value
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
  foot,
  zsh,
  zsh-autosuggestions,
  zsh-syntax-highlighting,
  ncurses,
  util-linux,
  inetutils,
  libfaketime,
  jq,
  makeFontsConf,
  dejavu_fonts,
  jetbrains-mono,
  fetchurl,
  mesa,
  libretro,
}:

let
  mortar-shell = callPackage ../../pkgs/mortar-shell { };
  mortar-ai = callPackage ../../pkgs/mortar-ai { inherit mortar-shell; };
  mortar-play = callPackage ../../pkgs/mortar-play { };
  choochoo = callPackage ../../pkgs/choochootracker { };
  pico8 = callPackage ../../pkgs/pico8 { }; # the user's own licensed copy

  libbet = fetchurl {
    url = "https://github.com/pinobatch/libbet/releases/download/v0.08/libbet.gb";
    sha256 = "1agi7zx7cmdr8diq00z7vm9bi18yb1pfk719g3w7qa6864h421rn";
  };
  core = pkg: "${pkg}/lib/retroarch/cores/${lib.replaceStrings [ "-" ] [ "_" ] pkg.core}_libretro.so";
  playConfig = writeText "mortar-play-showcase.json" (builtins.toJSON {
    romDirs = [ "/build/play/roms" ];
    biosDir = "/build/play/bios";
    saveDir = "/build/play/saves";
    stateDir = "/build/play/states";
    dataDir = "/build/play";
    systems = [
      { id = "gb"; name = "Game Boy"; tag = "GB"; folders = [ "gb" ]; extensions = [ "gb" ]; core = core libretro.gambatte; scaling = "integer"; order = 10;
        options = { gambatte_gb_colorization = "auto"; gambatte_gb_bootloader = "disabled"; }; }
      { id = "nes"; name = "NES"; tag = "NES"; folders = [ "nes" ]; extensions = [ "nes" ]; core = core libretro.fceumm; order = 20;
        options = { fceumm_aspect = "4:3"; }; }
    ];
  });
  play = [ (lib.getExe mortar-play) "--config" "${playConfig}" ];

  # SDL apps on Mesa's software GL (llvmpipe) as Wayland clients
  softGl = {
    __EGL_VENDOR_LIBRARY_FILENAMES = "${mesa}/share/glvnd/egl_vendor.d/50_mesa.json";
    LIBGL_DRIVERS_PATH = "${mesa}/lib/dri";
    LIBGL_ALWAYS_SOFTWARE = "1";
    GALLIUM_DRIVER = "llvmpipe";
  };

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
          say "About 41 °C on both: barely warm, even with ChooChoo playing in the background."
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

  # pw-dump stand-in: the codec sink and one stream, ChooChoo's, playing
  pwDump = writeShellScript "pw-dump" ''
    pid=$(mortar-shell ctl state | jq '[.apps[] | select(.id == "choochoo")][0].pid // 0')
    cat <<EOF
    [{"id": 31, "type": "PipeWire:Interface:Node", "info": {"props": {"media.class": "Audio/Sink", "node.name": "codec"}}},
     {"id": 77, "type": "PipeWire:Interface:Node", "info": {"state": "running",
      "props": {"media.class": "Stream/Output/Audio", "application.name": "choochootracker", "application.process.id": $pid},
      "params": {"Props": [{"volume": 1.0, "mute": false, "channelVolumes": [0.62, 0.62]}]}}}]
    EOF
  '';

  # The Brick's terminal (modules/mortar-shell.nix, modules/mortar-zsh.nix):
  # foot in JetBrains Mono and the Pond palette, zsh with the Mortar prompt,
  # autosuggestions and the keyboard's suggestion chips.
  footIni = writeText "foot.ini" ''
    font=JetBrains Mono:size=13
    pad=16x12 center
    initial-color-theme=light
    [cursor]
    style=block
    blink=no
    [colors-light]
    background=e1ebf7
    foreground=1d1d1d
    cursor=1d1d1d cef058
    regular0=1d1d1d
    regular1=b03a1e
    regular2=4a6b0e
    regular3=805400
    regular4=2a56a3
    regular5=6d35b3
    regular6=166b61
    regular7=566072
    bright0=5f6878
    bright1=c4452a
    bright2=557a12
    bright3=946200
    bright4=2f5fb3
    bright5=7a3fc4
    bright6=1d7a6e
    bright7=6b7385
  '';
  zshrc = writeText "zshrc" ''
    HISTFILE=$HOME/.zsh_history SAVEHIST=100 HISTSIZE=100
    setopt HIST_IGNORE_ALL_DUPS INTERACTIVE_COMMENTS NO_BEEP
    autoload -U compinit && compinit -C -d $HOME/.zcompdump
    ZSH_AUTOSUGGEST_STRATEGY=(history)
    ZSH_AUTOSUGGEST_HIGHLIGHT_STYLE='fg=8,italic'
    source ${zsh-autosuggestions}/share/zsh-autosuggestions/zsh-autosuggestions.zsh
    source ${zsh-syntax-highlighting}/share/zsh-syntax-highlighting/zsh-syntax-highlighting.zsh
    source ${mortar-shell}/share/mortar-shell/mortar-osk.zsh
    cd
    PROMPT='%B%~%b %K{#cef058}%F{#1d1d1d} › %f%k '
    print -P -- "%F{#cef058}▆▆%F{#cba6f7}▆▆%f %Bmortar%b%F{8} · up 2h 14m%f"
    print -P -- "%F{8}START: keyboard · ▲ from its top row: suggestions · R1: the first one%f"
  '';
  zdotdir = runCommand "showcase-zdotdir" { } "mkdir $out; cp ${zshrc} $out/.zshrc";

  config = writeText "mortar-shell-showcase.json" (
    builtins.toJSON {
      apps = [
        { id = "terminal"; name = "Terminal"; description = "foot · START for keyboard"; icon = "term"; art = "term"; category = "apps"; gamepad = "keyboard";
          singleInstance = true; env.ZDOTDIR = "${zdotdir}";
          command = [ "${lib.getExe foot}" "--config" "${footIni}" "${zsh}/bin/zsh" ]; }
        { id = "claude-pad"; name = "Claude"; description = "A allows · B denies"; icon = "claude"; category = "apps"; gamepad = "keyboard";
          command = [ "${lib.getExe mortar-ai}" "run" "--cwd" "/build/home/ai" ];
          env = { MORTAR_AI_CLAUDE = "${fakeClaude}"; MORTAR_AI_RUMBLE = "0"; }; }
        { id = "play"; name = "Games"; description = "Emulators · libretro"; icon = "pad"; category = "games";
          command = play; env.SDL_AUDIO_DRIVER = "dummy"; }
        { id = "pico8"; name = "PICO-8"; description = "Fantasy console, Splore"; icon = "pad"; category = "games"; gamepad = "keyboard";
          command = [ (lib.getExe pico8) "-splore" ]; env = softGl // { SDL_AUDIO_DRIVER = "dummy"; }; }
        { id = "choochoo"; name = "ChooChoo"; description = "Tracker · groovebox"; icon = "pad"; category = "music"; gamepad = "keyboard";
          command = [ (lib.getExe choochoo) ]; env = softGl // { SDL_AUDIO_DRIVER = "dummy"; }; }
      ];
      games = {
        list = play ++ [ "list" "--json" ];
        run = play ++ [ "run" ];
        library = "play";
        hibernateBelowMb = 0;
        env.SDL_AUDIO_DRIVER = "dummy";
      };
      actions = {
        volumeGet = [ "${volume}" ];
        volumeSet = [ "${volume}" "{step}" ];
        volumeMute = [ "true" ];
        pwDump = [ "${pwDump}" ];
        streamVolume = [ "true" ];
        streamMute = [ "true" ];
      };
      home.widgets = [
        { kind = "clock"; }
        { kind = "continue"; }
        { kind = "needs-you"; }
        { kind = "now-playing"; }
        { kind = "storage"; }
        { kind = "command"; title = "Library"; interval = 30; open = "play";
          command = [ "sh" "-c" "n=$(ls /build/play/roms/*/* | wc -l); echo \"{\\\"title\\\":\\\"Library\\\",\\\"value\\\":\\\"$n games\\\",\\\"accent\\\":\\\"lime\\\"}\"" ]; }
      ];
    }
  );

  # The filming script: shell, clips, frames.
  run = writeShellScript "showcase-run" ''
    log=$out/shell.log
    # Every process starts the wall clock at 10:30 (apps too, so they agree).
    off=$(( $(date -d '2026-11-01 10:30:00' +%s) - $(date +%s) ))
    [ $off -ge 0 ] && off=+$off
    MORTAR_SHELL_LOG=info,mortar_shell=debug NO_COLOR=1 \
      FAKETIME_DONT_FAKE_MONOTONIC=1 faketime -f "''${off}s" \
      mortar-shell run --backend headless --config ${config} --socket wayland-0 2> $log &
    shell=$!
    trap 'kill $shell 2>/dev/null || true' EXIT
    for _ in $(seq 100); do [ -S $XDG_RUNTIME_DIR/mortar-shell.sock ] && break; sleep 0.1; done

    ctl() { mortar-shell ctl "$@" >/dev/null; }
    wait_for() {
      for _ in $(seq 300); do
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
    keycode() {
      case $1 in
        q) echo 16;; w) echo 17;; e) echo 18;; r) echo 19;; t) echo 20;; y) echo 21;; u) echo 22;; i) echo 23;; o) echo 24;; p) echo 25;;
        a) echo 30;; s) echo 31;; d) echo 32;; f) echo 33;; g) echo 34;; h) echo 35;; j) echo 36;; k) echo 37;; l) echo 38;;
        z) echo 44;; x) echo 45;; c) echo 46;; v) echo 47;; b) echo 48;; n) echo 49;; m) echo 50;; " ") echo 57;; -) echo 12;; .) echo 52;;
        *) echo "no key for '$1'" >&2; exit 1;;
      esac
    }
    type_text() { # lowercase letters and spaces, $2 frames a character (0: not filmed)
      local t="$1" i
      for ((i = 0; i < ''${#t}; i++)); do
        printf '%s\tkey\t%s\n' $F "''${t:i:1}" >> $clipdir/events.tsv
        ctl key $(keycode "''${t:i:1}"); events
        [ "$2" -gt 0 ] && frames $2
      done
    }

    # -- the session before filming (real time) ---------------------------
    wait_for '.ui.booting == false'
    wait_for '.home == true'
    clip setup
    ctl launch choochoo
    wait_for "$(visible choochoo)"
    sleep 3
    ctl launch pico8
    wait_for "$(visible pico8)"
    sleep 4
    ctl launch claude-pad
    wait_for "$(visible claude-pad)"
    sleep 1
    type_text "how warm is the cpu right now" 0
    ctl press a
    wait_for '.attention | length == 1'
    sleep 1
    ctl game gb /build/play/roms/gb/libbet.gb
    wait_for '[.apps[] | select(.game and .visible and .windows == 1)] | length == 1'
    sleep 3
    ctl home
    wait_for '.home == true'
    sleep 5   # Now playing polls every 4 s while Home shows
    ctl time manual
    skip 2000
    rm -rf $out/clips/setup

    # -- filming ----------------------------------------------------------
    clip boot   # the boot replay lands on the Home dashboard
    ctl boot
    events
    frames 110
    frames 30
    tap right 22
    tap down 22
    tap left 22
    tap up 26

    clip running   # MENU: every app live, stacked; A switches; MENU A flips back
    tap menu 34
    tap down 20
    tap down 20
    tap down 22
    tap up 20
    tap up 24
    tap a 40
    tap menu 26
    tap a 40

    clip cross   # ▶ into the cross: Apps, Games, Music, Settings
    tap menu 24
    tap right 50
    tap down 28
    tap right 54
    tap right 54
    tap right 54
    tap down 30

    clip games   # Games: a system's page, a game, its panel
    tap left 22
    tap left 30
    tap down 14
    tap down 20
    tap a 40
    press a
    wait_for '[.apps[] | select(.game and (.name | test("NES")) and .visible and .windows == 1)] | length == 1'
    frames 60
    tap menu 26
    tap up 20
    tap y 36
    tap a 60
    tap b 16
    tap b 26

    # off camera: set where the cross's categories stand (each remembers
    # its row): Settings on Display, Apps on Terminal
    offcam() { clipdir=$TMPDIR/offcam F=0; mkdir -p $clipdir; }
    offcam
    press menu; press r1
    for _ in $(seq 12); do press up; done
    press down; press down
    press l1; press right
    for _ in $(seq 6); do press up; done
    press menu
    skip 1500

    clip settings   # Settings: Display, Sound > Mixer; the volume pill
    tap menu 16
    tap r1 34
    tap a 50
    tap b 18
    tap down 16
    tap a 30
    tap down 14
    tap a 56
    tap b 16
    tap b 18
    tap menu 26
    tap volup 10
    tap volup 10
    tap volup 50

    clip terminal   # zsh: the keyboard, suggestion chips, R1
    tap menu 20
    tap right 30
    press a
    wait_for "$(visible terminal)"
    frames 40
    type_text "un" 3
    frames 6
    tap start 34
    tap r1 24
    tap y 40
    tap start 24

    clip saver   # the cross left alone grows into the screensaver
    tap menu 18
    tap right 20
    tap right 30
    skip 59000
    frames 150
    tap a 45

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
    nativeBuildInputs = [ mortar-shell mortar-play jq util-linux inetutils libfaketime ];
    FONTCONFIG_FILE = makeFontsConf { fontDirectories = [ dejavu_fonts jetbrains-mono mortar-shell.fonts ]; };
    passthru = { inherit mortar-shell mortar-ai fakeClaude config run; };
  }
  ''
    export HOME=/build/home LANG=C.UTF-8
    export TERMINFO_DIRS=${foot.terminfo}/share/terminfo:${ncurses}/share/terminfo
    export XDG_RUNTIME_DIR=$TMPDIR/run
    mkdir -p -m 700 $XDG_RUNTIME_DIR $HOME $HOME/ai
    mkdir -p $out/clips $out/sfx $out/fonts
    cp ${mortar-shell.fonts}/share/fonts/mortar-ui/* $out/fonts/

    # no first-time tips; a little zsh history for the chips
    mkdir -p $HOME/.local/state/mortar-shell
    echo '{"tipsSeen":["menu","keyboard","game","controls"]}' > $HOME/.local/state/mortar-shell/prefs.json
    printf '%s\n' 'ls' 'uname -m' 'nix --version' > $HOME/.zsh_history

    # games: the built-in test ROM and an open homebrew
    mkdir -p /build/play/roms/gb /build/play/roms/nes /build/play/bios
    cp ${libbet} /build/play/roms/gb/libbet.gb
    chmod u+w /build/play/roms/gb/libbet.gb
    mortar-play make-test-rom /build/play/roms/nes/mortar-test.nes

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
