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
  src/shellui.rs     the shell UI: boot, transitions, OSD, toasts, app
                     states (pure state + scene building, unit-tested)
  src/home.rs        Home: the dashboard, its navigation, first-time tips
                     (a child module of shellui)
  src/cross.rs       the MENU: Running page, the cross, glass (a child
                     module of shellui)
  src/menupages.rs   the MENU's glass pages (a stack): Display, Sound, Mixer,
                     Wi-Fi, Bluetooth, Time zone, Updates, About, Controls, Power
  src/gamepages.rs   the MENU's games: the Games category, a system's page,
                     the game panel (a child module of shellui)
  src/games.rs       games as apps: library data, titles, the game socket
                     protocol, the hibernation policy (pure, unit-tested)
  src/widgets.rs     Home widgets: config, the 4 x 3 grid, command-widget worker
  src/attention.rs   "Needs you": apps asking for attention
  src/about.rs       version, generation, storage, battery health, temperature
  src/patterns.rs    the cross's background patterns (mosaics: zig-zags, Life, a scope, Truchet tiles)
  src/mixer.rs       PipeWire output streams from pw-dump (the Mixer)
  src/radio.rs       Wi-Fi / Bluetooth on/off (rfkill)
  src/motion.rs      springs and the Bouncy/Calm/Snappy presets
  src/scene.rs       display list (shapes, text, images) both renderers draw
  src/raster.rs      CPU renderer of scenes (tiny-skia)
  src/art.rs         palette + isometric app illustrations (vector)
  src/sfx.rs         UI sound synth + ALSA player thread
  src/haptics.rs     rumble ticks (gpio-vibrator FF device), thread
  src/osk.rs         on-screen keyboard + pad-as-keyboard mapping
  src/osk_suggest.rs suggestion chips above the OSK (zsh completions)
  zsh/mortar-osk.zsh zsh's side of the chips
  src/preview.rs     render shell screens / transition frames to PNG
  src/comp/          Linux only
    state.rs         Smithay state + handlers, focus, input dispatch
    backend.rs       element list (renderer-generic), headless backend, screenshots
    drm.rs           libseat session + DRM/KMS output
    input.rs         evdev devices via the session, classification, grabs
    pads.rs          per-app uinput gamepads
    ctl.rs           control socket
    bg.rs            MENU page backends off the loop (mortar-setupd, pw-dump/wpctl)
    gamectl.rs       games off the loop: library and pictures, game sockets,
                     hibernation, the save before suspend
    suggest.rs       OSK suggestion socket (zsh)
    scanout.rs       direct scanout of client dmabufs (PRIME import)
    ui.rs            the compositor side of the UI: view, actions, CPU
                     images, screenshots, toasts
    gl.rs            GLES 2 renderer of scenes on the stock PowerVR libs
    gpu.rs           GPU UI presenter: dumb-buffer targets, app textures
  test.nix           headless integration test (screenshots, transition frames)
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
  singleInstance, art, category (apps | games | music)`. Launching sets `WAYLAND_DISPLAY`,
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
  list (its newest window). MENU opens with the previous app selected, so
  MENU, A flips between two apps.
- Close: `xdg_toplevel.close`, then SIGTERM to the process group after 3 s
  and SIGKILL after 6 s. Exited apps with no window are dropped and the
  next app in MRU order is shown, or the home screen.
- Log out (MENU > Settings > Power) ends the shell (status 0); greetd
  returns to the greeter and all apps get SIGTERM.

## Games (mortar-play)

Every game is its own app (pkgs/mortar-play, `mortar.shell.games`, set by
modules/mortar-play.nix), so games sit in Running next to the M8 and the
terminal, each titled with its cleaned name and system ("Pocket King ·
GBC").

- **Launching.** Besides the static launcher entries the UI can ask for
  `Out::LaunchGame(GameRef)`: the compositor makes an entry on the fly
  (`games.run` + SYSTEM + ROM, i.e. `mortar-play run`, gamepad `forward`,
  the Games illustration and accent) and launches it like any other. Its id
  is `game:SYSTEM:PATH`, so single-instance works as for entries: starting
  a game that runs switches to it (zooming from the row), one that sleeps
  is woken. The app keeps the entry it was started from (`App::spec`), which
  waking uses. `ctl game SYSTEM [ROM]` does the same over the socket.
- **Library.** MENU > Games: Continue (the last game, its resume picture
  beside the row), up to five more recently played games, one row per
  system with games (the count), a no-content core such as 2048 as a row
  that starts it, then the other game entries (PICO-8, PuzzleScript) and
  "All systems" (the `games.library` entry: mortar-play's own library, as
  before). A on a system opens a glass page: its games, cleaned names, the
  region on the right, a RESUME tag where a resume state exists, the
  selected game's picture right of the list, 8 a page (L1/R1 page), A
  plays. The data is `mortar-play list --json` (mortar-play's own
  library.rs: the same scan, names, cue/m3u hiding and recents), run by the
  `games-lib` thread at most every 2 s while the MENU shows and only when
  one of the folders it names (`watch`: ROM folders and their
  sub-folders, state folders, the recents file's folder) has a new
  modification time; the event loop never scans or decodes. Pictures (the
  resume and slot thumbnails, 320 px PNGs) are decoded by the same thread
  when a screen wants them (`ShellUi::wanted_pics`) into `Src::Pic`
  images, at most 16 kept; the GPU renderer uploads each id once and frees
  it when no frame shows it.
- **Game panel.** Y on a game in Running (the hint bar says "Y Game menu")
  opens a glass panel over its window: Save state and Load state (◀▶ pick
  one of four slots; each shows its picture and age), Reset, Fast-forward
  (on/off, until switched off), Change disc (multi-disc games only), Quit.
  Load and Reset zoom into the game. Requests go to the game's control
  socket, `$XDG_RUNTIME_DIR/mortar-play-<pid>.sock` (one JSON line each
  way: `list_states`, `save_state`, `load_state`, `reset`, `fast_forward`,
  `change_disc`, `quit`), found by mapping the pid in its name to an app
  through pid ancestry, as windows are; a `games-ctl` thread sends them and
  the answers come back as toasts ("Saved to slot 2", "Disc 2 of 3") and a
  fresh slot list. SELECT+MENU still opens the game's own menu.
- **Hibernation.** Every 2 s with games open: if more than
  `mortar.shell.maxGames` (3) are awake, or MemAvailable is below
  `games.hibernateBelowMb` (180), the least recently used game that is
  neither in front nor starting (no window yet, or younger than 8 s) is put
  to sleep: the shell keeps its last frame (the buffer at half size, ~0.75
  MB) as a picture and asks it to `quit` quietly; it writes its `auto`
  state and exits. The app stays in Running (`App::asleep`; `alive()` keeps
  it) marked "asleep · saved" with that picture. A (or Wake in its panel)
  starts it again with the same uid and place: mortar-play resumes from
  `auto`, and the launch zoom shows the picture until the first frame, so
  the restart does not show. One game goes to sleep at a time; a process
  that does not quit gets the usual SIGTERM after 3 s.
- **Sleep safety.** Before `actions.sleep` the shell asks every awake game
  to save `auto` (in parallel, at most ~500 ms in all), so a battery that
  dies in suspend costs nothing.
- **Saved toast.** A game that quits with its state written (Quit, X in
  Running, its own menu, SIGTERM) sends `toast` to the shell's socket:
  "Saved · Pocket King · GBC". Hibernation quits quietly. X on a running
  game leaves the "Closed" toast to that.
- **Mixer.** A game's PipeWire stream belongs to its app by pid ancestry
  like any app's, so every game has its own channel.
- **Screenshots** (VOL+ and VOL−) with an app in front capture just the
  app's frame (and the keyboard strip): no toast or OSD over it.

## Screens

The look and motion follow the approved prototype (plan/mortar-motion.html):
Pond palette (day and night), Instrument Serif / Onest / Geist Mono, flat
blocks with 1 px ink outlines, isometric illustrations.

- Boot: four coloured blocks fly in and settle into a square (a tick and
  a haptic tick each), the serif wordmark rises with a chime, then the
  screen slides up (2.7 s; `ui.bootAnimation`).
- Home (no app in front, `home.rs`): a calm dashboard, not a launcher
  (apps start from the MENU). The status bar (time, "N running", NEEDS
  YOU, SSH, Wi-Fi, battery), then widgets on a 4 x 3 grid of Pond tiles
  (flat blocks, 1 px ink outlines; 221 x 186 px, 2x1 tiles 462 px wide;
  see "Home widgets" below): by default the big serif clock and date,
  Continue, Needs you, Now playing, battery, Wi-Fi and storage. The D-pad
  moves between the tiles that do something (the nearest one that way);
  the focused tile sits on a lavender offset block with a 2 px outline.
  A acts: Continue resumes the most recent running app (its window shows
  in the card; the app zooms out of the card), or, with nothing running,
  starts the last app on screen again (`last_app` in prefs); Needs you
  switches to the first app waiting; Now playing to the app making
  sound; a tile with `open` starts or switches to that entry. B does
  nothing (Home is the top). The drifting shapes are gone: Home is a
  still screen (no frames at rest); only the charging line under the
  date moves, while it charges.
- Launch: the app grows out of its MENU row (or Home's Continue card) in
  its accent colour (whoosh, double haptic) and fades in once its first
  buffer arrives; if it is slow, the accent fills the screen with the
  illustration and "STARTING…" until it maps (B: back, it keeps
  starting; 20 s timeout). Going Home is the reverse: the app shrinks
  into the Continue card (it is what Continue resumes).
- MENU (anywhere, on its own) opens the MENU, "Cross Menu 2"
  (`cross.rs` and `menupages.rs`, after the approved
  plan/mortar-cross-2.html), over the app, which stays on screen frosted:
  blurred (about 14 px) under a ground-colour veil at 62%, so its
  colours and layout show but its text does not read. Over home the home
  screen is frosted the same way (`Src::HomeBlur`: the home scene drawn
  once into the blur's quarter-size FBO when the MENU first needs it, the
  same gaussian rounds, kept while frames use it); a lightly veiled
  sharp home read through and clashed with the menu. The blur and veil
  fade in and out with the menu's open spring (Reduce motion: on and off
  at once). The power
  menu frosts the app the same way (with a lighter scrim); the glass
  pages are frosted themselves. MENU closes it, and everything opened
  from it, from anywhere.
  - Two pages side by side on one spring: Running, and the cross to its
    right. ▶ on Running slides it off to the left and the cross in from
    the right (on Apps; R1: on Settings); ◀ on Apps or L1 goes back.
  - Glass: the status bar (serif clock, SSH, Wi-Fi/USB, battery; no
    hairline, no category tabs) and the hint bar float over both pages
    as frosted glass, as do the category and item icons, the glass
    pages, the options popup and the window labels: the blurred backdrop
    (`Src::Blur`, cut to the pane in screen space) under its veil, a
    light ground at 40-62%, a light 1 px top edge. Selected things keep
    solid ink outlines. The veil adapts: over a dark backdrop it rises
    (62% to 90%, eased) until ink-soft text keeps 4.5:1.
  - Running: Home, then every running app, most recent first, each a
    live window edge to edge (1024 x 600: the app's middle rows; Home
    draws the real home screen), stacked vertically like a scrolling
    window manager (`y = 84 + d x 612`, a spring), the neighbours above
    and below showing at the top and bottom edges, dimmed (ground at
    35%), 3 px ink edges. The page eases from 104% to 100% as the MENU
    opens. The selected window has a strip in the app's accent down its
    left edge and, in its lower left, a label: the index on ink, the
    serif name and the app's state on glass (in the same words
    everywhere, `AppState`: on screen, in the background, starting,
    closing, asleep). An app waiting for you has a yellow NEEDS YOU badge
    with its text at the window's top right. MENU opens here with the previous app selected, so MENU, A
    flips between two apps. ▲▼ scroll, A switches (the app zooms from
    its window to the full screen while the MENU fades), X closes (its
    window goes at once, a "Closed" toast), Y opens Switch to it /
    Restart app / Close (Restart closes the app and starts its entry
    again once it has exited; apps nobody launched have no Restart). The
    selected window and its neighbours get `ui.cardFps` frame callbacks.
  - The cross: Apps, Games, Music, Settings across the middle (Games
    only with an entry in it). The selected category is lifted (up-left,
    scaled from 82%) in its accent (Apps yellow, Games lime, Music
    orange, Settings pink; Running lavender) with a 2 px ink outline and
    a hard ink shadow; the others are glass. The row scrolls so the
    selection stays in the second column. The selected item sits right
    under its category as a bold band in the accent, coming in from the
    left screen edge, with an ink edge and an offset ink shadow, its
    name in the serif italic; three items below continue down, the one
    above stays visible, dimmed, above the category row. Values (level
    bars and percentages, ›) line up in one right-hand column. Each
    category remembers its selection (across openings too). ◀▶ move a
    category, ▲▼ an item. (The mockup's large faint index number was
    dropped as clutter.)
  - Sizes: the mockup's 1024x768 stage scaled about 1.4x for the
    Brick's 3.2" panel (about 400 ppi, at arm's length): the selected
    title is serif 56 px, other titles 32 px with 20 px mono subtitles in
    ink-soft (over 4.5:1 on the frosted ground), category names 34 px
    over 116 px icons, the clock 48 px, hint labels 22 px with 38 px
    keycaps, the glass pages' rows 30 px; both bars are 72 px tall.
  - Apps: the tools, the launcher entries whose `category` is `apps`.
    Games, Music: the entries with that `category` (the module's
    `category` or `categories.<id>`, else guessed: M8 and trackers are
    music, PICO-8, PuzzleScript and emulators games; Games also lists
    the library, see "Games (mortar-play)"); games and music
    apps are not repeated under Apps. Music starts with the Mixer. A
    launches (zooming from the row's icon); a running single-instance
    entry switches to its app instead (its subtitle says its state). When
    something in Running waits for you, a yellow dot sits on the cross's
    left edge (the way to Running), level with the category icons.
  - Settings is the one place for settings: Brightness and Volume as
    quick rows (A enters an adjust mode: the bar turns yellow, ◀▶ change
    the level in steps of 10, through the volume thread; A or B leave it,
    ▲▼ leave it and move), Keyboard (only with a keyboard app in front:
    the on-screen keyboard), then a row per glass page: Display ›,
    Sound ›, Wi-Fi ›, Bluetooth ›, Time zone ›, Remote access ›
    (mortar-setup's page), Updates › (with mortar.update), About ›,
    Power ›. The old full-screen settings screen and its duplicate
    mortar-setup Wi-Fi/Bluetooth entries are gone (mortar-setup keeps
    those pages for first-boot setup; the Wi-Fi page's "Other network…"
    still opens its full list for WPS).
  - Glass pages (`menupages.rs`) slide in from the right between the
    bars, one over the other (a stack: Settings > Sound > Mixer,
    Settings > Time zone > Europe, About > Controls): a serif title, a
    mono status line, an icon block with a hard shadow, rows (the
    selected one a band in pink with its name in the serif; a row waiting
    for its second A turns orange). Rows are levels (A, then ◀▶ in steps
    of 10, A or B when done), choices (A or ▶: the next value, shown as
    "Day · Night" with the current one bold), links (A or ▶ opens the
    page), actions, facts (A: the whole line in a toast) and the Wi-Fi /
    Bluetooth rows. L1/R1 page through long lists. B or ◀ goes back one
    page.
    - Display: Brightness, Palette (Day/Night), Motion
      (Bouncy/Calm/Snappy), Reduce motion, Screensaver.
    - Sound: Volume, Mixer ›, UI sounds, UI sound level, Vibration.
    - Time zone: the regions, then a region's cities with their local
      time and offset (CURRENT marked); A on a city (or UTC) sets it and
      both pages slide away, back to Settings.
    - Updates: Check now / Restart to update (a second A), Auto-check
      daily.
    - About: Mortar (NixOS version and system generation), Storage (free
      of total on /), Battery (level, health, capacity against new,
      cycles, when the gauge reports them), Temperature (the hottest
      thermal zone), Device (host, kernel), Controls › (every button and
      chord, below). Read with the status bar (`about.rs`, every 2 s
      while a shell screen shows; file reads and a statvfs).
    - Power: Sleep, Sleep mode (Suspend / Screen off), Sleep when idle,
      Restart, Power off and Log out (a second A each).
    - Wi-Fi: an on/off switch (rfkill, radio.rs), the connected network
      (address, band, security, signal bars; A or Details: a toast with
      the details), Forget this network, the nearby networks (open or
      saved: A joins; secured: mortar-setup's password keyboard, then
      back here), Other network… (mortar-setup's full Wi-Fi page: WPS).
      Y scans. Opening the page scans.
    - Bluetooth: an on/off switch (rfkill, then BlueZ power), your
      devices (A connects or disconnects, X forgets), Pair new device (A
      starts or stops discovery; a passkey to compare shows here), the
      devices found (A pairs: mortar-setupd stops discovery, pairs,
      trusts and connects; its agent answers PIN and passkey requests).
    - Wi-Fi and Bluetooth go through mortar-setupd's control socket
      (`mortar_setup::control`: `status`, `wifi_scan`, `wifi_connect`,
      `wifi_forget`, `bt_power`, `bt_scan`, `bt_pair`, `bt_connect`,
      `bt_disconnect`, `bt_forget`); the root daemon talks to iwd and
      BlueZ over D-Bus (iwd only lets root and wheel in). A worker thread
      (comp/bg.rs) polls `status` every second while Settings or a page
      shows and sends the requests, so no socket round trip runs on the
      event loop.
    - Mixer (Settings > Sound, or Music): Master (the default sink: level
      through the volume thread, mute with `actions.volumeMute`), one
      fader per running app, UI sounds (the shell's own sound volume and
      on/off). ◀▶ pick a channel (◀ on Master goes back), ▲▼ ±5, A
      mutes. An app's channel is its PipeWire output
      streams: `pw-dump` (`actions.pwDump`) lists them, each belongs to
      the app whose pid is the stream client's or an ancestor (apps.rs),
      the level is the loudest stream's on wpctl's cubic scale, set with
      `wpctl set-volume` / `set-mute` (`actions.streamVolume`,
      `streamMute`). An app with no stream shows "no sound". A worker
      thread runs these, every second while the Mixer shows (every 4 s
      while Home shows, for Now playing: a stream whose node is
      `running` and not muted is playing). No level
      meters: PipeWire has no cheap way to read peaks without capturing
      every stream.
  - Behind the cross (not behind Running) runs an animated mosaic per
    category (`patterns.rs`) in a strip at the right edge (x 772 to the
    edge, full height, clear of the selected band and of every
    category's icon and name): solid tiles in full palette colours on a
    28 px grid with 4 px of grout, no hairlines, fades or translucency.
    Apps: zig-zags, fixed and perfectly regular paths two tiles thick
    (the same shape for every band, evenly spaced) in yellow, lavender
    and ink, of which only a window of 12 columns shows: it snakes along,
    its head tile growing in and its tail tile shrinking out a column
    every 0.2 s, each band out of phase so a few segments show at a time
    (alternate bands run the other way). Games: Conway's
    Game of Life on the grid (37 x 27, see below) stepped 4 times a second,
    each tile growing in and shrinking out over ~120 ms, newborn tiles
    yellow and older ones lime; it starts from an R-pentomino and two
    gliders, and a glider drops in when it dies out or repeats. Music:
    an oscilloscope as a mosaic, a Lissajous path (x at 3x, y at a
    slowly drifting ratio, its amplitude breathing) quantised to 20 px
    tiles: the newest stretch orange, the older lavender stepping off
    tile by tile, the head in ink (generative: no cheap way to read the
    audio level). Settings: Truchet tiles, the classic two-colour mosaic,
    56 px pink squares with lavender quarter discs at two opposite
    corners; a wave travels down the diagonal and turns each tile a
    quarter turn with an eased step, so the curves re-route. A category
    change pushes the old pattern out and the new one in, the way the
    categories move, on a spring; ▲▼ nudge it (the zig-zag windows hurry, one
    Life generation, the scope's next ratio, the Truchet wave pushed
    on). It runs only while the cross shows and nothing covers it;
    Reduce motion keeps it on a still frame. (A lava lamp came first, then
    thin-lined, faded patterns; the user preferred solid, chunky
    mosaics.) The patterns are laid out over the whole screen (37 grid
    columns from x -12, so the strip's edge falls on a tile boundary;
    Life is a 37 x 27 torus seeded in the strip, with a glider dropped
    into the strip when it has been empty for 3 s); the strip shows
    their right-hand end.
  - Screensaver: after a while with no input on the cross (Settings >
    Screensaver: Off, 30 s, 1 min (the default), 2 min, 5 min), its
    pattern grows from the strip over the whole screen in ~1.1 s while
    the menu, the bars and the list fade out; the scope's figure spreads
    as it grows. Any button (MENU too) shrinks it back, and that press
    does nothing else. Only the cross alone triggers it: not Running
    (live windows), a glass page or the options popup. It redraws at
    30 fps; after 3 minutes full screen it dims (ground at 35%) and drops
    to 20 fps. Reduce motion shows it as a still frame at once. The
    shell's idle sleep (`ui.idleMinutes`) counts input as before, so it
    still sleeps the screen on time. Full screen it is at most ~1,000
    solid quads a frame (Life and the scope fill a 37 x 27 grid).
  - The hint bar follows the selection, in the same words everywhere: ▲▼
    Choose, ◀▶ Category / Adjust / Channel, A Open / Switch / Adjust /
    Change / Done / Confirm (or the action: Sleep, Restart, Join, Pair),
    X Close app, Y Options, B Back (one level), MENU Close (everything).
  - Sounds: move (category), tick (item), tab (page), drawer / drawerUp
    (open, close, popup), open (go), close (kill), deny (nothing there),
    vol (values); haptic tick on moves, double on go/kill/confirm.
- Settings are saved in $XDG_STATE_HOME/mortar-shell/prefs.json over the
  module's defaults (`mortar.shell.ui.*`), with what the shell remembers
  for the person: the tips already shown and the last app.
- Home shortcuts: L1 or SELECT opens the MENU on Apps, R1 on Settings.
- Volume keys work everywhere: the OSD, a frosted-glass pill (72x300) at
  the right edge, fades and scales in (~120 ms), with a speaker (sun for
  brightness), an ink bar that springs to each new level and the number
  in mono; muted shows an empty bar, a crossed speaker and "off". It is
  the light ground at 42% over the blurred app (the MENU's `Src::Blur`,
  cut to the pill in spans; over home a flat 85% ground), with a light
  top edge and a faint outline. It hides 1.2 s after the last change,
  by the clock alone: `next_deadline` wakes the compositor for the hide
  and for the end of the fade, so no frames or spring positions are
  needed (it used to wait for its slide-out spring to pass 135 px). It
  keeps the compositor at full frame rate only while it moves. Each
  step is shown at once and plays a pitch-tracking blip; a volume
  thread (comp/ui.rs `VolumeWorker`) applies the newest level (absolute,
  `{step}` = "NN%") and reads back level and mute: `wpctl` set + get
  used to run on the event loop for every step and key repeat, stalling
  frames and input. With the MENU up the Settings rows show the level
  instead.
- Screenshot: VOL+ and VOL- together (the shell owns those keys, so no
  app or game ever sees the chord). Shutter flash, shutter sound, long
  buzz, PNG in `ui.screenshotDir` (~/Pictures/Screenshots), then a toast.
  `mortar-shell ctl shot` does the same remotely.
- Toasts: yellow block dropping from the top (info, or "achievement" with
  a star and the unlock jingle; achievements queue). Apps raise them with
  `mortar-shell toast [--achievement] [--sound NAME|none] [--secs S]
  TITLE [SUBTITLE]` (control socket `toast {json}`), e.g. mortar-play for
  RetroAchievements.

- Time zone (Settings > Time zone, glass pages): regions then cities (from
  the system tzdata's zone1970.tab; offsets and local times computed by a
  `mortar-shell tz-offsets` child process, since setting TZ in the
  threaded compositor is unsafe). A sets it with `timedatectl
  set-timezone` (`actions.timezoneSet`). The configuration carries no
  zone (`time.timeZone = null` in modules/mortar-base.nix: it would say
  where the Brick is), so NixOS leaves /etc/localtime to timedated and
  the choice survives deploys and reboots. The clock calls tzset() before
  localtime_r: with TZ unset glibc then re-reads /etc/localtime when it
  changed (verified on the device: a running process follows the change
  within a second); localtime_r alone never would.
- Power key (logind ignores it, see modules/mortar-shell.nix; the PMIC's
  own long-hold force-off is untouched): a tap sleeps, a tap or any
  button wakes; holding it shows a filling "hold for power menu" pill and
  at 0.8 s opens the power menu (Sleep, Restart, Power off; the last two
  need a second A, B backs out).
- Sleep (power tap, MENU > Settings > Power > Sleep, the power menu): the screen fades to
  black, then the backlight goes off (bl_power), nothing is redrawn, UI
  sounds and haptics are muted, and the front app is `suspended` like a
  background app (SDL apps see OCCLUDED and can idle). Apps keep running:
  audio from the M8 (m8c keeps its capture running) continues. Waking
  turns the backlight on first, then fades back in; the waking press does
  nothing else.
- Suspend (Settings > Power > Sleep mode = Suspend, the default on the Brick; module
  default `ui.suspend`): once the screen has been dark for
  `ui.suspendGraceMs` (3 s, so a second tap just wakes the screen) the
  shell runs `actions.sleep` (`systemctl suspend`: s2idle by default,
  where any button wakes; SCP deep sleep is optional and wakes on the
  power key only, see modules/trimui-brick.nix). The shell notices the resume as a jump of
  CLOCK_BOOTTIME against CLOCK_MONOTONIC (deep), or, while dark, a new
  count in /sys/power/suspend_stats/success (s2idle keeps MONOTONIC
  running), wakes the screen (also after an RTC or other non-key wake), swallows the waking power press (its
  release would otherwise sleep again), redraws and sends every app a
  frame callback. It stays DRM master throughout; the kernel restores
  the last committed state. Apps are frozen while suspended: m8c and
  ChooChoo audio stops, USB devices (the M8) may re-enumerate.
- Settings > Power > Sleep when idle (Off, 1-30 min; module default
  `ui.idleMinutes` = 0): no button or key input for that long sleeps
  like a power tap (and then suspends with Sleep = Suspend).

### Navigation: B and MENU

One rule everywhere: B goes back one level, and closes the MENU only at
its top (Running or the cross); MENU closes everything at once (pages,
popup, a setup page, the MENU). ◀ on a glass page goes back too, except
while a level is being adjusted (◀▶ adjust it then) and in the Mixer
(◀ picks the channel to the left; on Master it goes back). A unit test
(`b_and_menu_on_every_screen`) checks this table:

| Screen | B | MENU |
|---|---|---|
| Home | nothing (the top) | opens the MENU on Running |
| Running | closes the MENU | closes the MENU |
| The cross | closes the MENU | closes the MENU |
| The cross, adjusting a level | stops adjusting | closes the MENU |
| Options popup (Y) | back to Running | closes the MENU |
| A glass page (Display, Sound, Wi-Fi, Bluetooth, Updates, About, Power, Time zone) | back to the cross | closes the MENU |
| A page adjusting a level | stops adjusting | closes the MENU |
| A page waiting for its second A | back to the cross (forgets it) | closes the MENU |
| Sound > Mixer, Time zone > cities, About > Controls | back to the page under it | closes the MENU |
| mortar-setup page (Remote access, Wi-Fi's Other network, password keyboard) | its own back; at its first page, back to the MENU | closes everything |
| Power menu (hold the power key) | closes it | closes it |
| Power menu, waiting for its second A | forgets it | closes it |
| An app starting (the splash) | back home, it keeps starting | opens the MENU |
| On-screen keyboard | Backspace (it is a keyboard; START hides it) | opens the MENU over the app |
| Games > a system's page (mortar-play) | back to Games (◀ too) | closes the MENU |
| Game panel (Y on a game in Running) | back to Running (Y too) | closes the MENU |
| Screensaver | brings the cross back | brings the cross back |
| Asleep | wakes (nothing else) | wakes (nothing else) |

### App states

An app's state reads the same everywhere (Running's window label,
the cross's subtitles, Home's Continue card): on screen, in the
background, starting, closing, asleep (`AppState` in shellui.rs; asleep:
kept but not running, e.g. a hibernated game).

### Home widgets

`mortar.shell.home.widgets` (config `home.widgets`, `widgets.rs`) lists
the dashboard's tiles in reading order; each takes a 1x1 or 2x1 slot on
a 4 x 3 grid, placed first-fit (a 2x1 that does not fit at the end of a
row goes to the next one and a later 1x1 fills the gap; what does not
fit is left out). Kinds:

| kind | default size | shows | A |
|---|---|---|---|
| `clock` | 2x1 | the big serif clock, the date and host, the charging line | — |
| `continue` | 2x1 | the most recent app (its window, name, state), else the last app | resume / start it |
| `needs-you` | 2x1 | apps waiting for you (yellow), else "All quiet" | switch to the first |
| `now-playing` | 2x1 | the app making sound (orange) and its level | switch to it |
| `battery` | 1x1 | level, bar, charging or health | — |
| `wifi` | 1x1 | on / off / offline, the network | — |
| `storage` | 1x1 | free space, a bar, of how much | — |
| `command` | 1x1 | a script's output | `open`, if set |

Every widget takes `size`, `title`, `accent` (lime, lavender, yellow,
orange, pink, ground) and `open` (a launcher entry A starts or switches
to). A `command` widget runs `command` every `interval` seconds (at
least 2) while Home shows, on the widgets thread (never on the event
loop), each run in its own process group and killed after 10 s. It may
print text (the first line large, the second small) or a JSON object
`{"title": "…", "value": "…", "accent": "lime"}`; a failure shows its
output (or "exit N") with a "!" tag. `ctl state` reports the results
(`widgets`). Example:

```nix
mortar.shell.home.widgets = [
  { kind = "clock"; }
  { kind = "continue"; }
  { kind = "needs-you"; }
  { kind = "now-playing"; }
  { kind = "battery"; }
  { kind = "command"; title = "Uptime"; interval = 60;
    command = "uptime -p | sed 's/^up //'"; }
  { kind = "command"; size = "2x1"; open = "terminal"; interval = 300;
    command = [ "${pkgs.writeShellScript "disk" ''
      printf '{"title":"Disk","value":"%s free","accent":"lime"}\n' \
        "$(df -h --output=avail / | tail -1 | tr -d ' ')"
    ''}" ]; }
];
```

### Needs you

Any app (or script) can ask for attention: `mortar-shell attention set
[--app ID] TEXT` and `attention clear [--app ID]` (ID defaults to
`$MORTAR_APP`, which the shell sets for every app it starts; control
socket `attention {"op": "set"|"clear", "app", "text"}`). Raising it
shows one toast ("Claude needs you" with the text, double haptic; a new
text for the same app updates it quietly), a NEEDS YOU badge on the
app's window in Running, a dot on the cross's edge towards Running, the
Home widget and a NEEDS YOU chip in Home's status bar. An item for a
launcher entry goes when that app stops; items for other ids stay until
cleared. mortar-ai (the Claude controller app) sets it while a tool
approval waits ("Approve: …", "+N more") and clears it when answered
(`pkgs/mortar-ai/src/attention.rs`, from a short-lived thread).

### First-time tips

A tip is a toast shown once ever, the first time its feature matters,
when nothing else is on screen (no toast, MENU, transition): "MENU
switches apps" (the first app on screen), "START opens the keyboard"
(the first keyboard app), "SELECT + MENU: the game's menu" (the first
game), "Every button, in one place" (back Home with two apps running;
it points at Settings > About > Controls). The ids shown are saved in
prefs (`tipsSeen`).

### Motion

Springs (motion.rs) integrate like the prototype (semi-implicit Euler in
1/240 s slices): Bouncy k=320 zeta=0.62 (default), Calm k=260 zeta=1,
Snappy k=620 zeta=0.78; Reduce motion uses k=420 zeta=1 and stops the
drift. UI time is real time, clamped to 50 ms a step; `ctl time manual`
and `ctl time +MS` step it by hand for test frames.

The compositor redraws every vblank while something moves (`Pace::Full`),
at 20 fps while only the background drifts, at 30 fps while only the
cross's pattern moves (`Pace::Pattern`; full rate during its slides),
and not at all at rest; with no UI layer the app is shown exactly as
before (direct scanout), and with the MENU closed nothing of the
pattern runs.

### Sounds and haptics

`sfx.rs` synthesises the prototype's SFX table (PolyBLEP square,
triangle, sine with exponential pitch sweeps; white noise through a
swept RBJ band-pass; WebAudio-style attack + exponential decay) at 44.1
kHz and mixes voices in a thread that writes 256-frame periods to ALSA
"default" (PipeWire's ALSA plugin; the codec's dmix without PipeWire). The PCM is opened on the first sound and
closed after 4 s of silence. `mortar-shell sfx NAME [--wav FILE]` plays
or renders one.

`haptics.rs` uploads one FF_RUMBLE effect to the `gpio-vibrator` device
and times pulses in a thread: tick 8 ms, double 2 x 18 ms (72 ms apart),
long 60 ms (`ui.hapticMs`). The motor is on/off, so length is the feel.
Pulses arriving while one plays are dropped.

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
- Volume keys are handled by the shell (`wpctl` on PipeWire's default
  sink, so they follow Bluetooth headphones; amixer on the codec's DAC
  without PipeWire; `actions.volumeSet/volumeGet`); the power
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
- A strip along its top shows the pad legend, or, in a terminal running
  zsh, suggestion chips (below).
- It slides up and down on a critically damped spring (~180 ms;
  `comp/osk_slide.rs`; Reduce motion: at once), on the pixman and GLES
  paths alike. The app is reconfigured at the start of either slide; until
  it commits at the new size its old buffer stays, and the strip below a
  keyboard app that is shorter than the screen is filled with its own
  bottom row (pixman: a solid element in its edge colour; scene/GLES: the
  last row stretched), never the black clear colour. That black band, for
  the frames between closing and the terminal redrawing at full height
  (and, on the scene path, while it shrank), was the flash. The headless
  test steps UI time through both slides and fails on a frame that is
  black over more than 1% or one flat colour.
- Later: a text-input-v3 aware mode that pops up only when a field is
  focused, and a symbols layer.

### Suggestion chips (zsh)

`src/osk_suggest.rs` (model, protocol, drawing), `src/comp/suggest.rs`
(socket), `zsh/mortar-osk.zsh` (installed in
`share/mortar-shell/`, sourced by /etc/zshrc from
modules/mortar-zsh.nix).

- zsh in a terminal the shell started (MORTAR_APP set) connects once to
  `$XDG_RUNTIME_DIR/mortar-osk.sock` (0600; peers of another uid are
  refused) and watches it with `zle -F`. The connection belongs to the
  app whose launched process is an ancestor of the peer pid (SO_PEERCRED),
  like windows do.
- While the OSK is up over that app, the shell sends `q SEQ` 140 ms
  after the last key the OSK typed (at once when it opens). The handler
  runs zsh's own completion for the word at the cursor with `compadd`
  captured (the fzf-tab technique; nothing is inserted or listed) and
  answers `{"seq":SEQ,"chips":[{"k","d","t","l"}]}`: kind, backspaces,
  text to type, label. The first chip is the zsh-autosuggestions
  suggestion (history) when there is one. Command names come shortest
  first, everything else in the completion's own order; at most 12.
- Only answers to the newest question from the app on screen are shown,
  so another terminal cannot put chips over this one. Closing the OSK
  sends nothing more: zsh does no work per keystroke while it is closed.
  Enter and Ctrl/Alt chords clear the chips at once.
- Pad: Up from the top row of keys enters the strip (the key cursor
  hides), ◀▶ move along it, A types the chip, ▼ goes back to the keys;
  R1 types the highlighted chip, or the first one (the history line,
  like → in fish) from the keys. B, X, Y, L1, R2, L2, START keep their
  keyboard meaning. Chips are typed as key taps through the same
  wl_keyboard path as the keys (printable ASCII only; others are
  dropped).
- Chips: square blocks with a kind tag on the left (history ↑ yellow,
  command › lavender, directory / orange, option - pink, file · ground);
  the history chip dims what is already typed. The highlighted chip is
  lime.
- Cost per answer, measured in the aarch64 build VM (an Apple core,
  roughly 6-8x an A53): paths 4-5 ms, options 6-8 ms, command names
  11-16 ms, git subcommands 15 ms, `git commit --am` 22 ms (it runs git);
  so about 30-180 ms on the Brick, once per pause, never per key. `MORTAR_OSK_SKIP` lists commands
  whose completion is too slow (nix ...): they get the history chip
  only. Inside tmux (Claude Code's session) zsh's parent is the tmux
  server, not the terminal, so there are no chips there.

## Rendering

### The shell UI

The UI builds a display list (`scene::Scene`: polygons, strokes, cached
concave shapes such as the Claude Spark, text runs, images of app buffers
and CPU layers, clip rectangles) each frame it animates. Two renderers
draw it:

- GPU (`comp/gl.rs`, `comp/gpu.rs`), default when `mortar.gpu.enable`:
  the stock PowerVR EGL/GLES 2 loaded with dlopen from `ui.glLibDir`; a
  16x16 pbuffer context (as kms-gles); two sun4i dumb buffers (CMA,
  3 MB each) imported with EGL_EXT_image_dma_buf_import as FBO targets
  (4x MSAA through EXT_multisampled_render_to_texture when present,
  `ui.msaa`); glFinish, then a page flip on the DrmCompositor's own
  surface (the first frame after start commits the mode). Shapes are
  fanned or ear-clipped triangles, text comes from a 2048x1024 A8 glyph
  atlas, app images are the clients' dma-bufs imported once per buffer
  (no copy) or wl_shm buffers uploaded on commit; the keyboard strip and
  setup pages are CPU-drawn layers uploaded when they change. The
  frosted backdrop (`Src::Blur`) is the app's texture drawn into a
  quarter-size FBO (a 4x4 box: four bilinear taps), then three rounds of
  a 9-tap gaussian (five bilinear taps, sigma 2) each way between two
  quarter-size FBOs (256x192, 192 KB each), and drawn back scaled up with
  linear filtering. It is redone only when the app's buffer changed, at
  most every 66 ms, and the FBOs are freed with the menu; a fully frosted
  frame skips the sharp app. The CPU renderer does the same with
  `raster::blur_quarter` (box blurs of matching variance). Glass panes
  draw the same blurred texture again, cut to the pane, so the backdrop
  is blurred once a frame however many panes there are; the backdrop's
  mean colour is read back from the blur (eight rows) for the adaptive
  veil. The cross's patterns are ordinary fills and strokes (a few
  hundred small quads a frame, no shaders of their own; tiles off the
  visible part are not emitted at all). Frames are redrawn only where
  they changed (`damage.rs`): the buffer drawn into still holds the frame
  before last, so each item's fingerprint (its geometry, colour, text and
  the clip over it) and covered box are compared with what that buffer
  got; the differing boxes mark 32 px cells, merged into at most 6
  rectangles, and each rectangle is cleared and drawn again, scissored,
  with only the items that touch it (exactly the clips of a full frame).
  On the cross that is the pattern tiles that moved, a few percent of the
  screen; more than 60 % changed, an image changed (an app commit, a
  blur, a layer or picture) or a buffer that never had a frame: the
  whole frame. Text layouts are cached by run. `ctl gpucheck` compares
  the next partial frame with the same frame drawn in full into a spare
  buffer (the result comes with the following `gpucheck`, and in the
  log); `MORTAR_SHELL_FULL_REDRAW=1` always draws everything. When no UI
  layer shows, the DrmCompositor takes over again (`reset_state`, full
  frame): direct scanout of a GPU app, the opaque copy of an shm app.
  Any EGL/GL error, a failed import of a render target or three failed
  flips drop the GPU path for the session after an orderly teardown
  (glFinish, delete, eglTerminate) and the CPU renderer takes over. No
  EGL window surfaces are ever created (the DC path that latched the GPU
  in early tests). Client buffers' textures are freed when the buffer is
  destroyed, so their CMA is not held.
- CPU (`raster.rs`, tiny-skia + fontdue): the headless backend, previews,
  screenshots, `ui.renderer = "cpu"` and the fallback. It draws full
  frames into a memory buffer that pixman composites; app images are CPU
  copies made once per commit (dma-bufs read through a mapping, so
  GPU-app redraws under the UI are held to 5/s).

`MORTAR_SHELL_RENDERER=cpu|gpu` and `MORTAR_SHELL_GL_MSAA=0|4` override
the config for one run; `ctl state` reports `ui.renderer`, `frames`,
`gpu_frames`, `scanout_frames` and `gpu_ms` (last and smoothed UI frame
time, the last blur's time, the number of blurs and `damage`, the
smoothed share of the screen redrawn).

### Apps

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
  A viewported dma-buf that covers the screen goes on the primary plane
  with the plane's scaler doing the scaling (the A133 mixer's UI and VI
  channels all have one): mortar-play sends PlayStation frames at their
  own size (e.g. 320x240) with a 1024x768 destination and the shell copies
  nothing. Under the GPU UI renderer the same buffer is sampled scaled
  (the app image uses the viewport's destination size).
- Output: DRM/KMS through Smithay's `DrmCompositor`, swapchain buffers are
  plain KMS dumb buffers that pixman maps. GBM (Mesa's kms_swrast on
  sun4i) gives the same dumb buffers but loads libgallium, LLVM and the
  Vulkan drivers first: 2.4 s at boot and ~40 MB resident. It is used with
  `MORTAR_SHELL_ALLOCATOR=gbm`, or if the dumb compositor does not start.
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
   latch it until reboot. Not done for app composition; the shell's own
   UI layers do use the GPU now (see "The shell UI" above: own GL code,
   not Smithay's GlesRenderer, with a CPU fallback on any error).
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
| MENU / home / transitions / OSD / toasts | GPU UI renderer into its own dumb buffers (app images sampled from their dma-bufs); the app under the MENU is drawn blurred under the veil (re-blurred on its commits, at most ~15/s) and `suspended`, the app in the live preview gets `ui.cardFps` frame callbacks. CPU fallback: full frames by tiny-skia, composited by pixman |
| OSK over a GPU app (nothing else) | GPU UI renderer (app texture + keyboard layer), full rate; CPU fallback: pixman reads the app's dma-buf (write-combined), held to 5/s |
| shm app (foot, llvmpipe SDL) | opaque copy, as above |

The OSD could go on an overlay plane over the scanned-out app instead
(sun4i exposes UI overlay planes), but Smithay 0.7 only puts elements
with a Wayland buffer on planes, not memory buffers; the OSD would have to
be a dmabuf-backed element of our own. Not worth it for a 1.5 s toast.

Focus switching needs nothing special: a background GPU app keeps
rendering at the background frame rate (SDL's 50 ms swap timeout) or
idles if it honours `suspended` (m8c does); the shell holds only its
latest buffer. Switching back puts that buffer on the plane at once.

Measured on the Brick (2026-11-01):

| Case | App CPU | Shell CPU | Path |
|---|---|---|---|
| mortar-wlegl-demo, topdown | 5% | 4% | scanout, 60 fps |
| mortar-wlegl-demo, blit | 8% | 5% | scanout, 60 fps |
| mortar-wlegl-demo, transform | - | 99% | composited, 3.5 fps (fallback only) |
| m8c (M8 attached), front | 12% | 3% | scanout (was ~180% on llvmpipe) |
| m8c under MENU | 5% | 0% | overlay only |
| m8c, volume OSD up | 9% | 38% | composited at 5 fps (87%/16 fps before throttling); measured before the GPU UI renderer, which now draws OSD frames (not re-measured) |
| m8c front, PuzzleScript + PICO-8 behind (all GPU) | 7% | 14% | scanout; background commits cost the shell |

PICO-8 (SDL2 through sdl2-compat on sdl3-pvr) takes the same path.
With the CPU UI renderer (fallback), composited frames of a dmabuf app
are held to 5/s while the OSD or OSK covers it: pixman reads the app's
write-combined buffer at ~50 MB/s. With the GPU UI renderer both go
through GLES at full rate.

Costs and limits:

- CMA: each GPU app holds 2-4 dumb buffers of 3 MB (allocated as the
  compositor holds them: on screen, queued, rendering; buffers idle for
  60 swaps are freed), on top of the shell's swapchain, brick_dc_drm's 3
  buffers and the fbdev emulation (9 MB each). The kernel default of 64 MB
  ran out with three GPU apps; modules/brick-gpu.nix sets `cma=128M`
  (`mortar.gpu.cmaMegabytes`).
- The GPU latches until reboot after a fault; apps then fail at
  eglInitialize/surface creation and mortar-sdl-run falls back to
  software GL. The shell's UI renderer then fails its EGL setup and draws
  on the CPU.
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
| GPU UI | 2 dumb buffers x 3 MB (CMA), glyph atlas 2 MB and app textures (GPU memory; dma-buf imports share the client's buffer) |
| Per shm app | 2-3 client buffers x 3 MB, shared with the client |
| SDL app on llvmpipe | ~160 MB RSS, mostly shared libraries (pkgs/mortar-apps notes) |
| SDL app on the GPU (mortar-wlegl) | ~40-45 MB RSS (43 MB measured for m8c on the DC path) + 1-4 x 3 MB CMA dma-bufs |
| foot | ~15-20 MB |

So M8 + terminal + shell is roughly 250-280 MB of the ~970 MB, leaving room for
Claude Code (Node, ~150-250 MB) with zram behind it. The headless test
records the shell's RSS (`rss_kib` in state.json).

## Control socket

`$XDG_RUNTIME_DIR/mortar-shell.sock` (0600), one line per connection:
`state` (JSON; `overlay`, `category` ("Running" on the Running page),
`menu_page` (the page in front: display, sound, mixer, wifi, bluetooth,
timezone, timezone-cities, updates, about, controls, power) and
`menu_pages` (the stack) say what of the MENU shows; `game_panel`,
`game_page`, `games` (library summary, panel states), per app `game` /
`asleep`; `attention`, `widgets` (the command widgets' results)),
`press|down|up BUTTON`, `key CODE`, `launch ID`, `focus ID`, `close ID`,
`game SYSTEM [ROM]`, `hibernate UID`, `save-games`, `home`, `screenshot
PATH`, `shot` (the screenshot combo), `toast JSON`, `attention JSON`,
`boot` (replay), `time manual|real|+MS`, `prefs`, `quit`. Tests drive the
shell through it; over SSH it gives remote screenshots and app control
(`mortar-shell ctl state` as root finds the session's socket in
/run/user/*/).

## Session and NixOS integration

`modules/mortar-shell.nix`, off by default:

- Adds the greeter session "Mortar" (order 0, so it is first), running
  `mortar-shell-session`: `mortar-shell run` under `systemd-cat -t
  mortar-shell`. greetd + pam_systemd give it a logind session on seat0,
  which libseat uses (no seatd needed).
- `autologin.user` (the Brick: `brick`, in flake.nix): greetd's
  `initial_session` starts the same session for that user at boot, once
  (greetd's runfile), so the Brick boots straight into the shell; the
  greeter (`default_session`) appears only when the session ends. The
  session script ends on status 0 (Log out, `ctl quit`); a crash
  restarts the shell, but two failures in a row that each came within
  30 s of starting end the session (logged as mortar-shell-session), so
  a broken shell falls back to the greeter instead of looping. SIGTERM
  (shutdown) stops it at once. `mortar deploy --reboot` only waits for
  SSH and compares /run/current-system, which none of this touches.
- `home.widgets`: the dashboard (above).
- Imports the other greeter sessions as launcher entries (minus console-only
  ones, `excludeGreeterSessions`, default `terminal`), adds a foot
  Terminal entry (keyboard mode; Geist Mono, Mortar colours) and
  `mortar.shell.apps`. `exclusive = true` leaves only the shell on the
  greeter.
- Enables uinput (kernel option + `hardware.uinput`), puts greeter profile
  users in `uinput`, `input`, `video`, installs the Mortar fonts for
  fontconfig, and the backlight udev rule.

## Tests

(Games: games.rs and gamepages.rs unit tests cover the library rows,
titles, launching and switching, paging, the panel's requests and the
hibernation policy; mortar-play's `tests.shell` drives the whole flow in
the shell, see pkgs/mortar-play/DESIGN.md.)

- Unit tests (`cargo test`, also on macOS): config parsing, pid ancestry,
  MRU, button routing (MENU chord, releases on retarget, keyboard mode),
  OSK layout and typing, the MENU (previous app, the Running page and
  its live windows, Running/cross navigation, categories and their
  memory, Apps without games and music, Settings adjust mode, the glass
  pages (Display, Sound, Mixer, Wi-Fi, Bluetooth, Time zone, Updates,
  About, Power with its second A), options popup, the B / MENU table for
  every screen), Home (Continue, Needs you, focus, last app), attention
  and its toast, first-time tips, widgets (config, grid layout, script
  output, the worker and its time limit), About's sysfs readers, the row above clearing the lifted category icon and
  its shadow by 16 px for every category and selection), the patterns
  (Life's rules, it never dies out; moving, nudged, sliding on a
  category change, still with Reduce motion, stopped with the MENU
  closed), the adaptive veil (4.5:1 for ink-soft over black, 62% over
  light, eased), pw-dump parsing, rfkill state, gamepad blacklist.
- `passthru.tests.headless`: the compositor on the headless backend in
  the build sandbox; weston-fullscreen (wl_shm) and foot started from the
  MENU with injected buttons; typing through keys and the OSK;
  switching back; closing from Running; zsh's suggestion chips (history
  first, choosing one, R1 typing it); a screenshot after every step;
  the dashboard with a command widget's JSON result; MENU > Settings and
  its Display, Sound > Mixer, Wi-Fi, Bluetooth, About > Controls and
  Power pages; `mortar-shell attention` set and clear (toast, Running
  badge, Home);
  then UI time frozen and stepped for transition frames (`t-*.png`:
  MENU, the cross sliding in, its pattern moving, launch, close, OSD,
  toast, shutter, settings, night palette,
  time zone, power menu, boot), the screenshot file and the saved
  settings checked.
- `mortar-shell preview --screen ... [--at MS --frames N --every MS]`
  renders screens and transition frames on any OS for UI work (demo
  apps). With `MORTAR_SHELL_BENCH=N` it times N frames of the screen
  instead (step, build, per-item work, the damage diff) and prints how
  much of the screen the GPU path would redraw.

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
- MENU > Settings Wi-Fi / Bluetooth / Mixer pages: untested on the
  Brick. Writing /dev/rfkill relies on systemd's `uaccess` tag for the
  seat's user; if the XR829 registers no rfkill switch the switch row
  does nothing. Joining a hidden network ("Other network…" in the
  mockup) has no backend yet; the row opens mortar-setup's Wi-Fi page
  (WPS, the full list). The Mixer's level meters are left out: PipeWire
  peaks would need a capture stream per app.
- The mockup's per-app subtitles ("Song · playing") need data apps do
  not give the shell; window labels say on screen / background /
  starting / closing instead.
