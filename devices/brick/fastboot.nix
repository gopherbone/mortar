# SPDX-License-Identifier: GPL-2.0-or-later
# Boot time: Home sooner after power-on (mainline Brick). What takes time on
# the Brick is cold code running on A53s (a performance governor or more
# read-ahead change nothing), so the work is to do less.
#
#  - No initrd. The kernel has MMC and ext4 built in and mounts the card's
#    root partition itself. The systemd initrd cost ~4 s (82 MB to load and
#    unpack, its own udev, a switch-root). What it was for moves to
#    toplevel/init below: the root file system check, then NixOS stage 2.
#    (The extlinux builder skips generations without an initrd, so
#    toplevel/initrd is an empty one, which the kernel treats as none.)
#  - A boot that changes nothing skips the slow part of activation: the Perl
#    in `users` and `etc` (~2.5 s). Both run when the generation differs from
#    the one last fully activated, or /etc/static does not point at this
#    generation (e.g. an older generation was booted from the menu since).
#  - Quiet consoles: kernel messages at warning and up, no systemd status lines
#    (PID 1 writes those to the 115200 baud UART and waits). Everything is
#    still in dmesg, the journal and the black-box logs.
#  - Unused systemd generators masked; sessions do not wait for the network.
#
# To go back: mortar.fastboot.enable = false, or an older generation (SELECT
# held at power-on, or the boot menu; each keeps its own kernel, initrd and
# command line).
{ config, lib, pkgs, ... }:

let
  cfg = config.mortar.fastboot;
  stamp = "/var/lib/mortar-boot/activated";

  emptyInitrd = pkgs.runCommand "empty-initrd" { nativeBuildInputs = [ pkgs.cpio ]; } ''
    mkdir root
    (cd root && mkdir -p tmp && find . | cpio -o -H newc --quiet) > $out
  '';

  path = lib.makeBinPath [ pkgs.coreutils pkgs.util-linux pkgs.e2fsprogs ];
in
{
  options.mortar.fastboot = {
    enable = lib.mkEnableOption "the fast boot path (no initrd, activation shortcut, quiet consoles)";
    rootDevice = lib.mkOption {
      type = lib.types.str;
      default = "/dev/mmcblk0p2";
      description = "The root partition, for the kernel's root= (no udev yet, so no labels).";
    };
  };

  config = lib.mkIf cfg.enable {
    boot.initrd.enable = false;
    boot.kernelParams = [
      "root=${cfg.rootDevice}"
      "rootfstype=ext4"
      "ro"
      "systemd.show_status=false"
    ];
    boot.consoleLogLevel = lib.mkForce 4;
    # The menu is on the UART only (U-Boot has no video): 1 s to catch it.
    boot.loader.timeout = lib.mkDefault 1;

    # Generators this system has no use for (LUKS, verity, TPM, GPT
    # auto-discovery, ...); PID 1 runs all of them on every start and reload.
    systemd.generators = lib.genAttrs [
      "systemd-bless-boot-generator"
      "systemd-clonesetup-generator"
      "systemd-cryptsetup-generator"
      "systemd-debug-generator"
      "systemd-factory-reset-generator"
      "systemd-gpt-auto-generator"
      "systemd-hibernate-resume-generator"
      "systemd-imds-generator"
      "systemd-import-generator"
      "systemd-integritysetup-generator"
      "systemd-run-generator"
      "systemd-ssh-generator"
      "systemd-system-update-generator"
      "systemd-tpm2-generator"
      "systemd-veritysetup-generator"
    ] (_: "/dev/null");

    # The upstream unit waits for network.target, and everything that logs
    # in (user@.service, the shell) waits for it. Network services come up
    # beside the shell, not before it. At shutdown it still blocks logins
    # first (ordered by Conflicts=shutdown.target like any service).
    # (Suppressed: NixOS would make this text a drop-in on the upstream unit,
    # and a drop-in cannot remove its After=.)
    systemd.suppressedSystemUnits = [ "systemd-user-sessions.service" ];
    systemd.units."systemd-user-sessions.service".text = lib.mkForce ''
      [Unit]
      Description=Permit User Sessions
      Documentation=man:systemd-user-sessions.service(8)
      After=local-fs.target

      [Service]
      Type=oneshot
      RemainAfterExit=yes
      ExecStart=${config.systemd.package}/lib/systemd/systemd-user-sessions start
      ExecStop=${config.systemd.package}/lib/systemd/systemd-user-sessions stop
    '';

    # Written after a full activation: this generation's users and /etc are
    # in place.
    system.activationScripts.mortarBootStamp = {
      deps = [ "users" "etc" ];
      text = ''
        mkdir -p ${dirOf stamp}
        echo -n "$systemConfig" > ${stamp}.tmp && mv ${stamp}.tmp ${stamp}
      '';
    };

    system.systemBuilderCommands = lib.mkAfter ''
      ln -s ${emptyInitrd} $out/initrd

      # activate-boot: activate without the snippets that only matter when
      # the generation changes.
      ${pkgs.gawk}/bin/awk '
        /^#### Activation script snippet / {
          skip = ($5 == "users:" || $5 == "etc:" || $5 == "mortarBootStamp:")
        }
        /^# Make this configuration the current configuration/ { skip = 0 }
        !skip { print }
      ' $out/activate > $out/activate-boot
      chmod +x $out/activate-boot

      # Stage 2, choosing between the two.
      mv $out/init $out/stage-2
      substituteInPlace $out/stage-2 --replace-fail '$systemConfig/activate' '
        if [ "$(cat ${stamp} 2>/dev/null)" = "$systemConfig" ] &&
           [ "$(readlink -f /etc/static)" = "$(readlink -f "$systemConfig/etc")" ]; then
          "$systemConfig/activate-boot"
        else
          "$systemConfig/activate"
        fi'

      # The entry point: the kernel starts this with the root mounted
      # read-only. The check the initrd used to do (0.1 s on a clean file
      # system), then stage 2, which remounts it read-write.
      cat > $out/init <<EOF
      #!${pkgs.bash}/bin/bash
      export PATH=${path}
      mount -t proc proc /proc
      e2fsck -p ${cfg.rootDevice} > /dev/kmsg 2>&1
      rc=\$?
      if [ \$((rc & 2)) != 0 ]; then
        echo "mortar-fastboot: e2fsck changed the root, rebooting" > /dev/kmsg
        sync
        echo b > /proc/sysrq-trigger
      fi
      [ \$((rc & 4)) != 0 ] && echo "<3>mortar-fastboot: errors left on the root (e2fsck \$rc)" > /dev/kmsg
      umount -l /proc
      exec $out/stage-2 "\$@"
      EOF
      chmod +x $out/init
    '';
  };
}
