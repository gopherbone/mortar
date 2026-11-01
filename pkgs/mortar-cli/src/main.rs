// SPDX-License-Identifier: GPL-2.0-or-later
//! `mortar`: build Mortar on this computer and put it on a Brick over SSH.
//!
//! Shells out to `nix` and `ssh` (both expected on PATH) so it works the same
//! on macOS and Linux. No dependencies beyond std.

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

Defaults: HOST=$MORTAR_HOST or mortar.local, CONFIG=brick-mainline,
flake=$MORTAR_FLAKE or the current directory.";

const PROFILE: &str = "/nix/var/nix/profiles/system";

struct Opts {
    host: String,
    config: String,
    flake: String,
    boot_only: bool,
    reboot: bool,
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
        rest: Vec::new(),
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--host" => o.host = it.next().cloned().unwrap_or_else(|| die("--host needs a value")),
            "--config" => o.config = it.next().cloned().unwrap_or_else(|| die("--config needs a value")),
            "--boot" => o.boot_only = true,
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

fn ssh_cmd(host: &str) -> Command {
    let mut c = Command::new("ssh");
    c.args(["-o", "ConnectTimeout=10", "-o", "BatchMode=yes"]).arg(format!("root@{host}"));
    c
}

/// Run a shell command on the Brick, inheriting stdio. Returns success.
fn remote(host: &str, script: &str) -> bool {
    ssh_cmd(host).arg(script).status().map(|s| s.success()).unwrap_or(false)
}

fn remote_quiet(host: &str, script: &str) -> bool {
    ssh_cmd(host)
        .arg(script)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
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

fn wait_for_brick(host: &str) {
    say(&format!("Waiting for {host}"));
    sleep(Duration::from_secs(5));
    let deadline = Instant::now() + Duration::from_secs(180);
    while Instant::now() < deadline {
        if remote_quiet(host, "true") {
            say(&format!("{host} is back"));
            return;
        }
        sleep(Duration::from_secs(2));
    }
    die(&format!("{host} did not come back within 3 minutes"));
}

fn reboot(host: &str) {
    say(&format!("Rebooting {host}"));
    // The connection drops as the Brick goes down; that's expected.
    let _ = remote_quiet(host, "systemctl reboot");
    wait_for_brick(host);
    remote(host, "echo \"running: $(readlink /run/current-system)\"; systemctl is-system-running || true");
}

fn deploy(o: &Opts) {
    say(&format!("Building {}", o.config));
    let attr = format!("{}#nixosConfigurations.{}.config.system.build.toplevel", o.flake, o.config);
    let out = Command::new("nix")
        .args(["build", "--no-link", "--print-out-paths", &attr])
        .stderr(Stdio::inherit())
        .output()
        .unwrap_or_else(|e| die(&format!("nix: {e}")));
    if !out.status.success() {
        die("build failed");
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !path.starts_with("/nix/store/") {
        die(&format!("unexpected build output: {path}"));
    }

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
    }
    remote(&o.host, &format!("nix-env -p {PROFILE} --list-generations | tail -3"));
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let (cmd, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r),
        None => ("help", &[][..]),
    };
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
