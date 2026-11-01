# Showcase video

A short video of Mortar made from the real shell: `capture.nix` runs
mortar-shell on its headless backend (as `pkgs/mortar-shell/test.nix` does)
with real clients, freezes UI time and steps it 1/30 s per frame while
pressing buttons over the control socket, and screenshots every frame.
`capture.nix { part = "apps"; }` does the same for the apps: PICO-8's Splore
(your own licensed copy, see pkgs/pico8), ChooChooTracker on Mesa's
software GL, and mortar-play running open homebrew (Libbet and the Magic
Floor, zlib; µCity, GPLv3+ / CC BY-SA 4.0). `compose.py` puts long takes of
the screen on a flat Pond ground, with a title at the start and the name at
the end. The only sound is the shell's own UI sounds, put on the frames
where the shell played them (from its debug log).

```sh
# frames, sounds, fonts (aarch64-linux: a Linux machine or the linux-builder)
nix build .#packages.aarch64-linux.showcase-capture -o capture
nix build .#packages.aarch64-linux.showcase-capture-apps -o capture-apps
# the video (macOS or Linux)
nix run .#showcase -- --capture capture --capture capture-apps --layout wide --out mortar.mp4
nix run .#showcase -- --capture capture --capture capture-apps --layout square --out mortar-square.mp4
```

`--stills DIR --at 30,200` also saves those frames as PNG.

Clips (`capture/clips/<name>/`): `NNNN.png` at 30 fps and `events.tsv`
(frame, `press` / `key` / `sfx` / `haptic` / `note`, value). The storyboard
(which frames of which clip, in order) is `BOARD` in compose.py.

Nothing private goes in: the shell runs in the build sandbox with the
hostname `mortar`, UTC, no Wi-Fi, and a scripted stand-in for `claude -p`.
Render outputs are not committed.
