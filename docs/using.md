# Using Mortar

This describes the full configuration (`brick-full`) as its owner uses it. A bare
system has the shell, greeter, setup, Wi-Fi, Bluetooth and battery parts, and none
of the apps.

## What works on the full configuration

| Area | State |
|---|---|
| Boot | U-Boot SPL → TF-A → U-Boot → `extlinux.conf` → Linux 7.3 → NixOS. All 4 cores. |
| Shell | `mortar-shell`, a Wayland compositor (Rust, Smithay). Apps run fullscreen side by side; **MENU** opens your running apps as live full-screen windows; ▶ brings the cross menu: launchers by category, settings (Wi-Fi, Bluetooth, a per-app mixer) and power, over a quiet animated pattern per category. Its UI is drawn on the GPU with spring animations, synthesized UI sounds and haptic ticks. |
| GPU | PowerVR GE8300 on TrimUI's stock DDK 1.19 userspace. GPU apps inside the shell render into dma-bufs (`mortar-wlegl`) that the shell scans out directly; outside the shell, `brick_dc_drm` flips them at 60 fps. A busy M8 screen costs ~12% CPU. |
| Greeter | `mortar-ui` greeter (greetd): pick **Mortar** (the shell) or launch an app directly. |
| Apps | M8 (m8c; plug the M8 into the top port), ChooChooTracker, Little XM Tracker, PICO-8 (your own licensed copy), PuzzleScript, Games (emulators), Terminal. Claude Code is an optional pack for your own flake. |
| Emulators | `mortar-play`, a minimal libretro frontend with curated, preconfigured systems (GB/GBC/GBA, NES, PS1, PC Engine, …). SNES, Mega Drive, Master System, Game Gear and Arcade use cores with non-commercial licences (see `LICENSE`); `mortar.play.nonCommercialCores = false` turns them off. |
| Claude Code (`nixosModules.pack-ai`, optional) | Runs in a persistent tmux session, a controller UI for approving tool calls (A/B/X), and Remote Control from your phone. Sign in with a QR code. |
| Audio | PipeWire in the session: codec speaker and headphones, USB audio and USB MIDI on the top port, Bluetooth headphones. 5.8 ms quantum for the trackers. |
| USB-C, bottom port | USB device: Ethernet over USB, for pairing and SSH with a key (docs/pairing.md). The Brick is `10.55.0.1` / `mortar.local`. A watchdog recovers the link if it goes deaf. |
| USB-C, top port | USB host: the M8, USB audio, keyboards and controllers. |
| Accounts | The panel's sessions (shell, apps) run as the user `brick`; root is for SSH (deploys) and the UART pads. Time zone, power and backlight go through polkit/udev, Wi-Fi and Bluetooth through `mortar-setupd`. |
| SSH | Keys only, from `ssh-keys/root` (and optionally `ssh-keys/brick`) on the card's FAT boot partition, or added with `mortar pair`. No account has a password. |
| Setup | First-boot setup: a `mortar.toml` file on the card, setup screens on the panel (Wi-Fi list, on-screen keyboard, WPS), and `mortar pair` from your computer (docs/setup-file.md, docs/pairing.md). |
| Wi-Fi, Bluetooth | XR829: 2.4 GHz station mode via iwd; BlueZ with pairing in Settings > Bluetooth (headphones over A2DP, controllers forwarded to apps like the built-in pad). |
| Display | JD9168 DSI panel, 1024×768. |
| Updates | Over Wi-Fi: CI builds each push and publishes it to a binary cache; the Brick downloads, stages and asks you to restart (docs/updates.md). Or `nix run .#mortar -- deploy` over USB. Every generation stays in the boot menu. |
| Battery | Charger set up like stock (4.2 V, 960 mA, 128 mA termination, NTC/JEITA windows); the AXP2202 fuel gauge runs TrimUI's battery model. Charging/full shown in the status bars; warnings at 15 % and 5 %; clean power-off at 3 %. USB input: 500 mA from a computer, 2.5 A from a charger. `mortar-power status`. |
| Debug | Each boot saves logs to `mortar-logs/` on the card's FAT partition. |

If a new system won't boot, hold SELECT while powering on, until a second buzz: the previous system starts, once (docs/updates.md).

Tap the power key to sleep: the screen goes dark and the Brick suspends (s2idle) after 3 s; the power key or any gamepad button wakes it. Settings can also sleep it when idle; hold it for Sleep / Restart / Power off. Set the time zone in MENU > Settings > Time zone.

Not yet: deep suspend (it resumes with a fuzzy panel, see devices/brick/hardware.nix), LED effects.

## Using it

The Brick boots straight into the shell (as `brick`); the greeter appears
after Log out (Settings › Power) or if the shell keeps failing. Home is a
quiet full-screen dashboard: a big clock and the date, a small mosaic by the hour (Settings › Display › Home pattern), and only
what has something to say: **Needs you** (apps waiting for you, e.g. a
Claude approval), what is playing, **Recently closed** (A starts it again);
Wi-Fi, storage and scripts can be added (`mortar.shell.home.widgets`).
Your running apps show as a slim strip at the bottom: ▼ turns home into
the app switcher, a vertical stack of live windows (most recent first)
under the dashboard, now a card. Apps start from the cross (▶). Every
button and chord is listed in Settings › About › Controls.

| Button | Does |
|---|---|
| D-pad, A | Move, open |
| B | Back one step; at the top it closes the menu (at home: back to the dashboard) |
| MENU | Opens the stack over the app (the previous app selected); closes everything, from anywhere; at home: back to the dashboard |
| MENU, then A | Flip back to the previous app |
| In the stack: ▲▼, A, X, Y | Scroll, switch to it, close it, options (switch, restart, close) |
| In the stack, on the dashboard: A | With an app in front: go home (it keeps running). At home: into the tiles (D-pad between them, A opens, B leaves) |
| In the stack, on a game: Y | Game menu: save or load a state (◀▶ picks one of four slots, with pictures), reset, fast-forward, change disc, quit |
| In the stack: ▶ (R1) | The cross: Apps, Games, Music, Settings (R1: straight to Settings); ◀ from Apps or L1 goes back |
| In the cross: ◀▶, ▲▼, A | Category, item, open (Apps holds the tools; games and music have their own) |
| Games | Continue, recently played, one row per system (A: its games, L1/R1 page, A plays), PICO-8, PuzzleScript, All systems. Every game is its own window in the stack; a game you have not used for a while sleeps (saved) and wakes where it was |
| Settings | Brightness and volume (A, then ◀▶, A when done), then pages: Display (palette, motion, screensaver), Sound (Mixer, UI sounds, vibration), Wi-Fi, Bluetooth, Time zone, Remote access, Updates, About (version, storage, battery, Controls), Power (sleep, restart, power off, log out: press A twice). B or ◀ goes back a page |
| Settings › Wi-Fi | On/off, the network you are on (details, forget), networks nearby (A joins; a password keyboard for new secured ones) |
| Settings › Bluetooth | On/off, your devices (A connects or disconnects, X forgets), Pair new device |
| Mixer (Music or Settings › Sound) | A fader per app, Master and UI sounds: ◀▶ channel, ▲▼ level, A mute |
| VOL+ / VOL− | Volume (the shell owns these keys) |
| VOL+ and VOL− together | Screenshot, saved to `~/Pictures/Screenshots` |
| SELECT + MENU (or F1) | In a game: its own menu (save/load state, reset, quit) |
| START | In Terminal: on-screen keyboard |

Copy games over SSH, e.g. `scp game.gba brick:/var/lib/mortar-play/roms/gba/`
(PS1 needs a BIOS such as `scph5501.bin` in `/var/lib/mortar-play/bios/`;
CHD is the best format for discs). Run `mortar attention` on your computer
to make the Brick buzz. Scripts on the Brick can ask for you with
`mortar-shell attention set --app NAME "text"` (and `clear`): it shows
under Needs you.
