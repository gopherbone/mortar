# Showcase video

A short video of Mortar made from the real shell: `capture.nix` runs
mortar-shell on its headless backend (as `pkgs/mortar-shell/test.nix` does)
with real clients, freezes UI time and steps it 1/30 s per frame while
pressing buttons over the control socket, and screenshots every frame.
`compose.py` frames the footage in the Pond style, adds captions, the
buttons pressed and a soundtrack made of the shell's own UI sounds (put on
the frames where the shell played them, from its debug log) over a small
synthesized beat.

```sh
# frames, sounds, fonts (aarch64-linux: a Linux machine or the linux-builder)
nix build .#packages.aarch64-linux.showcase-capture -o capture
# the video (macOS or Linux)
nix run .#showcase -- --capture capture --cut rough --layout wide --out showcase.mp4
nix run .#showcase -- --capture capture --cut rough --layout square --out showcase-square.mp4
```

`--stills DIR --at 30,200` also saves those frames as PNG.

Clips (`capture/clips/<name>/`): `NNNN.png` at 30 fps and `events.tsv`
(frame, `press` / `key` / `sfx` / `haptic` / `note`, value). The storyboard
(which frames of which clip, captions, accent colours) is `storyboard()` in
compose.py.

Nothing private goes in: the shell runs in the build sandbox with the
hostname `mortar`, UTC, no Wi-Fi, and a scripted stand-in for `claude -p`.
Render outputs are not committed.
