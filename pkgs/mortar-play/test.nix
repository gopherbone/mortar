# SPDX-License-Identifier: GPL-2.0-or-later
# Headless integration test, in the build sandbox (aarch64-linux on the
# builder): no third-party ROMs. The NES program comes from mortar-play
# itself (`make-test-rom`, src/testrom.rs); the 2048 core needs no content.
#
#  1. selftest nes: 300 frames on fceumm with A held for a while;
#     screenshots (1024x768 composition + native), resampled audio (WAV),
#     checks: picture has colour, ~440 Hz tone at a sane level (880 Hz
#     while A is held), save state round trip is deterministic, slot file +
#     thumbnail, save RAM written.
#  2. selftest 2048: start without content, screenshots.
#  3. The library UI on the headless display, driven by a script: home,
#     game list, play, in-game menu, save to a slot, quit (auto state),
#     home with the resume picture, the "no games yet" and BIOS screens.
#
# Always succeeds so the screenshots survive a failure; `tests.integration`
# fails unless $out/result says PASS.
{
  lib,
  runCommand,
  writeText,
  jq,
  libretro,
  mortar-play,
}:

let
  core = pkg: "${pkg}/lib/retroarch/cores/${lib.replaceStrings [ "-" ] [ "_" ] pkg.core}_libretro.so";
  config = root: writeText "mortar-play-test.json" (builtins.toJSON {
    romDirs = [ "${root}/roms" ];
    biosDir = "${root}/bios";
    saveDir = "${root}/saves";
    stateDir = "${root}/states";
    dataDir = "${root}";
    systems = [
      {
        id = "nes";
        name = "NES";
        tag = "NES";
        folders = [ "nes" ];
        extensions = [ "nes" "fds" "unf" "unif" ];
        core = core libretro.fceumm;
        options = { fceumm_sndquality = "Low"; fceumm_overscan_v_top = "8"; fceumm_overscan_v_bottom = "8"; };
        scaling = "integer-aspect";
        order = 10;
      }
      {
        id = "2048";
        name = "2048";
        tag = "2048";
        folders = [ "2048" ];
        core = core libretro.twenty-fortyeight;
        noContent = true;
        order = 20;
      }
      {
        id = "psx";
        name = "PlayStation";
        tag = "PS1";
        folders = [ "psx" ];
        extensions = [ "cue" "chd" "m3u" ];
        core = "/nonexistent";
        bios = [ { files = [ "scph5501.bin" "scph1001.bin" ]; required = false; description = "PlayStation BIOS"; } ];
        order = 30;
      }
    ];
  });
  # paths in the sandbox are only known at build time: @ROOT@ is replaced
  root = "@ROOT@";
  cfgTemplate = config root;
  script = o: lib.concatStringsSep ", " [
    "wait 2" "shot ${o}/ui-1-home.png"
    "press a" "wait 2" "shot ${o}/ui-2-games.png"
    "press a" "wait 90" "shot ${o}/ui-3-game.png"
    "press f1" "wait 3" "shot ${o}/ui-4-menu.png"
    "press down" "wait 1" "press a" "wait 3" "shot ${o}/ui-5-saved.png"
    "press up" "wait 1" "press up" "wait 2" "shot ${o}/ui-6-quit.png"
    "press a" "wait 3" "shot ${o}/ui-7-games-after.png"
    "press b" "wait 2" "shot ${o}/ui-8-home-resume.png"
    "press down" "press down" "press down" "wait 2" "shot ${o}/ui-9-psx.png"
    "press a" "wait 2" "shot ${o}/ui-10-empty.png"
  ];
in
runCommand "mortar-play-integration-test"
  {
    nativeBuildInputs = [ mortar-play jq ];
  }
  ''
    export HOME=$TMPDIR/home
    R=$TMPDIR/play
    mkdir -p $HOME $out $R/roms/nes $R/roms/psx $R/bios
    fail() { echo "FAIL: $*" | tee -a $out/result; }
    sed "s|@ROOT@|$R|g" ${cfgTemplate} > $out/config.json
    cfg=$out/config.json

    mortar-play make-test-rom $R/roms/nes/mortar-test.nes

    echo "== selftest nes"
    mortar-play selftest nes $R/roms/nes/mortar-test.nes --config $cfg \
      --out $out/nes --frames 300 --shots 2,60,180,300 --press 200:a:60 \
      > $out/nes.json 2> $out/nes.log || true
    cat $out/nes.json; cat $out/nes.log
    r=$out/nes.json
    jq -e '.ok' $r >/dev/null || fail "nes: selftest not ok"
    jq -e '.distinct_colors >= 8' $r >/dev/null || fail "nes: picture has too few colours"
    jq -e '.audio_rms > 1000' $r >/dev/null || fail "nes: audio too quiet"
    jq -e '.audio_seconds > 4.8 and .audio_seconds < 5.2' $r >/dev/null || fail "nes: audio length off (DRC/resampler)"
    jq -e '.state_deterministic == true and .state_slot_saved and .state_thumb_saved' $r >/dev/null || fail "nes: save states"
    jq -e '.sram_saved and .sram_bytes == 8192' $r >/dev/null || fail "nes: save RAM"
    test -s $R/saves/nes/mortar-test.srm || fail "nes: no .srm file"
    test -s $R/states/nes/mortar-test.state.0.png || fail "nes: no slot thumbnail"

    # the tone: 440 Hz while A is up, 880 Hz while held (frames 200-260)
    echo "== selftest nes, tone only"
    mortar-play selftest nes $R/roms/nes/mortar-test.nes --config $cfg \
      --out $out/nes-tone --frames 120 > $out/nes-tone.json 2>/dev/null || true
    jq -e '.audio_hz > 430 and .audio_hz < 450' $out/nes-tone.json >/dev/null || fail "nes: tone is not ~440 Hz: $(jq .audio_hz $out/nes-tone.json)"
    mortar-play selftest nes $R/roms/nes/mortar-test.nes --config $cfg \
      --out $out/nes-tone-a --frames 120 --press 1:a:200 > $out/nes-tone-a.json 2>/dev/null || true
    jq -e '.audio_hz > 860 and .audio_hz < 900' $out/nes-tone-a.json >/dev/null || fail "nes: A did not reach the game (tone $(jq .audio_hz $out/nes-tone-a.json))"

    echo "== selftest 2048"
    mortar-play selftest 2048 --config $cfg --out $out/2048 --frames 120 --shots 120 \
      > $out/2048.json 2> $out/2048.log || true
    cat $out/2048.json
    jq -e '.ok and .width > 0' $out/2048.json >/dev/null || fail "2048: selftest"

    echo "== UI flow"
    rm -rf $R/states $R/recent.json
    mortar-play --config $cfg --headless --script "${script "$out"}" 2> $out/ui.log || fail "ui: exit status $?"
    cat $out/ui.log
    for s in 1-home 2-games 3-game 4-menu 5-saved 6-quit 7-games-after 8-home-resume 9-psx 10-empty; do
      test -s $out/ui-$s.png || fail "ui: no screenshot $s"
    done
    test -s $R/states/nes/mortar-test.state.auto || fail "ui: quitting did not write the resume state"
    test -s $R/states/nes/mortar-test.state.0 || fail "ui: menu save did not write slot 1"
    grep -q "mortar-test.nes" $R/recent.json || fail "ui: recents not written"

    [ -e $out/result ] || echo PASS > $out/result
    cat $out/result
  ''
