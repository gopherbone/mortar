# Showcase video

A short video of Mortar made from the real shell. `capture.nix` runs
mortar-shell on its headless backend (as `pkgs/mortar-shell/test.nix` does)
with real clients: zsh in foot, the Claude controller (mortar-ai, against a
scripted stand-in for `claude -p`), ChooChooTracker and PICO-8's Splore
(your own licensed copy, see pkgs/pico8) on Mesa's software GL, and games
through mortar-play (its built-in test ROM and Libbet and the Magic Floor,
zlib). It freezes UI time and steps it 1/30 s per frame while pressing
buttons over the control socket, screenshotting every frame, as one
session filmed in long takes: boot onto the Home dashboard, the Running
page, the cross and its mosaics, Games and the game panel, Settings and the
Mixer, the terminal with the keyboard's suggestion chips, the screensaver.

`compose.py` puts the takes on a flat Pond ground, with a title at the
start and the name at the end. The only sound is the shell's own UI sounds,
put on the frames where the shell played them (from its debug log).

```sh
# frames, sounds, fonts (aarch64-linux: a Linux machine or the linux-builder)
nix build .#packages.aarch64-linux.showcase-capture -o capture
# the video (macOS or Linux)
nix run .#showcase -- --capture capture --layout wide --out mortar.mp4
nix run .#showcase -- --capture capture --layout square --out mortar-square.mp4
```

`--stills DIR --at 30,200` also saves those frames as PNG.

Clips (`capture/clips/<name>/`): `NNNN.png` at 30 fps and `events.tsv`
(frame, `press` / `key` / `sfx` / `haptic`, value). The storyboard is
`BOARD` in compose.py; the shot list is the `clip` sections of the script
in capture.nix.

Nothing private goes in: the sandbox's hostname is `mortar`, the clock is
faked to 10:30 UTC (libfaketime, the shell and its apps alike), there is no
Wi-Fi, and no ROMs but the test ROM and the open homebrew. There is no
PipeWire in the sandbox, so `actions.pwDump` is a stand-in that reports
ChooChoo's stream as playing (Home's Now playing, the Mixer). Render outputs
are not committed.
