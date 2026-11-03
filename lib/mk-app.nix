# SPDX-License-Identifier: GPL-2.0-or-later
# An app the shell finds at runtime: a package plus the manifest that describes
# it (format in pkgs/mortar-shell/DESIGN.md, "Apps in their own profiles").
# Install the result as the profile /nix/var/nix/profiles/apps/<id> and it shows
# in the shell, with no system rebuild.
#
#   mortar.lib.mkApp pkgs {
#     package = pkgs.callPackage ./my-port { };
#     id = "my-port";
#     name = "My Port";
#     category = "games";
#     gpu = true;
#     requires = [ "~/.local/share/my-port/data.bin" ];
#   }
{ lib }:

pkgs:
{
  package,
  id,
  name,
  description ? "",
  # apps | games | music; null: guessed from the name
  category ? null,
  icon ? "pad",
  # The program, relative to the package, then its arguments.
  run ? [ "bin/${package.meta.mainProgram or id}" ],
  # Start it through the device's mortar-sdl-run.
  gpu ? false,
  gamepad ? "forward",
  # Absolute or under "~/": the entry hides until these exist. Your data goes
  # there, outside the profile, so removing the app leaves it alone.
  requires ? [ ],
}:

let
  check = cond: msg: lib.assertMsg cond "mkApp ${id}: ${msg}";
  prog = builtins.head run;
  manifest =
    {
      format = 1;
      inherit id name description icon run gpu gamepad requires;
    }
    // lib.optionalAttrs (category != null) { inherit category; };
in
assert check (builtins.match "[a-z0-9][a-z0-9_-]{0,63}" id != null && builtins.match ".*-[0-9]+-link" id == null)
  "id has to be lowercase letters, digits, - and _ (it names the profile), and not look like a profile's generation";
assert check (lib.strings.trim name != "") "name is empty";
assert check (lib.elem icon (import ./app-icons.nix)) "unknown icon ${icon}";
assert check (category == null || lib.elem category [ "apps" "games" "music" ]) "category is apps, games or music";
assert check (lib.elem gamepad [ "forward" "keyboard" "none" ]) "gamepad is forward, keyboard or none";
assert check (run != [ ]) "run is empty";
assert check (prog != "" && !lib.hasPrefix "/" prog && !lib.elem ".." (lib.splitString "/" prog))
  "run's program has to be a path inside the package";
assert check (lib.all (r: lib.hasPrefix "/" r || lib.hasPrefix "~/" r) requires) "requires are absolute or start with ~/";
pkgs.symlinkJoin {
  name = "${id}-app";
  paths = [ package ];
  postBuild = ''
    if [ ! -x "$out/${prog}" ] || [ -d "$out/${prog}" ]; then
      echo "mkApp ${id}: ${prog} is not a program in ${package}" >&2
      exit 1
    fi
    mkdir -p "$out/share/mortar"
    rm -f "$out/share/mortar/app.json"
    cp ${pkgs.writeText "app.json" (builtins.toJSON manifest)} "$out/share/mortar/app.json"
  '';
  passthru = {
    inherit package manifest;
  };
  meta = package.meta or { };
}
