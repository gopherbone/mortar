# mortar-play design

A libretro frontend for Mortar with the MinUI idea: a curated core per
system, preconfigured, no settings on the device. Copy games in, pick one,
play. It is not RetroArch: there are no per-game option menus, shaders,
overlays, netplay or achievements. All the choices that can be made ahead of
time are made in `modules/mortar-play.nix`.

## Pieces

```
pkgs/mortar-play/
  src/main.rs          CLI: library UI (default) | run SYSTEM [ROM] | selftest | make-test-rom
  src/app.rs           screens, launching, game loop (pacing, fast-forward, pause), in-game menu
  src/session.rs       a running game: core + SRAM autosave, state slots, resume state, thumbnails
  src/lr/              libretro: ffi.rs (API subset), core.rs (dlopen + callbacks), options.rs
  src/video.rs         core frame -> XRGB8888, rotation, layout (aspect/integer/...), CPU scaler
  src/audio.rs         cubic resampler + dynamic rate control, outputs, WAV/analysis for tests
  src/input.rs         Brick buttons -> RetroPad map, hotkey chords, SDL button mapping
  src/library.rs       ROM folders, display names, cue/m3u de-duplication, zip/7z, recents
  src/ui.rs            screens drawn with the mortar-ui toolkit (home, games, menu, messages)
  src/platform/        sdl.rs (SDL3 window/audio/pads), wayland.rs (wl_shm client), headless
  src/selftest.rs      headless core test with a JSON report (CI and on-device speed numbers)
  src/testrom.rs       a tiny NES program assembled in Rust (tests need no third-party ROM)
  test.nix             integration test (passthru.tests.integration)
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
asked (vertical arcade games), then laid out on the 1024x768 screen:

| mode | what | default for |
|---|---|---|
| `aspect` | as large as fits at the core's display aspect ratio | NES, SNES, MD, SMS, PCE, PS1, arcade |
| `integer` | whole multiples, square pixels (pixel perfect) | GB/GBC (5x = 800x720), GBA (4x = 960x640), GG, Lynx, NGP, WS |
| `integer-aspect` | whole multiple vertically, width from the aspect | (option; even scanlines for 224/240-line systems) |
| `fill` | stretch | - |

Presenters:

- **Inside mortar-shell** (WAYLAND_DISPLAY set): a minimal wl_shm client
  (`platform/wayland.rs`, wayland-client in pure Rust): one fullscreen
  xdg_toplevel, two XRGB8888 buffers in memfd shared memory. The frame is
  scaled on the CPU (nearest, precomputed column map, repeated rows are a
  memcpy) straight into whichever buffer the shell has released; only the
  game rectangle is damaged after the first frame of a layout, and if both
  buffers are still held the frame is dropped rather than waited for. The
  shell's composition is then a single damage-tracked copy into scanout.
  Why not SDL's Wayland backend: SDL renders through EGL, which inside the
  shell is Mesa's llvmpipe (the PowerVR has no GBM/Wayland path); a
  textured full-screen quad there costs most of an A53 core at 60 fps,
  time the emulator needs. The scaled write is ~3 MB/frame.
- **Standalone** (greeter session, `mortar-sdl-run`, so SDL's vivante
  backend on the PowerVR GPU; or KMSDRM + llvmpipe as the fallback; or the
  macOS window for development): SDL3 renderer, the native frame is
  uploaded to a streaming texture (at most 640x480x4) and scaled by the
  GPU, nearest or linear (`sharp`).
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
(after a stall). Unit tests cover length/pitch of the resampler and a
simulated 0.2% clock drift that settles instead of overflowing.

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
| PlayStation | pcsx-rearmed | GPL-2.0 | dynarec, threaded GPU + SPU; HLE BIOS unless scph*.bin |
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
| pcsx-rearmed | 40-70% (main) + GPU thread | | arm64 dynarec + NEON GPU; enhanced resolution off |
| fbneo | 30-80% | | CPS1/2 and Neo Geo fine; heavier boards (CPS3, Cave SH-3) not |
| handy | 15-25% | | |

Frontend overhead per frame: the CPU scale into shared memory (~3 MB
write, ~1-2 ms) plus the shell's copy of the damaged area; audio
resampling ~0.1 ms; the SDL/GPU path uploads at most 1.2 MB.

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

- `cargo test`: options parsing/presets, pixel formats, layouts, scaler,
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
- On the Mac: `./dev.sh` (SDL window, keyboard) with locally built cores.
