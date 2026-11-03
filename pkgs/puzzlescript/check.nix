# SPDX-License-Identifier: MIT
# Headless runs of the engine as a user would have it: no bundled games, games
# found in ~/.local/share/puzzlescript/games. `nix build .#checks.<system>.puzzlescript`;
# the screenshots are in $out.
{ runCommand, puzzlescript }:

runCommand "puzzlescript-check" { nativeBuildInputs = [ puzzlescript ]; } ''
  export HOME=$PWD/home XDG_DATA_HOME= XDG_CONFIG_HOME= SDL_AUDIODRIVER=dummy
  mkdir -p $out home
  data=$HOME/.local/share/puzzlescript

  # Nothing bundled in the package
  if test -e ${puzzlescript}/share/puzzlescript; then exit 1; fi

  # No games folder at all: it starts, says where to put them, and exits cleanly.
  printf 'wait 5\nshot empty\nend\n' > empty.txt
  puzzlescript --headless --script empty.txt --shots $out/empty 2> empty.log
  cat empty.log
  grep -F "games: $data/games" empty.log
  test -s $out/empty/empty.png

  # A game in the folder is listed, opens and plays, and its progress is saved.
  mkdir -p $data/games
  cp ${./sdl/testdata/fixture.txt} $data/games/fixture.txt
  cat > play.txt <<'SCRIPT'
  wait 5
  shot list
  select fixture.txt
  press a
  wait 20
  press a
  wait 30
  shot level
  end
  SCRIPT
  puzzlescript --headless --script play.txt --shots $out/play 2> play.log
  cat play.log
  grep -F "games: $data/games" play.log
  test -s $out/play/list.png
  test -s $out/play/level.png
  test -s $data/settings.txt
  cp $data/settings.txt $out/settings.txt
''
