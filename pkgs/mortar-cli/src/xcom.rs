// SPDX-License-Identifier: GPL-2.0-or-later
//! `mortar xcom --data PATH`: your own copy of UFO: Enemy Unknown for OpenXcom
//! (pkgs/openxcom). Checked here, then only the folders the game reads are
//! streamed over SSH into the session user's data directory. The files are
//! never put in the Nix store, a cache or a repository.

use crate::{die, say, ssh_cmd, Opts};
use std::path::Path;
use std::process::{Command, Stdio};

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
    let Some(data) = data else {
        die("`mortar xcom` needs --data PATH: your copy of UFO: Enemy Unknown");
    };
    if let Err(e) = check_data(data) {
        die(&format!("{data}: {e}"));
    }
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
    say("OpenXcom now shows under Games in the shell");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_folder_is_refused() {
        assert!(check_data("/nonexistent").is_err());
    }
}
