// SPDX-License-Identifier: GPL-2.0-or-later
//! `mortar xcom`: OpenXcom (pkgs/openxcom) on the Brick.
//!
//! The engine is an app in its own profile, outside the system closure; the
//! shell notices the profile by itself. The game data is your own copy of UFO:
//! Enemy Unknown: with --data it is checked here, then only the folders the game
//! reads are streamed over SSH into the session user's data directory. The
//! files are never put in the Nix store, a cache or a repository. Installing,
//! updating or removing the engine does not touch them or the saves next to them.

use crate::{build, die, remote, run, say, sq, ssh_cmd, Opts};
use std::path::Path;
use std::process::{Command, Stdio};

const APPS_DIR: &str = "/nix/var/nix/profiles/apps";
const PROFILE: &str = "/nix/var/nix/profiles/apps/openxcom";
const MANIFEST: &str = "share/mortar/app.json";
const SHELL_CONFIG: &str = "/etc/mortar-shell/config.json";

const FOLDERS: [&str; 9] = ["GEODATA", "GEOGRAPH", "MAPS", "ROUTES", "SOUND", "TERRAIN", "UFOGRAPH", "UFOINTRO", "UNITS"];
/// One file per folder that OpenXcom can't start without.
const NEEDED: [&str; 8] = [
    "GEODATA/WORLD.DAT",
    "GEOGRAPH/BASEBITS.PCK",
    "MAPS/AVENGER.MAP",
    "ROUTES/AVENGER.RMP",
    "SOUND/ADLIB.CAT",
    "TERRAIN/AVENGER.MCD",
    "UFOGRAPH/TAC00.SCR",
    "UNITS/XCOM_0.PCK",
];

/// What's wrong with the folder, if anything.
pub fn check_data(path: &str) -> Result<(), String> {
    let root = Path::new(path);
    for f in NEEDED {
        if !root.join(f).is_file() {
            return Err(format!("{f} is missing: point --data at the game's folder (the one holding GEODATA and UFOGRAPH)"));
        }
    }
    Ok(())
}

pub fn xcom(o: &Opts, data: Option<&str>) {
    if o.remove {
        say(&format!("Removing OpenXcom from {} (your game files and saves stay)", o.host));
        if !remote(&o.host, &remove_script()) {
            die("could not remove the profile");
        }
        return;
    }
    // check the game first: no point building for a copy that can't be used
    if let Some(data) = data {
        if let Err(e) = check_data(data) {
            die(&format!("{data}: {e}"));
        }
    }

    say("Building OpenXcom (no game data; pkgs/openxcom)");
    let path = build(&format!("{}#packages.aarch64-linux.openxcom", o.flake));
    if !Path::new(&path).join(MANIFEST).is_file() {
        die(&format!("{path} has no {MANIFEST}: is --flake an older Mortar? Update it"));
    }
    say(&format!("Copying to {}", o.host));
    run(
        Command::new("nix").args(["copy", "--no-check-sigs", "--to", &format!("ssh-ng://root@{}", o.host), &path]),
        "nix copy",
    );
    if !remote(&o.host, &install_script(&path)) {
        die("could not set the openxcom profile");
    }

    if let Some(data) = data {
        copy_data(o, data);
    }
    let game = format!("{}/openxcom/UFO/{}", o.data_home(), NEEDED[0]);
    let has_game = remote(&o.host, &format!("test -f {}", sq(&game)));
    if !has_game {
        say("No game files on the Brick yet: run `mortar xcom --data PATH` with your copy of UFO: Enemy Unknown");
    }
    // A system from before apps had profiles has no use for this one: its own
    // entry goes on working, and the next deploy moves over to the profile.
    if !remote(&o.host, &discovers_script()) {
        say("This Brick's system does not look for apps in profiles yet: OpenXcom stays as the system has it until your next `mortar deploy`");
    } else if has_game {
        say("OpenXcom now shows under Games in the shell");
    }
}

/// Stream the folders the game reads into the session user's data directory.
fn copy_data(o: &Opts, data: &str) {
    let (user, dir) = (&o.user, format!("{}/openxcom/UFO", o.data_home()));
    say(&format!("Copying your game files to {}:{dir}", o.host));
    let mut tar = Command::new("tar")
        .env("COPYFILE_DISABLE", "1")
        .arg("cf")
        .arg("-")
        .arg("-C")
        .arg(data)
        .args(FOLDERS.iter().filter(|f| Path::new(data).join(f).is_dir()))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| die(&format!("tar: {e}")));
    let script = format!(
        "set -e; runuser -u {user} -- mkdir -p {dir}; \
         tar xf - -C {dir} --no-same-owner --warning=no-unknown-keyword; \
         chown -R {user}:users {dir}; du -sh {dir}"
    );
    let ok = ssh_cmd(&o.host)
        .arg(script)
        .stdin(tar.stdout.take().expect("tar's output"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !(ok && tar.wait().map(|s| s.success()).unwrap_or(false)) {
        die("copying the game files failed");
    }
}

/// `mkdir`: the first app on a Brick (nix-env does not make the directory).
fn install_script(path: &str) -> String {
    format!(
        "mkdir -p -m 0755 {APPS_DIR} && nix-env -p {PROFILE} --set {p} && ls -l {PROFILE}/{MANIFEST}",
        p = sq(path)
    )
}

/// Only the profile and its generations; the game and saves are in the home.
fn remove_script() -> String {
    format!("rm -f {PROFILE} {PROFILE}-*-link && echo removed")
}

/// Succeeds if the running system's shell looks in the apps directory.
fn discovers_script() -> String {
    format!("grep -q '\"appProfiles\":\"' {SHELL_CONFIG}")
}

/// After a deploy, succeeds on a Brick that has the game, no profile for the
/// engine and no entry for it in the new system.
pub fn needs_install_script(data_home: &str) -> String {
    format!(
        "test -f {game} && ! test -e {PROFILE} && grep -q '\"appProfiles\":\"' {SHELL_CONFIG} && ! grep -q '\"id\":\"openxcom\"' {SHELL_CONFIG}",
        game = sq(&format!("{data_home}/openxcom/UFO/{}", NEEDED[0])),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_folder_is_refused() {
        assert!(check_data("/nonexistent").is_err());
    }

    #[test]
    fn scripts_touch_only_the_profile() {
        let install = install_script("/nix/store/abc-openxcom-app");
        assert!(install.starts_with("mkdir -p -m 0755 /nix/var/nix/profiles/apps && "), "{install}");
        assert!(install.contains("nix-env -p /nix/var/nix/profiles/apps/openxcom --set '/nix/store/abc-openxcom-app'"), "{install}");
        // the game and the saves live in the home: nothing may name it, or a
        // glob that could reach it
        let remove = remove_script();
        assert_eq!(remove, "rm -f /nix/var/nix/profiles/apps/openxcom /nix/var/nix/profiles/apps/openxcom-*-link && echo removed");
        assert!(!install.contains("home") && !remove.contains("home"));
    }

    #[test]
    fn deploy_hint_only_where_it_applies() {
        let s = needs_install_script("/home/brick/.local/share");
        assert!(s.contains("test -f '/home/brick/.local/share/openxcom/UFO/GEODATA/WORLD.DAT'"), "{s}");
        assert!(s.contains("! test -e /nix/var/nix/profiles/apps/openxcom"), "{s}");
        assert!(s.contains("! grep -q '\"id\":\"openxcom\"'"), "an older system still has its entry: {s}");
    }
}
