# SPDX-License-Identifier: GPL-2.0-or-later
#
# Physical rollback: U-Boot boots the previous generation while SELECT is held at
# power-on (firmware/mainline/u-boot.nix, README.md there). extlinux has no label
# meaning "previous", so this writes it down: extlinux/previous.conf, a copy of
# extlinux.conf holding only that generation, every time the boot entries change.
#
# NixOS generations come from the builder that writes extlinux.conf; this calls
# that builder with the arguments the module would, then the writer. A failure of
# the writer doesn't fail the install: the worst case is an old or no previous.conf,
# which U-Boot gets past.
{ config, lib, pkgs, modulesPath, ... }:

let
  cfg = config.boot.loader.generic-extlinux-compatible;
  dt = config.hardware.deviceTree;

  builder = import "${modulesPath}/system/boot/loader/generic-extlinux-compatible/extlinux-conf-builder.nix" {
    inherit lib pkgs;
  };
  builderArgs = lib.escapeShellArgs (
    [ "-g" (toString cfg.configurationLimit) "-t" (if config.boot.loader.timeout == null then "-1" else toString config.boot.loader.timeout) ]
    ++ lib.optionals (dt.name != null) [ "-n" dt.name ]
    ++ lib.optional (!cfg.useGenerationDeviceTree) "-r"
    ++ [ "-d" "/boot" ]
  );

  # The default entry, and generation entries newest first (nixos-<N>-default;
  # the builder's order), with `init=<toplevel>/init` telling which is which.
  # "Current" is the newest generation that is the default system; "previous" is
  # the newest one below it that is a different system. So after a rollback to
  # generation 6, previous is 5, not the 7 that was rolled back from.
  previousConf = pkgs.writeShellApplication {
    name = "mortar-previous-conf";
    runtimeInputs = with pkgs; [ coreutils gawk gnused ];
    text = ''
      dir=''${1:?usage: mortar-previous-conf <extlinux directory>}
      out=$dir/previous.conf
      tmp=$dir/previous.conf.tmp.$$
      trap 'rm -f "$tmp"' EXIT

      if awk '
        /^TIMEOUT /      { timeout = $0 }
        /^LABEL /        { label = $2; if (label ~ /^nixos-[0-9]+-default$/) gens[++n] = label }
        label != "" && NF { block[label] = block[label] $0 "\n" }
        /^ *APPEND /     {
          if (match($0, /init=[^ ]+/)) init[label] = substr($0, RSTART, RLENGTH)
        }
        END {
          want = init["nixos-default"]
          if (want == "") exit 2
          # gens[] is newest first: past the current one, take the first other system
          for (i = 1; i <= n; i++) if (init[gens[i]] == want) { cur = i; break }
          for (i = cur + 1; i <= n; i++) if (init[gens[i]] != "" && init[gens[i]] != want) {
            print "# Generated: the generation before the default, for U-Boot (SELECT held)."
            print "DEFAULT " gens[i]
            print timeout
            print ""
            printf "%s", block[gens[i]]
            exit 0
          }
          exit 1
        }
      ' "$dir/extlinux.conf" > "$tmp"; then
        mv -f "$tmp" "$out"
        echo "previous generation for the rollback key: $(sed -n 's/^DEFAULT //p' "$out")"
      else
        # one generation (or an unreadable file): nothing to go back to
        rm -f "$out"
      fi
      sync -f "$dir" 2>/dev/null || true
    '';
  };
in
{
  config = lib.mkIf cfg.enable {
    system.build.installBootLoader = lib.mkForce (
      pkgs.writeScript "install-extlinux-conf.sh" ''
        #!${pkgs.runtimeShell}
        set -e
        ${builder} ${builderArgs} -c "$@"
        ${lib.getExe previousConf} /boot/extlinux || echo "mortar: no previous.conf written" >&2
      ''
    );

    system.build.mortarPreviousConf = previousConf;
  };
}
