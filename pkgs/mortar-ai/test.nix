# SPDX-License-Identifier: GPL-2.0-or-later
# Headless integration test: mortar-shell on its headless backend in the
# build sandbox, with a fake `claude` (stream-json, `auth login` and
# interactive modes). Checks, with a screenshot after each step ($out/*.png):
#   - mortar-ai (controller UI): type a prompt, get a permission card, A
#     allows, the allow response reaches claude, the turn finishes
#   - Claude Code in foot + tmux: the first-run sign-in guide, the QR screen
#     (mortar-ai pair) found from the pane, the code posted to the pair page
#     with curl is typed into `claude auth login`, credentials are written,
#     Claude Code starts; closing the terminal keeps the tmux session
{
  lib,
  runCommand,
  writeText,
  writeShellScript,
  callPackage,
  mortar-ai,
  mortar-shell,
  foot,
  jq,
  curl,
  tmux,
  ncurses,
  makeFontsConf,
  dejavu_fonts,
}:

let
  fakeClaude = writeShellScript "claude" ''
    # A stand-in for Claude Code with the same I/O shapes (see DESIGN.md).
    creds="''${CLAUDE_CONFIG_DIR:-$HOME/.claude}/.credentials.json"
    if [ "''${1:-}" = auth ] && [ "''${2:-}" = login ]; then
      echo "Opening browser to sign in…"
      echo "If the browser didn't open, visit: https://claude.com/cai/oauth/authorize?code=true&client_id=00000000-0000-4000-8000-000000000000&response_type=code&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&scope=user%3Ainference&code_challenge=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA&code_challenge_method=S256&state=BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB"
      printf 'Paste code here if prompted > '
      read -r code
      if [ -n "$code" ]; then
        mkdir -p "$(dirname "$creds")"
        printf '{"fake":"%s"}\n' "$code" > "$creds"
        chmod 600 "$creds"
        echo "Login successful"
        exit 0
      fi
      exit 1
    fi
    if [ "''${1:-}" = -p ]; then
      emit() { printf '%s\n' "$1"; }
      emit '{"type":"system","subtype":"init","model":"fake-claude","cwd":"'"$PWD"'","permissionMode":"default"}'
      while IFS= read -r line; do
        printf '%s\n' "$line" >> "''${FAKE_CLAUDE_LOG:-/dev/null}"
        case "$line" in
          *'"subtype":"initialize"'*)
            emit '{"type":"control_response","response":{"subtype":"success","request_id":"x","response":{}}}' ;;
          *'"type":"user"'*)
            emit '{"type":"system","subtype":"status","status":"requesting"}'
            emit '{"type":"stream_event","event":{"type":"message_start","message":{"id":"m1"}}}'
            emit '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"I will check the free memory."}}}'
            emit '{"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"free -m","description":"Show free memory"}}]}}'
            emit '{"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"t1","input":{"command":"free -m","description":"Show free memory"},"description":"Show free memory","permission_suggestions":[{"type":"addRules","behavior":"allow","destination":"localSettings","rules":[{"toolName":"Bash","ruleContent":"free *"}]}]}}'
            ;;
          *'"behavior":"allow"'*)
            emit '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"       total  used  free\nMem:     970   412   201\nSwap:    485    12   473","is_error":false}]}}'
            emit '{"type":"stream_event","event":{"type":"message_start","message":{"id":"m2"}}}'
            emit '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"About 200 MB are free, and swap is barely used."}}}'
            emit '{"type":"result","subtype":"success","duration_ms":3100,"is_error":false}'
            ;;
          *'"behavior":"deny"'*)
            emit '{"type":"result","subtype":"success","duration_ms":900,"is_error":false}' ;;
        esac
      done
      exit 0
    fi
    signed=no
    [ -s "$creds" ] && signed=yes
    echo "fake Claude Code in $PWD (signed in: $signed)"
    exec sleep 1000
  '';
  fakeClaudePkg = runCommand "fake-claude" { } "mkdir -p $out/bin; ln -s ${fakeClaude} $out/bin/claude";

  scripts = callPackage ./scripts.nix {
    claude = fakeClaudePkg;
    inherit mortar-ai;
    workDir = "/build/home/ai"; # $HOME below
  };

  footIni = writeText "foot.ini" ''
    font=DejaVu Sans Mono:size=12
    pad=12x12
    [colors-dark]
    background=1d1d1d
    foreground=e1ebf7
  '';
  config = writeText "mortar-ai-test.json" (
    builtins.toJSON {
      # the boot animation takes the first presses
      ui.bootAnimation = false;
      apps = [
        {
          id = "ai";
          name = "Claude";
          description = "controller UI";
          icon = "pad";
          gamepad = "keyboard";
          command = [ "${lib.getExe mortar-ai}" "run" "--cwd" "/build/home/ai" ];
          env = { MORTAR_AI_CLAUDE = "${fakeClaude}"; MORTAR_AI_RUMBLE = "0"; };
        }
        {
          id = "claude";
          name = "Claude Code";
          description = "foot + tmux";
          icon = "term";
          gamepad = "keyboard";
          command = [ "${lib.getExe foot}" "--config" "${footIni}" "${scripts}/bin/mortar-claude" ];
        }
        {
          id = "remote";
          name = "Remote Control";
          description = "menu (no systemd here: shows stopped)";
          icon = "link";
          gamepad = "keyboard";
          command = [ "${lib.getExe foot}" "--config" "${footIni}" "${scripts}/bin/mortar-claude-remote" ];
        }
      ];
      actions = {
        volumeGet = [ "echo" "[50%]" ];
        volumeSet = [ "true" ];
      };
    }
  );
in
runCommand "mortar-ai-headless-test"
  {
    nativeBuildInputs = [ mortar-shell jq curl tmux ];
    FONTCONFIG_FILE = makeFontsConf { fontDirectories = [ dejavu_fonts ]; };
    passthru = { inherit scripts fakeClaude; };
  }
  ''
    export HOME=/build/home # = workDir's parent above (the Linux sandbox's TMPDIR)
    export TERMINFO_DIRS=${foot.terminfo}/share/terminfo:${ncurses}/share/terminfo
    export XDG_RUNTIME_DIR=$TMPDIR/run TMUX_TMPDIR=$TMPDIR
    mkdir -p -m 700 $XDG_RUNTIME_DIR $HOME
    mkdir -p $out
    export FAKE_CLAUDE_LOG=$out/claude-stdin.jsonl
    export MORTAR_AI_PAIR_ADDR=127.0.0.1 MORTAR_AI_PAIR_URL_FILE=$TMPDIR/pair-url
    log=$out/shell.log
    fail() { echo "FAIL: $*" | tee -a $out/result; }

    mortar-shell run --backend headless --config ${config} --socket wayland-test 2> $log &
    shell=$!
    trap 'kill $shell 2>/dev/null || true; tmux -L mortar-ai kill-server 2>/dev/null || true' EXIT
    for _ in $(seq 100); do [ -S $XDG_RUNTIME_DIR/mortar-shell.sock ] && break; sleep 0.1; done

    ctl() { mortar-shell ctl "$@"; }
    shot() { ctl screenshot "$out/$1.png" >/dev/null; echo "screenshot $1"; }
    wait_for() {
      for _ in $(seq 150); do
        ctl state | jq -e "$1" >/dev/null && return 0
        sleep 0.1
      done
      fail "timeout: $1"; ctl state | jq . || true
    }
    wait_file() {
      for _ in $(seq 150); do [ -s "$1" ] && return 0; sleep 0.1; done
      fail "no file $1"
    }
    visible() { echo "[.apps[] | select(.id == \"$1\" and .visible)] | length == 1"; }

    echo "== mortar-ai: start, type 'hi', A sends"
    ctl launch ai
    wait_for "$(visible ai)"
    sleep 0.5
    shot 01-ai-empty
    for k in 35 23; do ctl key $k >/dev/null; done
    ctl press a
    sleep 1
    shot 02-ai-permission
    grep -q '"content":"hi"' $FAKE_CLAUDE_LOG || fail "prompt not sent"

    echo "== A allows"
    ctl press a
    sleep 1
    shot 03-ai-allowed
    grep -q '"request_id":"r1".*"behavior":"allow"\|"behavior":"allow".*"request_id":"r1"' $FAKE_CLAUDE_LOG || fail "allow not sent"
    ctl state | jq . > $out/state-ai.json
    ctl close ai || true
    wait_for '[.apps[] | select(.id == "ai")] | length == 0'

    echo "== Claude Code: first run shows the sign-in guide"
    ctl launch claude
    wait_for "$(visible claude)"
    sleep 2
    shot 04-claude-signin-guide
    tmux -L mortar-ai has-session -t claude || fail "no tmux session"

    echo "== A: claude auth login; the QR screen opens from the pane"
    ctl press a
    wait_file $TMPDIR/pair-url
    sleep 1
    shot 05-pair-qr
    url=$(cat $TMPDIR/pair-url)
    curl -sf "$url" > $out/pair-page.html || fail "pair page GET"
    grep -q "Sign in to Claude" $out/pair-page.html || fail "pair page content"
    curl -s -o /dev/null -w '%{http_code}\n' "''${url%/*}/wrongtoken" > $out/pair-wrong-token.txt
    grep -qx 404 $out/pair-wrong-token.txt || fail "wrong token not refused"
    curl -sf --data-urlencode 'code=fakeCode_123-abc#state_456' "$url" > $out/pair-posted.html || fail "pair POST"
    wait_file $HOME/.claude/.credentials.json
    grep -q 'fakeCode_123-abc#state_456' $HOME/.claude/.credentials.json || fail "code not typed into login"
    sleep 2
    shot 06-claude-signed-in
    tmux -L mortar-ai capture-pane -p -t claude > $out/pane.txt || true
    grep -q "signed in: yes" $out/pane.txt || fail "claude did not start signed in"

    echo "== memory of what is running now (RSS/PSS kB; the claude here is a fake)"
    ctl launch ai
    wait_for "$(visible ai)"
    sleep 1
    for d in /proc/[0-9]*; do
      c=$(cat $d/comm 2>/dev/null) || continue
      case "$c" in mortar-*|foot|tmux*|sleep) ;; *) continue ;; esac
      awk -v c="$c" '/^Rss:/{r=$2} /^Pss:/{p=$2} END{print c, r, p}' $d/smaps_rollup 2>/dev/null || true
    done | sort > $out/mem.txt
    cat $out/mem.txt
    ctl close ai || true
    wait_for '[.apps[] | select(.id == "ai")] | length == 0'

    echo "== closing the terminal keeps the session"
    ctl close claude || true
    wait_for '[.apps[] | select(.id == "claude")] | length == 0'
    tmux -L mortar-ai has-session -t claude || fail "tmux session gone after closing foot"
    ctl launch claude
    wait_for "$(visible claude)"
    sleep 1
    shot 07-claude-reattached

    echo "== Remote Control menu"
    ctl launch remote
    wait_for "$(visible remote)"
    sleep 1.5
    shot 08-remote-menu

    ctl quit || true
    wait $shell || true
    [ -e $out/result ] || echo PASS > $out/result
    cat $out/result
  ''
