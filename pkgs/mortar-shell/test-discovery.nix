# SPDX-License-Identifier: GPL-2.0-or-later
# Apps coming and going in a running shell: the headless compositor in the build
# sandbox, a temporary apps directory that does not exist yet, and profiles made
# the way nix-env makes them (a generation link, then a rename). Checks the
# launcher through `ctl state`, and the log. tests/app-profiles.nix does the same
# with real profiles.
{
  writeText,
  writeShellScript,
  runCommand,
  mortar-shell,
  jq,
  makeFontsConf,
  dejavu_fonts,
}:

let
  config = dir: writeText "mortar-shell-discovery.json" (builtins.toJSON {
    apps = [
      { id = "term"; name = "Terminal"; command = [ "/bin/false" ]; icon = "term"; }
      { id = "shadow"; name = "From config"; command = [ "/bin/false" ]; }
    ];
    appProfiles = dir;
    sdlRun = "${sdlRun}";
    ui = { bootAnimation = false; sounds = false; haptics = false; };
    home.widgets = [ ];
  });
  # Records how it was started, then runs the app: stands in for the device's.
  sdlRun = writeShellScript "mortar-sdl-run" ''
    echo "$@" > "$HOME/sdl-run-args"
    exec "$@"
  '';
  # An app that stays up and has no window.
  appScript = writeShellScript "app" ''
    echo running > "$HOME/$MORTAR_APP.ran"
    exec sleep 600
  '';
in
runCommand "mortar-shell-discovery-test"
  {
    nativeBuildInputs = [ mortar-shell jq ];
    FONTCONFIG_FILE = makeFontsConf { fontDirectories = [ dejavu_fonts ]; };
  }
  ''
    export XDG_RUNTIME_DIR=$TMPDIR/run HOME=$TMPDIR/home
    mkdir -p -m 700 $XDG_RUNTIME_DIR $HOME
    mkdir $out
    log=$out/shell.log
    fail() { echo "FAIL: $1"; echo "--- log"; cat $log; echo "--- state"; mortar-shell ctl state | jq '{launcher, apps}'; exit 1; }

    root=$TMPDIR/profiles
    apps=$root/apps
    store=$TMPDIR/store
    sed "s|@APPS@|$apps|" ${config "@APPS@"} > $TMPDIR/config.json

    # A store path stand-in (args: tag, then the manifest)
    mkstore() {
      mkdir -p $store/$1/bin $store/$1/share/mortar
      cp ${appScript} $store/$1/bin/app
      printf '%s' "$2" > $store/$1/share/mortar/app.json
      echo $store/$1
    }
    # As nix-env -p: generation link, temporary name, rename over the profile
    install_profile() { # id generation store
      ln -s $3 $apps/$1-$2-link
      ln -s $1-$2-link $apps/.$1.tmp
      mv -T $apps/.$1.tmp $apps/$1
    }
    manifest() { # id, extra JSON fields
      echo "{\"format\":1,\"id\":\"$1\",\"name\":\"App $1\",\"run\":[\"bin/app\"],\"category\":\"games\"$2}"
    }
    launcher() { mortar-shell ctl state | jq -c '[.launcher[].id]'; }
    # Up to 5 s: discovery has to be quick, and this is not how long it takes.
    expect() { # JSON array
      for _ in $(seq 50); do [ "$(launcher)" = "$1" ] && { echo "launcher: $1"; return 0; }; sleep 0.1; done
      fail "launcher is $(launcher), wanted $1"
    }

    echo "== the directory does not exist yet: the shell starts anyway"
    mkdir -p $root
    mortar-shell run --backend headless --config $TMPDIR/config.json --socket wayland-test 2> $log &
    shell=$!
    trap 'kill $shell 2>/dev/null || true' EXIT
    for _ in $(seq 100); do [ -S $XDG_RUNTIME_DIR/mortar-shell.sock ] && break; sleep 0.1; done
    expect '["term","shadow"]'

    echo "== first app: the directory appears, then a profile in it"
    mkdir $apps
    install_profile one 1 $(mkstore one1 "$(manifest one ',"gpu":true')")
    expect '["term","shadow","one"]'

    echo "== a bad manifest: a warning, no entry, the shell lives"
    install_profile junk 1 $(mkstore junk1 '{ not json')
    install_profile future 1 $(mkstore future1 '{"format":99,"id":"future"}')
    install_profile thief 1 $(mkstore thief1 "$(manifest one)")
    install_profile two 1 $(mkstore two1 "$(manifest two)")
    expect '["term","shadow","one","two"]'
    grep -q 'junk.*not JSON.*skipped' $log || fail "no warning about junk"
    grep -q 'future.*format 99.*skipped' $log || fail "no warning about future"
    grep -q "thief.*id 'one' is not the profile's name" $log || fail "no warning about thief"
    [ "$(grep -c 'junk.*skipped' $log)" = 1 ] || fail "the warning about junk repeats"

    echo "== config wins over a manifest with the same id, and the log says so"
    install_profile shadow 1 $(mkstore shadow1 "$(manifest shadow)")
    for _ in $(seq 50); do grep -q "entry 'shadow' wins" $log && break; sleep 0.1; done
    grep -q "entry 'shadow' wins" $log || fail "the log does not say the config won"
    mortar-shell ctl state | jq -e '.launcher[] | select(.id == "shadow") | .name == "From config"' >/dev/null || fail "the config's entry did not win"
    expect '["term","shadow","one","two"]'

    echo "== requires: offered once the file is there, as for a config entry (rechecked every 3 s while looked at)"
    install_profile needs 1 $(mkstore needs1 "$(manifest needs ',"requires":["~/data/file"]')")
    sleep 0.5
    expect '["term","shadow","one","two"]'
    mkdir $HOME/data; touch $HOME/data/file
    expect '["term","shadow","needs","one","two"]'
    rm $HOME/data/file
    said=$(mortar-shell ctl launch needs 2>&1 || true)
    case "$said" in *"not installed"*) ;; *) fail "started without its required file: $said" ;; esac

    echo "== launch: gpu goes through sdl-run, and the program is in the profile"
    mortar-shell ctl launch one
    for _ in $(seq 50); do [ -e $HOME/one.ran ] && break; sleep 0.1; done
    [ -e $HOME/one.ran ] || fail "the app did not start"
    grep -qx "$apps/one/bin/app" $HOME/sdl-run-args || fail "sdl-run got: $(cat $HOME/sdl-run-args)"
    mortar-shell ctl launch two
    for _ in $(seq 50); do [ -e $HOME/two.ran ] && break; sleep 0.1; done
    [ -e $HOME/two.ran ] || fail "the second app did not start"
    grep -qx "$apps/one/bin/app" $HOME/sdl-run-args || fail "two went through sdl-run without gpu"

    echo "== the profile of a running app goes: it leaves the launcher and keeps running"
    pid=$(mortar-shell ctl state | jq -r '.apps[] | select(.id == "one") | .pid')
    rm $apps/one $apps/one-1-link
    expect '["term","shadow","two"]'
    sleep 1
    kill -0 $pid || fail "the running app died with its profile"
    mortar-shell ctl state | jq -e '.apps[] | select(.id == "one") | .pid != null' >/dev/null || fail "the running app left Running"
    grep -q 'the running app keeps running' $log || fail "no log line for the running app"

    echo "== it comes back: starting it again switches to the running copy"
    install_profile one 2 $(mkstore one2 "$(manifest one)")
    expect '["term","shadow","one","two"]'
    [ "$(mortar-shell ctl launch one)" = "$(mortar-shell ctl state | jq -r '.apps[] | select(.id == "one") | .uid')" ] || fail "a second copy started"
    [ "$(mortar-shell ctl state | jq '[.apps[] | select(.id == "one")] | length')" = 1 ] || fail "two copies"

    echo "== an update: the same id, a new manifest"
    install_profile two 2 $(mkstore two2 "$(manifest two | sed 's/App two/Two 2/')")
    for _ in $(seq 50); do mortar-shell ctl state | jq -e '.launcher[] | select(.id == "two") | .name == "Two 2"' >/dev/null && break; sleep 0.1; done
    mortar-shell ctl state | jq -e '.launcher[] | select(.id == "two") | .name == "Two 2"' >/dev/null || fail "the update did not show"

    echo "== the whole directory goes, and comes back"
    rm -rf $apps
    expect '["term","shadow"]'
    mkdir $apps
    install_profile three 1 $(mkstore three1 "$(manifest three)")
    expect '["term","shadow","three"]'

    echo "== while the MENU shows it: the row appears, and when the last game goes so does Games"
    mortar-shell ctl press menu >/dev/null
    for _ in 1 2 3; do
      mortar-shell ctl state | jq -e '.category == "Games"' >/dev/null && break
      mortar-shell ctl press right >/dev/null; sleep 0.5
    done
    mortar-shell ctl state | jq -e '.category == "Games"' >/dev/null || fail "no Games category with a game installed"
    install_profile four 1 $(mkstore four1 "$(manifest four | sed 's/App four/Four/')")
    expect '["term","shadow","three","four"]'
    sleep 1
    mortar-shell ctl screenshot $out/games.png >/dev/null
    # (the app running from before keeps Running full; only the entries go)
    rm $apps/three $apps/three-1-link $apps/four $apps/four-1-link
    expect '["term","shadow"]'
    for _ in $(seq 50); do mortar-shell ctl state | jq -e '.category != "Games"' >/dev/null && break; sleep 0.1; done
    mortar-shell ctl state | jq -e '.category != "Games"' >/dev/null || fail "still on an empty Games"
    sleep 1
    mortar-shell ctl screenshot $out/no-games.png >/dev/null
    mortar-shell ctl press b >/dev/null
    mortar-shell ctl press menu >/dev/null

    echo "== no timer of ours: quiet directory, nothing logged"
    n=$(wc -l < $log)
    sleep 3
    [ "$(wc -l < $log)" = "$n" ] || fail "the shell logged while nothing happened"

    kill -0 $shell || fail "the shell died"
    mortar-shell ctl state | jq '{launcher, apps: [.apps[] | {id, pid}]}' > $out/state.json
    echo PASS | tee $out/result
  ''
