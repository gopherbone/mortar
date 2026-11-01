# mortar-shell design

A Wayland compositor that turns Mortar into a multitasking handheld: every
app is fullscreen, MENU switches between them, and apps you are not looking
at keep running (the M8 keeps playing while you use Claude Code in a
terminal). It is built for the TrimUI Brick: 1024x768 panel on sun4i
DRM/KMS, 4x Cortex-A53, about 970 MB RAM plus zram, buttons from evdev, and
a PowerVR GE8300 with no GBM.

## Pieces

```
pkgs/mortar-shell/
  src/main.rs        CLI: run | ctl | preview
  src/config.rs      /etc/mortar-shell/config.json (written by the module)
  src/apps.rs        running apps: launch, pid ancestry -> app, MRU, close, reap
  src/router.rs      who gets a pad button (pure, unit-tested)
  src/overlay.rs     home screen + MENU overlay (mortar-ui toolkit)
  src/osk.rs         on-screen keyboard + pad-as-keyboard mapping
  src/preview.rs     render shell screens to PNG without a compositor
  src/comp/          Linux only
    state.rs         Smithay state + handlers, focus, input dispatch
    backend.rs       element list (renderer-generic), headless backend, screenshots
    drm.rs           libseat session + DRM/KMS output
    input.rs         evdev devices via the session, classification, grabs
    pads.rs          per-app uinput gamepads
    ctl.rs           control socket
  test.nix           headless integration test (screenshots)
modules/mortar-shell.nix
```

The UI code (software renderer, fonts, icons, theme tokens, widgets,
evdev-to-button mapping, status probes) is the `mortar-ui` library
(`pkgs/mortar-ui`, `default-features = false`), shared with the greeter.

One process, one calloop event loop: Wayland clients, DRM vblanks, session
events, input fds, the control socket and a 2 s device rescan timer.

## Wayland surface

Globals: wl_compositor/subcompositor, wl_shm, zwp_linux_dmabuf_v1 (formats
the pixman renderer can import: linear), xdg_wm_base (toplevels and
popups), wl_seat (keyboard), wl_output + xdg-output, wl_data_device_manager
(clipboard). No pointer or touch yet (the Brick has neither), no
server-side decorations (apps are fullscreen anyway), no XWayland.

Every toplevel is configured fullscreen at the output size and `activated`
only while it is on screen. A client that picks a smaller size is centred
on black.

## Apps and focus

- Launcher entries come from the config: `id, name, description, icon,
  command (argv), env, gamepad (forward | keyboard | none),
  singleInstance`. Launching sets `WAYLAND_DISPLAY`,
  `SDL_VIDEO_DRIVER=wayland`, `XDG_SESSION_TYPE=wayland`, `MORTAR_APP`, the
  gamepad blacklist (below) and the entry's env, and puts the app in its
  own process group.
- A new toplevel belongs to the app whose launched pid is the client's pid
  or an ancestor (`/proc/<pid>/stat`), so helpers and child windows land in
  the right app. Clients nobody launched (e.g. started over SSH with
  `WAYLAND_DISPLAY=wayland-0`) become their own "external" app and are
  shown at once.
- An app the user starts is shown when its first buffer arrives; until
  then the previous screen stays.
- Focus is a most-recently-used list. The visible app is the head of the
  list (its newest window). MENU, A in the switcher goes back to the
  previous app, so flipping between two apps is two presses.
- Close: `xdg_toplevel.close`, then SIGTERM to the process group after 3 s
  and SIGKILL after 6 s. Exited apps with no window are dropped and the
  next app in MRU order is shown, or the home screen.
- Log out (quick settings) ends the shell; greetd returns to the greeter
  and all apps get SIGTERM.

## Screens

- Home (no app visible): launcher tiles (mockup 7) and the SSH strip;
  MENU flips between Launch and Settings.
- MENU over an app (mockup 10, "switch session" became "switch app"):
  tabs Apps (switch with A, close with X twice), Launch (tiles; starting a
  running single-instance app switches to it) and Settings (brightness and
  volume with left/right, on-screen keyboard, Wi-Fi state, SSH sessions,
  sleep, log out, restart, power off; destructive items need a second
  press). B or MENU returns to the app.
- Volume keys work everywhere; over an app a small lime OSD shows the level
  for 1.5 s.
- All of it is drawn with the mortar-ui toolkit in the shared theme
  (style v2: pale ground, ink outlines, square blocks, lime selection),
  into a CPU canvas that is uploaded to a Smithay memory buffer only when
  something changed.

## Input ownership

The problem: SDL apps read the gamepad themselves (evdev through udev), and
with several apps running every one of them would see every press. Only
the visible app may react, MENU must reach the shell, and apps must not
see stuck buttons when they lose the screen.

Options considered:

1. Revoke per client (`EVIOCREVOKE`, logind `TakeDevice`). Revocation
   works on fds the compositor hands out. SDL opens /dev/input itself, so
   the shell never holds the app's fd; and a revoked fd stays dead, so
   handing the pad back after a switch would need the app to reopen it.
   Apps also all run as the same user, so permissions cannot separate them.
2. Freeze background apps (SIGSTOP). Breaks the point: the M8 must keep
   streaming audio.
3. **The shell owns the pads and forwards to per-app virtual pads.**
   Chosen.

How it works:

- The shell opens every input device through the session (libseat; fds
  are revoked on VT switch) and `EVIOCGRAB`s every gamepad, so no other
  reader gets its events.
- For each running app in `forward` mode it creates, before launching the
  app, a uinput clone of every physical pad: same name plus
  "(Mortar <slot>)", same key and axis set, bus VIRTUAL, vendor `0x6d72`,
  product `0x4d00 | slot<<4 | pad`. SDL builds the same automatic gamepad
  mapping from the evdev codes as for the real pad (it already does that
  for the Brick's gpio-keys pad, see pkgs/m8c), so app configs keep
  working.
- Each app starts with `SDL_JOYSTICK_BLACKLIST_DEVICES` and
  `SDL_GAMECONTROLLER_IGNORE_DEVICES` naming gpio-keys (`0x0001/0x0001`),
  the physical pads and every *other* slot's clones, so an SDL app opens
  only its own clone.
- Only the visible app's clones are fed. When the receiver changes
  (overlay opens, another app is shown, the app closes) every key the old
  app saw go down is released on its clone, and hats are centred.
- MENU on its own toggles the overlay. MENU pressed while another pad
  button is held goes to the app (m8c quits on SELECT+MENU).
- `keyboard` mode (terminals): pad buttons become key presses on the
  shell's wl_keyboard (D-pad arrows, A Enter, B Backspace, X Tab, Y Space,
  L1/R1 PgUp/PgDn, L2 Esc, R2 held Ctrl, SELECT Ctrl+C) with real
  down/up, so the client's own key repeat works. START opens the
  on-screen keyboard.
- Real keyboards (USB/Bluetooth) go to the visible app over wl_keyboard
  (xkb layout from the config). A tap of the Logo key toggles the overlay;
  with the overlay up, arrows/Enter/Esc navigate it.
- Volume keys are handled by the shell (amixer, configurable); the power
  key is left to logind.

Limits and notes:

- Isolation relies on SDL honouring the blacklist hints. A non-SDL app
  that opens every /dev/input node itself would see all clones. If that
  ever matters: launch apps in a mount namespace whose /dev/input holds
  only their own clone (all apps run as the session user, which can do
  this with `unshare` and a private tmpfs; sketched, not built).
- The Brick kernel defconfig has `CONFIG_INPUT_UINPUT` off. The module
  turns it on (`mortar.shell.kernelUinput`, a kernel rebuild) and enables
  `hardware.uinput`. Without /dev/uinput the shell still runs; forward-mode
  apps just get no pad, and the log says so.
- Pads hotplugged while apps run get clones for every running app; apps
  see them as new joysticks.

## On-screen keyboard

Implemented in the compositor rather than as a separate input-method
client: the shell already owns the pad and the seat keyboard, so the OSK is
a drawing plus a pad mapping that sends keycodes through the same
wl_keyboard (US keymap; shifted characters are wrapped in Shift). No
`zwp_input_method`/`virtual_keyboard` protocols are needed, and it works
with any client that takes keyboard input.

- START in a keyboard-mode app shows it along the bottom; the app is
  reconfigured to the height above it, so nothing is covered.
- D-pad moves (vertical moves keep the column), A types, B Backspace,
  X Space, Y Enter, L1 Shift (twice: caps lock), R2 Ctrl (one shot),
  L2 Esc, START or "hide" closes it.
- Later: a text-input-v3 aware mode that pops up only when a field is
  focused, word suggestions, and a symbols layer.

## Rendering

Now: Smithay's pixman (CPU) renderer everywhere.

- Composition is mostly one copy: the visible app's buffer is copied into
  the scanout buffer, damage-tracked, so an app that updates part of its
  window costs only that part. With the overlay up the app is not drawn at
  all (the overlay is opaque), so there is no blending; blending over a
  write-combined scanout buffer would read it back, which is slow on the
  A133 (see the notes in pkgs/m8c and pkgs/puzzlescript).
- Output: DRM/KMS through Smithay's `DrmCompositor`, swapchain buffers from
  GBM (Mesa's kms_swrast on sun4i, i.e. dumb buffers that pixman maps). If
  libgbm cannot open the card (or `MORTAR_SHELL_ALLOCATOR=dumb`), plain KMS
  dumb buffers are used instead; only direct scanout is lost.
- Direct scanout: when the only element is a client dmabuf that covers the
  screen, `DrmCompositor` puts it on the primary plane with no copy (the
  GBM framebuffer exporter imports client dmabufs). `ctl state` reports
  `scanout_frames`. Today's clients (SDL on Mesa llvmpipe, foot) use
  wl_shm, so they are copied; llvmpipe on Wayland would need a dmabuf
  path (a render node) to benefit. `MORTAR_SHELL_NO_SCANOUT=1` disables it.
- Frame pacing: the visible app gets frame callbacks after each presented
  frame (vblank). Background apps get callbacks at `backgroundFps`
  (default 2) so SDL render loops keep ticking slowly instead of
  stalling; their audio threads are unaffected.
- The element list (`backend::elements`) is generic over the Smithay
  renderer (`Renderer + ImportAll + ImportMem`). That is the seam for a GPU
  renderer.

Later: GLES on the PowerVR. What is needed:

- The stock DDK 1.19 userspace has no GBM and only the NULL WSEGL. Smithay's
  `GlesRenderer` needs an EGLDisplay plus `EGL_EXT_image_dma_buf_import`
  to sample client buffers and to render into dmabufs. The display could
  come from `EGL_PLATFORM_SURFACELESS` or the NULL WSEGL default display;
  rendering into the GBM/dumb scanout buffers needs dmabuf import as a
  render target (EGLImage + FBO). The README notes GLES already renders
  into dma-bufs for KMS apps, so the pieces may exist; they need checking
  against Smithay's EGL requirements (`EGL_KHR_surfaceless_context`,
  `GL_OES_EGL_image`, the formats/modifiers query).
- shm client buffers would be uploaded as textures (the same copy as now,
  done by the GPU) unless clients render on the GPU too.
- Apps on the GPU (mortar-sdl-run's vivante path) present through
  brick_dc_drm, which flips the CRTC itself. That cannot coexist with a
  compositor that owns KMS; inside the shell those apps must stay on
  software GL (the module sets `MORTAR_SDL=software` for every app), or
  get a Wayland EGL platform on the PowerVR (not in the stock DDK).
- Selection: a `MORTAR_SHELL_RENDERER=gles` switch with pixman as the
  fallback; the DRM compositor already takes the renderer as a type
  parameter.

## Memory budget

| Part | Estimate |
|---|---|
| mortar-shell RSS | 58 MB measured in the headless test with two apps, of which ~12 MB are the clients' mapped shm buffers and ~6 MB the screenshot renderer; the rest: UI canvas, overlay and OSK buffers (~8 MB), their pixman textures, fonts and glyph caches, Smithay state. Expect ~40-60 MB on the device. |
| Scanout | 2-3 swapchain buffers x 3 MB (CMA/dumb) |
| Per shm app | 2-3 client buffers x 3 MB, shared with the client |
| SDL app on llvmpipe | ~160 MB RSS, mostly shared libraries (pkgs/mortar-apps notes) |
| foot | ~15-20 MB |

So M8 + terminal + shell is roughly 250-280 MB of the ~970 MB, leaving room for
Claude Code (Node, ~150-250 MB) with zram behind it. The headless test
records the shell's RSS (`rss_kib` in state.json).

## Control socket

`$XDG_RUNTIME_DIR/mortar-shell.sock` (0600), one line per connection:
`state` (JSON), `press|down|up BUTTON`, `key CODE`, `launch ID`,
`focus ID`, `close ID`, `home`, `screenshot PATH`, `quit`. Tests drive the
shell through it; over SSH it gives remote screenshots and app control
(`XDG_RUNTIME_DIR=/run/user/0 mortar-shell ctl state`).

## Session and NixOS integration

`modules/mortar-shell.nix`, off by default:

- Adds the greeter session "Mortar" (order 0, so it is first), running
  `mortar-shell-session`: `mortar-shell run` under `systemd-cat -t
  mortar-shell`. greetd + pam_systemd give it a logind session on seat0,
  which libseat uses (no seatd needed).
- Imports the other greeter sessions as launcher entries (minus console-only
  ones, `excludeGreeterSessions`, default `terminal`), adds a foot
  Terminal entry (keyboard mode; Geist Mono, Mortar colours) and
  `mortar.shell.apps`. `exclusive = true` leaves only the shell on the
  greeter.
- Enables uinput (kernel option + `hardware.uinput`), puts greeter profile
  users in `uinput`, `input`, `video`, installs the Mortar fonts for
  fontconfig, and the backlight udev rule.

## Tests

- Unit tests (`cargo test`, also on macOS): config parsing, pid ancestry,
  MRU, button routing (MENU chord, releases on retarget, keyboard mode),
  OSK layout and typing, overlay navigation, gamepad blacklist.
- `passthru.tests.headless`: the compositor on the headless backend in
  the build sandbox; weston-fullscreen (wl_shm) and foot started from the
  launcher with injected buttons; typing through keys and the OSK;
  switching back; closing; a screenshot after every step.
- `mortar-shell preview --screen ...` renders the shell's screens on any OS
  for UI work.

## Open questions

- Suspend: does SDL/m8c audio survive `systemctl suspend`, and should the
  shell send `xdg_toplevel` `suspended` to background apps (SDL3 treats it
  as occluded; it may then stop pacing on frame callbacks and spin)?
- How do SDL3's Wayland backend and m8c behave at 2 frame callbacks per
  second in the background (CPU use, audio buffer underruns)? Tune
  `backgroundFps`, or 0 if SDL's own timeout is enough.
- Does Mesa llvmpipe pick wl_shm or dmabuf here? (No render node, so
  presumably wl_shm.) Measure the per-frame copy at 60 fps.
- Is `EVIOCGRAB` respected for the gpio-keys pad when another process
  (an app started outside the shell) already has it open? Grab only
  affects later reads, so it should be.
- Power key: logind suspends; should a short press open the shell's
  Settings instead?
- Settings screens (Wi-Fi, SSH) are still the mockups' future work; the
  quick settings link to them with a toast.
