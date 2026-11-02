// SPDX-License-Identifier: GPL-2.0-or-later
//! `mortar acgc`: Animal Crossing (pkgs/acgc) on the Brick.
//!
//! Like PICO-8 the port lives in its own profile, outside the system closure.
//! The game data is the user's own disc image: checked here (Animal Crossing
//! USA, GAFE01 revision 0, a GameCube image whose files are where its file
//! table says), then streamed over SSH straight into the session user's data
//! directory. It is never put in the Nix store, a cache or a repository.

use crate::{build, die, remote, run, say, sq, ssh_cmd, Opts};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::process::{Command, Stdio};

pub const PROFILE: &str = "/nix/var/nix/profiles/acgc";
const USER: &str = "brick";
const ROM_DIR: &str = "/home/brick/.local/share/acgc/rom";

fn be32(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// What the disc image is, or why it can't be used.
pub fn check_iso(path: &str) -> Result<String, String> {
    let mut f = File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    let mut head = vec![0u8; 0x440];
    f.read_exact(&mut head).map_err(|_| "too short for a GameCube disc image".to_string())?;
    match &head[..4] {
        b"CISO" => return Err("a CISO image: convert it to a plain ISO first (Dolphin: Convert File, ISO)".into()),
        b"RVZ\x01" | b"WIA\x01" => return Err("an RVZ/WIA image: convert it to a plain ISO first (Dolphin: Convert File, ISO)".into()),
        _ => {}
    }
    if be32(&head, 0x1C) != 0xC233_9F3D {
        return Err("not a GameCube disc image (no disc magic at 0x1C)".into());
    }
    let id = String::from_utf8_lossy(&head[..6]).to_string();
    if id != "GAFE01" {
        return Err(format!("game ID {id}: the port needs Animal Crossing USA (GAFE01)"));
    }
    if head[6] != 0 || head[7] != 0 {
        return Err(format!("disc {} revision {}: the port needs disc 0, revision 0", head[6], head[7]));
    }
    // The port reads files at the offsets in the file table: make sure they
    // are there (a full ISO, or an NKit image that kept them in place).
    let (fst_off, fst_len) = (be32(&head, 0x424) as u64, be32(&head, 0x428) as usize);
    if fst_off + fst_len as u64 > len || fst_len < 12 {
        return Err("the file table lies outside the image (truncated?)".into());
    }
    let mut fst = vec![0u8; fst_len];
    f.seek(SeekFrom::Start(fst_off)).and_then(|_| f.read_exact(&mut fst)).map_err(|e| e.to_string())?;
    let n = be32(&fst, 8) as usize;
    if n * 12 > fst_len {
        return Err("corrupt file table".into());
    }
    let mut checked = 0;
    for i in 1..n {
        if fst[i * 12] != 0 {
            continue; // directory
        }
        let name_off = n * 12 + (be32(&fst, i * 12) & 0xFF_FFFF) as usize;
        let name: String = fst.get(name_off..).unwrap_or(&[]).iter().take_while(|&&c| c != 0).map(|&c| c as char).collect();
        let (off, size) = (be32(&fst, i * 12 + 4) as u64, be32(&fst, i * 12 + 8) as u64);
        if off + size > len {
            return Err(format!("{name} lies beyond the end of the image (truncated, or an NKit image that moved files: convert it to ISO in Dolphin)"));
        }
        let magic: &[u8] = if name.ends_with(".arc") { b"RARC" } else if name.ends_with(".szs") { b"Yaz0" } else { continue };
        let mut m = [0u8; 4];
        f.seek(SeekFrom::Start(off)).and_then(|_| f.read_exact(&mut m)).map_err(|e| e.to_string())?;
        if m != magic {
            return Err(format!("{name} is not at its offset (an NKit image that moved files: convert it to ISO in Dolphin)"));
        }
        checked += 1;
    }
    let mut nkit = [0u8; 4];
    f.seek(SeekFrom::Start(0x200)).and_then(|_| f.read_exact(&mut nkit)).map_err(|e| e.to_string())?;
    let kind = if &nkit == b"NKIT" { "NKit image, files in place" } else { "ISO" };
    Ok(format!("Animal Crossing (USA) GAFE01 rev 0, {kind}, {} MB, {checked} archives checked", len / 1_000_000))
}

pub fn acgc(o: &Opts, iso: Option<&str>) {
    if o.remove {
        say(&format!("Removing the Animal Crossing port from {} (your disc image and saves stay)", o.host));
        if !remote(&o.host, &format!("rm -f {PROFILE} {PROFILE}-*-link && echo removed")) {
            die("could not remove the profile");
        }
        return;
    }
    // check the disc first: no point building without one
    if let Some(iso) = iso {
        match check_iso(iso) {
            Ok(what) => say(&format!("Disc image: {what}")),
            Err(e) => die(&format!("{iso}: {e}")),
        }
    }

    say("Building the Animal Crossing port (no game data; pkgs/acgc)");
    let path = build(&format!("{}#packages.aarch64-linux.acgc", o.flake));
    say(&format!("Copying to {}", o.host));
    run(
        Command::new("nix").args(["copy", "--no-check-sigs", "--to", &format!("ssh-ng://root@{}", o.host), &path]),
        "nix copy",
    );
    if !remote(&o.host, &format!("nix-env -p {PROFILE} --set {p} && ls -l {PROFILE}/bin/acgc", p = sq(&path))) {
        die("could not set the acgc profile");
    }

    match iso {
        Some(iso) => copy_iso(&o.host, iso),
        None => {
            if !remote(&o.host, &format!("test -s {ROM_DIR}/GAFE01.iso")) {
                say("No disc image on the Brick yet: run `mortar acgc --iso PATH` with your Animal Crossing (USA) ISO");
                return;
            }
        }
    }
    say("Animal Crossing installed; it shows under Games in the shell");
}

/// Stream the image into ~brick/.local/share/acgc/rom/GAFE01.iso, owned by
/// the session user (skipped when an image of the same size is there).
fn copy_iso(host: &str, iso: &str) {
    let len = std::fs::metadata(iso).map(|m| m.len()).unwrap_or_else(|e| die(&format!("{iso}: {e}")));
    let dst = format!("{ROM_DIR}/GAFE01.iso");
    if crate::remote_output(host, &format!("stat -c %s {dst}")).is_some_and(|s| s == len.to_string()) {
        say("The Brick already has this disc image");
        return;
    }
    say(&format!("Copying your disc image ({} MB) to {host}:{dst}", len / 1_000_000));
    let script = format!(
        "set -e; umask 022; \
         runuser -u {USER} -- mkdir -p {ROM_DIR}; \
         cat > {dst}.part; chown {USER}:users {dst}.part; chmod 0644 {dst}.part; mv -f {dst}.part {dst}; \
         ls -l {dst}"
    );
    let file = File::open(iso).unwrap_or_else(|e| die(&format!("{iso}: {e}")));
    let ok = ssh_cmd(host).arg(script).stdin(Stdio::from(file)).status().map(|s| s.success()).unwrap_or(false);
    if !ok {
        remote(host, &format!("rm -f {dst}.part"));
        die("copying the disc image failed");
    }
}
