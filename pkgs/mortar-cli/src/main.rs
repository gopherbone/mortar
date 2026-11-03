// SPDX-License-Identifier: GPL-2.0-or-later
//! `mortar`: build Mortar on this computer and put it on a Brick over SSH.
//!
//! Shells out to `nix` and `ssh` (both expected on PATH) so it works the same
//! on macOS and Linux. `mortar pair` speaks the pairing protocol
//! (pkgs/mortar-pair) itself. `mortar publish` pushes a build to the
//! over-the-air update bucket (docs/updates.md).

mod acgc;
mod liar;
mod xcom;
mod pair;
mod publish;

use std::env;
use std::process::{exit, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

const USAGE: &str = "\
mortar deploy [--boot] [--reboot] [--host HOST] [--config NAME]
    Build the system, copy it to the Brick, make it the default boot entry
    and switch to it now (--boot: only on next boot; --reboot: reboot into it
    and wait until it is back).
mortar rollback [--host HOST]     switch back to the previous generation
mortar generations [--host HOST]  list the Brick's system generations
mortar reboot [--host HOST]       reboot and wait until SSH answers again
mortar attention [--host HOST]    buzz the Brick so whoever holds it looks
mortar ssh [--host HOST] [-- CMD...]
mortar pico8 [--host HOST] [--remove]
    Build PICO-8 from your own zip (pkgs/pico8) and install it into its own
    profile on the Brick (/nix/var/nix/profiles/pico8). It is never part of
    the system, so over-the-air updates neither ship nor remove it.
mortar acgc [--host HOST] [--iso PATH] [--remove]
    Animal Crossing (GameCube decomp port, pkgs/acgc): check your own disc
    image (USA, GAFE01 rev 0), build the port and install it into its own
    profile (/nix/var/nix/profiles/acgc), and copy the image to the Brick
    (~brick/.local/share/acgc/rom/GAFE01.iso). Neither is ever part of the
    system. --remove takes the port away and leaves the image and saves.
mortar xcom --data PATH [--host HOST]
    OpenXcom (pkgs/openxcom): check your own copy of UFO: Enemy Unknown and
    copy the folders the game reads to the Brick
    (~brick/.local/share/openxcom/UFO). It is never part of the system.
mortar liar [--data PATH] [--host HOST] [--remove]
    Redemption: Liar (GP2X Wiz, pkgs/liar): check your own files from the card
    (PATH: the card or its Liar_e folder), build the loader and install it into
    its own profile (/nix/var/nix/profiles/liar), and copy the files to the Brick
    (~brick/.local/share/liar). Neither is ever part of the system. --remove
    takes the loader away and leaves the files and saves.
mortar publish [--config NAME] [--path STORE_PATH] [--channel C] [--key FILE]
               [--credentials FILE] [--force] [--dry-run]
               [--endpoint URL --bucket NAME [--region R]]
    Over-the-air updates without CI: build, push the closure signed to the
    private bucket (mortar.update in the flake, or --endpoint/--bucket),
    then write the channel manifest (docs/updates.md). Key:
    ~/.config/mortar/signing-key; bucket credentials:
    $AWS_ACCESS_KEY_ID/$AWS_SECRET_ACCESS_KEY or ~/.config/mortar/r2-write.
mortar update [--host HOST]       check for an update on the Brick now
mortar update-creds [--host HOST] [--key-id ID]
    Give the Brick its read-only bucket credentials (asks for the secret,
    or $MORTAR_UPDATE_SECRET), over SSH.
mortar pair [--host HOST] [--key FILE.pub] [--name ALIAS] [--code CODE] [--no-ssh-config]
    Put this computer's SSH key on a Brick using the code on its screen
    (over the USB cable), check its host key, and add `Host ALIAS`
    (default: brick) to ~/.ssh/config so `ssh brick` works.

Defaults: HOST=$MORTAR_HOST or mortar.local, CONFIG=brick-mainline,
flake=$MORTAR_FLAKE or the current directory.";

const PROFILE: &str = "/nix/var/nix/profiles/system";

struct Opts {
    host: String,
    config: String,
    flake: String,
    boot_only: bool,
    reboot: bool,
    remove: bool,
    key_id: Option<String>,
    iso: Option<String>,
    data: Option<String>,
    rest: Vec<String>,
}

fn die(msg: &str) -> ! {
    eprintln!("mortar: {msg}");
    exit(1)
}

fn say(msg: &str) {
    println!("\x1b[1m==>\x1b[0m {msg}");
}

fn parse(args: &[String]) -> Opts {
    let mut o = Opts {
        host: env::var("MORTAR_HOST").unwrap_or_else(|_| "mortar.local".into()),
        config: "brick-mainline".into(),
        flake: env::var("MORTAR_FLAKE").unwrap_or_else(|_| ".".into()),
        boot_only: false,
        reboot: false,
        remove: false,
        key_id: None,
        iso: None,
        data: None,
        rest: Vec::new(),
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--host" => o.host = it.next().cloned().unwrap_or_else(|| die("--host needs a value")),
            "--config" => o.config = it.next().cloned().unwrap_or_else(|| die("--config needs a value")),
            "--boot" => o.boot_only = true,
            "--remove" => o.remove = true,
            "--iso" => o.iso = Some(it.next().cloned().unwrap_or_else(|| die("--iso needs a value"))),
            "--data" => o.data = Some(it.next().cloned().unwrap_or_else(|| die("--data needs a value"))),
            "--key-id" => o.key_id = Some(it.next().cloned().unwrap_or_else(|| die("--key-id needs a value"))),
            "--reboot" => {
                o.reboot = true;
                o.boot_only = true;
            }
            "--" => {
                o.rest = it.cloned().collect();
                break;
            }
            other => die(&format!("unknown option: {other}")),
        }
    }
    o
}

fn parse_pair(args: &[String]) -> pair::Opts {
    let mut o = pair::Opts { host: env::var("MORTAR_HOST").ok(), key: None, name: "brick".into(), code: None, ssh_config: true };
    let mut it = args.iter();
    let val = |it: &mut std::slice::Iter<String>, n: &str| it.next().cloned().unwrap_or_else(|| die(&format!("{n} needs a value")));
    while let Some(a) = it.next() {
        match a.as_str() {
            "--host" => o.host = Some(val(&mut it, a)),
            "--key" => o.key = Some(val(&mut it, a).into()),
            "--name" => o.name = val(&mut it, a),
            "--code" => o.code = Some(val(&mut it, a)),
            "--no-ssh-config" => o.ssh_config = false,
            other => die(&format!("unknown option: {other}")),
        }
    }
    o
}

fn ssh_cmd(host: &str) -> Command {
    let mut c = Command::new("ssh");
    c.args(["-o", "ConnectTimeout=10", "-o", "BatchMode=yes"]).arg(format!("root@{host}"));
    c
}

/// Run a shell command on the Brick, inheriting stdio. Returns success.
fn remote(host: &str, script: &str) -> bool {
    ssh_cmd(host).arg(script).status().map(|s| s.success()).unwrap_or(false)
}

fn run(cmd: &mut Command, what: &str) {
    match cmd.status() {
        Ok(s) if s.success() => {}
        Ok(s) => die(&format!("{what} failed ({s})")),
        Err(e) => die(&format!("{what}: {e}")),
    }
}

/// Single-quote for a POSIX shell.
fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Output of a command on the Brick (None if SSH or the command failed).
fn remote_output(host: &str, script: &str) -> Option<String> {
    let out = ssh_cmd(host).arg(script).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

const BOOT_ID: &str = "cat /proc/sys/kernel/random/boot_id";

/// Wait until SSH answers again with a different boot id than `old`.
fn wait_for_brick(host: &str, old: Option<&str>) {
    say(&format!("Waiting for {host}"));
    sleep(Duration::from_secs(5));
    let deadline = Instant::now() + Duration::from_secs(180);
    while Instant::now() < deadline {
        if let Some(id) = remote_output(host, BOOT_ID) {
            if old.is_some_and(|o| o == id) {
                // still the old boot (going down, or it never went)
                sleep(Duration::from_secs(2));
                continue;
            }
            say(&format!("{host} is back"));
            return;
        }
        sleep(Duration::from_secs(2));
    }
    die(&format!("{host} did not come back (rebooted) within 3 minutes"));
}

fn reboot(host: &str) {
    say(&format!("Rebooting {host}"));
    let old = remote_output(host, BOOT_ID);
    // The connection may drop as the Brick goes down, so only a refusal
    // that systemctl reports (e.g. a "destructive transaction" with a job
    // stuck in the queue) is fatal.
    let out = ssh_cmd(host).arg("systemctl reboot").output();
    if let Ok(o) = &out {
        if o.status.code().is_some_and(|c| c != 0 && c != 255) {
            eprint!("{}", String::from_utf8_lossy(&o.stderr));
            die("systemctl reboot was refused (see `systemctl list-jobs` on the Brick)");
        }
    }
    wait_for_brick(host, old.as_deref());
    remote(host, "echo \"running: $(readlink /run/current-system)\"; systemctl is-system-running || true");
}

fn parse_publish(args: &[String]) -> publish::Opts {
    let mut o = publish::Opts {
        flake: env::var("MORTAR_FLAKE").unwrap_or_else(|_| ".".into()),
        config: "brick-mainline".into(),
        path: None,
        channel: None,
        endpoint: None,
        bucket: None,
        region: None,
        key: None,
        creds: None,
        publisher: env::var("MORTAR_PUBLISHER").unwrap_or_else(|_| "mortar publish".into()),
        force: false,
        dry_run: false,
    };
    let mut it = args.iter();
    let val = |it: &mut std::slice::Iter<String>, n: &str| it.next().cloned().unwrap_or_else(|| die(&format!("{n} needs a value")));
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config" => o.config = val(&mut it, a),
            "--flake" => o.flake = val(&mut it, a),
            "--path" => o.path = Some(val(&mut it, a)),
            "--channel" => o.channel = Some(val(&mut it, a)),
            "--endpoint" => o.endpoint = Some(val(&mut it, a)),
            "--bucket" => o.bucket = Some(val(&mut it, a)),
            "--region" => o.region = Some(val(&mut it, a)),
            "--key" => o.key = Some(val(&mut it, a).into()),
            "--credentials" => o.creds = Some(val(&mut it, a).into()),
            "--publisher" => o.publisher = val(&mut it, a),
            "--force" => o.force = true,
            "--dry-run" => o.dry_run = true,
            other => die(&format!("unknown option: {other}")),
        }
    }
    o
}

/// `nix build` an installable here; the store path.
fn build(installable: &str) -> String {
    let out = Command::new("nix")
        .args(["build", "--no-link", "--print-out-paths", installable])
        .stderr(Stdio::inherit())
        .output()
        .unwrap_or_else(|e| die(&format!("nix: {e}")));
    if !out.status.success() {
        die("build failed");
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !path.starts_with("/nix/store/") || path.contains('\n') {
        die(&format!("unexpected build output: {path}"));
    }
    path
}

/// Build the system (aarch64-linux; on a Mac through its Linux builder).
fn build_toplevel(flake: &str, config: &str) -> String {
    say(&format!("Building {config}"));
    build(&format!("{flake}#nixosConfigurations.{config}.config.system.build.toplevel"))
}

const PICO8_PROFILE: &str = "/nix/var/nix/profiles/pico8";

/// PICO-8 lives in its own profile on the Brick, outside the system closure
/// (it is commercial: never built by CI or put in the update cache).
fn pico8(o: &Opts) {
    if o.remove {
        say(&format!("Removing PICO-8 from {}", o.host));
        if !remote(&o.host, &format!("rm -f {PICO8_PROFILE} {PICO8_PROFILE}-*-link && echo removed")) {
            die("could not remove the profile");
        }
        return;
    }
    say("Building PICO-8 (needs your zip in the Nix store, see pkgs/pico8)");
    let path = build(&format!("{}#packages.aarch64-linux.pico8", o.flake));
    say(&format!("Copying to {}", o.host));
    run(
        Command::new("nix").args(["copy", "--no-check-sigs", "--to", &format!("ssh-ng://root@{}", o.host), &path]),
        "nix copy",
    );
    if !remote(&o.host, &format!("nix-env -p {PICO8_PROFILE} --set {p} && ls -l {PICO8_PROFILE}/bin/pico8", p = sq(&path))) {
        die("could not set the pico8 profile");
    }
    say("PICO-8 installed; it shows in the greeter and the shell's launcher");
}

fn deploy(o: &Opts) {
    let path = build_toplevel(&o.flake, &o.config);

    say(&format!("Copying to {}", o.host));
    // The Brick doesn't trust this computer's signing key; the closure was
    // built here from source we control, so skip the signature check.
    run(
        Command::new("nix").args(["copy", "--no-check-sigs", "--to", &format!("ssh-ng://root@{}", o.host), &path]),
        "nix copy",
    );

    say("Registering generation");
    let action = if o.boot_only { "boot" } else { "switch" };
    let script = format!(
        "nix-env -p {PROFILE} --set {p} && {p}/bin/switch-to-configuration {action}",
        p = sq(&path)
    );
    if !remote(&o.host, &script) {
        die("switch-to-configuration failed");
    }
    if o.reboot {
        reboot(&o.host);
        let running = remote_output(&o.host, "readlink /run/current-system").unwrap_or_default();
        if running != path {
            die(&format!("the Brick is running {running}, not the deployed {path}"));
        }
    }
    remote(&o.host, &format!("nix-env -p {PROFILE} --list-generations | tail -3"));
    // PICO-8 lives in its own profile (never in the system closure or the
    // update cache), so a deploy never brings it back: say so when it's gone.
    if !remote(&o.host, &format!("test -x {PICO8_PROFILE}/bin/pico8")) {
        say("PICO-8 is not installed on the Brick; run `mortar pico8` to add it");
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let (cmd, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r),
        None => ("help", &[][..]),
    };
    if cmd == "pair" {
        pair::run(&parse_pair(rest));
        return;
    }
    if cmd == "publish" {
        publish::publish(&parse_publish(rest));
        return;
    }
    let o = parse(rest);
    match cmd {
        "deploy" => deploy(&o),
        "rollback" => {
            say(&format!("Rolling back {}", o.host));
            let ok = remote(
                &o.host,
                &format!("nix-env -p {PROFILE} --rollback && {PROFILE}/bin/switch-to-configuration switch"),
            );
            if !ok {
                die("rollback failed");
            }
        }
        "generations" => {
            remote(&o.host, &format!("nix-env -p {PROFILE} --list-generations"));
        }
        "reboot" => reboot(&o.host),
        "pico8" => pico8(&o),
        "acgc" => acgc::acgc(&o, o.iso.as_deref()),
        "xcom" => xcom::xcom(&o, o.data.as_deref()),
        "liar" => liar::liar(&o, o.data.as_deref()),
        "update" => publish::update_now(&o.host),
        "update-creds" => publish::update_creds(&o.host, o.key_id.clone()),
        "attention" => {
            if !remote(&o.host, "mortar-rumble 300 2") {
                die("could not buzz the Brick");
            }
        }
        "ssh" => {
            let mut c = Command::new("ssh");
            c.arg(format!("root@{}", o.host)).args(&o.rest);
            let code = c.status().map(|s| s.code().unwrap_or(1)).unwrap_or(1);
            exit(code);
        }
        "help" | "-h" | "--help" => println!("{USAGE}"),
        other => die(&format!("unknown command: {other} (try: mortar help)")),
    }
}
