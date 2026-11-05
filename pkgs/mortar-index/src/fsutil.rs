// SPDX-License-Identifier: GPL-2.0-or-later

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// Write via a temp file + fsync + rename. A new parent directory gets
/// `dir_mode`. Works on FAT too (modes are then ignored).
pub fn write_atomic_in(path: &Path, data: &[u8], mode: u32, dir_mode: u32) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    if !dir.exists() {
        fs::create_dir_all(dir)?;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(dir_mode));
    }
    let tmp = dir.join(format!(".{}.tmp", path.file_name().and_then(|n| n.to_str()).unwrap_or("file")));
    {
        let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(&tmp)?;
        // an existing temp file keeps its old mode
        let _ = f.set_permissions(fs::Permissions::from_mode(mode));
        f.write_all(data)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

pub fn write_atomic(path: &Path, data: &[u8], mode: u32) -> io::Result<()> {
    write_atomic_in(path, data, mode, 0o700)
}

/// Free bytes for unprivileged users on the file system holding `p`.
pub fn free_bytes(p: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(p.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs fills the zeroed struct; c is a valid C string
    unsafe {
        let mut s: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut s) != 0 {
            return None;
        }
        Some(s.f_bavail as u64 * s.f_frsize as u64)
    }
}
