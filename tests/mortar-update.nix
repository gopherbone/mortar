# SPDX-License-Identifier: GPL-2.0-or-later
# End to end: over-the-air updates through an S3 binary cache.
#
#   ci     Garage (an S3 server standing in for Cloudflare R2, region
#          "auto" like R2) and the publisher: `mortar publish --path …`
#          pushes a signed closure and the manifest with a write key
#   brick  modules/core/update.nix with a read-only key from the card's
#          mortar.toml; `mortar-update check` downloads and stages
#
# Also: the read key cannot write; unsigned or tampered manifests, closures
# not signed by the publisher (even with a key nix.conf trusts), another
# channel's or configuration's manifest and older ones are all refused; low
# battery waits, offline is reported, rollback, and a rolled-back Brick does
# not take the same manifest again.
#
#   nix build .#checks.aarch64-linux.mortar-update   (a Mac: via its linux-builder)
{ pkgs }:

let
  # A throwaway signing key made at build time (nothing secret in the repo).
  testKey = pkgs.runCommand "mortar-update-test-key" { nativeBuildInputs = [ pkgs.nix ]; } ''
    export HOME=$TMPDIR
    mkdir $out
    nix --extra-experimental-features nix-command key generate-secret --key-name mortar-test-1 > $out/secret
    nix --extra-experimental-features nix-command key convert-secret-to-public < $out/secret > $out/public
  '';
  publicKey = builtins.readFile "${testKey}/public";

  # A second key the Brick's Nix trusts as if for another cache, but the update
  # path must not.
  otherKey = pkgs.runCommand "mortar-update-test-other-key" { nativeBuildInputs = [ pkgs.nix ]; } ''
    export HOME=$TMPDIR
    mkdir $out
    nix --extra-experimental-features nix-command key generate-secret --key-name other-cache-1 > $out/secret
    nix --extra-experimental-features nix-command key convert-secret-to-public < $out/secret > $out/public
  '';

  # The "new" system: small (a container config), but a real toplevel
  # with bin/switch-to-configuration.
  target =
    (pkgs.nixos {
      boot.isContainer = true;
      networking.hostName = "updated";
      environment.etc."mortar-test-generation".text = "2";
      system.stateVersion = "25.05";
    }).config.system.build.toplevel;

  # Looks like a system, but nobody signs it.
  unsigned = pkgs.writeTextFile {
    name = "nixos-system-unsigned";
    destination = "/bin/switch-to-configuration";
    executable = true;
    text = "#!/bin/sh\necho should never run; exit 1\n";
  };

  # Signed, but by the other key.
  otherSigned = pkgs.writeTextFile {
    name = "nixos-system-other-signer";
    destination = "/bin/switch-to-configuration";
    executable = true;
    text = "#!/bin/sh\necho should never run; exit 1\n";
  };

  mortar = pkgs.callPackage ../pkgs/mortar-cli { };
in
pkgs.testers.runNixOSTest {
  name = "mortar-update";

  nodes.ci =
    { ... }:
    {
      services.garage = {
        enable = true;
        package = pkgs.garage_2;
        settings = {
          replication_factor = 1;
          consistency_mode = "consistent";
          rpc_bind_addr = "[::]:3901";
          rpc_public_addr = "[::1]:3901";
          rpc_secret = "5c1915fa04d0b6739675c61bf5907eb0fe3d9c69850c83820f51b4d25d13868c";
          s3_api = {
            s3_region = "auto";
            api_bind_addr = "[::]:3900";
            root_domain = ".s3.garage";
          };
        };
      };
      networking.firewall.allowedTCPPorts = [ 3900 ];
      environment.systemPackages = [ mortar pkgs.curl pkgs.jq ];
      virtualisation.additionalPaths = [ target unsigned otherSigned ];
      virtualisation.memorySize = 2048;
      virtualisation.diskSize = 4096;
      nix.settings.experimental-features = [ "nix-command" ];
    };

  nodes.brick =
    { ... }:
    {
      imports = [ ../modules/core/update.nix ];
      mortar.update = {
        enable = true;
        bucket = "cache";
        endpoint = "http://ci:3900";
        inherit publicKey;
        configName = "test";
        minFreeMiB = 64;
      };
      nix.settings.trusted-public-keys = [ (builtins.readFile "${otherKey}/public") ];
      environment.systemPackages = [ pkgs.jq ];
      virtualisation.memorySize = 2048;
      virtualisation.diskSize = 4096;
      virtualisation.writableStore = true;
      # test VMs leave out switch-to-configuration; a Brick has it (rollback)
      system.switch.enable = true;
    };

  testScript = ''
    import json, re

    start_all()
    ci.wait_for_unit("garage.service")
    ci.wait_for_open_port(3900)
    node_id = ci.succeed("garage node id").split("@")[0]
    ci.succeed(f"garage layout assign -z test -c 1G {node_id}")
    ci.succeed("garage layout apply --version 1")
    ci.succeed("garage bucket create cache")

    def field(pattern: str, text: str) -> str:
        m = re.search(pattern, text, re.IGNORECASE)
        assert m is not None, (pattern, text)
        return m.group(1)

    def make_key(name, *perms):
        out = ci.succeed(f"garage key create {name}")
        kid = field(r"Key ID:\s*(\S+)", out)
        sec = field(r"Secret key:\s*(\S+)", out)
        ci.succeed(f"garage bucket allow {' '.join(perms)} cache --key {name}")
        return kid, sec

    # CI's write key and the Brick's read-only key, like the two R2 tokens
    w_id, w_sec = make_key("ci", "--read", "--write")
    r_id, r_sec = make_key("brick", "--read")
    w_env = f"AWS_ACCESS_KEY_ID={w_id} AWS_SECRET_ACCESS_KEY={w_sec}"
    store = "s3://cache?endpoint=http://localhost:3900&region=auto"

    def s3put(local, key, kid=w_id, sec=w_sec):
        return ci.succeed(f"curl -s -o /dev/null -w '%{{http_code}}' --aws-sigv4 aws:amz:auto:s3 --user {kid}:{sec} "
                          f"-T {local} http://localhost:3900/cache/{key}").strip()

    def s3cat(key):
        return ci.succeed(f"curl -sf --aws-sigv4 aws:amz:auto:s3 --user {w_id}:{w_sec} http://localhost:3900/cache/{key}")

    def status():
        return json.loads(brick.succeed("cat /run/mortar-update/status.json"))

    def check(expect_ok=True):
        if expect_ok:
            print(brick.succeed("mortar-update check 2>&1"))
        else:
            print(brick.fail("mortar-update check 2>&1"))
        return status()

    def publish(path, extra=""):
        return ci.succeed(
            f"{w_env} MORTAR_REV=0123456789abcdef mortar publish --endpoint http://ci:3900 --bucket cache "
            f"--key /root/signing-key --config test --path {path} {extra} 2>&1"
        )

    # the publisher's key, as CI writes it from a secret
    ci.succeed("install -m 600 ${testKey}/secret /root/signing-key")
    ci.succeed("install -m 600 ${otherKey}/secret /root/other-key")

    # a manifest by hand; sign=None leaves it unsigned
    def put_manifest(toplevel, published, sign="/root/signing-key", channel="mortar", config="test"):
        m = {"schema": 1, "channel": channel, "system": "${pkgs.stdenv.hostPlatform.system}", "config": config,
             "toplevel": toplevel, "published": published, "date": "1970-01-01T00:16:40Z", "changelog": ["evil"]}
        ci.succeed(f"cat > /tmp/m.json <<'EOF'\n{json.dumps(m)}\nEOF")
        if sign is not None:
            ci.succeed(f"mortar sign-manifest --key {sign} < /tmp/m.json > /tmp/signed.json && mv /tmp/signed.json /tmp/m.json")
        assert s3put("/tmp/m.json", f"channels/{channel}.json") == "200"

    brick.wait_for_unit("mortar-update-init.service")
    running = brick.succeed("readlink -f /run/current-system").strip()
    # test VMs boot without a system profile; an installed Brick has one
    brick.succeed(f"nix-env -p /nix/var/nix/profiles/system --set {running}")

    with subtest("the read key cannot write"):
        ci.succeed("echo x > /tmp/x")
        assert s3put("/tmp/x", "channels/mortar.json", r_id, r_sec) == "403"

    with subtest("no credentials: unconfigured"):
        assert status()["state"] == "unconfigured", status()
        assert check(expect_ok=False)["state"] == "unconfigured"

    with subtest("credentials from the card's mortar.toml"):
        brick.succeed("mkdir -p /boot && cat > /boot/mortar.toml <<'EOF'\n"
                      "hostname = \"brick\"\n\n[update]\n"
                      f"access_key_id = \"{r_id}\"\n"
                      f"secret_access_key = \"{r_sec}\"\n"
                      "auto_check = false\nEOF")
        brick.succeed("systemctl restart mortar-update-init.service")
        brick.succeed("test \"$(stat -c %a /var/lib/mortar-update/credentials)\" = 600")
        brick.fail(f"grep -q {r_sec} /boot/mortar.toml")
        brick.succeed(f"grep -q 'access_key_id = \"{r_id}\"' /boot/mortar.toml")
        brick.succeed("grep -q 'hostname = \"brick\"' /boot/mortar.toml")
        s = status()
        assert s["state"] == "idle" and s["autoCheck"] is False, s

    with subtest("nothing published yet"):
        s = check()
        assert s["state"] == "up-to-date", s

    with subtest("an unsigned manifest is refused"):
        put_manifest("${target}", 1000, sign=None)
        s = check(expect_ok=False)
        assert s["state"] == "error" and "isn't signed" in s["detail"], s
        brick.fail("nix-store --check-validity ${target}")

    with subtest("a tampered manifest is refused"):
        put_manifest("${target}", 1000)
        ci.succeed("jq '.published = 2000' /tmp/m.json > /tmp/t.json")
        assert s3put("/tmp/t.json", "channels/mortar.json") == "200"
        s = check(expect_ok=False)
        assert s["state"] == "error" and "bad signature" in s["detail"], s

    with subtest("a manifest signed by another key is refused"):
        put_manifest("${target}", 1000, sign="/root/other-key")
        s = check(expect_ok=False)
        assert s["state"] == "error" and "doesn't trust" in s["detail"], s

    with subtest("another channel's manifest, signed, is refused"):
        put_manifest("${target}", 1000, channel="testing")
        ci.succeed("curl -sf --aws-sigv4 aws:amz:auto:s3 --user " + w_id + ":" + w_sec + " -T /tmp/m.json http://localhost:3900/cache/channels/mortar.json")
        s = check(expect_ok=False)
        assert s["message"] == "Wrong channel", s

    with subtest("another configuration's manifest, signed, is refused"):
        put_manifest("${target}", 1000, config="brick-full")
        s = check(expect_ok=False)
        assert s["message"] == "Different configuration" and "brick-full" in s["detail"], s
        brick.fail("nix-store --check-validity ${target}")

    with subtest("an unsigned closure is refused"):
        ci.succeed(f"{w_env} nix copy --to '{store}' ${unsigned}")
        put_manifest("${unsigned}", 1000)
        s = check(expect_ok=False)
        assert s["state"] == "error" and "signed" in s["detail"], s
        brick.fail("nix-store --check-validity ${unsigned}")  # the VM sees the host store; validity is what counts
        assert brick.succeed("readlink -f /nix/var/nix/profiles/system").strip() == running

    with subtest("a closure signed with a key only nix.conf trusts is refused"):
        brick.succeed("grep -q other-cache-1 /etc/nix/nix.conf")
        ci.succeed(f"{w_env} nix copy --to '{store}&secret-key=/root/other-key' ${otherSigned}")
        put_manifest("${otherSigned}", 1000)
        s = check(expect_ok=False)
        assert s["state"] == "error" and "signed" in s["detail"], s
        brick.fail("nix-store --check-validity ${otherSigned}")

    with subtest("publish a signed system"):
        print(publish("${target}"))
        m = json.loads(s3cat("channels/mortar.json"))
        assert m["toplevel"] == "${target}" and m["rev"] == "0123456789abcdef" and m["published"] > 1000, m
        assert m["system"] == "${pkgs.stdenv.hostPlatform.system}", m
        assert m["signature"].startswith("mortar-test-1:"), m
        # publishing the same system again changes nothing
        out = publish("${target}")
        assert "Unchanged" in out, out

    with subtest("low battery waits"):
        brick.succeed("mkdir -p /tmp/ps/bat && echo Battery > /tmp/ps/bat/type && echo 12 > /tmp/ps/bat/capacity && echo Discharging > /tmp/ps/bat/status")
        brick.succeed("jq '.powerSupplyDir = \"/tmp/ps\"' /etc/mortar-update/config.json > /tmp/lowbat.json")
        brick.succeed("mortar-update --config /tmp/lowbat.json check")
        s = status()
        assert s["state"] == "low-battery" and s["available"]["toplevel"] == "${target}", s
        brick.fail("nix-store --check-validity ${target}")

    with subtest("download and stage"):
        brick.succeed("systemctl start mortar-update.service")
        s = status()
        assert s["state"] == "ready" and s["message"] == "Restart to update", s
        assert brick.succeed("readlink -f /nix/var/nix/profiles/system").strip() == "${target}"
        assert brick.succeed("readlink -f /run/current-system").strip() == running
        brick.succeed("nix --extra-experimental-features nix-command path-info --sigs ${target} | grep -q mortar-test-1")
        brick.succeed("grep -qx 2 ${target}/etc/mortar-test-generation")
        assert brick.succeed("nix-env -p /nix/var/nix/profiles/system --list-generations | wc -l").strip() == "2"
        # again: already staged
        assert check()["state"] == "ready"

    with subtest("offline"):
        ci.succeed("systemctl stop garage.service")
        s = check()
        assert s["state"] == "offline", s
        ci.succeed("systemctl start garage.service")
        ci.wait_for_open_port(3900)

    with subtest("rollback, and the same manifest is not taken again"):
        brick.succeed("mortar-update rollback")
        assert brick.succeed("readlink -f /nix/var/nix/profiles/system").strip() == running
        s = check()
        assert s["state"] == "up-to-date", s
        assert brick.succeed("readlink -f /nix/var/nix/profiles/system").strip() == running

    with subtest("an older manifest, properly signed, is a downgrade"):
        put_manifest("${target}", 1500)
        s = check(expect_ok=False)
        assert s["message"] == "Update is older", s
        assert brick.succeed("readlink -f /nix/var/nix/profiles/system").strip() == running
        brick.succeed("mortar-update check --force 2>&1 | grep -q 'Update is older' || true")
        assert brick.succeed("readlink -f /nix/var/nix/profiles/system").strip() == running

    with subtest("--allow-downgrade takes it"):
        brick.succeed("mortar-update check --allow-downgrade")
        assert status()["state"] == "ready", status()
        assert brick.succeed("readlink -f /nix/var/nix/profiles/system").strip() == "${target}"
        brick.succeed("mortar-update rollback")

    with subtest("--allow-config-change takes another configuration"):
        put_manifest("${target}", 9999999999, config="brick-full")
        assert check(expect_ok=False)["message"] == "Different configuration"
        brick.succeed("mortar-update check --allow-config-change")
        assert status()["state"] == "ready", status()
        brick.succeed("mortar-update rollback")

    with subtest("auto-check toggle"):
        brick.succeed("systemctl start mortar-update-auto@on.service")
        assert status()["autoCheck"] is True
        brick.succeed("systemctl start mortar-update-auto@off.service")
        assert status()["autoCheck"] is False
        # the daily run does nothing while it is off
        brick.succeed("systemctl start mortar-update-auto.service")
        brick.succeed("journalctl -u mortar-update-auto.service | grep -q 'auto-check is off'")
  '';
}
