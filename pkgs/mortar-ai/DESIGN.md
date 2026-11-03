# Mortar AI design

Claude Code on the TrimUI Brick, inside the Mortar shell: a 1024x768
handheld with a gamepad, Wi-Fi, ~970 MB of RAM plus zram, and maybe a
Bluetooth or USB keyboard. This is the "AI" session from the plan's
"Sessions and login" section: hosted models through the Claude Code
harness, nothing on-device.

## What there is

`modules/packs/ai.nix` (`mortar.ai.enable`, off by default) adds three
launcher entries to the shell and the plumbing behind them:

| Entry | What it runs | For |
|---|---|---|
| Claude Code | foot → `tmux -L mortar-ai new-session -A -s claude` → `mortar-claude-session` → `claude` | the full Claude Code TUI; keyboard or pad-as-keys + on-screen keyboard |
| Claude | `mortar-ai run` (this package): a Wayland app over `claude -p` stream-json | controller-first: permission cards with A allow / B deny / X always |
| Remote Control | foot → `mortar-claude-remote` menu → systemd `mortar-claude-remote` → tmux → `claude remote-control` | drive the Brick's Claude Code from the Claude app or claude.ai/code |

```
pkgs/mortar-ai/
  src/main.rs      CLI: run | ask | pair | preview
  src/proto.rs     the claude child: stream-json in/out, control requests
  src/model.rs     transcript, permission queue, keys -> actions (unit-tested)
  src/view.rs      drawing with the mortar-ui toolkit
  src/chat.rs      model + view + claude as a Screen
  src/pair.rs      sign-in helper: QR code + one-time phone page
  src/wl.rs        Wayland window (wl_shm, xdg-shell, wl_keyboard), Linux only
  src/screen.rs    the Screen trait wl.rs drives
  src/cli.rs       `ask`: the same session in a plain terminal
  src/preview.rs   demo states to PNG (any OS)
  scripts/*.sh     the terminal side (built by scripts.nix with the module's paths)
  test.nix         headless test in mortar-shell with a fake claude
modules/packs/ai.nix
```

## Persistence: tmux

Claude Code must survive the user switching to the M8 and back, and closing
the terminal by accident. The "Claude Code" entry is therefore only a
viewer: foot attaches to a tmux session (`new-session -A`), and the
session's program is `mortar-claude-session`, which loops sign-in →
`claude` → a small menu (A again, X `--continue`, Y shell). Closing foot
detaches; relaunching reattaches. Over SSH the same session is
`tmux -L mortar-ai attach -t claude`, which is handy for pasting.

tmux config (`scripts/tmux.conf`): `tmux-256color`, extended keys for foot
(Shift+Enter), 10 ms escape time (Esc reaches claude promptly), no mouse,
a lime status line naming the buttons, and `WAYLAND_DISPLAY` /
`XDG_RUNTIME_DIR` in `update-environment` so helpers started from the pane
find the current compositor after a shell restart.

## Sign-in

Remote Control needs a claude.ai subscription login (a setup-token can't
establish it, per the docs), so the flow is `claude auth login --claudeai`.
Observed with 2.1.316 on a pty with `$BROWSER` set:

```
Opening browser to sign in…
If the browser didn't open, visit: https://claude.com/cai/oauth/authorize?code=true&client_id=…&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&…&state=…
Paste code here if prompted >
```

The link is ~450 characters and the code shown after signing in is long;
neither is reasonable on an on-screen keyboard. So:

1. `mortar-claude-session` explains what will happen (screenshot
   `04-claude-signin-guide`), then runs `BROWSER=true claude auth login`
   (a no-op browser: nothing to open, and it still prints the link).
2. A watcher polls the pane (`tmux capture-pane -J`) for `visit: https://…`
   and starts `mortar-ai pair --url LINK`, a fullscreen window
   (`05-pair-qr`): a QR code of `http://<brick-ip>:<port>/<128-bit token>`,
   a page served by the Brick only while that window is open.
3. The phone opens that page: a "Sign in to Claude" button (the OAuth link)
   and a field for the code. After signing in on the phone, paste the code
   and Send. `mortar-ai pair` prints the code and exits; the watcher types
   it into the waiting prompt with `tmux send-keys -l` + Enter.
4. X on the QR screen switches to a QR of the OAuth link itself (phone not
   on the Brick's network; type the code with START). The guide also names
   the SSH route (`claude auth login` over SSH signs in the same user).

The page accepts exactly one code made of `[A-Za-z0-9#._~-]`, 8-512 chars,
on the token path only (404 elsewhere), so it cannot type anything else
into the pane. It is plain HTTP on the LAN for the few minutes it is open;
the code is single-use and bound to the PKCE verifier inside the waiting
`claude`, so a sniffed code is worthless to anyone else.

Credentials: Claude Code writes `~/.claude/.credentials.json` (0600) in the
user's home on the ext4 root partition. The module creates `~/.claude` 0700
and the work directory (`~/ai`; not `$HOME`, where folder trust is never
saved) with tmpfiles, asserts the home is not under `/boot` (the FAT
partition), and never puts a credential in the store or the repo. An
`ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN` in the environment also
counts as signed in for the scripts.

## Remote Control

`claude remote-control` must keep running and may ask once on a terminal
(folder trust, `[y/N]`), so the unit runs it in a tmux server of its own
(`-L mortar-remote`, `Type=forking`, `User=` the AI user, `--name` the host
name, `--spawn same-dir` given explicitly so it never asks for a spawn
mode). Stopping the unit kills the tmux server and claude with it. The
launcher's menu (`08-remote-menu`) starts/stops the unit and attaches:
claude prints the session URL, Space (the pad's Y) toggles its QR code for
the Claude app, and R1 (Page Down, bound in that server only) detaches back
to the menu. If the user isn't signed in, the menu says to open Claude Code
first. For a non-root user the module adds a polkit rule for just this
unit. `mortar.ai.remoteControl.autoStart` starts it at boot.

## The controller app (`mortar-ai run`)

### Why `claude -p` stream-json and not the Agent SDK

The Agent SDKs (TypeScript, Python) spawn the same `claude` binary and talk
to it over stdin/stdout with stream-json plus a small control protocol. On
the Brick an SDK would add a Node or Python runtime (tens of MB of RSS) to
reach the same process. `mortar-ai` speaks that protocol directly from
Rust:

```
claude -p --input-format stream-json --output-format stream-json --verbose \
       --include-partial-messages --permission-prompt-tool stdio [--model M]
```

Checked against Claude Code 2.1.316 (off-device, a haiku turn that runs
`echo hi > note.txt`):

```
-> {"type":"control_request","request_id":"init1","request":{"subtype":"initialize"}}
-> {"type":"user","message":{"role":"user","content":"…"}}
<- {"type":"control_response","response":{"subtype":"success","request_id":"init1",…}}
<- {"type":"system","subtype":"init","model":…,"cwd":…,"permissionMode":"default",…}
<- {"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta",…}}}
<- {"type":"assistant","message":{"id":…,"content":[{"type":"tool_use","id":"toolu_…","name":"Bash","input":{…}}]}}
<- {"type":"control_request","request_id":"<uuid>","request":{"subtype":"can_use_tool","tool_name":"Bash",
     "input":{…},"description":"Write \"hi\" into note.txt","tool_use_id":"toolu_…",
     "permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Bash","ruleContent":"echo hi *"}],
       "behavior":"allow","destination":"localSettings"}, …]}}
-> {"type":"control_response","response":{"subtype":"success","request_id":"<uuid>",
     "response":{"behavior":"allow","updatedInput":{…}}}}        (A)
   … "response":{"behavior":"deny","message":"…"}                 (B)
   … "behavior":"allow","updatedInput":…,"updatedPermissions":[suggestions]   (X: wrote
     .claude/settings.local.json {"permissions":{"allow":["Bash(echo hi *)"]}})
<- {"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":…,"is_error":false,…}]}}
<- {"type":"result","subtype":"success","duration_ms":…,"is_error":false,…}
```

Both answers were verified end to end (`mortar-ai ask` against the real
CLI: the file was written after `y`; denied with `n`). Other control
requests (hook callbacks, SDK MCP) get an error response. Interrupt is
`{"type":"control_request","request":{"subtype":"interrupt"}}`. Not logged
in shows up as an error `result`, which the app turns into "open Claude
Code to sign in".

### Screen and controls

The shell runs the app in `keyboard` gamepad mode, so the pad arrives as
keys on wl_keyboard and START opens the shell's on-screen keyboard (the
window is resized above it; the card drops its title before its command
when space is short, see the 1024x430 preview). No uinput or evdev in the
app; any real keyboard works the same.

| Button (key) | Composing | Permission card |
|---|---|---|
| A (Enter) | send | allow once |
| B (Backspace) | delete | deny |
| X (Tab) | – | always (Claude Code's suggested rule, or "accept edits" for the session) |
| Y (Space) | space | – |
| D-pad ↑↓ / L1 R1 (PgUp/PgDn) | scroll the transcript | scroll |
| L2 (Esc) | stop the turn (interrupt) | deny and stop |
| SELECT (Ctrl+C) | stop, or clear the field | – |
| START | on-screen keyboard (shell) | – |

Enter/Tab/Esc never auto-repeat, so a held A can't approve two prompts.
Requests queue ("1 OF 2"). A new request buzzes the rumble motor
(`mortar-rumble 120 1`; `MORTAR_AI_RUMBLE=0` turns it off), so the Brick
can sit in a lap while Claude works.

Layout: status bar; transcript scrolled from the bottom (user lines with a
lavender bar, assistant text in Onest, tool calls as a mono line with a
state square: yellow running, lavender waiting, lime done, orange denied or
failed, plus the first 6 lines of output, spaces kept); the compose field
or the permission card (lavender band, title, the command or a -/+ diff
excerpt, three button blocks); the hint bar with model and folder on the
left. Screenshots: `mortar-ai preview --screen
empty|chat|bash|edit|busy|error|exited|pair|pair-direct --out x.png`.

Rendering: the mortar-ui software renderer into an XRGB wl_shm buffer
(up to 3, reused), full redraw paced to at most 20 fps while text streams,
nothing while idle; wrapped lines are cached per transcript entry.

### Not done (yet)

- Sessions: `--continue` exists (`mortar-ai run --continue`) but there is no
  picker for older sessions.
- AskUserQuestion / plan-mode exit come through as ordinary tool permission
  requests (allow/deny only, no option picker).
- Images and files from the Brick (screenshots of the M8?) are not wired.
- Markdown is only lightly handled (code fences, headings, bold markers).

## Memory

Measured off-device (headless test in the build sandbox, aarch64, RSS / PSS
in MB):

| Process | RSS | PSS |
|---|---|---|
| mortar-shell (3 apps, screenshots on) | 54-61 | 47-53 |
| mortar-ai (1024x768 canvas + 3 shm buffers + fonts) | 29-32 | 26-28 |
| foot | 31 | 25 |
| tmux server + client | 10 | 4 |
| session script | 3 | 1 |

Sizes: `claude` is a 280 MiB native binary (file-backed: its pages can be
dropped and re-read from the card under pressure, which zram does not
help with); claude-code + deps add ~395 MiB to the closure (system closure
4.65 → 5.07 GB). mortar-ai is 3.2 MB.

Earlier on-device: Claude Code ~200 MB RSS at startup, ~8 s to start.

Budget for "Claude Code + shell + one app":

| | MB |
|---|---|
| kernel, systemd, journald, sshd, iwd (measure) | ~80-120 |
| mortar-shell | ~50-60 |
| foot + tmux | ~40 |
| claude (interactive), start → long session | 200 → 300+ |
| m8c (SDL on llvmpipe; mostly shared libs) | ~160 |
| total | ~530-680 of ~970 |

So one Claude Code plus the M8 fits without swapping; zram covers
transcript growth. Each of the three entries runs its own `claude` process
(the controller app, the tmux session and Remote Control); running all three
at once is ~600 MB of claude alone and will lean on zram. Use one at a time,
or Remote Control alone (it runs Claude Code for the phone; nothing else
needs to be open).

To measure on the device (`mortar-ai-mem` prints all of it):

1. Idle shell: `MemAvailable`, `zramctl`, `/proc/pressure/memory`.
2. Claude Code open, after start and after ~10 turns with tool use: RSS,
   PSS and Swap of `claude` (smaps_rollup), MemAvailable, zram ratio.
3. Same with the M8 streaming audio in the background: any underruns
   (m8c log), memory PSI `some avg10` (want ~0; >10 means stalls).
4. Remote Control running alone for an hour with the phone attached.
5. Controller app + Claude Code at once (two claude): does the shell stay
   responsive; `journalctl -k | grep -i oom`.
6. Startup time: `time claude --version`, and the controller app's first
   `system/init` (the status line shows the model once it arrives).

## Tests

- `cargo test` (any OS): protocol messages, the model (streaming, tool
  flow, allow/deny/always, queued requests, unknown control requests,
  errors, restart, scrolling and Ctrl keys), the US key map, the pair
  page's code filter and HTML escaping, the demo screens.
- `passthru.tests.headless` (aarch64-linux, in the build sandbox):
  mortar-shell headless with a fake `claude` that mimics the stream-json,
  `auth login` and interactive modes. It types a prompt, checks the
  permission card, allows with A and checks the allow response reached
  claude; opens Claude Code (foot + tmux), checks the sign-in guide, the
  QR window found from the pane, GETs the pair page, checks a wrong token
  gets 404, POSTs a code and checks it was typed into the login (the
  fake writes it to `.credentials.json`) and that Claude Code then starts
  signed in; closes the terminal and checks the tmux session survived;
  shows the Remote Control menu. Screenshots `01`-`08` in `$out`.
- `mortar-ai ask --model haiku "…"` checks the protocol against the real
  CLI from any terminal (off-device or over SSH).
