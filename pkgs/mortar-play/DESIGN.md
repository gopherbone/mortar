# mortar-play design

A libretro frontend for Mortar with the MinUI idea: a curated core per
system, preconfigured, no settings on the device. Copy games in, pick one,
play. It is not RetroArch: there are no per-game option menus, shaders,
overlays, netplay or achievements. All the choices that can be made ahead of
time are made in `modules/mortar-play.nix`.

## Pieces

```
pkgs/mortar-play/
  src/main.rs          CLI: library UI (default) | run SYSTEM [ROM] | list --json | selftest | make-test-rom
  src/app.rs           screens, launching, game loop (pacing, fast-forward, pause), in-game menu
  src/session.rs       a running game: core + SRAM autosave, state slots, resume state, thumbnails
  src/lr/              libretro: ffi.rs (API subset), core.rs (dlopen + callbacks), options.rs
  src/video.rs         core frame -> XRGB8888, rotation, layout (aspect/integer/...), CPU scaler
  src/audio.rs         cubic resampler + dynamic rate control, outputs, WAV/analysis for tests
  src/input.rs         Brick buttons -> RetroPad map, hotkey chords, SDL button mapping
  src/library.rs       ROM folders, display names, cue/m3u de-duplication, zip/7z, recents,
                       the library as JSON for mortar-shell (`list --json`)
  src/control.rs       control socket for mortar-shell's game panel, the "Saved" toast
  src/ui.rs            screens drawn with the mortar-ui toolkit (home, games, menu, messages)
  src/platform/        sdl.rs (SDL3 window/audio/pads), wayland.rs (wl_shm client), headless
  src/selftest.rs      headless core test with a JSON report (CI and on-device speed numbers)
  src/testrom.rs       a tiny NES program assembled in Rust (tests need no third-party ROM)
  test.nix             integration test (passthru.tests.integration)
  test-shell.nix       inside mortar-shell, games as apps (passthru.tests.shell)
modules/mortar-play.nix  curated systems, presets, dirs, launcher entries
```

## Frontend core

- **Loading.** `libloading` dlopens the core, every `retro_*` symbol is
  resolved up front, `retro_api_version` must be 1. One core at a time
  (libretro is a global API); the library is dlclosed when the game ends,
  so the next game gets a fresh core.
- **Callbacks** carry no user pointer, so the frontend state they touch
  (`Host`: frame, audio, pad bits, options, directories, AV info) is
  reached through one atomic pointer set while a `Core` exists. They only
  run inside calls into the core, on the calling thread; the variadic
  log callback is formatted by a few lines of C (`csrc/log_shim.c`) because
  stable Rust can't define variadic functions.
- **Environment.** Pixel formats 0RGB1555/XRGB8888/RGB565; system (BIOS),
  save and core-assets directories; variables: legacy SET_VARIABLES, core
  options v1 and v2 (and their _INTL forms, using the US definitions);
  GET_VARIABLE answers with the system preset when it names a valid value,
  else the core's default; unknown preset keys or values are logged once.
  Also: input descriptors, log, perf, SET_GEOMETRY / SET_SYSTEM_AV_INFO,
  rotation, frame-time callback, audio-buffer-status callback (lets cores
  with "auto" frameskip see the queue), disk control v0/v1 (multi-disc
  PlayStation games: "Change disc" in the menu), input bitmasks, message
  (logged), SHUTDOWN, fast-forwarding state, JIT capable, username,
  language, can-dupe. Refused: HW render (see below), VFS, LED, rumble,
  sensors, camera, MIDI, GAME_INFO_EXT.
- **Content.** Cores that want a path get a path, the rest get the file in
  memory (kept alive until unload). `.zip` / `.7z`: handed over as-is if
  the core lists the extension or the system keeps archives (arcade sets),
  otherwise the first entry with a usable extension is extracted to
  `$XDG_RUNTIME_DIR/mortar-play` (RAM). Saves are named after the file the
  user picked, so an archive keeps its saves.

### Video

The core's frame is converted to XRGB8888 in the video callback (a copy of
at most 640x480; a dupe frame keeps the previous one), rotated if the core
asked (vertical arcade games), then laid out on the 1024x768 screen.

**Display** (`video::View`: a filter and a fit) is the user's choice:
◀▶ on the Display row of the in-game menu or of the shell's game panel
steps through them and applies at once; A keeps the choice for the game,
X for all the system's games (`src/display.rs`, kept in
`<stateDir>/display.json`, a game's own choice over its system's, over the
system's default `scaling` / `aspect` in modules/mortar-play.nix).

| filter | what | inside the shell | CPU per frame (PS1 320x240, 1008 MHz) | default for |
|---|---|---|---|---|
| `smooth` | bilinear-like | the frame at its own size, the display plane scales | 0.27 ms (a copy) | - |
| `sharp-bilinear` | nearest to the largest whole multiple, then smooth: crisp and even | x3 (960x720) on the CPU, the plane does the last 1.07x | 2.2 ms | PS1 |
| `sharp` | nearest to the full size | CPU scaled to the screen, scanned out | 2.6 ms | NES, SNES, MD, SMS, PCE, arcade, 2600 |
| `integer` | whole multiples only, black borders (pixel perfect) | CPU scaled to the screen, scanned out | 2.5 ms | GB/GBC (5x), GBA (4x), GG, Lynx, NGP, WS |

Fit: `fit` keeps the core's display aspect ratio (integer: whole multiples
vertically, the width from the aspect, so square-pixel systems get whole
multiples both ways); `stretch` fills the screen (integer: whole multiples
on each axis). The display engine's scaler has one filter (polyphase
Lanczos-2 coefficients fixed in the sun8i driver, no nearest mode), so
everything sharp is nearest on the CPU first; smooth and sharp bilinear at
640x480 (no whole multiple fits) cost nothing in the shell or the
frontend.

Presenters:

- **Inside mortar-shell** (WAYLAND_DISPLAY set): a minimal Wayland client
  (`platform/wayland.rs`, wayland-client in pure Rust), one fullscreen
  xdg_toplevel. As in gamescope, the game is drawn at (about) its own size
  and the display scales it; nothing is composited. Buffers, best first:
  1. **plane-scaled** (smooth, sharp bilinear): the core's frame (sharp
     bilinear: first scaled up by a whole multiple, nearest) is copied into
     a KMS dumb buffer (`platform/dumb.rs`: sun4i-drm, CMA, linear
     XRGB8888, shared as a zwp_linux_dmabuf_v1 buffer; three of them) with
     a wp_viewport destination of 1024x768. Letterboxed pictures get black
     borders in the buffer in the same proportion, so the buffer always
     covers the screen. The shell puts it straight on the primary plane and
     the display engine's scaler (a soft Lanczos-2 filter) scales it.
  2. **screen-size dma-buf** (sharp, integer, the menus): the CPU nearest
     scaler writes into a 1024x768 dumb buffer that the shell scans out
     unscaled, so the shell copies nothing. The mapping is write-combined,
     so the scaler only ever writes (each source row is gathered once into
     a cached row, then copied to every screen row that repeats it).
  3. **wl_shm** (no display device, e.g. the sandbox tests, a refused
     dma-buf, or `MORTAR_PLAY_PRESENT=shm`): two memfd buffers, CPU scaled;
     the shell copies the damaged rectangle into scanout.

  Only the changed rectangle is damaged after the first frame of a layout,
  and if every buffer is still held the frame is dropped rather than
  waited for (the count is logged at exit). Why not SDL's Wayland backend:
  it renders through EGL (mortar-wlegl on the PowerVR inside the shell),
  and a texture upload plus a full-screen quad a frame is more work than
  handing the display a 0.3 MB buffer.
- **Standalone** (greeter session, `mortar-sdl-run`, so SDL's vivante
  backend on the PowerVR GPU; or KMSDRM + llvmpipe as the fallback; or the
  macOS window for development): SDL3 renderer, the native frame is
  uploaded to a streaming texture (at most 640x480x4) and scaled by the
  GPU, linear for smooth, nearest for the rest.
- **Headless**: the same CPU scaler into memory, for screenshots/tests.

### Audio

Cores produce samples at their own rate (32040.5 Hz SNES, 32768 Hz GB/GBA,
44.1 or 48 kHz others). The device runs at 44.1 kHz (dmix on the codec,
`modules/mortar-apps.nix`); SDL3 opens the default device, and the stream's
input format is set to the device format so SDL does no conversion of its
own. `audio::Resampler` is a cubic Hermite resampler on interleaved stereo
i16 that carries its history across frames (no seams).

**Dynamic rate control** (as in RetroArch): video is paced by the frame
clock, audio by the sound card's clock, and they drift (a few hundred ppm,
or 0.16% when a 60.0988 Hz SNES runs against a 60 Hz present). Each frame
the resampling ratio is nudged by at most ±0.5% from the queue fill:
`ratio = base * (1 + 0.005 * (1 - 2 * fill))`, `fill = queued / (2 *
target)`, target 64 ms. The queue settles where the ratio cancels the drift,
so there is no underrun crackle and no growing latency; ±0.5% (8.6 cents)
is inaudible. A hard cap drops audio if the queue ever exceeds 4x target
(after a stall). The other way round, DRC is far too slow to build a queue
from nothing (0.5% of 64 ms takes ~13 s), so a queue under a quarter of
the target (the game starting, or a stall that drained it) is first
filled to the target with silence: one short gap instead of seconds of
near-underruns. (Measured on the Brick under PipeWire, PS1, first 10 s:
5 empty and 75 low pushes and 15 sink xruns before; steady play after
that never went under ~49 ms.) Every 10 s with a problem the log says
how many frames started late, the lowest queue, and how many pushes found
it low or empty (`MORTAR_PLAY_DEBUG_AUDIO=1`: always). Unit tests cover
length/pitch of the resampler, a simulated 0.2% clock drift that settles
instead of overflowing, and a device draining 1024-frame periods from an
empty queue that never runs dry.

Fast-forward mutes audio (the queue isn't fed) and runs `fastForwardSpeed`
frames per presented frame.

### Pacing

`Pacer` sleeps until the next frame is due at the core's fps (absolute
deadlines, so jitter doesn't accumulate; more than 4 frames behind resets
instead of racing). SDL's renderer vsync is off: on the PowerVR path the
DC flip may still block on vblank, in which case the loop runs at the
display's rate and DRC absorbs the 0.1-0.5% difference. Inside the shell
commits never block.

### Input

The shell owns the physical pad and forwards to a per-app uinput clone
that SDL opens (the shell puts the other pads on SDL's ignore list), so
the frontend reads pads through SDL's gamepad API whether it runs inside
the shell or not. Mapping is by position, like mortar-ui: east = A,
south = B, north = X, west = Y. The RetroPad is SNES-shaped, so every
system gets the natural layout by default (NES B/A = Brick B/A, Mega Drive
A/B/C = Y/B/A, PlayStation square/cross/circle/triangle = Y/B/A/X);
`controls` remaps per system. SDL's automatic mapping of the gpio-keys pad
has no slot for BTN_TRIGGER_HAPPY1/2, so F1/F2 are read as raw joystick
buttons 15/16 of a pad named "TrimUI Brick" (SDL numbers buttons in evdev
code order). Triggers (L2/R2 as axes) and the left stick (D-pad) are
latched with hysteresis. Real keyboards work in both modes (SDL keys, or
wl_keyboard evdev codes through mortar-ui's table inside the shell).
`MORTAR_PLAY_DEBUG_INPUT=1` logs every SDL input event.

**Hotkeys and the shell's MENU rule.** mortar-shell takes a lone MENU
press for its overlay; MENU pressed while another button is held goes to
the app. So the in-game menu opens with **SELECT held + MENU**, **F1**, or
plain **MENU** when running outside the shell (all three are the default
`hotkeys.menu`). While the shell's overlay is up (or another app is in
front) the shell un-activates our toplevel; the frontend pauses emulation
and audio until it is activated again, so opening the shell menu never
costs a life. **Fast-forward** is held: R2 on systems without an R2 of
their own, F2 on PlayStation and arcade (configurable).

### Saves

- **SRAM** (`RETRO_MEMORY_SAVE_RAM`, plus RTC) is loaded after the game
  and written (atomically, tmp + rename) when it changed: every
  `autosaveSeconds` (10 s), on quit, on SIGTERM/SIGINT (the shell closing
  the app) and when the session is dropped. A fresh all-zero/0xff SRAM
  never replaces nothing-yet with garbage.
- **States**: 4 slots plus `auto`, in `states/<system>/<game>.state.<n>`
  with a 320x240 PNG thumbnail next to each. Quitting from the menu (or
  the shell closing the app) writes `auto`; starting the game again loads
  it (`autoResume`). The home screen's "Continue" item and the game list
  show the resume picture. Load from the menu resumes immediately.
- `selftest` checks that serialize -> run 30 frames -> unserialize -> run
  30 frames gives identical pictures.

## Inside mortar-shell: one game, one app

The shell runs every game as its own app, `mortar-play run SYSTEM ROM`
(mortar-shell/DESIGN.md, "Games"), so the frontend's own library is only
the "All systems" entry. What the frontend does for that:

- `mortar-play list --json` prints the library: every system (id, name,
  tag, order, no-content, count) with its games (path, cleaned name, first
  tag as `region`, whether a resume state exists and its picture), the six
  most recent games, and `watch`: the folders whose modification times
  change when the answer would (ROM folders and their sub-folders, the
  state folders, the data folder with recent.json). The shell runs it off
  its event loop and only when one of those changed.
- **Control socket** (src/control.rs), opened when running under the shell
  (`MORTAR_APP` set): `$XDG_RUNTIME_DIR/mortar-play-<pid>.sock`, 0600, one
  JSON request line and one reply line per connection, polled once per
  loop iteration (a frame while playing, ~16 ms while paused or hidden):
  `list_states` (four slots with age and thumbnail path, the auto state,
  discs, fast-forward), `save_state` / `load_state` (slot 1-4 or "auto";
  loading leaves the in-game menu), `reset`, `fast_forward` (true, false
  or "toggle": runs at `fastForwardSpeed` until switched off, held R2 still
  works), `change_disc` ("next"), `display` (`"step": ±1` for the next or
  previous choice, `"keep": "game"|"system"`; `list_states` reports it as
  `display`: label, key, scope), `quit` (writes `auto` and SRAM, replies
  `{"saved": …}`, exits; `"quiet": true` for the shell's hibernation).
  Replies are `{"ok": true, …}` or `{"ok": false, "error": …}`.
- **"Saved" toast.** When a game ends with its resume state written (menu
  quit, the shell closing the window, SIGTERM, `quit`) the frontend sends
  `toast {"title":"Saved","sub":"Pocket King · GBC"}` to the shell's
  socket ($MORTAR_SHELL_SOCKET or $XDG_RUNTIME_DIR/mortar-shell.sock),
  unless the quit was quiet.
- Waking a game the shell put to sleep is just `run` again: auto resume
  loads `auto`.

Measured in the sandbox test (aarch64, NES test program, fceumm): a game
process is about 50 MB RSS (52 MB and 50 MB for two), of which ~6 MB are
its two 1024x768 shm buffers (shared with the shell) and the rest SDL3,
the core, fonts and the UI canvas. Heavier cores add their own: expect
roughly 55-70 MB for mgba / snes9x / genesis-plus-gx, 80-120 MB for
pcsx-rearmed with a disc (its RAM, VRAM, SPU and dynarec cache). With
`maxGames` = 3 that is ~150-300 MB of games before the oldest sleeps.

## UI

mortar-ui toolkit and theme (style v2, Pond: flat blocks, ink outlines,
lime selection, Instrument Serif titles), rendered on the CPU into a
1024x768 canvas that is presented only when something changed.

- **Home**: Continue (last game, with its resume picture), Recently played,
  systems with games (count + tag), systems without games dimmed at the end
  (A shows where to copy games and the `scp` line). No-content cores (2048)
  start directly.
- **Games**: the system's list (names cleaned of "(USA)" etc., the first tag
  kept on the right), L1/R1 page, "Resume where you left off" on games with
  a resume state, picture on the right.
- **In-game menu**: Resume, Save state, Load state (←/→ picks the slot; the
  picture shows the slot's thumbnail and age), Change disc (multi-disc),
  Display (←/→ steps through the scaling choices; A keeps it for the
  game, X for all the system's games),
  Reset, Save and quit.
- **Messages**: missing BIOS (required: can't start; optional, e.g. the PS1
  with its HLE BIOS: warned once per run, "Play anyway"), core load errors,
  empty folders.

## Systems (defaults in modules/mortar-play.nix)

| System | Core (nixpkgs `libretro.*`) | Licence | Notes |
|---|---|---|---|
| Game Boy / Color | gambatte | GPL-2.0 | GBC palettes for DMG games ("auto") |
| Game Boy Advance | mgba | MPL-2.0 | skip BIOS, idle-loop removal; gba_bios.bin optional |
| NES / Famicom | fceumm | GPL-2.0 | 8 px overscan crop; disksys.rom for FDS |
| Super Nintendo | snes9x2005-plus | non-commercial | the fast snes9x branch with the blargg APU |
| Mega Drive, Master System, Game Gear | genesis-plus-gx | non-commercial | |
| PC Engine (+CD) | beetle-pce-fast | GPL-2.0 | syscard3.pce for CD |
| PlayStation | pcsx-rearmed | GPL-2.0 | dynarec, threaded GPU + SPU, auto frameskip; sharp bilinear (nearest x3, the display plane finishes); HLE BIOS unless scph*.bin |
| Arcade / Neo Geo | fbneo | non-commercial | zips as-is; neogeo.zip for Neo Geo |
| Atari 2600 | stella2014 | GPL-2.0 | |
| Atari Lynx | handy | zlib | needs lynxboot.img |
| Neo Geo Pocket (Color) | beetle-ngp | GPL-2.0 | |
| WonderSwan (Color) | beetle-wswan | GPL-2.0 | |
| 2048 | 2048 | Unlicense | no content; a toy and a smoke test |

All are available for aarch64-linux in the pinned nixpkgs. The three
non-commercial ("unfree redistributable") cores need the flake's
`allowUnfreePredicate` (names in `mortar.play.unfreeCores`).

## Performance on the Brick (A133, 4x Cortex-A53 @ up to 2 GHz)

Expected cost of the emulation alone (`retro_run`) as a share of one core
at full speed, from the cores' known behaviour on A53-class handhelds
(RG35XX/Plus at 1.5 GHz, Miyoo Flip, TrimUI Smart Pro); the measured
column is `mortar-play selftest` on the device (`ms_per_frame`,
`core_load`), filled in as games are tested:

| Core | Expected load | Measured | Comment |
|---|---|---|---|
| gambatte | 5-10% | | trivial |
| fceumm | 5-10% | | trivial |
| stella2014, beetle-ngp, beetle-wswan | 10-20% | | |
| genesis-plus-gx (SMS/GG) | 10-15% | | |
| genesis-plus-gx (MD) | 30-45% | | full speed; picodrive is the fallback if a game struggles |
| beetle-pce-fast | 20-30% | | CD games similar |
| snes9x2005-plus | 30-50% | | SuperFX/SA-1 titles 60-90%; snes9x2010 costs ~1.5x, current snes9x ~2-3x |
| mgba | 50-75% | | full speed with idle-loop removal; gpsp (dynarec) is ~2x faster if needed |
| pcsx-rearmed | 40-70% (main) + GPU thread | 13% main + 9% GPU thread at 1008 MHz; 8% main at 1992 MHz (a light 2D scene, 320x240) | arm64 dynarec + NEON GPU; enhanced resolution off |
| fbneo | 30-80% | | CPS1/2 and Neo Geo fine; heavier boards (CPS3, Cave SH-3) not |
| handy | 15-25% | | |

Frontend overhead per frame, by Display filter (see Video): smooth is a
~0.3 MB copy into a dma-buf the display scales (0.27 ms at 1008 MHz);
sharp bilinear a nearest x3 into 960x720 (2.2 ms); sharp and integer a
nearest scale to the screen (~2-3 MB written; 2.5-2.6 ms at 1008 MHz,
2.1 ms at 1992 MHz). The shell copies nothing in any of them.
Audio resampling ~0.1 ms; the SDL/GPU path uploads at most 1.2 MB.

### PlayStation, measured (2026-11-01)

`mortar-play selftest psx GAME --resume [--paced]` from a resume state
(a light 320x240 2D scene; heavy 3D games cost more, the split is the
same), unpaced unless noted. `cpu` is the whole process (all threads).

| Clock | retro_run | GPU thread | SPU thread | CPU scale to 1024x768 | cpu |
|---|---|---|---|---|---|
| 1008 MHz, before (old scaler) | 2.09-2.17 ms | 1.6-1.7 ms | 0.5 ms | 3.1-3.3 ms | 7.4-7.7 ms |
| 1008 MHz, write-only scaler | 2.09 ms | 1.5 ms | 0.5 ms | 2.5-2.7 ms | 6.6-7.0 ms |
| 1416 MHz | 1.85 ms | | | 2.75 ms | 6.0 ms |
| 1992 MHz | 1.34 ms | | | 2.10 ms | 4.9 ms |
| 1008 MHz, GPU thread off | 3.28 ms | - | 0.5 ms | 2.6 ms | 6.5 ms |
| 1008 MHz, core built with -mcpu=cortex-a53 | 2.09 ms | 1.5 ms | 0.5 ms | 2.6 ms | 6.9 ms |

Paced (60 fps, 2400 frames at 1008 MHz): 2 late frames, RSS 70 MB, no swap.
The threaded GPU takes 1.2 ms a frame off the emulation thread. Building
the core for the A53 bought nothing measurable (nixpkgs already builds it
with -Ofast, ari64 and the NEON renderer), so the cached build stays.

In the shell (the same game and state as a shell client, 30 s at
1008 MHz, `top`-style share of one core; audio to SDL's dummy driver):

| Presenter | shell | emulation thread | GPU thread | SPU | total |
|---|---|---|---|---|---|
| before: wl_shm, CPU scale, shell copies | 24.5% | 21.4% | 10.5% | 2.5% | 59% |
| wl_shm with the write-only scaler | 26.1% | 21.2% | 13.5% | 3.2% | 64% |
| screen-size dma-buf (CPU scale, scanout) | 4.3% | 21.1% | 13.3% | 2.6% | 41% |
| smooth: native 320x240 dma-buf + viewport (plane scaler) | 4.5% | 14.6% | 13.9% | 2.7% | 36% |
| sharp bilinear: 960x720 (nearest x3) + plane | 4.3% | 19.8% | 10.5% | 2.3% | 37% |

Every frame of the last two went straight to the panel (the shell's
`scanout_frames` counted them all); debugfs showed the primary plane with a
320x240 framebuffer and a 1024x768 CRTC rectangle. 1269 frames presented,
0 dropped for lack of a free buffer (the game draws at 30 fps).

**CPU clock.** Until the kernel change in this commit series the A53s ran
at the 1008 MHz boot OPP all the time: the defconfig's default governor
was `userspace` and nothing set a speed. schedutil is the default now. A
10-minute soak with two cores spinning at the 1992 MHz setpoint (a stand-in
for a heavy game) averaged 63 C on cpu-thermal (peak 71 C); the 63 C
passive trip (step_wise) held the clock at 1992/1800/1608 MHz for about
47/49/5% of the time, so ~1.9 GHz sustained, no fan needed. zram: no swap
activity during play (pswpin/pswpout unchanged over the runs).

## Hardware rendering (designed, not built)

No curated core needs it (pcsx-rearmed, the GPU-heavy one, renders in
software with NEON). The hook, for later (e.g. flycast, mupen64plus-next,
parallel-n64 in GLES2 mode):

1. SET_HW_RENDER is recorded in `Host::hw_render` and refused today. To
   serve it: accept `RETRO_HW_CONTEXT_OPENGLES2/3` only (the GE8300 does
   GLES 3.2; no desktop GL).
2. Only on the SDL path (standalone, PowerVR via the vivante backend):
   create the window with `SDL_WINDOW_OPENGL`, an ES context
   (`SDL_GL_CONTEXT_PROFILE_ES`, major/minor from the request) and,
   instead of the SDL renderer, an FBO with a colour texture of
   `max_width x max_height` plus a depth/stencil renderbuffer if asked.
3. `get_current_framebuffer` returns that FBO; `get_proc_address` is
   `SDL_GL_GetProcAddress`; `context_reset` is called after creation (and
   after any context loss), `context_destroy` before teardown.
4. `video_refresh(RETRO_HW_FRAME_BUFFER_VALID, w, h, _)` draws the FBO
   texture's `w x h` region (flipped if `bottom_left_origin`) to the
   default framebuffer with one textured quad, then swaps. The menu draws
   the UI texture with the same quad program.
5. Inside mortar-shell there is no GPU path (no GBM/EGL Wayland platform
   on the PowerVR stack), so HW cores would be "standalone only" until the
   GBM/EGL shim in notes/gpu/sdl-apps.md (option 2) exists.

## Testing

- `cargo test`: options parsing/presets, pixel formats, layouts and Display choices (kept per game / system), scaler,
  rotation, thumbnails, resampler length/pitch/transparency, DRC
  direction and drift, button maps, chords, config parsing, library scan
  (cue/m3u hiding), zip extraction, recents, script parsing.
- `passthru.tests.integration` (aarch64-linux, in the sandbox): the built-in
  NES program on fceumm (screenshots; 440 Hz tone, 880 Hz while A is held,
  i.e. input reaches the game; 5 s of audio for 5 s of frames, i.e. the
  resampler and rate are right; state determinism; slot + thumbnail; 8 KiB
  SRAM file), 2048 without content, and the whole UI driven by a script on
  the headless display (home, list, play, menu, save, quit with resume
  state, resume picture, empty-folder screen). Screenshots stay in `$out`.
- `passthru.tests.shell` (aarch64-linux, in the sandbox): the frontend
  inside mortar-shell on its headless backend: the wl_shm presenter,
  pausing under the shell's MENU, the in-game menu, closing with the resume
  state; then games as apps with two copies of the built-in test program:
  `list --json`, MENU > Games and the NES page, a game started from it
  (its title), the game panel (save to slot 1, its picture, Display stepped and kept
  for the game, load the slot), the
  save before suspend (`ctl save-games`), the screenshot combo, a second
  game putting the first to sleep (`maxGames` = 1), waking it, starting it
  again (switches, no second copy), X with the "Saved" toast. Screenshots
  and each game's RSS (`rss.txt`) in `$out`.
- On the Mac: `./dev.sh` (SDL window, keyboard) with locally built cores.
