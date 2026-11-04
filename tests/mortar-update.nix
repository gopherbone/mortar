# SPDX-License-Identifier: GPL-2.0-or-later
# End to end: over-the-air updates, both ways a Brick reads them.
#
#   ci      Garage (an S3 server standing in for Cloudflare R2, region
#           "auto" like R2) and the publisher: `mortar publish --path …`
#           pushes a signed closure and the manifest with a write key.
#           Also, for HTTPS mode, plain HTTP: cloudflare/worker.js in front of a
#           directory holding what `mortar publish --upstream` put in the bucket
#           (:8080), and a second cache standing in for cache.nixos.org (:8081).
#   brick   S3 mode: modules/core/update.nix with a read-only key from the card's
#           mortar.toml; `mortar-update check` downloads and stages
#   public  HTTPS mode: the same module and no credentials at all
#
# Also: the read key cannot write; unsigned or tampered manifests, closures
# not signed by the publisher (even with a key nix.conf trusts), another
# channel's or configuration's manifest and older ones are all refused; low
# battery waits, offline is reported, rollback, and a rolled-back Brick does
# not take the same manifest again. All of that which depends on the source holds
# for `public` too, and a path its upstream serves is checked against that
# upstream's key and the publisher's and no others.
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

  # What stands in for cache.nixos.org signs with this.
  upstreamKey = pkgs.runCommand "mortar-update-test-upstream-key" { nativeBuildInputs = [ pkgs.nix ]; } ''
    export HOME=$TMPDIR
    mkdir $out
    nix --extra-experimental-features nix-command key generate-secret --key-name upstream-1 > $out/secret
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

  # In the next system's closure and only on the upstream cache.
  upstreamOnly = pkgs.writeText "from-the-upstream-cache" "upstream\n";

  # HTTPS mode's system: most of it is the publisher's, this one path the upstream's.
  publicTarget =
    (pkgs.nixos {
      boot.isContainer = true;
      networking.hostName = "updated-public";
      environment.etc."mortar-test-generation".text = "3";
      environment.etc."mortar-test-upstream".source = upstreamOnly;
      system.stateVersion = "25.05";
    }).config.system.build.toplevel;

  # On the upstream cache, but signed by a key that isn't the upstream's.
  upstreamWrongSigner = pkgs.writeTextFile {
    name = "nixos-system-upstream-wrong-signer";
    destination = "/bin/switch-to-configuration";
    executable = true;
    text = "#!/bin/sh\necho should never run; exit 1\n";
  };

  # Properly signed, and its NAR is changed afterwards.
  corrupted = pkgs.writeTextFile {
    name = "nixos-system-corrupted";
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
      # the Worker over HTTP, in front of a directory (what R2 is to it at the public URL)
      systemd.services.public-cache = {
        wantedBy = [ "multi-user.target" ];
        serviceConfig.ExecStart = "${pkgs.nodejs}/bin/node ${../cloudflare}/test/serve.mjs /srv/public 8080";
      };
      systemd.services.upstream-cache = {
        wantedBy = [ "multi-user.target" ];
        serviceConfig.ExecStart = "${pkgs.python3}/bin/python3 -m http.server 8081 --directory /srv/upstream";
      };
      systemd.tmpfiles.rules = [ "d /srv/public 0755 root root -" "d /srv/upstream 0755 root root -" ];
      networking.firewall.allowedTCPPorts = [ 3900 8080 8081 ];
      environment.systemPackages = [ mortar pkgs.curl pkgs.jq ];
      virtualisation.additionalPaths = [ target unsigned otherSigned publicTarget upstreamWrongSigner corrupted ];
      virtualisation.memorySize = 2048;
      virtualisation.diskSize = 6144;
      nix.settings.experimental-features = [ "nix-command" ];
    };

  nodes.brick =
    { ... }:
    {
      imports = [ ../modules/core/update.nix ];
      # The Brick's bootloader, on every platform (x86's default is GRUB, which won't
      # install on the test VM's disk).
      boot.loader.grub.enable = false;
      boot.loader.generic-extlinux-compatible.enable = true;
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

  nodes.public =
    { ... }:
    {
      imports = [ ../modules/core/update.nix ];
      # The Brick's bootloader, on every platform (x86's default is GRUB, which won't
      # install on the test VM's disk).
      boot.loader.grub.enable = false;
      boot.loader.generic-extlinux-compatible.enable = true;
      mortar.update = {
        enable = true;
        baseUrl = "http://ci:8080";
        upstreams = [ { url = "http://ci:8081"; publicKey = builtins.readFile "${upstreamKey}/public"; } ];
        inherit publicKey;
        channel = "testing";
        configName = "test";
        minFreeMiB = 64;
      };
      nix.settings.trusted-public-keys = [ (builtins.readFile "${otherKey}/public") ];
      environment.systemPackages = [ pkgs.curl pkgs.jq ];
      virtualisation.memorySize = 2048;
      virtualisation.diskSize = 4096;
      virtualisation.writableStore = true;
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
        assert s3put("/tmp/m.json", "channels/mortar/test.json") == "200"

    brick.wait_for_unit("mortar-update-init.service")
    running = brick.succeed("readlink -f /run/current-system").strip()
    # test VMs boot without a system profile; an installed Brick has one
    brick.succeed(f"nix-env -p /nix/var/nix/profiles/system --set {running}")

    with subtest("the read key cannot write"):
        ci.succeed("echo x > /tmp/x")
        assert s3put("/tmp/x", "channels/mortar/test.json", r_id, r_sec) == "403"

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
        assert s3put("/tmp/t.json", "channels/mortar/test.json") == "200"
        s = check(expect_ok=False)
        assert s["state"] == "error" and "bad signature" in s["detail"], s

    with subtest("a manifest signed by another key is refused"):
        put_manifest("${target}", 1000, sign="/root/other-key")
        s = check(expect_ok=False)
        assert s["state"] == "error" and "doesn't trust" in s["detail"], s

    with subtest("another channel's manifest, signed, is refused"):
        put_manifest("${target}", 1000, channel="testing")
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
        m = json.loads(s3cat("channels/mortar/test.json"))
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

    with subtest("testing is not the release channel, until it's promoted"):
        # the release channel holds something else
        put_manifest("${unsigned}", 9999999999)
        before = s3cat("channels/mortar/test.json")
        print(publish("${target}", "--channel testing"))
        t = json.loads(s3cat("channels/testing/test.json"))
        assert t["channel"] == "testing" and t["toplevel"] == "${target}", t
        assert s3cat("channels/mortar/test.json") == before
        assert brick.succeed("readlink -f /nix/var/nix/profiles/system").strip() == running

        # the wrong commit is refused
        ci.fail(f"{w_env} mortar promote --endpoint http://ci:3900 --bucket cache --key /root/signing-key --config test --rev nope 2>&1")
        assert s3cat("channels/mortar/test.json") == before

        print(ci.succeed(f"{w_env} mortar promote --endpoint http://ci:3900 --bucket cache --key /root/signing-key --config test --rev 0123456789abcdef 2>&1"))
        m = json.loads(s3cat("channels/mortar/test.json"))
        assert m["channel"] == "mortar" and m["toplevel"] == "${target}" and m["published"] > 9999999999, m
        assert check()["state"] == "ready", status()
        assert brick.succeed("readlink -f /nix/var/nix/profiles/system").strip() == "${target}"
        brick.succeed("mortar-update rollback")

    with subtest("auto-check toggle"):
        brick.succeed("systemctl start mortar-update-auto@on.service")
        assert status()["autoCheck"] is True
        brick.succeed("systemctl start mortar-update-auto@off.service")
        assert status()["autoCheck"] is False
        # the daily run does nothing while it is off
        brick.succeed("systemctl start mortar-update-auto.service")
        brick.succeed("journalctl -u mortar-update-auto.service | grep -q 'auto-check is off'")

    ### HTTPS mode: no credentials, nothing but a served directory

    upstream_hash = "${upstreamOnly}".split("/")[-1][:32]
    target_hash = "${publicTarget}".split("/")[-1][:32]
    bucket_url = "http://localhost:3900/public"
    public_args = "--endpoint http://ci:3900 --bucket public --key /root/signing-key --config test --channel testing"

    def bucket_get(key):
        return ci.succeed(f"curl -sf --aws-sigv4 aws:amz:auto:s3 --user {w_id}:{w_sec} '{bucket_url}/{key}'")

    def mirror():
        # the bucket, as the Worker's binding sees it at the public URL
        keys = []
        while True:
            after = f"&start-after={keys[-1]}" if keys else ""
            listing = bucket_get(f"?list-type=2{after}")
            keys += re.findall("<Key>([^<]+)</Key>", listing)
            if "<IsTruncated>true" not in listing:
                break
        listed = "\n".join(keys)
        # store paths don't change; everything else is fetched again
        ci.succeed(f"while read -r k; do case \"$k\" in nar/*|*.narinfo) [ -e \"/srv/public/$k\" ] && continue ;; esac; "
                   f"curl -sf --create-dirs --aws-sigv4 aws:amz:auto:s3 --user {w_id}:{w_sec} "
                   f"-o \"/srv/public/$k\" \"{bucket_url}/$k\"; done <<'EOF'\n{listed}\nEOF")
        return keys

    def pub_status():
        return json.loads(public.succeed("cat /run/mortar-update/status.json"))

    def pub_check(expect_ok=True, args=""):
        if expect_ok:
            print(public.succeed(f"mortar-update check {args} 2>&1"))
        else:
            print(public.fail(f"mortar-update check {args} 2>&1"))
        return pub_status()

    def serve_manifest(toplevel, published, sign="/root/signing-key", channel="testing", config="test"):
        m = {"schema": 1, "channel": channel, "system": "${pkgs.stdenv.hostPlatform.system}", "config": config,
             "toplevel": toplevel, "published": published, "date": "1970-01-01T00:16:40Z", "changelog": ["evil"]}
        ci.succeed("mkdir -p /srv/public/channels/testing")
        ci.succeed(f"cat > /tmp/p.json <<'EOF'\n{json.dumps(m)}\nEOF")
        if sign is not None:
            ci.succeed(f"mortar sign-manifest --key {sign} < /tmp/p.json > /tmp/signed.json && mv /tmp/signed.json /tmp/p.json")
        ci.succeed("cp /tmp/p.json /srv/public/channels/testing/test.json")

    def assert_not_taken(path):
        public.fail(f"nix-store --check-validity {path}")
        assert public.succeed("readlink -f /nix/var/nix/profiles/system").strip() == pub_running

    ci.wait_for_unit("public-cache.service")
    ci.wait_for_unit("upstream-cache.service")
    ci.wait_for_open_port(8080)
    ci.wait_for_open_port(8081)
    ci.succeed("install -m 600 ${upstreamKey}/secret /root/upstream-key")
    public.wait_for_unit("mortar-update-init.service")
    pub_running = public.succeed("readlink -f /run/current-system").strip()
    public.succeed(f"nix-env -p /nix/var/nix/profiles/system --set {pub_running}")

    with subtest("https: set up with no credentials"):
        s = pub_status()
        assert s["state"] == "idle" and s["message"] == "Check now", s
        public.fail("test -e /var/lib/mortar-update/credentials")
        out = public.fail("echo 'access_key_id = \"x\"' | mortar-update provision --stdin 2>&1")
        assert "no credentials" in out, out
        public.fail("test -e /var/lib/mortar-update/credentials")

    with subtest("https: the card's [update] section needs no credentials"):
        public.succeed("mkdir -p /boot && printf '[update]\nauto_check = false\n' > /boot/mortar.toml")
        public.succeed("systemctl restart mortar-update-init.service")
        s = pub_status()
        assert s["state"] == "idle" and s["autoCheck"] is False, s

    with subtest("https: nothing published yet"):
        s = pub_check()
        assert s["state"] == "up-to-date" and "nothing published" in s["detail"], s

    with subtest("https: refused manifests"):
        serve_manifest("${publicTarget}", 1000, sign=None)
        s = pub_check(expect_ok=False)
        assert s["state"] == "error" and "isn't signed" in s["detail"], s

        serve_manifest("${publicTarget}", 1000)
        ci.succeed("jq '.published = 2000' /tmp/p.json > /srv/public/channels/testing/test.json")
        s = pub_check(expect_ok=False)
        assert s["state"] == "error" and "bad signature" in s["detail"], s

        serve_manifest("${publicTarget}", 1000, sign="/root/other-key")
        s = pub_check(expect_ok=False)
        assert s["state"] == "error" and "doesn't trust" in s["detail"], s

        # the upstream's key vouches for its store paths, not for what a Brick should run
        serve_manifest("${publicTarget}", 1000, sign="/root/upstream-key")
        s = pub_check(expect_ok=False)
        assert s["state"] == "error" and "doesn't trust" in s["detail"], s

        serve_manifest("${publicTarget}", 1000, channel="mortar")
        assert pub_check(expect_ok=False)["message"] == "Wrong channel"

        serve_manifest("${publicTarget}", 1000, config="brick-full")
        s = pub_check(expect_ok=False)
        assert s["message"] == "Different configuration" and "brick-full" in s["detail"], s
        assert_not_taken("${publicTarget}")

    with subtest("https: refused closures"):
        ci.succeed("rm -f /srv/public/channels/testing/test.json")
        # no signature at all
        ci.succeed("nix copy --to file:///srv/public ${unsigned}")
        serve_manifest("${unsigned}", 1000)
        s = pub_check(expect_ok=False)
        assert s["state"] == "error" and "signed" in s["detail"], s
        assert_not_taken("${unsigned}")

        # signed by a key that nix.conf trusts, and this update doesn't
        public.succeed("grep -q other-cache-1 /etc/nix/nix.conf")
        ci.succeed("nix copy --to 'file:///srv/public?secret-key=/root/other-key' ${otherSigned}")
        serve_manifest("${otherSigned}", 1000)
        s = pub_check(expect_ok=False)
        assert s["state"] == "error" and "signed" in s["detail"], s
        assert_not_taken("${otherSigned}")

        # coming through the upstream changes nothing
        ci.succeed("nix copy --to 'file:///srv/upstream?secret-key=/root/other-key' ${upstreamWrongSigner}")
        serve_manifest("${upstreamWrongSigner}", 1000)
        s = pub_check(expect_ok=False)
        assert s["state"] == "error" and "signed" in s["detail"], s
        assert_not_taken("${upstreamWrongSigner}")

        # signed, and the bytes changed since: the store path never appears
        ci.succeed("nix copy --to 'file:///srv/public?secret-key=/root/signing-key&compression=none' ${corrupted}")
        h = "${corrupted}".split("/")[-1][:32]
        nar = field("URL: (\\S+)", ci.succeed(f"cat /srv/public/{h}.narinfo"))
        ci.succeed(f"printf x | dd of=/srv/public/{nar} bs=1 seek=200 conv=notrunc")
        serve_manifest("${corrupted}", 1000)
        public.succeed("mortar-update check >/dev/null 2>&1 || true")
        s = pub_status()
        assert s["state"] in ("error", "offline"), s
        assert_not_taken("${corrupted}")

    with subtest("https: publish, and only what the upstream doesn't have is uploaded"):
        ci.succeed("rm -rf /srv/public/* /srv/public/.[!.]*")
        ci.succeed("garage bucket create public && garage bucket allow --read --write public --key ci")
        ci.succeed("nix copy --to 'file:///srv/upstream?secret-key=/root/upstream-key&compression=zstd' ${upstreamOnly}")
        out = ci.succeed(f"{w_env} MORTAR_REV=fedcba9876543210 mortar publish {public_args} --upstream http://ci:8081 --path ${publicTarget} 2>&1")
        print(out)
        keys = mirror()
        print(keys)
        assert f"{upstream_hash}.narinfo" not in keys, keys
        assert f"{target_hash}.narinfo" in keys, keys
        assert "channels/testing/test.json" in keys, keys
        infos = [k for k in keys if k.endswith(".narinfo")]
        closure = ci.succeed("nix-store --query --requisites ${publicTarget}").split()
        assert len(infos) == len(closure) - 1, (len(infos), len(closure))
        for k in infos:
            url = field("URL: (\\S+)", ci.succeed(f"cat /srv/public/{k}"))
            assert url in keys, (k, url)
        info = ci.succeed("curl -sf http://localhost:8080/nix-cache-info")
        assert "StoreDir: /nix/store" in info and "WantMassQuery: 1" in info and "Priority: 41" in info, info
        assert "Sig: mortar-test-1:" in ci.succeed(f"cat /srv/public/{target_hash}.narinfo")

    with subtest("https: the Worker answers Nix and curl as a binary cache"):
        public.succeed("nix --extra-experimental-features nix-command path-info --store http://ci:8080 ${publicTarget}")
        code = public.succeed(f"curl -s -o /tmp/part -r 0-3 -w '%{{http_code}}' http://ci:8080/{target_hash}.narinfo").strip()
        assert code == "206", code
        assert public.succeed("cat /tmp/part") == "Stor"
        assert public.succeed(f"curl -s -o /dev/null -w '%{{http_code}}' -X PUT -d x http://ci:8080/{target_hash}.narinfo").strip() == "405"
        assert public.succeed("curl -s -o /dev/null -w '%{http_code}' http://ci:8080/").strip() == "200"
        assert public.succeed("curl -s -o /dev/null -w '%{http_code}' http://ci:8080/channels/").strip() == "404"
        assert "immutable" in public.succeed(f"curl -sI http://ci:8080/{target_hash}.narinfo")

    with subtest("https: download and stage, each path from its cache"):
        public.succeed("systemctl start mortar-update.service")
        s = pub_status()
        assert s["state"] == "ready" and s["message"] == "Restart to update", s
        assert public.succeed("readlink -f /nix/var/nix/profiles/system").strip() == "${publicTarget}"
        assert public.succeed("readlink -f /run/current-system").strip() == pub_running
        public.succeed("nix --extra-experimental-features nix-command path-info --sigs ${publicTarget} | grep -q mortar-test-1")
        public.succeed("nix --extra-experimental-features nix-command path-info --sigs ${upstreamOnly} | grep -q upstream-1")
        public.succeed("grep -qx upstream ${upstreamOnly}")
        public.succeed("grep -qx 3 ${publicTarget}/etc/mortar-test-generation")
        public.fail("test -e /var/lib/mortar-update/credentials")
        assert pub_check()["state"] == "ready"

    with subtest("https: publishing again uploads nothing"):
        out = ci.succeed(f"{w_env} MORTAR_REV=fedcba9876543210 mortar publish {public_args} --upstream http://ci:8081 --path ${publicTarget} --force 2>&1")
        print(out)
        assert " 0 to upload" in out, out
        mirror()
        # --force gave it a new date: the Brick notes that, and has nothing to fetch
        assert pub_check()["state"] == "ready"

    with subtest("https: offline"):
        ci.succeed("systemctl stop public-cache.service")
        s = pub_check()
        assert s["state"] == "offline", s
        ci.succeed("systemctl start public-cache.service")
        ci.wait_for_open_port(8080)

    with subtest("https: rollback, and the same manifest is not taken again"):
        public.succeed("mortar-update rollback")
        assert public.succeed("readlink -f /nix/var/nix/profiles/system").strip() == pub_running
        s = pub_check()
        assert s["state"] == "up-to-date", s
        assert public.succeed("readlink -f /nix/var/nix/profiles/system").strip() == pub_running

    with subtest("https: an older manifest, properly signed, is a downgrade"):
        serve_manifest("${publicTarget}", 1500)
        s = pub_check(expect_ok=False)
        assert s["message"] == "Update is older", s
        assert public.succeed("readlink -f /nix/var/nix/profiles/system").strip() == pub_running
        public.succeed("mortar-update check --allow-downgrade")
        assert pub_status()["state"] == "ready", pub_status()
        assert public.succeed("readlink -f /nix/var/nix/profiles/system").strip() == "${publicTarget}"
        public.succeed("mortar-update rollback")

    with subtest("images: published on testing, promoted by hand with the manifest"):
        ci.succeed("head -c 3000000 /dev/urandom > /tmp/image.img.zst")
        print(ci.succeed(f"{w_env} MORTAR_REV=fedcba9876543210 mortar publish-image --image /tmp/image.img.zst "
                         "--endpoint http://ci:3900 --bucket public --config test --channel testing 2>&1"))
        keys = mirror()
        for k in ["images/testing/test-latest.img.zst", "images/testing/test-latest.img.zst.sha256",
                  "images/testing/v/test-fedcba987654.img.zst", "images/testing/v/test-fedcba987654.img.zst.sha256"]:
            assert k in keys, (k, keys)
        assert "images/mortar/test-latest.img.zst" not in keys, keys

        print(ci.succeed(f"{w_env} mortar promote --endpoint http://ci:3900 --bucket public --key /root/signing-key "
                         "--config test --from testing --channel mortar --rev fedcba9876543210 2>&1"))
        keys = mirror()
        assert "channels/mortar/test.json" in keys, keys
        assert json.loads(bucket_get("channels/mortar/test.json"))["channel"] == "mortar"

        # as someone downloading it: through the Worker, resumed, checked
        public.succeed("mkdir -p /tmp/dl && cd /tmp/dl && "
                       "curl -sf -r 0-99999 -o test-latest.img.zst http://ci:8080/images/mortar/test-latest.img.zst && "
                       "curl -sf -C - -o test-latest.img.zst http://ci:8080/images/mortar/test-latest.img.zst && "
                       "curl -sf -O http://ci:8080/images/mortar/test-latest.img.zst.sha256 && "
                       "sha256sum -c test-latest.img.zst.sha256")
        assert "max-age=60" in public.succeed("curl -sI http://ci:8080/images/mortar/test-latest.img.zst")
        assert "immutable" in public.succeed("curl -sI http://ci:8080/images/mortar/v/test-fedcba987654.img.zst")
  '';
}
