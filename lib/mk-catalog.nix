# SPDX-License-Identifier: GPL-2.0-or-later
# A repository's catalog from lib.mkApp packages (format: docs/packages.md). The
# result is the catalog as JSON, and it refers to every app, so its closure holds
# them all. It's not signed and has no date or channel yet: `mortar publish --catalog`
# adds those, pushes what's listed, and writes channels/<channel>/catalog.json.
#
#   mortar.lib.mkCatalog pkgs {
#     devices.brick = [
#       { app = wpe-browser; licence = "MIT, LGPL-2.1, others"; source = "https://…"; }
#       openxcom-app            # version, licence and source from its package's meta
#     ];
#   }
#
# `tools` names large indexes of terminal tools (pkgs/mortar-index/src/compact.rs),
# as store paths with a tools.tsv: [ { system; path; count; } ]. CI adds one, made
# by scripts/make-tools-index.sh; such a path can't have references, as that file says.
{ lib }:

pkgs:
{
  devices,
  tools ? [ ],
}:

let
  entry =
    device: item:
    let
      spec = if item ? app then item else { app = item; };
      app = spec.app;
      m = app.manifest or (throw "mkCatalog: ${app.name or "a package"} isn't from lib.mkApp");
      package = app.package or { };
      meta = package.meta or { };
      licences = lib.toList (meta.license or [ ]);
      licenceName = l: l.spdxId or l.shortName or (toString l);
    in
    {
      inherit (m) id name description icon requires;
      version = spec.version or package.version or (throw "mkCatalog ${m.id}: no version; give one");
      category = m.category or "apps";
      kind = "nix";
      licence = spec.licence or (lib.concatMapStringsSep ", " licenceName licences);
      source = spec.source or meta.homepage or "";
      builds = [
        {
          inherit device;
          system = app.stdenv.hostPlatform.system or pkgs.stdenv.hostPlatform.system;
          # a string with its context: the catalog refers to the app
          path = "${app}";
        }
      ];
    };

  all = lib.concatLists (lib.mapAttrsToList (device: map (entry device)) devices);
  byId = builtins.groupBy (p: p.id) all;
  # one package, with a build per device
  merge =
    id: ps:
    let
      newest = lib.last ps;
      versions = lib.unique (map (p: p.version) ps);
    in
    assert lib.assertMsg (lib.length versions == 1) "mkCatalog ${id}: different versions per device: ${toString versions}";
    newest // { builds = lib.concatMap (p: p.builds) ps; };
  packages = lib.mapAttrsToList merge byId;
in
pkgs.writeTextFile {
  name = "mortar-catalog.json";
  text = builtins.toJSON {
    format = 1;
    channel = "";
    published = 0;
    inherit packages tools;
  };
  passthru = {
    inherit packages tools;
  };
}
