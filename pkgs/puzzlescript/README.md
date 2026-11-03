# PuzzleScript (engine)

An engine for [PuzzleScript](https://www.puzzlescript.net) games. It derives
from [increpare/PuzzleScript](https://github.com/increpare/PuzzleScript) by
Stephen Lavelle, the original JavaScript engine and language, which is under
the MIT licence.

gopherbone reworked it into a native C port for the Playdate, and then into
this engine with an SDL3 shell in Rust. Compared with upstream:

- `src/`: parser, compiler, engine, RNG and sound effects rewritten in C from
  `parser.js`, `compiler.js`, `engine.js`, `rng.js` and `sfxr.js`, with the
  font and palettes generated from `font.js` and `colors.js`. No browser and
  no JavaScript; levels compile on the device.
- `sdl/`: a new shell in Rust on SDL3: game browser, level select, pause menu,
  saves, gamepad and keyboard input. Upstream's editor and web player are not
  here.
- The Playdate shell, its tests and the bundled games are not vendored here.

## Licence

This directory is under the MIT licence (`LICENSE`, `SPDX-License-Identifier:
MIT`), keeping upstream's copyright notice. That is separate from the rest of
Mortar, which is GPL-2.0-or-later.

## Games

No games are included. Put PuzzleScript source files (`.txt`) in

    ~/.local/share/puzzlescript/games/

An empty or missing folder gives a screen that names the folder. Saves go to
`~/.local/share/puzzlescript/saves/<file>/`, so they follow a game's file name.
`--games DIR` or `$PUZZLESCRIPT_GAMES` use another folder; `--data DIR` or
`$PUZZLESCRIPT_DATA` move saves, settings and that default games folder.
`sdl/testdata/fixture.txt` is a tiny test level set for the checks, not a game.

## Controls

| | keyboard | gamepad |
|---|---|---|
| move | arrows / WASD | D-pad / left stick |
| action | X, Space, Enter, C | A |
| undo | Z, U, Backspace | B (hold 1 s: restart), X |
| restart | R | Y |
| menu | Esc | Start / Guide |
| level select | Tab, L | Select (Back) |
| page lists | PageUp / PageDown | L / R |

Overrides go in `$XDG_CONFIG_HOME/puzzlescript/controls.cfg`
(`puzzlescript --print-controls` shows the format).

## Scripted runs

`puzzlescript --headless --script FILE --shots DIR` runs at a fixed 60 steps/s
with no window and writes PNGs. Commands, one per line: `wait N`, `press BTN`,
`hold BTN`, `release BTN`, `shot NAME`, `select FILE`, `play N FILE`, `end`.
`check.nix` uses this (`checks.<system>.puzzlescript` in the flake).
