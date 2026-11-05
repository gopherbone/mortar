# SPDX-License-Identifier: GPL-2.0-or-later
# End to end: the Packages service on a real Nix store, in the style of
# mortar-update.nix.
#
#   ci     a bucket (Garage) for `mortar publish --catalog` and `promote`; the Worker in
#          front of a directory (:8080), a stand-in for cache.nixos.org (:8081), a slow
#          cache for another repository (:8082), and a third party's cache (:8083)
#   brick  modules/core/packages.nix and update.nix, with the shell's headless backend
#          running as the session user, so discovery is the real thing
#
# Packages: install; a bad signature, the wrong device, an old index and too little room
# refused; cancel leaving nothing; remove while it runs; update and rollback; the daily
# check; tools on Terminal's path, from a large index, and the shell seeing it all.
#
# The system guard: a path of a new system planted in the store with a third party's
# signature blocks the update; the same contents, or no signature at all (a card, a
# deploy from a computer), don't.
#
#   nix build .#checks.aarch64-linux.mortar-packages   (a Mac: via its linux-builder)
{ pkgs, mkApp, mkCatalog }:

let
  makeKey = name: pkgs.runCommand "mortar-packages-test-key-${name}" { nativeBuildInputs = [ pkgs.nix ]; } ''
    export HOME=$TMPDIR
    mkdir $out
    nix --extra-experimental-features nix-command key generate-secret --key-name ${name} > $out/secret
    nix --extra-experimental-features nix-command key convert-secret-to-public < $out/secret > $out/public
  '';
  testKey = makeKey "mortar-test-1";
  upstreamKey = makeKey "upstream-1";
  otherKey = makeKey "other-repo-1";
  thirdKey = makeKey "third-party-1";
  publicKey = builtins.readFile "${testKey}/public";
  system = pkgs.stdenv.hostPlatform.system;

  mortar = pkgs.callPackage ../pkgs/mortar-cli { };
  mortar-shell = pkgs.callPackage ../pkgs/mortar-shell { };
  apps = "/nix/var/nix/profiles/apps";

  # An app, with some bytes to download.
  app =
    { id, version, payload ? 4, requires ? [ ], manifestId ? id }:
    mkApp pkgs {
      package =
        (pkgs.runCommand "${id}-${version}" {
          inherit version;
          meta = {
            mainProgram = id;
            homepage = "https://example.org/${id}";
            license = pkgs.lib.licenses.mit;
          };
        } ''
          mkdir -p $out/bin $out/share/${id}
          head -c ${toString (payload * 1024 * 1024)} /dev/urandom > $out/share/${id}/data
          cat > $out/bin/${id} <<EOF
          #!${pkgs.runtimeShell}
          echo "\$\$" > "\$HOME/${id}.pid"
          echo ${version} > "\$HOME/${id}.version"
          exec ${pkgs.coreutils}/bin/sleep 3600
          EOF
          chmod +x $out/bin/${id}
        '');
      id = manifestId;
      name = "Demo ${id}";
      description = "A made-up app";
      category = "games";
      inherit requires;
    };

  demo1 = app { id = "demo"; version = "1.0"; };
  demo2 = app { id = "demo"; version = "2.0"; };
  demo3 = app { id = "demo"; version = "3.0"; };
  slow = app { id = "slowpoke"; version = "1.0"; payload = 40; };
  needsFiles = app { id = "needy"; version = "1.0"; requires = [ "~/needy/data.bin" ]; };
  huge = app { id = "huge"; version = "1.0"; payload = 8; };
  liar = app { id = "liar-app"; version = "1.0"; manifestId = "something-else"; };

  # A terminal tool, on the upstream's cache and signed there.
  tool =
    { name, version, command }:
    pkgs.runCommand "${name}-${version}" { } ''
      mkdir -p $out/bin
      cat > $out/bin/${command} <<EOF
      #!${pkgs.runtimeShell}
      echo ${name} ${version}
      EOF
      chmod +x $out/bin/${command}
    '';
  ripgrep1 = tool { name = "ripthing"; version = "1.0"; command = "rt"; };
  ripgrep2 = tool { name = "ripthing"; version = "2.0"; command = "rt"; };
  # a curated tool, the repository's own
  curatedTool = tool { name = "curated"; version = "1.0"; command = "curated-cmd"; };
  # on the repository's cache, not the upstream's: a large index can't get it installed
  notUpstream = tool { name = "notupstream"; version = "1.0"; command = "nu"; };

  # As a generator that has only evaluated nixpkgs: it knows the paths' names, and has
  # none of them as inputs. Given the packages as inputs, Nix would find their hashes in
  # the file and make them its references, and fetching the index would fetch them all.
  toolsIndex =
    { rows, name ? "mortar-tools-index", withInputs ? false }:
    pkgs.runCommand name { } ''
      mkdir $out
      printf '#mortar-tools\t1\n' > $out/tools.tsv
      ${pkgs.lib.concatMapStringsSep "\n" (r: ''
        p=${if withInputs then toString r.path else builtins.unsafeDiscardStringContext (toString r.path)}; b=$(basename $p)
        printf '%s\t%s\t%s\t%s\t%s\n' ${r.attr} "''${b:33}" "''${b:0:32}" ${r.program} ${pkgs.lib.escapeShellArg r.description} >> $out/tools.tsv
      '') rows}
    '';
  tsv1 = toolsIndex {
    rows = [
      { attr = "notupstream"; path = notUpstream; program = "nu"; description = "Only on the repository's cache"; }
      { attr = "ripthing"; path = ripgrep1; program = "rt"; description = "Rips through things"; }
    ];
  };
  dragsTools = tool { name = "dragged-along"; version = "1.0"; command = "da"; };
  tsvBad = toolsIndex {
    name = "mortar-tools-index-that-refers";
    withInputs = true;
    rows = [ { attr = "dragged"; path = dragsTools; program = "da"; description = "Comes with the list"; } ];
  };

  tsv2 = toolsIndex {
    name = "mortar-tools-index-2";
    rows = [ { attr = "ripthing"; path = ripgrep2; program = "rt"; description = "Rips through things"; } ];
  };

  # mkCatalog on the real thing: what `mortar publish --catalog` takes.
  catalog = mkCatalog pkgs {
    devices.brick = [
      { app = demo1; licence = "MIT"; source = "https://example.org/demo"; }
      needsFiles
    ];
  };

  # What stands in for a new system: the bits of one the update looks at, and one
  # path it would run as root.
  fakeSystem =
    name: dep:
    pkgs.writeTextFile {
      name = "nixos-system-${name}";
      destination = "/bin/switch-to-configuration";
      executable = true;
      text = ''
        #!${pkgs.runtimeShell}
        # refers to ${dep}
        exit 0
      '';
    };
  plantedEvil = pkgs.writeText "planted-evil" "what Mortar published\n";
  plantedSame = pkgs.writeText "planted-same" "what Mortar published\n";
  plantedFake = pkgs.writeText "planted-fake-signature" "what Mortar published\n";
  plantedBare = pkgs.writeText "planted-no-signature" "what Mortar published\n";
  plantedOwn = pkgs.writeText "planted-own-build" "what Mortar published\n";
  sysEvil = fakeSystem "evil" plantedEvil;
  sysSame = fakeSystem "same" plantedSame;
  sysFake = fakeSystem "fake" plantedFake;
  sysBare = fakeSystem "bare" plantedBare;
  sysOwn = fakeSystem "own" plantedOwn;

  # The evil twin: another NAR under the path of something real, signed by whoever.
  makeNarinfo = pkgs.writeText "make-narinfo.py" ''
    import base64, hashlib, os, subprocess, sys
    import nacl.signing
    # make-narinfo CACHE_DIR STORE_PATH CONTENT_FILE [KEY_FILE] [FAKE_NAME]
    cache, path, content = sys.argv[1:4]
    key = sys.argv[4] if len(sys.argv) > 4 and sys.argv[4] != "-" else None
    fake = sys.argv[5] if len(sys.argv) > 5 else None
    nar = subprocess.check_output(["nix-store", "--dump", content])
    os.makedirs(f"{cache}/nar", exist_ok=True)
    h = hashlib.sha256(nar).hexdigest()
    name = f"nar/{h}.nar"
    open(f"{cache}/{name}", "wb").write(nar)
    b32 = subprocess.check_output(["nix-hash", "--type", "sha256", "--to-base32", h], text=True).strip()
    lines = [f"StorePath: {path}", f"URL: {name}", "Compression: none", f"FileHash: sha256:{b32}", f"FileSize: {len(nar)}",
             f"NarHash: sha256:{b32}", f"NarSize: {len(nar)}", "References: "]
    fingerprint = f"1;{path};sha256:{b32};{len(nar)};".encode()
    if key is not None:
        kname, b = open(key).read().strip().split(":", 1)
        sk = nacl.signing.SigningKey(base64.b64decode(b)[:32])
        lines.append(f"Sig: {kname}:{base64.b64encode(sk.sign(fingerprint).signature).decode()}")
    if fake is not None:
        lines.append(f"Sig: {fake}:{base64.b64encode(bytes(64)).decode()}")
    lines.append("")
    open(f"{cache}/{path.split('/')[-1][:32]}.narinfo", "w").write("\n".join(lines))
    if not os.path.exists(f"{cache}/nix-cache-info"):
        open(f"{cache}/nix-cache-info", "w").write("StoreDir: /nix/store\nWantMassQuery: 1\nPriority: 50\n")
  '';
  py = pkgs.python3.withPackages (ps: [ ps.pynacl ]);

  # A binary cache that dawdles over NARs: time to look at an install, and to cancel it.
  slowServer = pkgs.writeText "slow-server.py" ''
    import http.server, sys, time
    class H(http.server.SimpleHTTPRequestHandler):
        def copyfile(self, source, outputfile):
            slow = self.path.startswith("/nar/")
            while True:
                b = source.read(256 * 1024)
                if not b:
                    return
                outputfile.write(b)
                if slow:
                    outputfile.flush()
                    time.sleep(0.25)
    http.server.ThreadingHTTPServer.allow_reuse_address = True
    http.server.ThreadingHTTPServer(("", int(sys.argv[1])), lambda *a: H(*a, directory=sys.argv[2])).serve_forever()
  '';

  sdlRun = pkgs.writeShellScriptBin "mortar-sdl-run" ''exec "$@"'';
  shellConfig = pkgs.writeText "shell.json" (builtins.toJSON {
    appProfiles = apps;
    sdlRun = "${sdlRun}/bin/mortar-sdl-run";
    ui = { bootAnimation = false; sounds = false; haptics = false; };
  });
  ctl = "XDG_RUNTIME_DIR=/run/user/1000 ${mortar-shell}/bin/mortar-shell ctl";
in
pkgs.testers.runNixOSTest {
  name = "mortar-packages";

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
      systemd.services.public-cache = {
        wantedBy = [ "multi-user.target" ];
        serviceConfig.ExecStart = "${pkgs.nodejs}/bin/node ${../cloudflare}/test/serve.mjs /srv/public 8080";
      };
      systemd.services.upstream-cache = {
        wantedBy = [ "multi-user.target" ];
        serviceConfig.ExecStart = "${pkgs.python3}/bin/python3 -m http.server 8081 --directory /srv/upstream";
      };
      systemd.services.other-repo = {
        wantedBy = [ "multi-user.target" ];
        serviceConfig.ExecStart = "${pkgs.python3}/bin/python3 ${slowServer} 8082 /srv/other";
      };
      systemd.services.third-party = {
        wantedBy = [ "multi-user.target" ];
        serviceConfig.ExecStart = "${pkgs.python3}/bin/python3 -m http.server 8083 --directory /srv/third";
      };
      systemd.tmpfiles.rules = [ "d /srv/public 0755 root root -" "d /srv/upstream 0755 root root -" "d /srv/other 0755 root root -" "d /srv/third 0755 root root -" ];
      networking.firewall.allowedTCPPorts = [ 3900 8080 8081 8082 8083 ];
      environment.systemPackages = [ mortar pkgs.curl pkgs.jq py ];
      virtualisation.additionalPaths = [
        catalog demo1 demo2 demo3 slow needsFiles huge liar
        ripgrep1 ripgrep2 curatedTool notUpstream tsv1 tsv2 tsvBad dragsTools
        sysEvil sysSame sysFake sysBare sysOwn
      ];
      virtualisation.memorySize = 2048;
      virtualisation.diskSize = 8192;
      nix.settings.experimental-features = [ "nix-command" ];
    };

  nodes.brick =
    { ... }:
    {
      imports = [ ../modules/device.nix ../modules/core/update.nix ../modules/core/packages.nix ];
      boot.loader.grub.enable = false;
      boot.loader.generic-extlinux-compatible.enable = true;
      mortar.device.id = "brick";
      mortar.update = {
        enable = true;
        baseUrl = "http://ci:8080";
        upstreams = [ { url = "http://ci:8081"; publicKey = builtins.readFile "${upstreamKey}/public"; } ];
        inherit publicKey;
        channel = "testing";
        configName = "test";
        minFreeMiB = 64;
        autoCheck = false;
      };
      mortar.packages = {
        enable = true;
        idleSeconds = 4;
        repositories = [
          {
            id = "mortar";
            name = "Mortar";
            url = "http://ci:8080";
            publicKeys = [ publicKey ];
            channel = "testing";
            upstreams = [ { url = "http://ci:8081"; publicKey = builtins.readFile "${upstreamKey}/public"; } ];
            official = true;
          }
          {
            id = "other";
            name = "Someone else";
            url = "http://ci:8082";
            publicKeys = [ (builtins.readFile "${otherKey}/public") ];
            channel = "main";
          }
        ];
      };
      users.users.player = { isNormalUser = true; uid = 1000; linger = true; extraGroups = [ "mortar-packages" ]; };
      users.users.stranger = { isNormalUser = true; uid = 1001; };
      environment.systemPackages = [ pkgs.jq pkgs.curl ];
      virtualisation.memorySize = 2048;
      virtualisation.diskSize = 4096;
      virtualisation.writableStore = true;
      # test VMs leave out switch-to-configuration; a Brick has it (rollback)
      system.switch.enable = true;
      nix.settings.experimental-features = [ "nix-command" ];
      systemd.user.services.shell = {
        wantedBy = [ "default.target" ];
        environment.MORTAR_UI_FONTS = "${mortar-shell.fonts}/share/fonts/mortar-ui";
        serviceConfig.ExecStart = "${mortar-shell}/bin/mortar-shell run --backend headless --config ${shellConfig} --socket wayland-test";
      };
    };

  testScript = ''
    import json, re, time

    start_all()
    ci.wait_for_unit("multi-user.target")
    brick.wait_for_unit("multi-user.target")
    for port in [3900, 8080, 8081, 8082, 8083]:
        ci.wait_for_open_port(port)
    brick.wait_for_file("/run/user/1000/mortar-shell.sock")

    ci.succeed("install -m 600 ${testKey}/secret /root/signing-key")
    ci.succeed("install -m 600 ${upstreamKey}/secret /root/upstream-key")
    ci.succeed("install -m 600 ${otherKey}/secret /root/other-key")
    ci.succeed("install -m 600 ${thirdKey}/secret /root/third-key")

    SYSTEM = "${system}"

    def ask(op, repo=None, id=None, query=None, refresh=False, user="player"):
        r = {"op": op}
        if repo: r["repo"] = repo
        if id: r["id"] = id
        if query is not None: r["query"] = query
        if refresh: r["refresh"] = True
        line = json.dumps(r)
        cmd = f"mortar-packagesd ask '{line}'"
        out = brick.succeed(f"su {user} -c {json.dumps(cmd)}").strip().splitlines()
        return [json.loads(l) for l in out]

    def overview(refresh=False):
        return ask("overview", refresh=refresh)[0]

    def row(ov, id):
        for r in ov["rows"]:
            if r["id"] == id:
                return r
        raise Exception(f"{id} isn't listed: {ov}")

    def wait_job(timeout=120):
        end = time.time() + timeout
        while time.time() < end:
            r = ask("status")[0]
            if r["reply"] == "job" and r["phase"] in ("done", "failed", "cancelled"):
                return r
            time.sleep(0.5)
        raise Exception(f"the job didn't end: {r}")

    def install(repo, id, op="install"):
        r = ask(op, repo, id)[0]
        assert r["reply"] == "job", r
        return wait_job()

    def launcher():
        return json.loads(brick.succeed("su player -c \"${ctl} state\" | jq -c '[.launcher[].id]'"))

    def launcher_has(id, yes=True):
        for _ in range(60):
            if (id in launcher()) == yes:
                return
            time.sleep(0.25)
        raise Exception(f"launcher: {launcher()}, wanted {id} {yes}")

    def profile(id):
        return brick.succeed(f"readlink -f ${apps}/{id}").strip()

    def no_profile(id):
        brick.fail(f"test -e ${apps}/{id}")
        brick.succeed(f"! ls ${apps}/ 2>/dev/null | grep -q '^{id}-'")

    def pkg(id, path, version="1.0", name=None, requires=None, kind=None, device="brick", program=None):
        p = {"id": id, "name": name or f"Demo {id}", "description": "made up", "version": version, "category": "games", "icon": "pad",
             "licence": "MIT", "source": "https://example.org",
             "builds": [{"device": device, "system": SYSTEM, "path": path}]}
        if requires: p["requires"] = requires
        if kind: p["kind"] = kind
        if program: p["program"] = program
        return p

    def write_catalog(dest, packages, published, key="/root/signing-key", channel="testing", tools=None):
        c = {"format": 1, "channel": channel, "published": published, "packages": packages}
        if tools: c["tools"] = tools
        ci.succeed(f"cat > /tmp/c.json <<'EOF'\n{json.dumps(c)}\nEOF")
        if key:
            ci.succeed(f"mortar sign-catalog --key {key} < /tmp/c.json > /tmp/signed.json && mv /tmp/signed.json /tmp/c.json")
        ci.succeed(f"mkdir -p $(dirname {dest}) && cp /tmp/c.json {dest}")

    def serve(packages, published, **kw):
        write_catalog(f"/srv/public/channels/{kw.get('channel', 'testing')}/catalog.json", packages, published, **kw)

    def push(path, to="/srv/public", key="/root/signing-key"):
        ci.succeed(f"nix copy --to 'file://{to}?secret-key={key}&compression=none' {path}")

    def clear_cache():
        ci.succeed("rm -rf /srv/public/* /srv/public/.[!.]* || true")

    PUB = 2000000000

    ### Publishing: mkCatalog, through a fake bucket

    node_id = ci.succeed("garage node id").split("@")[0]
    ci.succeed(f"garage layout assign -z test -c 2G {node_id}")
    ci.succeed("garage layout apply --version 1")
    ci.succeed("garage bucket create cache")
    out = ci.succeed("garage key create ci")
    def field(pattern: str, text: str) -> str:
        m = re.search(pattern, text, re.IGNORECASE)
        assert m is not None, (pattern, text)
        return m.group(1)

    kid = field(r"Key ID:\s*(\S+)", out)
    sec = field(r"Secret key:\s*(\S+)", out)
    ci.succeed("garage bucket allow --read --write cache --key ci")
    w_env = f"AWS_ACCESS_KEY_ID={kid} AWS_SECRET_ACCESS_KEY={sec}"
    pub_args = "--endpoint http://ci:3900 --bucket cache --key /root/signing-key --config test"

    def bucket(key):
        return ci.succeed(f"curl -sf --aws-sigv4 aws:amz:auto:s3 --user {kid}:{sec} http://localhost:3900/cache/{key}")

    with subtest("mkCatalog builds a catalog from mkApp packages"):
        c = json.loads(ci.succeed("cat ${catalog}"))
        assert c["format"] == 1 and "signature" not in c, c
        p = {x["id"]: x for x in c["packages"]}
        assert p["demo"]["version"] == "1.0" and p["demo"]["licence"] == "MIT" and p["demo"]["kind"] == "nix", p
        assert p["demo"]["builds"] == [{"device": "brick", "system": SYSTEM, "path": "${demo1}"}], p
        assert p["needy"]["requires"] == ["~/needy/data.bin"] and p["needy"]["source"] == "https://example.org/needy", p
        ci.succeed("nix-store --query --requisites ${catalog} | grep -qF ${demo1}")

    with subtest("publish writes it, signed, with what it lists"):
        print(ci.succeed(f"{w_env} mortar publish {pub_args} --channel testing --catalog ${catalog} --catalog-only 2>&1"))
        t = json.loads(bucket("channels/testing/catalog.json"))
        assert t["channel"] == "testing" and t["published"] > 1000 and t["signature"].startswith("mortar-test-1:"), t
        h = "${demo1}".split("/")[-1][:32]
        assert "Sig: mortar-test-1:" in bucket(f"{h}.narinfo")
        # the same list again changes nothing
        assert "Unchanged" in ci.succeed(f"{w_env} mortar publish {pub_args} --channel testing --catalog ${catalog} --catalog-only 2>&1")
        ci.fail(f"curl -sf --aws-sigv4 aws:amz:auto:s3 --user {kid}:{sec} http://localhost:3900/cache/channels/mortar/catalog.json")

    with subtest("promote copies it to the other channel, signed for that"):
        print(ci.succeed(f"{w_env} mortar promote {pub_args} --from testing --channel mortar --catalog-only 2>&1"))
        m = json.loads(bucket("channels/mortar/catalog.json"))
        assert m["channel"] == "mortar" and m["packages"] == t["packages"] and m["signature"] != t["signature"], m

    ### Install, from here on against the directory behind the Worker

    with subtest("the socket: the session user only, and it's started by a connection"):
        brick.succeed("systemctl is-active mortar-packages.socket")
        brick.fail("systemctl is-active mortar-packages.service")
        assert "root mortar-packages" in brick.succeed("stat -c '%U %G' /run/mortar-packages/socket")
        assert brick.succeed("stat -c %a /run/mortar-packages/socket").strip() == "660"
        out = brick.fail("su stranger -c 'mortar-packagesd ask \"{\\\"op\\\":\\\"overview\\\"}\" 2>&1'")
        assert "denied" in out.lower(), out
        brick.fail("systemctl is-active mortar-packages.service")

    with subtest("a request with a path or a URL is refused"):
        for extra in ['"path":"${demo1}"', '"url":"http://ci:8083"']:
            line = '{"op":"install","repo":"mortar","id":"demo",' + extra + '}'
            out = brick.succeed(f"printf '%s\\n' '{line}' | su player -c 'exec ${pkgs.socat}/bin/socat - UNIX-CONNECT:/run/mortar-packages/socket'")
            assert '"bad-request"' in out, out

    with subtest("nothing published: an empty list, and no refusal"):
        ov = overview(refresh=True)
        assert ov["rows"] == [] and [r["id"] for r in ov["repos"]] == ["mortar", "other"], ov
        assert ov["repos"][0]["problem"] is None, ov

    for p in ["${demo1}", "${demo2}", "${demo3}", "${needsFiles}", "${liar}", "${huge}"]:
        push(p)

    with subtest("listed, with the size of what's missing"):
        serve([pkg("demo", "${demo1}"), pkg("needy", "${needsFiles}", requires=["~/needy/data.bin"])], PUB)
        ov = overview(refresh=True)
        d = row(ov, "demo")
        assert d["state"] == "available" and d["repoName"] == "Mortar" and d["official"], d
        d = ask("details", "mortar", "demo")[0]
        assert d["reply"] == "details" and d["size"]["paths"] >= 1 and 3 * 1048576 < d["size"]["unpacked"] < 6 * 1048576, d
        assert d["free"] > 0 and d["powerOk"], d
        brick.fail("nix-store --check-validity ${demo1}")

    with subtest("install: appears in the shell, with no restart, and is the profile"):
        launcher_has("demo", yes=False)
        j = install("mortar", "demo")
        assert j["phase"] == "done", j
        assert profile("demo") == "${demo1}"
        launcher_has("demo")
        d = row(overview(), "demo")
        assert d["state"] == "installed" and d["installedVersion"] == "1.0" and not d["canRollBack"], d
        brick.succeed("nix --extra-experimental-features nix-command path-info --sigs ${demo1} | grep -q mortar-test-1")

    with subtest("the service exits when idle, and costs nothing until asked"):
        brick.wait_until_fails("systemctl is-active mortar-packages.service", timeout=30)
        assert row(overview(), "demo")["state"] == "installed"

    with subtest("it stays through garbage collection"):
        brick.succeed("nix-collect-garbage")
        brick.succeed("test -x ${demo1}/bin/demo")

    ### Refusals: nothing changes, nothing is downloaded

    def refused(id, code, repo="mortar", op="install"):
        r = ask(op, repo, id)[0]
        if r["reply"] == "refused":
            assert r["code"] == code, r
            return r
        j = wait_job()
        assert j["phase"] == "failed" and j["problem"]["code"] == code, j
        return j["problem"]

    with subtest("a bad signature is refused, and the last good list stays"):
        serve([pkg("needy", "${needsFiles}")], PUB + 10, key="/root/other-key")
        refused("needy", "bad-signature")
        brick.fail("nix-store --check-validity ${needsFiles}")
        ov = overview(refresh=True)
        assert ov["repos"][0]["problem"]["code"] == "bad-signature", ov
        assert row(ov, "demo")["state"] == "installed", ov
        # unsigned
        serve([pkg("needy", "${needsFiles}")], PUB + 10, key=None)
        refused("needy", "bad-signature")
        # tampered after signing
        serve([pkg("needy", "${needsFiles}")], PUB + 10)
        ci.succeed("jq '.packages[0].builds[0].path = \"${liar}\"' /srv/public/channels/testing/catalog.json > /tmp/t.json && cp /tmp/t.json /srv/public/channels/testing/catalog.json")
        refused("needy", "bad-signature")
        brick.fail("nix-store --check-validity ${liar}")

    with subtest("a catalog can't be passed off as a system manifest, or the reverse"):
        # a manifest signed by the right key, where the catalog goes
        m = {"schema": 1, "channel": "testing", "system": SYSTEM, "config": "test", "toplevel": "${sysEvil}", "published": PUB + 20, "date": "x"}
        ci.succeed(f"cat > /tmp/m.json <<'EOF'\n{json.dumps(m)}\nEOF")
        ci.succeed("mortar sign-manifest --key /root/signing-key < /tmp/m.json > /tmp/sm.json")
        ci.succeed("jq '.format = 1 | .packages = []' /tmp/sm.json > /srv/public/channels/testing/catalog.json")
        ov = overview(refresh=True)
        assert ov["repos"][0]["problem"]["code"] == "bad-signature", ov
        # a catalog signed by the right key, where the manifest goes
        write_catalog("/tmp/cat.json", [], PUB + 20)
        ci.succeed("jq '. + {schema: 1, system: \"" + SYSTEM + "\", config: \"test\", toplevel: \"${sysEvil}\", date: \"x\"}' /tmp/cat.json > /srv/public/channels/testing/test.json")
        print(brick.fail("mortar-update check 2>&1"))
        s = json.loads(brick.succeed("cat /run/mortar-update/status.json"))
        assert s["state"] == "error" and "bad signature" in s["detail"], s
        ci.succeed("rm /srv/public/channels/testing/test.json")

    with subtest("the wrong device is refused, and not even listed"):
        serve([pkg("demo", "${demo1}"), pkg("needy", "${needsFiles}", device="another-handheld")], PUB + 30)
        ov = overview(refresh=True)
        assert [r["id"] for r in ov["rows"]] == ["demo"], ov
        refused("needy", "wrong-device")
        brick.fail("nix-store --check-validity ${needsFiles}")

    with subtest("the wrong channel is refused"):
        ci.succeed("rm -rf /srv/public/channels/mortar")
        write_catalog("/srv/public/channels/testing/catalog.json", [pkg("needy", "${needsFiles}")], PUB + 40, channel="mortar")
        refused("needy", "wrong-channel")

    with subtest("an older index is refused, and it's remembered over a restart"):
        serve([pkg("demo", "${demo1}"), pkg("needy", "${needsFiles}")], PUB + 50)
        assert overview(refresh=True)["repos"][0]["published"] == PUB + 50
        serve([pkg("demo", "${demo1}"), pkg("needy", "${needsFiles}")], PUB + 45)
        refused("needy", "older")
        brick.succeed("systemctl stop mortar-packages.service")
        brick.succeed("rm /var/lib/mortar-packages/repos/mortar/catalog.json")
        ov = overview(refresh=True)
        assert ov["repos"][0]["problem"]["code"] == "older", ov
        brick.succeed("jq -e '.seen.mortar == %d' /var/lib/mortar-packages/state.json" % (PUB + 50))
        brick.fail("nix-store --check-validity ${needsFiles}")

    with subtest("a future format asks politely, and leaves what's installed alone"):
        ci.succeed("""echo '{"format":2,"who":"knows"}' > /srv/public/channels/testing/catalog.json""")
        ov = overview(refresh=True)
        p = ov["repos"][0]["problem"]
        assert p["code"] == "newer-format" and p["headline"] == "Update Mortar to see what's new", ov
        assert profile("demo") == "${demo1}"
        launcher_has("demo")

    with subtest("a bundle is listed and can't be installed"):
        serve([pkg("demo", "${demo1}"), pkg("needy", "${needsFiles}"), {"id": "bundled", "name": "Bundled", "version": "1", "kind": "bundle", "builds": []}], PUB + 60)
        ov = overview(refresh=True)
        assert ov["repos"][0]["problem"] is None, ov
        assert row(ov, "bundled")["state"] == "not-yet", ov
        refused("bundled", "not-yet")

    with subtest("not an app, or not the app it says: refused before anything shows"):
        serve([pkg("liar-app", "${liar}")], PUB + 70)
        overview(refresh=True)
        refused("liar-app", "bad-package")
        no_profile("liar-app")

    def with_config(jq_filter):
        brick.succeed("systemctl stop mortar-packages.service")
        brick.succeed(f"jq '{jq_filter}' /etc/mortar-packages/config.json > /tmp/packages.json")
        brick.succeed("systemctl set-environment MORTAR_PACKAGES_CONFIG=/tmp/packages.json")

    def default_config():
        brick.succeed("systemctl stop mortar-packages.service")
        brick.succeed("systemctl unset-environment MORTAR_PACKAGES_CONFIG")

    with subtest("not enough room: refused, nothing fetched"):
        serve([pkg("demo", "${demo1}"), pkg("huge", "${huge}")], PUB + 80)
        overview(refresh=True)
        with_config(".minFreeMib = 100000000")
        p = refused("huge", "no-room")
        assert "needs" in p["detail"] and "free" in p["detail"], p
        no_profile("huge")
        brick.fail("nix-store --check-validity ${huge}")
        default_config()

    with subtest("low battery waits, and charging doesn't"):
        brick.succeed("mkdir -p /tmp/ps/bat && echo Battery > /tmp/ps/bat/type && echo 12 > /tmp/ps/bat/capacity && echo Discharging > /tmp/ps/bat/status")
        with_config('.powerSupplyDir = "/tmp/ps"')
        refused("huge", "low-battery")
        no_profile("huge")
        d = ask("details", "mortar", "huge")[0]
        assert not d["powerOk"] and d["battery"] == 12, d
        brick.succeed("echo Charging > /tmp/ps/bat/status")
        assert ask("details", "mortar", "huge")[0]["powerOk"]
        default_config()

    ### Another repository: only offered, and slow enough to watch

    write_catalog("/srv/other/channels/main/catalog.json", [pkg("slowpoke", "${slow}")], PUB, key="/root/other-key", channel="main")
    ci.succeed("nix copy --to 'file:///srv/other?secret-key=/root/other-key&compression=none' ${slow}")

    with subtest("several repositories, each with its own key"):
        ov = overview(refresh=True)
        s = row(ov, "slowpoke")
        assert s["repo"] == "other" and s["repoName"] == "Someone else" and not s["official"], s
        # a catalog naming it under Mortar's key doesn't reach its cache
        assert ov["repos"][1]["problem"] is None, ov

    with subtest("progress, then cancel: nothing in Apps, and the old state stays"):
        r = ask("install", "other", "slowpoke")[0]
        assert r["reply"] == "job", r
        seen = None
        for _ in range(120):
            st = ask("status")[0]
            if st["phase"] == "downloading" and st["done"] > 0:
                seen = st
                break
            time.sleep(0.25)
        assert seen is not None, st
        assert 30 * 1048576 < seen["total"] < 50 * 1048576 and seen["done"] < seen["total"], seen
        # one at a time
        r = ask("install", "mortar", "huge")[0]
        assert r["reply"] == "refused" and r["code"] == "busy", r
        # a watcher hears it, and leaving doesn't stop it
        out = brick.succeed("timeout 2 su player -c \"mortar-packagesd ask '{\\\"op\\\":\\\"watch\\\"}'\" || true")
        assert '"downloading"' in out, out
        st = ask("status")[0]
        assert st["phase"] == "downloading", st
        launcher_has("slowpoke", yes=False)
        assert ask("cancel")[0]["reply"] == "ok"
        j = wait_job()
        assert j["phase"] == "cancelled", j
        no_profile("slowpoke")
        launcher_has("slowpoke", yes=False)
        brick.fail("test -e /nix/var/nix/gcroots/mortar-packages/installing-slowpoke")
        brick.fail("nix-store --check-validity ${slow}")
        assert row(overview(), "slowpoke")["state"] == "available"
        # nothing's in the way of doing it again
        ci.succeed("systemctl stop other-repo")
        j = install("other", "slowpoke")
        assert j["phase"] == "failed" and j["problem"]["code"] in ("offline", "fetch"), j
        ci.succeed("systemctl start other-repo")
        ci.wait_for_open_port(8082)

    with subtest("a repository whose paths aren't signed by its key: refused, nothing fetched"):
        ci.succeed("nix copy --to 'file:///srv/other?secret-key=/root/third-key&compression=none' ${huge}")
        write_catalog("/srv/other/channels/main/catalog.json", [pkg("slowpoke", "${slow}"), pkg("huge", "${huge}")], PUB + 1, key="/root/other-key", channel="main")
        # (the same app is also on Mortar's cache, which this repository can't reach for)
        p = refused("huge", "unsigned", repo="other")
        no_profile("huge")
        brick.fail("nix-store --check-validity ${huge}")

    ### Remove, update, rollback

    with subtest("remove while running: leaves the launcher, keeps running, data stays"):
        brick.succeed("su player -c '${ctl} launch demo'")
        brick.wait_for_file("/home/player/demo.pid")
        pid = brick.succeed("cat /home/player/demo.pid").strip()
        brick.succeed("su player -c 'mkdir -p ~/.local/share/demo && echo saved > ~/.local/share/demo/save'")
        r = ask("remove", "mortar", "demo")[0]
        assert r["reply"] == "ok", r
        launcher_has("demo", yes=False)
        no_profile("demo")
        # the collection runs in the background, and the service stays up for it
        brick.succeed(f"kill -0 {pid}")
        brick.wait_until_fails("systemctl is-active mortar-packages.service", timeout=60)
        brick.succeed(f"kill -0 {pid}")
        brick.succeed("su player -c '${ctl} state' >/dev/null")
        brick.fail("nix-store --check-validity ${demo1}")
        brick.succeed("grep -qx saved /home/player/.local/share/demo/save")
        r = ask("remove", "mortar", "demo")[0]
        assert r["reply"] == "refused" and r["code"] == "not-installed", r
        brick.succeed(f"kill {pid}")
        brick.succeed("rm /home/player/demo.pid")

    with subtest("what's put here from a computer is left alone"):
        brick.succeed("mkdir -p /tmp/from-computer")
        j = install("mortar", "demo")  # gives the store a real app to point at
        path = profile("demo")
        ask("remove", "mortar", "demo")
        brick.wait_until_fails("systemctl is-active mortar-packages.service", timeout=60)
        # installed by hand, as `mortar xcom` does: no record of it here
        j = install("mortar", "huge")
        assert j["phase"] == "done", j
        brick.succeed("systemctl stop mortar-packages.service")
        brick.succeed("jq 'del(.installed.huge)' /var/lib/mortar-packages/state.json > /tmp/s.json && cp /tmp/s.json /var/lib/mortar-packages/state.json")
        assert row(overview(), "huge")["state"] == "elsewhere"
        refused("huge", "elsewhere")
        r = ask("remove", "mortar", "huge")[0]
        assert r["code"] == "not-installed", r
        assert profile("huge") == "${huge}"
        brick.succeed("rm -f ${apps}/huge ${apps}/huge-*-link")

    with subtest("update: the same flow, and the shell takes the new one at the next start"):
        serve([pkg("demo", "${demo1}", version="1.0")], PUB + 90)
        overview(refresh=True)
        assert install("mortar", "demo")["phase"] == "done"
        assert profile("demo") == "${demo1}"
        assert not row(overview(), "demo")["canRollBack"]

        serve([pkg("demo", "${demo2}", version="2.0")], PUB + 100)
        d = row(overview(refresh=True), "demo")
        assert d["state"] == "update" and d["version"] == "2.0" and d["installedVersion"] == "1.0", d
        # sorted first
        assert overview()["rows"][0]["id"] == "demo"
        # what it would fetch is what's missing
        s = ask("details", "mortar", "demo")[0]["size"]
        assert 3 * 1048576 < s["unpacked"] < 6 * 1048576, s
        j = install("mortar", "demo", op="update")
        assert j["phase"] == "done", j
        assert profile("demo") == "${demo2}"
        d = row(overview(), "demo")
        assert d["state"] == "installed" and d["installedVersion"] == "2.0" and d["canRollBack"], d
        launcher_has("demo")
        # one only updates what's here
        r = ask("update", "mortar", "needy")[0]
        if r["reply"] == "job":
            assert wait_job()["problem"]["code"] in ("not-installed", "no-package"), r

    with subtest("rollback: one step, and the daily check doesn't undo it"):
        brick.succeed("nix-store --check-validity ${demo1}")
        r = ask("rollback", "mortar", "demo")[0]
        assert r["reply"] == "ok" and "1.0" in r["message"], r
        assert profile("demo") == "${demo1}"
        launcher_has("demo")
        d = row(overview(), "demo")
        assert d["state"] == "update" and d["installedVersion"] == "1.0" and not d["canRollBack"], d
        r = ask("rollback", "mortar", "demo")[0]
        assert r["reply"] == "refused" and r["code"] == "no-previous", r
        assert brick.succeed("ls ${apps} | grep -c '^demo-[0-9]*-link'").strip() == "1"

        brick.succeed("systemctl stop mortar-packages.service")
        brick.succeed("mkdir -p /var/lib/mortar-update && echo '{\"autoCheck\":true}' > /var/lib/mortar-update/state.json")
        print(brick.succeed("systemctl start mortar-packages-auto.service; journalctl -u mortar-packages-auto.service --no-pager | tail -3"))
        assert profile("demo") == "${demo1}", "the daily check undid a rollback"

    with subtest("the daily check: official packages with Auto-check on, others never"):
        # a newer build than the one rolled back from
        serve([pkg("demo", "${demo3}", version="3.0")], PUB + 110)
        ci.succeed("nix copy --to 'file:///srv/other?secret-key=/root/other-key&compression=none' ${demo2}")
        write_catalog("/srv/other/channels/main/catalog.json", [pkg("slowpoke", "${slow}"), pkg("demo-other", "${demo2}")], PUB + 2, key="/root/other-key", channel="main")

        brick.succeed("echo '{\"autoCheck\":false}' > /var/lib/mortar-update/state.json")
        brick.succeed("systemctl start mortar-packages-auto.service")
        assert profile("demo") == "${demo1}", "Auto-check is off"

        brick.succeed("echo '{\"autoCheck\":true}' > /var/lib/mortar-update/state.json")
        # from another repository: installed by hand, and it stays where it is
        j = install("other", "slowpoke")
        assert j["phase"] == "done", j
        slow_before = profile("slowpoke")
        write_catalog("/srv/other/channels/main/catalog.json", [pkg("slowpoke", "${huge}", version="2.0")], PUB + 3, key="/root/other-key", channel="main")

        # the timer's unit brings the other half along
        brick.succeed("systemctl start mortar-update-auto.service || true")
        brick.wait_until_succeeds("test $(readlink -f ${apps}/demo) = ${demo3}", timeout=60)
        assert profile("slowpoke") == slow_before
        o = row(overview(refresh=True), "slowpoke")
        assert o["state"] == "update", o

    ### Terminal tools

    ci.succeed("nix copy --to 'file:///srv/upstream?secret-key=/root/upstream-key&compression=none' ${ripgrep1} ${ripgrep2}")
    push("${tsv1}")
    push("${curatedTool}")
    push("${notUpstream}")

    with subtest("a large index that would bring its packages along is refused"):
        push("${tsvBad}")
        serve([pkg("demo", "${demo3}", version="3.0")], PUB + 115, tools=[{"system": SYSTEM, "path": "${tsvBad}", "count": 1}])
        overview(refresh=True)
        r = ask("search", query="dragged")[0]
        assert r["reply"] == "refused" and r["code"] == "bad-index", r
        brick.fail("nix-store --check-validity ${tsvBad}")
        brick.fail("nix-store --check-validity ${dragsTools}")

    with subtest("a large index: found by search, without being in the catalog"):
        tools = [{"system": SYSTEM, "path": "${tsv1}", "count": 2}]
        serve([pkg("demo", "${demo3}", version="3.0"), pkg("curated", "${curatedTool}", kind="tool", program="curated-cmd")], PUB + 120, tools=tools)
        ov = overview(refresh=True)
        assert ov["repos"][0]["tools"] == 2, ov
        assert row(ov, "curated")["kind"] == "tool", ov
        f = ask("search", query="rips")[0]
        assert f["reply"] == "found" and [r["id"] for r in f["rows"]] == ["ripthing"], f
        r = f["rows"][0]
        assert r["version"] == "1.0" and r["kind"] == "tool" and r["program"] == "rt" and r["state"] == "available", r
        # only the list came: none of what it lists
        brick.fail("nix-store --check-validity ${ripgrep1}")
        # it stays through a collection, and doesn't need fetching again
        brick.succeed("nix-collect-garbage")
        brick.succeed("nix-store --check-validity ${tsv1}")

    with subtest("a tool goes on Terminal's path, and not on the Apps page"):
        j = install("mortar", "ripthing")
        assert j["phase"] == "done", j
        brick.succeed("test $(readlink -f /nix/var/nix/profiles/tools/ripthing) = ${ripgrep1}")
        assert brick.succeed("su - player -c rt").strip() == "ripthing 1.0"
        assert "ripthing" not in launcher()
        assert ask("search", query="ripthing")[0]["rows"][0]["state"] == "installed"
        brick.succeed("nix --extra-experimental-features nix-command path-info --sigs ${ripgrep1} | grep -q upstream-1")

    with subtest("a tool in a large index has to carry the upstream's signature"):
        p = refused("notupstream", "unsigned")
        brick.fail("nix-store --check-validity ${notUpstream}")
        assert not brick.succeed("ls /nix/var/nix/profiles/tools").count("notupstream")

    with subtest("a curated tool is the repository's own"):
        j = install("mortar", "curated")
        assert j["phase"] == "done", j
        assert brick.succeed("su - player -c curated-cmd").strip() == "curated 1.0"
        assert "curated" not in launcher()

    with subtest("tools update, roll back and go"):
        push("${tsv2}")
        tools = [{"system": SYSTEM, "path": "${tsv2}", "count": 1}]
        serve([pkg("demo", "${demo3}", version="3.0"), pkg("curated", "${curatedTool}", kind="tool", program="curated-cmd")], PUB + 130, tools=tools)
        overview(refresh=True)
        assert ask("search", query="rt")[0]["rows"][0]["state"] == "update"
        assert install("mortar", "ripthing", op="update")["phase"] == "done"
        assert brick.succeed("su - player -c rt").strip() == "ripthing 2.0"
        assert ask("rollback", "mortar", "ripthing")[0]["reply"] == "ok"
        assert brick.succeed("su - player -c rt").strip() == "ripthing 1.0"
        assert ask("remove", "mortar", "ripthing")[0]["reply"] == "ok"
        brick.fail("su - player -c 'command -v rt'")
        brick.fail("test -e /nix/var/nix/profiles/tools/ripthing")
        assert ask("remove", "mortar", "curated")[0]["reply"] == "ok"

    ### The system guard

    def plant(path, content, key="-", fake=""):
        # a cache holding its own contents under that path
        ci.succeed(f"mkdir -p /tmp/content && printf '%s' {json.dumps(content)} > /tmp/content/f")
        ci.succeed(f"${py}/bin/python3 ${makeNarinfo} /srv/third {path} /tmp/content/f {key} {fake}")

    def take_from_third(path, keys):
        out = brick.execute(f"nix-store --realise {path} --option substituters http://ci:8083 --option trusted-public-keys '{keys}' "
                            "--option require-sigs true --option max-jobs 0 --option narinfo-cache-negative-ttl 0 --option narinfo-cache-positive-ttl 0 2>&1")
        print(out)
        assert out[0] == 0, out
        brick.succeed(f"nix-store --check-validity {path}")

    def publish_system(toplevel, published):
        # signed by the publisher, what it really is, with everything it needs
        push(toplevel)
        m = {"schema": 1, "channel": "testing", "system": SYSTEM, "config": "test", "toplevel": toplevel, "published": published, "date": "x"}
        ci.succeed(f"cat > /tmp/m.json <<'EOF'\n{json.dumps(m)}\nEOF")
        ci.succeed("mortar sign-manifest --key /root/signing-key < /tmp/m.json > /srv/public/channels/testing/test.json")

    def status():
        return json.loads(brick.succeed("cat /run/mortar-update/status.json"))

    def staged():
        return brick.succeed("readlink -f /nix/var/nix/profiles/system").strip()

    THIRD = "${builtins.readFile "${thirdKey}/public"}".strip()
    running = brick.succeed("readlink -f /run/current-system").strip()
    brick.succeed(f"nix-env -p /nix/var/nix/profiles/system --set {running}")

    with subtest("guard: a third party's contents under a path of the new system block the update"):
        plant("${plantedEvil}", "evil, with a signature", key="/root/third-key")
        take_from_third("${plantedEvil}", THIRD)
        assert brick.succeed("cat ${plantedEvil}") == "evil, with a signature"
        publish_system("${sysEvil}", PUB + 200)
        print(brick.fail("mortar-update check 2>&1"))
        s = status()
        assert s["state"] == "error" and s["message"] == "Update blocked", s
        assert "planted-evil" in s["detail"], s
        assert staged() == running
        brick.succeed("journalctl -b --no-pager > /dev/null")

    with subtest("guard: --allow-foreign-paths takes it, when asked for by name"):
        brick.succeed("mortar-update check --allow-foreign-paths")
        assert status()["state"] == "ready" and staged() == "${sysEvil}", status()
        brick.succeed("mortar-update rollback")
        assert staged() == running

    with subtest("guard: removing what the third party brought lets the update through"):
        brick.succeed("nix-env -p /nix/var/nix/profiles/system --delete-generations old || true")
        brick.succeed("rm -f /nix/var/nix/profiles/system-*-link /nix/var/nix/profiles/system")
        brick.succeed(f"nix-env -p /nix/var/nix/profiles/system --set {running}")
        brick.succeed("nix-store --delete ${sysEvil} ${plantedEvil}")
        push("${plantedEvil}")
        publish_system("${sysEvil}", PUB + 201)
        print(brick.succeed("mortar-update check --force 2>&1"))
        assert status()["state"] == "ready", status()
        assert brick.succeed("cat ${plantedEvil}") == "what Mortar published\n"
        brick.succeed("mortar-update rollback")

    with subtest("guard: the very same contents signed by someone else are no harm"):
        ci.succeed("nix copy --to 'file:///srv/third?secret-key=/root/third-key&compression=none' ${plantedSame}")
        take_from_third("${plantedSame}", THIRD)
        out = brick.succeed("nix --extra-experimental-features nix-command path-info --sigs ${plantedSame}")
        assert "third-party-1" in out and "mortar-test-1" not in out, out
        push("${plantedSame}")
        publish_system("${sysSame}", PUB + 210)
        print(brick.succeed("mortar-update check 2>&1"))
        assert status()["state"] == "ready", status()
        out = brick.succeed("nix --extra-experimental-features nix-command path-info --sigs ${plantedSame}")
        assert "mortar-test-1" in out, out
        brick.succeed("mortar-update rollback")

    with subtest("guard: writing the publisher's name on a signature doesn't help"):
        plant("${plantedFake}", "evil, with a forged name", key="/root/third-key", fake="mortar-test-1")
        take_from_third("${plantedFake}", THIRD)
        assert "mortar-test-1" in brick.succeed("nix --extra-experimental-features nix-command path-info --sigs ${plantedFake}")
        publish_system("${sysFake}", PUB + 220)
        before = staged()
        brick.fail("mortar-update check 2>&1")
        s = status()
        assert s["message"] == "Update blocked" and "planted-fake-signature" in s["detail"], s
        assert staged() == before and before != "${sysFake}"

    with subtest("guard: no signature at all is what a card and a deploy leave, and passes"):
        # `mortar deploy`: nix copy --no-check-sigs from a computer. The same contents
        ci.succeed("nix copy --to 'file:///srv/third?compression=none' ${plantedBare}")
        print(brick.succeed("nix --extra-experimental-features nix-command copy --no-check-sigs --from http://ci:8083 ${plantedBare} 2>&1"))
        out = brick.succeed("nix --extra-experimental-features nix-command path-info --sigs ${plantedBare}")
        assert ":" not in out.split("/")[-1], out
        push("${plantedBare}")
        publish_system("${sysBare}", PUB + 230)
        print(brick.succeed("mortar-update check 2>&1"))
        assert status()["state"] == "ready", status()
        brick.succeed("mortar-update rollback")

    with subtest("guard: and so does your own build of a path, which isn't the published one"):
        plant("${plantedOwn}", "my own build of it")
        print(brick.succeed("nix --extra-experimental-features nix-command copy --no-check-sigs --from http://ci:8083 ${plantedOwn} 2>&1"))
        assert brick.succeed("cat ${plantedOwn}") == "my own build of it"
        publish_system("${sysOwn}", PUB + 240)
        print(brick.succeed("mortar-update check 2>&1"))
        assert status()["state"] == "ready", status()
  '';
}
