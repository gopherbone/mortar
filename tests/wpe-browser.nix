# SPDX-License-Identifier: GPL-2.0-or-later
# WPE Browser (pkgs/wpe-browser) in the real shell, on a machine without its GPU: the
# headless backend, Mesa's software GL and copied frames, so everything but the
# GPU path. A fake pad with the key set of the Brick's gpio-keys pad gives the browser the
# same uinput clone as on the Brick. Pages come from a server in the machine that logs
# what they ask for, which is how the test sees the browser act.
#
#   nix build .#packages.aarch64-linux.wpe-browser-test    (builds WebKit the first time)
#
# Screenshots land in the result.
{ pkgs, app }:

let
  mortar-shell = pkgs.callPackage ../pkgs/mortar-shell { };
  apps = "/nix/var/nix/profiles/apps";
  user = "/home/player";
  ctl = "XDG_RUNTIME_DIR=/run/user/1000 ${mortar-shell}/bin/mortar-shell ctl";
  python = pkgs.python3.withPackages (p: [ p.evdev ]);

  sdlRun = pkgs.writeShellScriptBin "mortar-sdl-run" ''
    export SDL_RENDER_DRIVER=software LIBGL_ALWAYS_SOFTWARE=1
    export __EGL_VENDOR_LIBRARY_DIRS=${pkgs.mesa}/share/glvnd/egl_vendor.d
    export WPE_TSP_INPUT_DEBUG=1
    exec "$@" >> "$HOME/browser.log" 2>&1
  '';
  config = pkgs.writeText "shell.json" (builtins.toJSON {
    appProfiles = apps;
    sdlRun = "${sdlRun}/bin/mortar-sdl-run";
    ui = { bootAnimation = false; sounds = false; haptics = false; };
  });

  # /proc/bus/input/devices on the Brick: KEY=3 0 0 f00000000 0 0 0 1fdb000000000000 0 0 0 0
  fakePad = pkgs.writeText "fakepad.py" ''
    import time
    from evdev import UInput, ecodes as e
    keys = [e.BTN_SOUTH, e.BTN_EAST, e.BTN_NORTH, e.BTN_WEST, e.BTN_TL, e.BTN_TR, e.BTN_TL2, e.BTN_TR2,
            e.BTN_SELECT, e.BTN_START, e.BTN_MODE, e.BTN_DPAD_UP, e.BTN_DPAD_DOWN, e.BTN_DPAD_LEFT,
            e.BTN_DPAD_RIGHT, e.BTN_TRIGGER_HAPPY1, e.BTN_TRIGGER_HAPPY2]
    ui = UInput({e.EV_KEY: keys}, name="TrimUI Brick Gamepad", bustype=0x19, vendor=1, product=1, version=0x100)
    while True:
        time.sleep(3600)
  '';

  # A field filling the page (the pointer starts in the middle of the screen), and frames
  # counted to the server: they have to stop while the browser isn't on screen.
  server = pkgs.writeText "server.py" ''
    import http.server
    PAGE = b"""<!doctype html><title>wpe test</title><body style="margin:0;background:#2a6">
    <form action=/got><input name=q placeholder="type here" style="width:100%;height:100vh;font-size:40px"></form>
    <script>
    let n = 0;
    function frame() { if (++n % 30 == 0) fetch('/frames'); requestAnimationFrame(frame); }
    requestAnimationFrame(frame);
    </script>"""
    class H(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            open("/tmp/requests", "a").write(self.path + "\n")
            self.send_response(200); self.send_header("content-type", "text/html"); self.end_headers()
            self.wfile.write(PAGE if not self.path.startswith("/got") else b"<title>got</title><body>got")
        def log_message(self, *a): pass
    http.server.ThreadingHTTPServer(("127.0.0.1", 8000), H).serve_forever()
  '';

  settings = pkgs.writeText "settings.conf" ''
    HOME_URL="http://127.0.0.1:8000/"
    POINTER_HIDE_SECONDS=0
  '';
in
pkgs.testers.runNixOSTest {
  name = "mortar-wpe-browser";
  nodes.machine = {
    users.users.player = { isNormalUser = true; uid = 1000; linger = true; extraGroups = [ "input" "uinput" ]; };
    hardware.uinput.enable = true;
    hardware.graphics.enable = true;
    virtualisation.additionalPaths = [ app ];
    virtualisation.writableStore = true;
    virtualisation.memorySize = 3072;
    virtualisation.cores = 4;
    virtualisation.diskSize = 8192;
    nix.settings.experimental-features = [ "nix-command" ];
    systemd.services.fakepad = {
      wantedBy = [ "multi-user.target" ];
      serviceConfig.ExecStart = "${python}/bin/python3 ${fakePad}";
    };
    systemd.services.pages = {
      wantedBy = [ "multi-user.target" ];
      serviceConfig.ExecStart = "${pkgs.python3}/bin/python3 ${server}";
    };
    # Started by the test once the pad's node is readable: the shell only rescans on hotplug.
    systemd.user.services.shell = {
      environment.MORTAR_UI_FONTS = "${mortar-shell.fonts}/share/fonts/mortar-ui";
      serviceConfig.ExecStart = "${mortar-shell}/bin/mortar-shell run --backend headless --input direct --config ${config} --socket wayland-test";
    };
  };
  testScript = ''
    import json, time

    def ctl(cmd):
        return machine.succeed(f"su player -c '${ctl} {cmd}'")

    def state():
        return json.loads(ctl("state"))

    def shot(name):
        ctl(f"screenshot /tmp/{name}.png")
        machine.copy_from_machine(f"/tmp/{name}.png", "")

    def requests(what):
        return machine.succeed(f"grep -c '^{what}' /tmp/requests || true").strip()

    def browser_log():
        return machine.succeed("cat ${user}/browser.log")

    machine.wait_for_unit("multi-user.target")
    machine.wait_for_unit("fakepad.service")
    machine.wait_until_succeeds("grep -q 'TrimUI Brick Gamepad' /proc/bus/input/devices")
    machine.succeed("udevadm settle")
    machine.wait_for_unit("user@1000.service")
    machine.succeed("su player -c 'XDG_RUNTIME_DIR=/run/user/1000 systemctl --user start shell'")
    machine.wait_for_file("/run/user/1000/mortar-shell.sock")
    machine.wait_until_succeeds("""su player -c "${ctl} state" | ${pkgs.jq}/bin/jq -e '.inputs | map(select(.[1] | startswith("Pad"))) | length == 1'""", timeout=60)

    with subtest("installed as a profile: in the launcher, with no restart"):
        machine.succeed("mkdir -p -m 0755 ${apps} && nix-env -p ${apps}/wpe-browser --set ${app}")
        machine.wait_until_succeeds("""su player -c "${ctl} state" | ${pkgs.jq}/bin/jq -e '.launcher | map(.id) | index("wpe-browser")'""", timeout=30)

    with subtest("loads a page"):
        machine.succeed("su player -c 'mkdir -p ${user}/.config/wpe-browser && install -m 0644 ${settings} ${user}/.config/wpe-browser/settings.conf'")
        ctl("launch wpe-browser")
        machine.wait_until_succeeds("grep -q '^/$' /tmp/requests", timeout=600)
        machine.wait_until_succeeds("grep -q '^/frames' /tmp/requests", timeout=600)
        time.sleep(5)
        shot("page")
        print(browser_log())

    with subtest("SELECT opens the browser's own menu, B closes it"):
        ctl("press select")
        time.sleep(2)
        shot("menu")
        ctl("press b")
        time.sleep(1)

    with subtest("hidden behind the shell's MENU: frames stop, and start again"):
        ctl("press menu")
        machine.wait_until_succeeds("grep -q 'hidden: page paused' ${user}/browser.log", timeout=30)
        time.sleep(5)
        before = requests("/frames")
        time.sleep(20)
        after = requests("/frames")
        assert before == after, f"frames went on while hidden: {before} to {after}"
        shot("hidden")
        ctl("press menu")
        machine.wait_until_succeeds("[ $(grep -c 'on screen' ${user}/browser.log) -ge 2 ]", timeout=30)
        machine.wait_until_succeeds(f"[ $(grep -c '^/frames' /tmp/requests) -gt {after} ]", timeout=120)
        shot("back")

    with subtest("the on-screen keyboard: click the field, type, leave and come back, Enter"):
        ctl("press down")  # the pointer stays over the field
        time.sleep(1)
        ctl("press a")
        time.sleep(3)
        shot("keyboard")
        ctl("press a")
        time.sleep(1)
        ctl("press right")
        time.sleep(1)
        ctl("press a")
        time.sleep(2)
        shot("typed")
        ctl("press menu")
        machine.wait_until_succeeds("[ $(grep -c 'hidden: page paused' ${user}/browser.log) -ge 3 ]", timeout=30)
        time.sleep(2)
        ctl("press menu")
        machine.wait_until_succeeds("[ $(grep -c 'on screen' ${user}/browser.log) -ge 3 ]", timeout=30)
        time.sleep(2)
        shot("keyboard-back")
        ctl("press a")
        time.sleep(1)
        ctl("press start")
        machine.wait_until_succeeds("grep -E '^/got[?]q=[^+&]{3}$' /tmp/requests", timeout=120)
        print(machine.succeed("grep '^/got' /tmp/requests"))
        time.sleep(2)
        shot("submitted")

    with subtest("every button reaches the browser as the clone's, in evdev order"):
        order = ["b", "a", "x", "y", "l1", "r1", "l2", "r2", "select", "start", "menu", "up", "down", "left", "right", "f1", "f2"]
        for i, b in enumerate(order):
            if b == "menu":
                continue  # the shell's
            ctl(f"press {b}")
            machine.wait_until_succeeds(f"grep -q 'pad button {i} up' ${user}/browser.log", timeout=30)

    with subtest("data is where it should be"):
        machine.succeed("test -s ${user}/.config/wpe-browser/settings.conf")
        machine.succeed("grep -q POINTER_HIDE_SECONDS=0 ${user}/.config/wpe-browser/settings.conf")
        machine.succeed("grep -q USER_AGENT ${user}/.config/wpe-browser/settings.conf")  # defaults added
        machine.wait_until_succeeds("ls ${user}/.local/share/wpe-browser/content-filters | grep -q .", timeout=900)
        print(machine.succeed("find ${user}/.local ${user}/.cache/wpe-browser ${user}/.config -maxdepth 3 | sort"))

    with subtest("closes, with its processes"):
        ctl("close wpe-browser")
        machine.wait_until_fails("pgrep -x WPEWebProcess", timeout=30)
        machine.wait_until_fails("pgrep -x wpe-tsp", timeout=30)
        print(browser_log())

    with subtest("the shell never stopped"):
        state()
  '';
}
