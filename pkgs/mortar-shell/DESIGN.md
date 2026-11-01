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
    scanout.rs       direct scanout of client dmabufs (PRIME import)
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
the pixman renderer can import: linear; GPU apps use XRGB8888), xdg_wm_base (toplevels and
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

The compositor renders with Smithay's pixman (CPU) renderer. GPU apps
render on the PowerVR into dma-bufs that the shell puts straight on the
display plane, so in the common case (one fullscreen app) the compositor
copies nothing and the CPU is idle.

- Composition is mostly one copy: the visible app's buffer is copied into
  the scanout buffer, damage-tracked, so an app that updates part of its
  window costs only that part. With the overlay up the app is not drawn at
  all (the overlay is opaque), so there is no blending; blending over a
  write-combined scanout buffer would read it back, which is slow on the
  A133 (see the notes in pkgs/m8c and pkgs/puzzlescript).
- The visible app's wl_shm buffer is copied (damaged rectangles only)
  into an XRGB memory buffer and drawn with SRC. SDL apps on llvmpipe
  commit ARGB buffers with junk alpha while declaring the surface opaque;
  drawn as ARGB, the damage tracker skips clearing under the "opaque"
  region and pixman blends, so stale swapchain contents showed through
  (first device test: garbled M8 over the home screen, flashing as the
  swapchain rotated) and blending read back write-combined memory (slow).
  `MORTAR_SHELL_NO_OPAQUE_COPY=1` restores the generic path. Apps with
  subsurfaces or popups, and dmabuf clients, use the generic path.
- wp_viewporter is supported (SDL3 sizes fullscreen windows with it).
- Output: DRM/KMS through Smithay's `DrmCompositor`, swapchain buffers from
  GBM (Mesa's kms_swrast on sun4i, i.e. dumb buffers that pixman maps). If
  libgbm cannot open the card (or `MORTAR_SHELL_ALLOCATOR=dumb`), plain KMS
  dumb buffers are used instead.
- Direct scanout: when the only element is a client dmabuf that covers the
  screen, `DrmCompositor` puts it on the primary plane with no copy.
  Smithay's own exporters do not do that here (the GBM one only imports
  client buffers when given an import node, the dumb one never), so
  `comp/scanout.rs` (`ClientScanout`) wraps either exporter and
  PRIME-imports single-plane linear XRGB/ARGB (XBGR/ABGR) client dmabufs
  into sun4i and AddFB2s them; Smithay caches the framebuffer per buffer.
  Anything else is composited. `ctl state` reports `scanout_frames`, the
  log says when scanout starts and stops, `MORTAR_SHELL_NO_SCANOUT=1`
  disables it.
- Frame pacing: the visible app gets frame callbacks after each presented
  frame (vblank). Background apps get callbacks at `backgroundFps`
  (default 2) so SDL render loops keep ticking slowly instead of
  stalling; their audio threads are unaffected.
- The element list (`backend::elements`) is generic over the Smithay
  renderer (`Renderer + ImportAll + ImportMem`). That is the seam for a GPU
  renderer, should one be needed (see below).

### GPU apps

Measured before: m8c in the shell on llvmpipe ~180% CPU; the same m8c from
the greeter on the PowerVR (sdl3-pvr's vivante path, presenting through
brick_dc_drm) 13%. The vivante path cannot run in the shell: brick_dc_drm
drives the CRTC itself (as an in-kernel DRM client its commits get -EBUSY
while the shell is DRM master, so the app would render to nowhere), and
only one process can own it.

The constraints: the stock DDK 1.19 userspace (GLES 3.2) has no GBM and no
Wayland or DRM WSEGL, only the NULL one. It does have
`EGL_EXT_image_dma_buf_import(_modifiers)`, `EGL_KHR_fence_sync` and
`EGL_KHR_surfaceless_context`, and pkgs/mortar-gpu-tests' `kms-gles
import` showed it rendering into sun4i dumb buffers imported as EGLImages
(FBO target, 1.5 ms/frame, 60 fps on page flips). The display has no
IOMMU, so a scanout buffer must be CMA (contiguous); sun4i dumb buffers
are.

Options:

1. **Client side: a Wayland EGL platform for the stock EGL (chosen).**
   `pkgs/mortar-wlegl`: a library that sits in front of the stock
   libEGL/libGLESv2 (SDL loads it as both, via `SDL_EGL_LIBRARY` /
   `SDL_OPENGL_LIBRARY`; whatever it does not wrap falls through to the
   stock libs it links). `eglGetPlatformDisplay(WAYLAND)` returns the stock
   default display, `eglCreateWindowSurface(wl_egl_window)` our surface:
   up to 4 sun4i dumb buffers (PRIME-exported, imported once as EGLImages
   and as `zwp_linux_dmabuf_v1` wl_buffers, XRGB8888 linear), the context
   current surfaceless, "framebuffer 0" redirected to an FBO
   (`glBindFramebuffer(0)`, `glGetIntegerv(FRAMEBUFFER_BINDING)`,
   `GL_BACK` in DrawBuffers/ReadBuffer/Invalidate are wrapped). Swap:
   discard depth/stencil, `glFinish` (no implicit dma-buf fencing in this
   DDK config), attach + damage + commit, take a buffer the compositor
   released. SDL's own Wayland backend works unchanged on top. Apps that
   are not SDL work too (any EGL Wayland client that loads this library).
   Several GPU apps can run at once (no DC involved). Zero copies when
   the app renders top-down (below); the shell scans the buffer out.
2. **Teach sdl3-pvr a Wayland mode** that renders into dma-buf EGLImages
   itself. Same data path as 1, but SDL-only and a bigger SDL patch; 1
   keeps SDL's Wayland backend as is, and gets the SDL-specific part down
   to a few lines in the GLES2 renderer (orientation, below).
3. **Compositor side: a GLES renderer for Smithay on the stock EGL**
   (surfaceless display, dmabuf EGLImages, render into the scanout
   buffers). It would make composited frames (OSD over an app, shm apps)
   cheaper, but it does not by itself give apps a GPU path, and with
   direct scanout the fullscreen GPU app needs no composition at all. It
   would also put the shell, which must never die, on a GPU whose faults
   latch it until reboot. Not done; mortar-wlegl already answers
   `EGL_PLATFORM_SURFACELESS_MESA`, the first thing it would need.
4. **Let apps keep presenting through brick_dc_drm** and have the shell
   scan out the DC's buffers. Rejected: the DC drives the CRTC, it is one
   process at a time, and those apps would not be Wayland clients (no
   focus, frame callbacks or input routing).

Orientation. GL's window origin is bottom-left, but an FBO texture keeps
GL row 0 in memory row 0: rendered naively, the dma-buf is upside down for
the display. mortar-wlegl has three modes, best first:

- **topdown**: the app draws the window top row first, like a render
  target. Negotiated per context: `glGetString(GL_EXTENSIONS)` lists
  `GL_MORTAR_window_topdown` and the app calls `glWindowTopDownMORTAR(GL_TRUE)`.
  sdl3-pvr's GLES2 renderer does (`pkgs/sdl3-pvr/wayland-pvr.patch`: SDL
  already draws render targets that way, so it is the target branch of
  viewport, scissor and projection, and no row flip in ReadPixels). The
  FBO renders into the dma-buf, which goes to the plane: no copy at all.
- **blit** (default for other apps): the app renders into a private
  renderbuffer, blitted Y-flipped into the dma-buf at swap
  (`glBlitFramebuffer`). One GPU copy (~3 MB), no CPU.
- **transform** (GLES 2 contexts without blit): render into the dma-buf
  and set `wl_surface.set_buffer_transform(FLIPPED_180)`; Smithay then
  composites it (the sun4i planes have no reflect property).

`MORTAR_WLEGL_MODE` forces one, `MORTAR_WLEGL_DEBUG=1` logs.

Wiring: `mortar-sdl-run` (modules/brick-gpu.nix) has a `MORTAR_SDL=wayland-gpu`
mode (sdl3-pvr's `envWayland`: SDL's Wayland backend, mortar-wlegl as EGL
and GLES library, GLES2 renderer, `SDL_MORTAR_GL_DEFAULT_PROFILE=es2` so
m8c's early `SDL_WINDOW_OPENGL` window gets a GLES config). The shell
module sets it for every app when `mortar.shell.gpuApps` (default: when
the GPU is enabled), else `software`. An app that fails within 5 s is run
again on Mesa llvmpipe, as before.

What is on screen when:

| Screen | Path |
|---|---|
| GPU app alone | its dma-buf on the primary plane; the shell only flips |
| MENU overlay / home | the opaque overlay, drawn by pixman into the swapchain; the app is not drawn; it is `suspended` and gets background-rate frame callbacks |
| volume OSD or OSK over a GPU app | composited: pixman reads the app's dma-buf (write-combined, so slower than an shm copy) for those frames only |
| shm app (foot, llvmpipe SDL) | opaque copy, as above |

The OSD could go on an overlay plane over the scanned-out app instead
(sun4i exposes UI overlay planes), but Smithay 0.7 only puts elements
with a Wayland buffer on planes, not memory buffers; the OSD would have to
be a dmabuf-backed element of our own. Not worth it for a 1.5 s toast.

Focus switching needs nothing special: a background GPU app keeps
rendering at the background frame rate (SDL's 50 ms swap timeout) or
idles if it honours `suspended` (m8c does); the shell holds only its
latest buffer. Switching back puts that buffer on the plane at once.

Costs and limits:

- CMA: each GPU app holds 1-4 dumb buffers of 3 MB (allocated as the
  compositor holds them: on screen, queued, rendering), on top of the
  shell's swapchain, brick_dc_drm's 3 buffers and the fbdev emulation.
- The GPU latches until reboot after a fault; apps then fail at
  eglInitialize/surface creation and mortar-sdl-run falls back to
  software GL. The shell itself never touches the GPU.
- eglInitialize on the stock libs needs a registered DC device, so
  brick_dc_drm stays loaded; mortar-wlegl never creates DC window
  surfaces.
- One wl_display per process; a surface current in one context at a time;
  no MSAA window configs.

## Memory budget

| Part | Estimate |
|---|---|
| mortar-shell RSS | 58 MB measured in the headless test with two apps, of which ~12 MB are the clients' mapped shm buffers and ~6 MB the screenshot renderer; the rest: UI canvas, overlay and OSK buffers (~8 MB), their pixman textures, fonts and glyph caches, Smithay state. Expect ~40-60 MB on the device. |
| Scanout | 2-3 swapchain buffers x 3 MB (CMA/dumb) |
| Per shm app | 2-3 client buffers x 3 MB, shared with the client |
| SDL app on llvmpipe | ~160 MB RSS, mostly shared libraries (pkgs/mortar-apps notes) |
| SDL app on the GPU (mortar-wlegl) | ~40-45 MB RSS (43 MB measured for m8c on the DC path) + 1-4 x 3 MB CMA dma-bufs |
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

- Background CPU: SDL3's Wayland swap waits at most 50 ms for a frame
  callback, so a background SDL app still renders at ~20 fps whatever the
  shell sends (measured: m8c ~100% CPU in the background with 0, 2, 10
  or 30 callbacks/s; ~180% in front at 30 fps, same as on KMSDRM). The
  shell sends `xdg_toplevel` `suspended` to background windows, which
  SDL3 reports as SDL_EVENT_WINDOW_OCCLUDED; apps that idle on it (a small
  m8c patch: skip rendering while occluded) would drop to near zero.
- Suspend: does SDL/m8c audio survive `systemctl suspend`?
- Does Mesa llvmpipe pick wl_shm or dmabuf here? (No render node, so
  presumably wl_shm.) Measure the per-frame copy at 60 fps.
- Is `EVIOCGRAB` respected for the gpio-keys pad when another process
  (an app started outside the shell) already has it open? Grab only
  affects later reads, so it should be.
- Power key: logind suspends; should a short press open the shell's
  Settings instead?
- Settings screens (Wi-Fi, SSH) are still the mockups' future work; the
  quick settings link to them with a toast.
