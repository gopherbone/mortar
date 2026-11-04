# SPDX-License-Identifier: GPL-2.0-or-later
# A computer on the USB link of a Brick that doesn't trust it (the default,
# docs/pairing.md): the real usb.nix and setup.nix, with the VM's second network
# card renamed to usb0 in place of the gadget.
#
#   no key: SSH refuses, other ports are closed, DHCP and pairing answer
#   `mortar pair` with the code on the screen: the key lands, SSH works with it
#   `nix copy` over ssh-ng, which is how `mortar deploy` moves a system
#   ssh-copy-id with the code as the password, for a computer without `mortar`
{ pkgs }:

let
  mortar = pkgs.callPackage ../pkgs/mortar-cli { };
  payload = pkgs.writeText "deployed" "a closure for nix copy\n";
in
pkgs.testers.runNixOSTest {
  name = "mortar-usb-pairing";

  nodes.brick =
    { ... }:
    {
      imports = [
        ../modules/device.nix
        ../modules/core/usb.nix
        ../modules/core/usb-watchdog.nix
        ../modules/core/setup.nix
      ];
      mortar.setup = {
        enable = true;
        ui.enable = false;
      };
      # no UDC in a VM
      systemd.services.mortar-usb-gadget.enable = false;
      mortar.usb.watchdog.enable = false;
      mortar.usb.blackbox.enable = false;
      virtualisation.writableStore = true;
      nix.settings.experimental-features = [ "nix-command" ];
      environment.systemPackages = [ pkgs.socat pkgs.jq ];
      networking.firewall.enable = true;
      services.openssh.settings.UseDns = false;
    };

  nodes.computer =
    { ... }:
    {
      environment.systemPackages = [ mortar pkgs.busybox pkgs.netcat pkgs.openssh pkgs.ssh-copy-id pkgs.sshpass ];
      nix.settings.experimental-features = [ "nix-command" ];
      virtualisation.additionalPaths = [ payload ];
      # the name `mortar pair` writes; a real computer resolves it over mDNS
      networking.hosts."10.55.0.1" = [ "brick.local" ];
    };

  testScript = ''
    start_all()
    brick.wait_for_unit("multi-user.target")
    computer.wait_for_unit("multi-user.target")

    # the gadget's usb0 is this VM's link to the computer
    brick.succeed("ip link set eth1 down && ip link set eth1 name usb0 && ip link set usb0 up")
    brick.succeed("networkctl reload")
    brick.wait_until_succeeds("ip -4 addr show usb0 | grep -q 10.55.0.1/24")
    brick.succeed("systemctl restart mortar-setupd.service")
    brick.wait_until_succeeds("mortar-setup status | jq -e .pairing.code")

    computer.succeed("ip addr flush dev eth1 && ip link set eth1 up")

    with subtest("DHCP answers: the computer gets an address"):
        out = computer.succeed("busybox udhcpc -i eth1 -n -q -f -s ${pkgs.coreutils}/bin/true 2>&1")
        print(out)
        assert "10.55.0." in out, out
        computer.succeed("ip addr add 10.55.0.50/24 dev eth1")
        computer.wait_until_succeeds("ping -c1 -W1 10.55.0.1")

    with subtest("usb0 isn't trusted: a listener on another port can't be reached"):
        brick.succeed("systemd-run --unit=probe socat TCP-LISTEN:9000,fork,reuseaddr -")
        brick.wait_until_succeeds("ss -ltn | grep -q :9000")
        computer.fail("nc -z -w 3 10.55.0.1 9000")

    with subtest("no key, no password: SSH refuses"):
        computer.succeed("ssh-keygen -q -t ed25519 -N ''' -f /root/.ssh/id_ed25519 </dev/null")
        computer.fail("ssh -o StrictHostKeyChecking=no -o BatchMode=yes root@10.55.0.1 true")

    with subtest("a wrong code adds nothing"):
        computer.fail("mortar pair --host 10.55.0.1 --code TIDE-0000 --name wrong 2>&1")
        brick.fail("grep -q ssh-ed25519 /boot/ssh-keys/root")

    with subtest("mortar pair with the code on the screen"):
        code = brick.succeed("mortar-setup status | jq -r .pairing.code").strip()
        print(computer.succeed(f"mortar pair --host 10.55.0.1 --code {code} 2>&1"))
        brick.succeed("grep -q ssh-ed25519 /boot/ssh-keys/root")

    with subtest("SSH with the key, the host key pinned by pairing"):
        computer.succeed("ssh -o BatchMode=yes -o StrictHostKeyChecking=yes brick true")

    with subtest("what mortar deploy does over it"):
        computer.succeed("nix copy --no-check-sigs --to ssh-ng://brick ${payload}")
        brick.succeed("nix-store --check-validity ${payload}")
        brick.succeed("grep -q closure ${payload}")

    with subtest("no password but the code, while a code is showing"):
        brick.succeed("""echo '{"op":"set_password_login","on":true}' | socat - UNIX-CONNECT:/run/mortar-setup/control.sock""")
        brick.succeed("mortar-setup new-code")
        brick.succeed("echo 'root:rootpass' | chpasswd")
        nokey = "-o PubkeyAuthentication=no -o StrictHostKeyChecking=no root@10.55.0.1 true"
        computer.fail(f"sshpass -p rootpass ssh {nokey}")
        computer.fail(f"sshpass -p ''' ssh {nokey}")

    with subtest("ssh-copy-id with the code as the password: nothing to install"):
        brick.succeed("""echo '{"op":"set_password_login","on":true}' | socat - UNIX-CONNECT:/run/mortar-setup/control.sock""")
        code = brick.succeed("mortar-setup new-code").strip()
        computer.succeed("ssh-keygen -q -t ed25519 -N ''' -f /root/second </dev/null")
        second = "-i /root/second -o IdentitiesOnly=yes -o BatchMode=yes -o StrictHostKeyChecking=no root@10.55.0.1"
        computer.fail(f"ssh {second} true")
        computer.fail("sshpass -p WRONG-0000 ssh-copy-id -f -i /root/second.pub -o PubkeyAuthentication=no -o StrictHostKeyChecking=no root@10.55.0.1")
        # sshd turns this address away for a while after a failed login
        computer.wait_until_succeeds(f"sshpass -p {code} ssh-copy-id -f -i /root/second.pub -o PubkeyAuthentication=no -o StrictHostKeyChecking=no root@10.55.0.1", timeout=120)
        computer.wait_until_succeeds(f"ssh {second} true", timeout=120)
        # the code was for one key: with one added (the daemon looks once a second), it
        # isn't a password any more
        brick.wait_until_succeeds("mortar-setup status | jq -e '.password_login == false'")
        computer.fail(f"sshpass -p {code} ssh -o PubkeyAuthentication=no -o StrictHostKeyChecking=no root@10.55.0.1 true")

    with subtest("Wi-Fi from a terminal says so when there's none"):
        out = brick.succeed("mortar-setup wifi")
        assert "no Wi-Fi" in out, out
        assert brick.succeed("mortar-setup wifi --if-needed") == ""
  '';
}
