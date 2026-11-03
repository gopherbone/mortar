# SPDX-License-Identifier: GPL-2.0-or-later
# End to end: an app made with lib.mkApp, installed the way `mortar xcom` does on a
# real Nix store, in a machine with the shell's headless backend running as a
# normal user. The profile stays through garbage collection, a running app stays
# through its profile being removed and collected, and the user's data is not
# touched by any of it.
#
#   nix build .#checks.aarch64-linux.app-profiles   (a Mac: via its linux-builder)
{ pkgs, mkApp }:

let
  mortar-shell = pkgs.callPackage ../pkgs/mortar-shell { };
  apps = "/nix/var/nix/profiles/apps";

  demo = mkApp pkgs {
    package = pkgs.writeShellApplication {
      name = "demo";
      text = ''
        echo "$$" > "$HOME/demo.pid"
        exec ${pkgs.coreutils}/bin/sleep 3600
      '';
    };
    id = "demo";
    name = "Demo";
    category = "games";
    gpu = true;
    requires = [ "~/.local/share/demo/data.bin" ];
  };

  sdlRun = pkgs.writeShellScriptBin "mortar-sdl-run" ''exec "$@"'';
  config = pkgs.writeText "shell.json" (builtins.toJSON {
    appProfiles = apps;
    sdlRun = "${sdlRun}/bin/mortar-sdl-run";
    ui = { bootAnimation = false; sounds = false; haptics = false; };
  });
  ctl = "XDG_RUNTIME_DIR=/run/user/1000 ${mortar-shell}/bin/mortar-shell ctl";
  launcher = "${ctl} state | ${pkgs.jq}/bin/jq -c '[.launcher[].id]'";
in
pkgs.testers.runNixOSTest {
  name = "mortar-app-profiles";
  nodes.machine = {
    users.users.player = { isNormalUser = true; uid = 1000; linger = true; };
    # In the store but not in the system: only the profile can keep it there.
    virtualisation.additionalPaths = [ demo ];
    virtualisation.writableStore = true;
    virtualisation.memorySize = 1024;
    nix.settings.experimental-features = [ "nix-command" ];
    systemd.user.services.shell = {
      wantedBy = [ "default.target" ];
      environment.MORTAR_UI_FONTS = "${mortar-shell.fonts}/share/fonts/mortar-ui";
      serviceConfig.ExecStart = "${mortar-shell}/bin/mortar-shell run --backend headless --config ${config} --socket wayland-test";
    };
  };
  testScript = ''
    def launcher_is(want):
        machine.wait_until_succeeds("""su player -c "${launcher}" | grep -qxF '%s'""" % want, timeout=10)

    machine.wait_for_unit("multi-user.target")
    machine.wait_for_file("/run/user/1000/mortar-shell.sock")

    with subtest("no apps directory yet: the shell runs, with nothing to list"):
        machine.fail("test -e ${apps}")
        launcher_is("[]")

    with subtest("installed: listed once the data is there, with no rebuild or restart"):
        machine.succeed("mkdir -p -m 0755 ${apps} && nix-env -p ${apps}/demo --set ${demo}")
        machine.succeed("ls -l ${apps} >&2; test -f ${apps}/demo/share/mortar/app.json")
        machine.succeed("su player -c 'mkdir -p ~/.local/share/demo && echo saved > ~/.local/share/demo/data.bin'")
        launcher_is('["demo"]')

    with subtest("the profile is a GC root"):
        machine.succeed("nix-collect-garbage")  # the last subtest shows it would go
        machine.succeed("test -x ${demo}/bin/demo")
        launcher_is('["demo"]')

    with subtest("reinstalling the same path does not disturb anything"):
        machine.succeed("nix-env -p ${apps}/demo --set ${demo}")
        launcher_is('["demo"]')

    with subtest("it starts through the profile"):
        machine.succeed("su player -c '${ctl} launch demo'")
        machine.wait_for_file("/home/player/demo.pid")
        pid = machine.succeed("cat /home/player/demo.pid").strip()
        machine.succeed(f"kill -0 {pid}")

    with subtest("removed while it runs: leaves the launcher, keeps running, even collected"):
        machine.succeed("rm -f ${apps}/demo ${apps}/demo-*-link")
        launcher_is("[]")
        machine.succeed("nix-collect-garbage")
        machine.fail("test -e ${demo}")
        machine.succeed(f"kill -0 {pid}")
        machine.succeed("""su player -c "${ctl} state" | ${pkgs.jq}/bin/jq -e '.apps[] | select(.id == "demo") | .pid != null'""")
        machine.succeed("journalctl --user -M player@ -u shell --no-pager 2>/dev/null | grep -q 'keeps running' || journalctl --no-pager | grep -q 'keeps running'")

    with subtest("the data stays"):
        machine.succeed("grep -qx saved /home/player/.local/share/demo/data.bin")

    with subtest("the shell never stopped"):
        machine.succeed("su player -c '${ctl} state' >/dev/null")
  '';
}
