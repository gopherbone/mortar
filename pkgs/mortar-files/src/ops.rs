// SPDX-License-Identifier: GPL-2.0-or-later
//! The operations: copy, move, trash, restore, delete, rename, new folder.
//! No UI in here. Everything that touches user data goes through this file,
//! and its tests in `ops_tests.rs` run against real temp folders.
//!
//! The rules, and how they hold:
//!
//! - Nothing is overwritten silently. Every final name is claimed with a
//!   no-replace rename (RENAME_NOREPLACE, RENAME_EXCL), and a clash goes to
//!   `Ctl::ask`. Only an answered Replace uses a plain rename, and that swaps
//!   atomically: the old file is there until the new one is complete.
//! - A file or folder appears under its final name only when it is complete.
//!   It is built under a hidden `.mortar-part-*` name beside the target, then
//!   renamed. A failure or a cancel removes the part; the source was only read.
//! - A move across disks is a copy, a sync of the destination disk, and only
//!   then a removal of the original.
//! - Symlinks are objects, never paths to walk: they are copied as links and
//!   removed as links, and walks use lstat and d_type.
//! - Read-only places (`Guard`) are refused here, not only hidden in the UI.

use std::ffi::{CString, OsStr, OsString};
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant, SystemTime};

// ---------------------------------------------------------------- errors ----

/// An io error in plain words, for a person holding a handheld.
pub fn plain(e: &io::Error) -> String {
    match e.raw_os_error() {
        Some(libc::EACCES) | Some(libc::EPERM) => "You don't have permission.".into(),
        Some(libc::ENOENT) => "It isn't there any more.".into(),
        Some(libc::ENOSPC) => "The disk is full.".into(),
        Some(libc::EDQUOT) => "The disk is full.".into(),
        Some(libc::EROFS) => "This disk is read-only.".into(),
        Some(libc::ENAMETOOLONG) => "That name is too long for this disk.".into(),
        Some(libc::EEXIST) => "Something with that name is already there.".into(),
        Some(libc::ENOTEMPTY) => "That folder isn't empty.".into(),
        Some(libc::EBUSY) => "It's in use.".into(),
        Some(libc::EFBIG) => "It's too big for this disk.".into(),
        Some(libc::EIO) => "The disk reported a read or write error.".into(),
        Some(libc::ELOOP) => "It's a link that goes in circles.".into(),
        Some(libc::EINVAL) => "This disk doesn't allow that name.".into(),
        Some(libc::ENOTDIR) => "That isn't a folder.".into(),
        _ => {
            let s = e.to_string();
            match s.find(" (os error") {
                Some(i) => format!("{}.", &s[..i]),
                None => s,
            }
        }
    }
}

// ----------------------------------------------------------------- guard ----

/// What may never be written to or removed.
#[derive(Clone, Debug, Default)]
pub struct Guard {
    /// Whole trees shown read-only.
    pub read_only: Vec<PathBuf>,
    /// Folders that may hold changes but not be removed or moved themselves
    /// (the places, the Trash).
    pub pinned: Vec<PathBuf>,
    pub home: Option<PathBuf>,
}

/// The system's read-only trees.
pub const READ_ONLY: [&str; 5] = ["/nix", "/proc", "/sys", "/dev", "/boot"];

fn canon_or_self(p: &Path) -> PathBuf {
    fs::canonicalize(p).unwrap_or_else(|_| lexical(p))
}

/// `.` and `..` resolved without looking at the disk.
pub fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The real path of what `p` names, without following `p` itself if it is a
/// link: the parent is resolved, the last part stays.
pub fn real_object(p: &Path) -> PathBuf {
    let p = lexical(p);
    match (p.parent(), p.file_name()) {
        (Some(d), Some(n)) => canon_or_self(d).join(n),
        _ => p,
    }
}

impl Guard {
    pub fn system(home: Option<PathBuf>, pinned: Vec<PathBuf>) -> Guard {
        Guard { read_only: READ_ONLY.iter().map(PathBuf::from).collect(), pinned, home }
    }

    fn under_read_only(&self, p: &Path) -> bool {
        self.read_only.iter().any(|r| p.starts_with(r) || p.starts_with(canon_or_self(r)))
    }

    /// Is this folder read-only by policy? Looks through links: a link in
    /// your home that points into /nix/store is /nix/store.
    pub fn is_read_only(&self, dir: &Path) -> bool {
        self.under_read_only(&lexical(dir)) || self.under_read_only(&canon_or_self(dir))
    }

    /// May new things appear in this folder?
    pub fn check_dest(&self, dir: &Path) -> Result<(), String> {
        if self.is_read_only(dir) {
            return Err("This place is read-only.".into());
        }
        Ok(())
    }

    /// May this object be removed, moved or renamed?
    pub fn check_removable(&self, p: &Path) -> Result<(), String> {
        let real = real_object(p);
        let parent = real.parent().map(Path::to_path_buf).unwrap_or_default();
        if self.under_read_only(&real) || self.under_read_only(&lexical(p)) {
            return Err("This place is read-only.".into());
        }
        if real.components().count() <= 2 {
            return Err("That's part of the system.".into());
        }
        if let Some(h) = &self.home
            && canon_or_self(h).starts_with(&real)
        {
            return Err("That holds your home folder.".into());
        }
        if self.pinned.iter().any(|x| canon_or_self(x) == real) {
            return Err("Mortar needs that folder.".into());
        }
        if let (Ok(a), Ok(b)) = (fs::symlink_metadata(&real), fs::symlink_metadata(&parent))
            && a.is_dir()
            && a.dev() != b.dev()
        {
            return Err("That's a mounted disk.".into());
        }
        Ok(())
    }
}

// ----------------------------------------------------------------- names ----

/// A file name a person can give something. Returns why not.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("It needs a name.".into());
    }
    if name == "." || name == ".." {
        return Err("That name is reserved.".into());
    }
    if name.contains('/') {
        return Err("A name can't have / in it.".into());
    }
    if name.contains('\0') || name.chars().any(|c| c.is_control()) {
        return Err("A name can't have control characters in it.".into());
    }
    if name.len() > 255 {
        return Err("That name is too long.".into());
    }
    Ok(())
}

/// `name.ext` to `name (2).ext`: the first free name, by bytes, so names
/// that aren't UTF-8 keep working, and within the 255 byte limit.
pub fn unique_name(dir: &Path, name: &OsStr) -> OsString {
    if fs::symlink_metadata(dir.join(name)).is_err() {
        return name.to_os_string();
    }
    let b = name.as_bytes();
    let dot = b.iter().rposition(|c| *c == b'.').filter(|i| *i > 0);
    let (stem, ext) = match dot {
        Some(i) if b.len() - i <= 16 => (&b[..i], &b[i..]),
        _ => (b, &b[b.len()..]),
    };
    for n in 2u32.. {
        let suffix = format!(" ({n})");
        let room = 255usize.saturating_sub(suffix.len() + ext.len());
        let mut out = stem[..stem.len().min(room)].to_vec();
        out.extend_from_slice(suffix.as_bytes());
        out.extend_from_slice(ext);
        let cand = OsString::from_vec(out);
        if fs::symlink_metadata(dir.join(&cand)).is_err() {
            return cand;
        }
    }
    unreachable!()
}

fn cstr(p: &Path) -> io::Result<CString> {
    CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))
}

/// rename(2) that fails with EEXIST rather than replace anything.
pub fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    let (a, b) = (cstr(from)?, cstr(to)?);
    #[cfg(target_os = "linux")]
    let r = unsafe { libc::renameat2(libc::AT_FDCWD, a.as_ptr(), libc::AT_FDCWD, b.as_ptr(), libc::RENAME_NOREPLACE) };
    #[cfg(target_os = "macos")]
    let r = unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_EXCL) };
    if r == 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    match e.raw_os_error() {
        // this filesystem can't promise it: the next best thing
        Some(libc::ENOSYS) | Some(libc::EINVAL) | Some(libc::ENOTSUP) => {
            if fs::symlink_metadata(to).is_ok() {
                return Err(io::Error::from_raw_os_error(libc::EEXIST));
            }
            fs::rename(from, to)
        }
        _ => Err(e),
    }
}

static FAKE_SPACE: std::sync::Mutex<Option<(u64, u64)>> = std::sync::Mutex::new(None);

/// The preview says what a made-up disk holds, not this machine's.
pub fn fake_space(v: Option<(u64, u64)>) {
    *FAKE_SPACE.lock().unwrap() = v;
}

/// (free, total) bytes for this user on the disk holding `p`.
pub fn free_space(p: &Path) -> Option<(u64, u64)> {
    if let Some(v) = *FAKE_SPACE.lock().unwrap() {
        return Some(v);
    }
    let c = cstr(p).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let f = s.f_frsize as u64;
    Some((s.f_bavail as u64 * f, s.f_blocks as u64 * f))
}

/// Everything written to the disk holding `p` is on the disk when this
/// returns Ok.
fn sync_disk(p: &Path) -> io::Result<()> {
    let f = File::open(p)?;
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::syncfs(f.as_raw_fd()) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = &f;
        unsafe { libc::sync() };
    }
    Ok(())
}

// ------------------------------------------------------------ small ops ----

/// Rename within a folder. Never replaces anything.
pub fn rename(guard: &Guard, path: &Path, new_name: &str) -> Result<PathBuf, String> {
    validate_name(new_name)?;
    guard.check_removable(path)?;
    let dir = path.parent().ok_or("That can't be renamed.")?;
    guard.check_dest(dir)?;
    let to = dir.join(new_name);
    rename_noreplace(path, &to).map_err(|e| plain(&e))?;
    Ok(to)
}

pub fn new_folder(guard: &Guard, dir: &Path, name: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    guard.check_dest(dir)?;
    let p = dir.join(name);
    fs::create_dir(&p).map_err(|e| plain(&e))?;
    Ok(p)
}

// ------------------------------------------------------------- job types ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Copy,
    Move,
    Trash,
    Delete,
    Restore,
}

#[derive(Clone, Debug)]
pub struct JobSpec {
    pub kind: JobKind,
    pub sources: Vec<PathBuf>,
    /// The folder things land in (Copy, Move).
    pub dest: Option<PathBuf>,
    /// The Trash (Trash, Restore).
    pub trash: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub current: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    Skip,
    KeepBoth,
    Replace,
    Merge,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Side {
    pub size: u64,
    pub mtime: i64,
    pub is_dir: bool,
}

/// A name that is already taken: what the person is asked.
#[derive(Clone, Debug, PartialEq)]
pub struct Clash {
    pub name: String,
    pub existing: Side,
    pub incoming: Side,
    pub options: Vec<Choice>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Answer {
    /// None: cancel the job.
    pub choice: Option<Choice>,
    /// Do the same for the rest of this job's clashes of this sort.
    pub for_rest: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub name: String,
    pub why: String,
}

/// What happened, told plainly (`Report::lines`).
#[derive(Clone, Debug)]
pub struct Report {
    pub kind: JobKind,
    /// Items in the request.
    pub total: usize,
    pub done: usize,
    pub skipped: usize,
    pub failed: Vec<Failure>,
    pub cancelled: bool,
    pub bytes: u64,
    /// Names things landed under, in the destination.
    pub landed: Vec<OsString>,
    /// A problem with the whole job, before anything happened.
    pub refused: Option<String>,
    /// Originals still in place in spite of a finished copy.
    pub kept_originals: usize,
}

impl Report {
    fn new(kind: JobKind, total: usize) -> Report {
        Report { kind, total, done: 0, skipped: 0, failed: Vec::new(), cancelled: false, bytes: 0, landed: Vec::new(), refused: None, kept_originals: 0 }
    }

    pub fn clean(&self) -> bool {
        self.failed.is_empty() && !self.cancelled && self.refused.is_none() && self.kept_originals == 0
    }

    /// The short sentence for a toast or a title.
    pub fn headline(&self) -> String {
        if let Some(r) = &self.refused {
            return r.clone();
        }
        let n = self.done;
        let things = |n: usize| if n == 1 { "1 item".to_string() } else { format!("{n} items") };
        let verb = match self.kind {
            JobKind::Copy => "Copied",
            JobKind::Move => "Moved",
            JobKind::Trash => "In the Trash:",
            JobKind::Delete => "Deleted",
            JobKind::Restore => "Restored",
        };
        if self.cancelled {
            return format!("Stopped. {verb} {} first.", things(n)).replace("In the Trash: ", "Trashed ");
        }
        if !self.failed.is_empty() {
            return if n == 0 { "That didn't work.".into() } else { format!("{verb} {} of {}.", n, self.total) };
        }
        if n == 0 && self.skipped > 0 {
            return "Nothing changed.".into();
        }
        format!("{verb} {}.", things(n))
    }

    /// What the person needs to know beyond the headline.
    pub fn details(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.refused.is_some() {
            out.push("Nothing was changed.".to_string());
            return out;
        }
        if self.skipped > 0 {
            out.push(if self.skipped == 1 { "1 item skipped.".to_string() } else { format!("{} items skipped.", self.skipped) });
        }
        if self.cancelled || !self.failed.is_empty() {
            match self.kind {
                JobKind::Copy => out.push("Your originals are untouched.".into()),
                JobKind::Move => out.push("Whatever didn't finish moving is still where it was.".into()),
                JobKind::Trash | JobKind::Restore => out.push("Whatever is left wasn't touched.".into()),
                JobKind::Delete => {}
            }
        }
        if self.kept_originals > 0 {
            out.push(if self.kept_originals == 1 { "1 item is now in both places.".to_string() } else { format!("{} items are now in both places.", self.kept_originals) });
        }
        out
    }
}

/// How a running job talks to whoever started it.
pub trait Ctl {
    fn progress(&mut self, p: &Progress);
    fn ask(&mut self, c: &Clash) -> Answer;
    fn cancelled(&self) -> bool;
}

/// Stops a transfer.
enum Stop {
    Cancelled,
    Io(io::Error),
    /// Said plainly.
    Why(String),
}

impl From<io::Error> for Stop {
    fn from(e: io::Error) -> Stop {
        Stop::Io(e)
    }
}

fn side(m: &Metadata) -> Side {
    Side { size: m.len(), mtime: m.mtime(), is_dir: m.is_dir() }
}

pub fn display_name(n: &OsStr) -> String {
    crate::listing::display(n)
}

fn base_name(p: &Path) -> OsString {
    p.file_name().map(OsStr::to_os_string).unwrap_or_else(|| p.as_os_str().to_os_string())
}

// ------------------------------------------------------------------ plan ----

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub files: u64,
    pub bytes: u64,
    /// A folder inside holds another disk's mount.
    pub crosses_mount: bool,
}

/// Count what's under `p` without following a link.
pub fn measure(p: &Path, cancel: &dyn Fn() -> bool) -> Totals {
    fn walk(p: &Path, root_dev: u64, t: &mut Totals, cancel: &dyn Fn() -> bool) {
        if cancel() {
            return;
        }
        let Ok(m) = fs::symlink_metadata(p) else { return };
        if m.dev() != root_dev {
            t.crosses_mount = true;
            return;
        }
        t.files += 1;
        if m.is_dir() {
            if let Ok(rd) = fs::read_dir(p) {
                for e in rd.flatten() {
                    walk(&e.path(), root_dev, t, cancel);
                }
            }
        } else if m.is_file() {
            t.bytes += m.len();
        }
    }
    let mut t = Totals::default();
    if let Ok(m) = fs::symlink_metadata(p) {
        walk(p, m.dev(), &mut t, cancel);
    }
    t
}

// -------------------------------------------------------------- transfer ----

static PART: AtomicU64 = AtomicU64::new(0);

fn part_name() -> OsString {
    OsString::from(format!(".mortar-part-{}-{}", std::process::id(), PART.fetch_add(1, Ordering::Relaxed)))
}

struct Run<'a> {
    ctl: &'a mut dyn Ctl,
    prog: Progress,
    last: Instant,
    buf: Vec<u8>,
    sticky_file: Option<Choice>,
    sticky_dir: Option<Choice>,
    /// Set when a tree copy left something out (a socket, a FIFO).
    left_out: u64,
}

impl Run<'_> {
    fn tick(&mut self, force: bool) -> Result<(), Stop> {
        if self.ctl.cancelled() {
            return Err(Stop::Cancelled);
        }
        if force || self.last.elapsed() >= Duration::from_millis(200) {
            self.last = Instant::now();
            let p = self.prog.clone();
            self.ctl.progress(&p);
        }
        Ok(())
    }

    fn copy_file(&mut self, src: &Path, to: &Path, m: &Metadata) -> Result<(), Stop> {
        // O_NOFOLLOW: if `src` turned into a link since the lstat, stop.
        let mut r = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(src)?;
        let mut w = OpenOptions::new().write(true).create_new(true).mode(0o600).open(to)?;
        loop {
            let n = r.read(&mut self.buf)?;
            if n == 0 {
                break;
            }
            w.write_all(&self.buf[..n])?;
            self.prog.bytes_done += n as u64;
            self.tick(false)?;
        }
        // times and mode last: only what got copied in full looks finished.
        // Some disks can't hold these (FAT): that's no reason to fail.
        let _ = w.set_permissions(fs::Permissions::from_mode(m.mode() & 0o777));
        if let Ok(t) = m.modified() {
            let _ = w.set_modified(t);
        }
        Ok(())
    }

    /// Copy any object to exactly `to`, which must not exist. A folder's
    /// children go in under their own names.
    fn copy_object(&mut self, src: &Path, to: &Path) -> Result<(), Stop> {
        let m = fs::symlink_metadata(src)?;
        self.prog.current = display_name(&base_name(src));
        self.tick(false)?;
        let t = m.file_type();
        if t.is_symlink() {
            symlink(fs::read_link(src)?, to)?;
            self.prog.files_done += 1;
        } else if t.is_file() {
            self.copy_file(src, to, &m)?;
            self.prog.files_done += 1;
        } else if t.is_dir() {
            fs::create_dir(to)?;
            let _ = fs::set_permissions(to, fs::Permissions::from_mode(0o700));
            for e in fs::read_dir(src)? {
                let e = e?;
                self.copy_object(&e.path(), &to.join(e.file_name()))?;
            }
            let _ = fs::set_permissions(to, fs::Permissions::from_mode(m.mode() & 0o777));
            if let Ok(tm) = m.modified()
                && let Ok(d) = File::open(to)
            {
                let _ = d.set_modified(tm);
            }
            self.prog.files_done += 1;
        } else {
            // sockets, FIFOs, devices aren't files you can copy
            self.left_out += 1;
        }
        Ok(())
    }

    /// Build `src` beside its target, then give it the target's name.
    fn copy_atomic(&mut self, src: &Path, dest_dir: &Path, name: &OsStr, replace: bool) -> Result<(), Stop> {
        let part = dest_dir.join(part_name());
        let to = dest_dir.join(name);
        let done = self.copy_object(src, &part).and_then(|_| {
            if fs::symlink_metadata(&part).is_err() {
                // a lone socket or FIFO: nothing to give a name to
                return Err(Stop::Why("That isn't a file that can be copied.".into()));
            }
            if replace { fs::rename(&part, &to)? } else { rename_noreplace(&part, &to)? }
            Ok(())
        });
        if done.is_err() {
            remove_part(&part);
        }
        done
    }

    fn resolve(&mut self, sm: &Metadata, dm: &Metadata, name: &OsStr) -> Option<Choice> {
        let both_dirs = sm.is_dir() && dm.is_dir();
        let any_dir = sm.is_dir() || dm.is_dir();
        let options = if both_dirs {
            vec![Choice::Skip, Choice::KeepBoth, Choice::Merge]
        } else if any_dir {
            vec![Choice::Skip, Choice::KeepBoth]
        } else {
            vec![Choice::Skip, Choice::KeepBoth, Choice::Replace]
        };
        let sticky = if both_dirs { self.sticky_dir } else { self.sticky_file };
        if let Some(s) = sticky
            && options.contains(&s)
        {
            return Some(s);
        }
        let a = self.ctl.ask(&Clash { name: display_name(name), existing: side(dm), incoming: side(sm), options });
        if a.for_rest {
            if both_dirs {
                self.sticky_dir = a.choice;
            } else {
                self.sticky_file = a.choice;
            }
        }
        a.choice
    }

    /// One thing to one folder, with whatever that asks of the person.
    fn transfer(&mut self, src: &Path, name: &OsStr, dest_dir: &Path, moving: bool, report: &mut Report, top: bool) -> Result<bool, Stop> {
        let sm = fs::symlink_metadata(src)?;
        let name = name.to_os_string();
        let mut land = name.clone();
        let mut replace = false;
        if let Ok(dm) = fs::symlink_metadata(dest_dir.join(&name)) {
            let same_folder = canon_or_self(src.parent().unwrap_or(src)) == canon_or_self(dest_dir);
            if same_folder {
                if moving {
                    report.skipped += top as usize;
                    return Ok(false);
                }
                // copying something where it already is: obviously a copy
                land = unique_name(dest_dir, &name);
            } else {
                if sm.dev() == dm.dev() && sm.ino() == dm.ino() {
                    report.skipped += top as usize;
                    return Ok(false);
                }
                match self.resolve(&sm, &dm, &name) {
                    None => return Err(Stop::Cancelled),
                    Some(Choice::Skip) => {
                        report.skipped += top as usize;
                        return Ok(false);
                    }
                    Some(Choice::KeepBoth) => land = unique_name(dest_dir, &name),
                    Some(Choice::Replace) => replace = true,
                    Some(Choice::Merge) => {
                        let target = dest_dir.join(&name);
                        for e in fs::read_dir(src)? {
                            let e = e?;
                            self.transfer(&e.path(), &e.file_name(), &target, moving, report, false)?;
                        }
                        if moving {
                            // only an emptied folder goes; a clash that was
                            // skipped leaves it there
                            let _ = fs::remove_dir(src);
                        }
                        if top {
                            report.landed.push(name);
                        }
                        return Ok(true);
                    }
                }
            }
        }
        if moving {
            let to = dest_dir.join(&land);
            let r = if replace { fs::rename(src, &to) } else { rename_noreplace(src, &to) };
            match r {
                Ok(()) => {
                    let t = measure(&to, &|| false);
                    self.prog.files_done += t.files;
                    self.prog.bytes_done += t.bytes;
                    self.tick(true)?;
                    if top {
                        report.landed.push(land);
                    }
                    return Ok(true);
                }
                Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {}
                Err(e) => return Err(e.into()),
            }
        }
        let left_before = self.left_out;
        self.copy_atomic(src, dest_dir, &land, replace)?;
        if top {
            report.landed.push(land);
        }
        if moving {
            if self.left_out != left_before {
                // something couldn't come along: the original stays whole
                report.kept_originals += 1;
                return Ok(true);
            }
            // only now, with the copy safely on its disk
            sync_disk(dest_dir).map_err(|e| Stop::Why(format!("The copy may not be safe: {} The original was kept.", plain(&e))))?;
            if let Err(e) = remove_tree(src, &|| false) {
                report.failed.push(Failure { name: display_name(&name), why: format!("Copied, but the original couldn't be removed: {e}") });
                report.kept_originals += 1;
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn say(s: Stop) -> Option<String> {
    match s {
        Stop::Cancelled => None,
        Stop::Io(e) => Some(plain(&e)),
        Stop::Why(w) => Some(w),
    }
}

/// Clean up a partial copy: it may hold folders we made read-only.
fn remove_part(p: &Path) {
    fn open_up(p: &Path) {
        if let Ok(m) = fs::symlink_metadata(p)
            && m.is_dir()
        {
            let _ = fs::set_permissions(p, fs::Permissions::from_mode(0o700));
            if let Ok(rd) = fs::read_dir(p) {
                for e in rd.flatten() {
                    open_up(&e.path());
                }
            }
        }
    }
    open_up(p);
    let _ = match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(p),
        Ok(_) => fs::remove_file(p),
        Err(_) => Ok(()),
    };
}

/// Remove `p`: a link is removed as a link, whatever it points at. A folder
/// that holds another disk is refused rather than entered. std's
/// `remove_dir_all` does the walking below the first level (it works through
/// directory handles and never follows a link); the first level is here so a
/// cancel takes effect in a big folder.
pub fn remove_tree(p: &Path, cancel: &dyn Fn() -> bool) -> Result<(), String> {
    let m = fs::symlink_metadata(p).map_err(|e| plain(&e))?;
    if !m.is_dir() {
        return fs::remove_file(p).map_err(|e| plain(&e));
    }
    if measure(p, cancel).crosses_mount {
        return Err("There's a mounted disk inside it.".into());
    }
    for e in fs::read_dir(p).map_err(|e| plain(&e))? {
        if cancel() {
            return Err("Stopped.".into());
        }
        let e = e.map_err(|e| plain(&e))?;
        let child = e.path();
        let t = e.file_type().map_err(|e| plain(&e))?;
        let r = if t.is_dir() { fs::remove_dir_all(&child) } else { fs::remove_file(&child) };
        r.map_err(|e| format!("{}: {}", display_name(&base_name(&child)), plain(&e)))?;
    }
    fs::remove_dir(p).map_err(|e| plain(&e))
}

// ----------------------------------------------------------------- trash ----

/// The freedesktop.org home Trash: `files/` and `info/`.
#[derive(Clone, Debug)]
pub struct Trash {
    pub root: PathBuf,
}

/// Percent-encode a path as a .trashinfo Path= value.
pub fn encode_path(p: &Path) -> String {
    let mut out = String::new();
    for &b in p.as_os_str().as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn decode_path(s: &str) -> PathBuf {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(a), Some(c)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(a << 4 | c);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    PathBuf::from(OsString::from_vec(out))
}

/// Local time as the trash spec wants it.
fn trash_date() -> String {
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t = now as libc::time_t;
    unsafe { libc::localtime_r(&t, &mut tm) };
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// One thing in the Trash.
#[derive(Clone, Debug, PartialEq)]
pub struct Trashed {
    /// The name in files/.
    pub id: OsString,
    pub origin: PathBuf,
    pub deleted: String,
}

impl Trash {
    pub fn home(home: &Path) -> Trash {
        Trash { root: home.join(".local/share/Trash") }
    }
    pub fn files(&self) -> PathBuf {
        self.root.join("files")
    }
    pub fn info(&self) -> PathBuf {
        self.root.join("info")
    }
    fn info_of(&self, id: &OsStr) -> PathBuf {
        let mut n = id.to_os_string();
        n.push(".trashinfo");
        self.info().join(n)
    }

    fn ensure(&self) -> io::Result<()> {
        for d in [self.root.clone(), self.files(), self.info()] {
            fs::create_dir_all(&d)?;
        }
        let _ = fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700));
        Ok(())
    }

    /// The nearest existing ancestor of the Trash is on the same disk as `p`:
    /// a rename can take it there.
    pub fn can_take(&self, p: &Path) -> bool {
        let Ok(m) = fs::symlink_metadata(p) else { return false };
        let mut t: &Path = &self.root;
        loop {
            if let Ok(tm) = fs::metadata(t) {
                return tm.dev() == m.dev();
            }
            match t.parent() {
                Some(up) => t = up,
                None => return false,
            }
        }
    }

    /// Move one object into the Trash. Never crosses a disk: that would make
    /// "delete" a long copy; the caller says so instead.
    pub fn put(&self, guard: &Guard, p: &Path) -> Result<Trashed, String> {
        guard.check_removable(p)?;
        if real_object(p).starts_with(canon_or_self(&self.root)) {
            return Err("That's already in the Trash.".into());
        }
        if !self.can_take(p) {
            return Err("It's on another disk, so it can't go in the Trash.".into());
        }
        self.ensure().map_err(|e| plain(&e))?;
        let name = base_name(p);
        let origin = real_object(p);
        // the spec's order: claim the name by creating the info file first
        // room for ".trashinfo" and a number: the origin keeps the full name
        let short = |extra: &[u8]| {
            let mut b = name.as_bytes()[..name.len().min(200)].to_vec();
            b.extend_from_slice(extra);
            OsString::from_vec(b)
        };
        let mut id = short(b"");
        let mut n = 1u32;
        let mut info = loop {
            let f = OpenOptions::new().write(true).create_new(true).mode(0o600).open(self.info_of(&id));
            match f {
                Ok(f) if fs::symlink_metadata(self.files().join(&id)).is_err() => break f,
                Ok(_) => {
                    let _ = fs::remove_file(self.info_of(&id));
                }
                Err(e) if e.raw_os_error() == Some(libc::EEXIST) => {}
                Err(e) => return Err(plain(&e)),
            }
            n += 1;
            id = short(format!(".{n}").as_bytes());
        };
        let deleted = trash_date();
        let body = format!("[Trash Info]\nPath={}\nDeletionDate={deleted}\n", encode_path(&origin));
        if let Err(e) = info.write_all(body.as_bytes()) {
            let _ = fs::remove_file(self.info_of(&id));
            return Err(plain(&e));
        }
        drop(info);
        if let Err(e) = rename_noreplace(p, &self.files().join(&id)) {
            let _ = fs::remove_file(self.info_of(&id));
            return Err(if e.raw_os_error() == Some(libc::EXDEV) { "It's on another disk, so it can't go in the Trash.".into() } else { plain(&e) });
        }
        Ok(Trashed { id, origin, deleted })
    }

    pub fn read_info(&self, id: &OsStr) -> Option<Trashed> {
        let s = fs::read(self.info_of(id)).ok()?;
        let s = String::from_utf8_lossy(&s);
        let path = s.lines().find_map(|l| l.strip_prefix("Path="))?;
        let deleted = s.lines().find_map(|l| l.strip_prefix("DeletionDate=")).unwrap_or("").to_string();
        Some(Trashed { id: id.to_os_string(), origin: decode_path(path), deleted })
    }

    /// Remove from the Trash for good.
    pub fn purge(&self, id: &OsStr, cancel: &dyn Fn() -> bool) -> Result<(), String> {
        let p = self.files().join(id);
        if id.as_bytes().contains(&b'/') {
            return Err("That isn't in the Trash.".into());
        }
        if fs::symlink_metadata(&p).is_ok() {
            remove_tree(&p, cancel)?;
        }
        let _ = fs::remove_file(self.info_of(id));
        Ok(())
    }

    pub fn count(&self) -> usize {
        fs::read_dir(self.files()).map(|r| r.flatten().count()).unwrap_or(0)
    }
}

// ------------------------------------------------------------------ jobs ----

/// Run a job to the end on this thread.
pub fn run_job(spec: &JobSpec, guard: &Guard, ctl: &mut dyn Ctl) -> Report {
    let mut report = Report::new(spec.kind, spec.sources.len());
    match spec.kind {
        JobKind::Copy | JobKind::Move => transfer_job(spec, guard, ctl, &mut report),
        JobKind::Trash => trash_job(spec, guard, ctl, &mut report),
        JobKind::Delete => delete_job(spec, guard, ctl, &mut report),
        JobKind::Restore => restore_job(spec, guard, ctl, &mut report),
    }
    report
}

fn new_run(ctl: &mut dyn Ctl) -> Run<'_> {
    Run { ctl, prog: Progress::default(), last: Instant::now() - Duration::from_secs(1), buf: vec![0; 256 * 1024], sticky_file: None, sticky_dir: None, left_out: 0 }
}

fn transfer_job(spec: &JobSpec, guard: &Guard, ctl: &mut dyn Ctl, report: &mut Report) {
    let moving = spec.kind == JobKind::Move;
    let Some(dest) = spec.dest.clone() else {
        report.refused = Some("There's nowhere to put it.".into());
        return;
    };
    if let Err(e) = guard.check_dest(&dest) {
        report.refused = Some(e);
        return;
    }
    match fs::metadata(&dest) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => {
            report.refused = Some("That isn't a folder.".into());
            return;
        }
        Err(e) => {
            report.refused = Some(plain(&e));
            return;
        }
    }
    // the plan: totals for progress, and is there room
    let mut totals = Totals::default();
    let mut needs_room = 0u64;
    let dest_dev = fs::metadata(&dest).map(|m| m.dev()).unwrap_or(0);
    for s in &spec.sources {
        let cancelled = || ctl.cancelled();
        let t = measure(s, &cancelled);
        totals.files += t.files;
        totals.bytes += t.bytes;
        let same_disk = fs::symlink_metadata(s).map(|m| m.dev() == dest_dev).unwrap_or(false);
        if !(moving && same_disk) {
            needs_room += t.bytes;
        }
    }
    if needs_room > 0
        && let Some((free, _)) = free_space(&dest)
        && needs_room > free
    {
        report.refused = Some(format!("Not enough room: it needs {}, and {} is free.", crate::listing::human_size(needs_room), crate::listing::human_size(free)));
        return;
    }
    let mut run = new_run(ctl);
    run.prog.files_total = totals.files;
    run.prog.bytes_total = totals.bytes;
    for s in &spec.sources {
        let name = display_name(&base_name(s));
        if moving
            && let Err(why) = guard.check_removable(s)
        {
            report.failed.push(Failure { name, why });
            continue;
        }
        if fs::symlink_metadata(s).map(|m| m.is_dir()).unwrap_or(false) && canon_or_self(&dest).starts_with(canon_or_self(s)) {
            report.failed.push(Failure { name, why: "A folder can't go inside itself.".into() });
            continue;
        }
        match run.transfer(s, &base_name(s), &dest, moving, report, true) {
            Ok(true) => report.done += 1,
            Ok(false) => {}
            Err(Stop::Cancelled) => {
                report.cancelled = true;
                break;
            }
            Err(st) => {
                report.failed.push(Failure { name, why: say(st).unwrap_or_default() });
            }
        }
    }
    report.bytes = run.prog.bytes_done;
    run.prog.current.clear();
    let _ = run.tick(true);
    // Removing a card right after a copy shouldn't lose it.
    let _ = sync_disk(&dest);
}

fn trash_job(spec: &JobSpec, guard: &Guard, ctl: &mut dyn Ctl, report: &mut Report) {
    let Some(root) = spec.trash.clone() else {
        report.refused = Some("There's no Trash here.".into());
        return;
    };
    let trash = Trash { root };
    let mut prog = Progress { files_total: spec.sources.len() as u64, ..Default::default() };
    for s in &spec.sources {
        if ctl.cancelled() {
            report.cancelled = true;
            break;
        }
        let name = display_name(&base_name(s));
        prog.current = name.clone();
        ctl.progress(&prog);
        match trash.put(guard, s) {
            Ok(t) => {
                report.done += 1;
                report.landed.push(t.id);
            }
            Err(why) => report.failed.push(Failure { name, why }),
        }
        prog.files_done += 1;
    }
    ctl.progress(&prog);
}

fn delete_job(spec: &JobSpec, guard: &Guard, ctl: &mut dyn Ctl, report: &mut Report) {
    let trash = spec.trash.clone().map(|root| Trash { root });
    let mut prog = Progress { files_total: spec.sources.len() as u64, ..Default::default() };
    for s in &spec.sources {
        if ctl.cancelled() {
            report.cancelled = true;
            break;
        }
        let name = display_name(&base_name(s));
        prog.current = name.clone();
        ctl.progress(&prog);
        // things in the Trash are handled with their info file
        let in_trash = trash.as_ref().filter(|t| s.parent().is_some_and(|d| canon_or_self(d) == canon_or_self(&t.files())));
        let r = match in_trash {
            Some(t) => {
                let c = || ctl.cancelled();
                t.purge(&base_name(s), &c)
            }
            None => guard.check_removable(s).and_then(|_| {
                let c = || ctl.cancelled();
                remove_tree(s, &c)
            }),
        };
        match r {
            Ok(()) => report.done += 1,
            Err(_) if ctl.cancelled() => {
                report.cancelled = true;
                break;
            }
            Err(why) => report.failed.push(Failure { name, why }),
        }
        prog.files_done += 1;
    }
    ctl.progress(&prog);
}

fn restore_job(spec: &JobSpec, guard: &Guard, ctl: &mut dyn Ctl, report: &mut Report) {
    let Some(root) = spec.trash.clone() else {
        report.refused = Some("There's no Trash here.".into());
        return;
    };
    let trash = Trash { root };
    let mut run = new_run(ctl);
    run.prog.files_total = spec.sources.len() as u64;
    for s in &spec.sources {
        let id = base_name(s);
        let Some(info) = trash.read_info(&id) else {
            report.failed.push(Failure { name: display_name(&id), why: "Mortar doesn't know where this came from.".into() });
            continue;
        };
        let name = display_name(&base_name(&info.origin));
        let parent = info.origin.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/"));
        if let Err(why) = guard.check_dest(&parent) {
            report.failed.push(Failure { name, why });
            continue;
        }
        if let Err(e) = fs::create_dir_all(&parent) {
            report.failed.push(Failure { name, why: plain(&e) });
            continue;
        }
        match run.transfer(s, &base_name(&info.origin), &parent, true, report, true) {
            Ok(true) => {
                report.done += 1;
                let _ = fs::remove_file(trash.info_of(&id));
            }
            Ok(false) => {}
            Err(Stop::Cancelled) => {
                report.cancelled = true;
                break;
            }
            Err(st) => report.failed.push(Failure { name, why: say(st).unwrap_or_default() }),
        }
    }
    let _ = run.tick(true);
}

// --------------------------------------------------------------- threads ----

/// What a job thread sends.
#[derive(Debug)]
pub enum JobMsg {
    Progress(Progress),
    Clash(Clash),
    Done(Report),
}

/// A job on its own thread.
pub struct Job {
    pub rx: Receiver<JobMsg>,
    pub answers: Sender<Answer>,
    pub cancel: Arc<AtomicBool>,
}

struct ThreadCtl {
    tx: Sender<JobMsg>,
    answers: Receiver<Answer>,
    cancel: Arc<AtomicBool>,
    wake: crate::bg::Wake,
}

impl Ctl for ThreadCtl {
    fn progress(&mut self, p: &Progress) {
        let _ = self.tx.send(JobMsg::Progress(p.clone()));
        (self.wake)();
    }
    fn ask(&mut self, c: &Clash) -> Answer {
        let _ = self.tx.send(JobMsg::Clash(c.clone()));
        (self.wake)();
        // the app going away counts as a cancel
        self.answers.recv().unwrap_or(Answer { choice: None, for_rest: false })
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

pub fn spawn(spec: JobSpec, guard: Guard, wake: crate::bg::Wake) -> Job {
    let (tx, rx) = channel();
    let (atx, arx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let c2 = cancel.clone();
    std::thread::Builder::new()
        .name("files-job".into())
        .spawn(move || {
            let mut ctl = ThreadCtl { tx: tx.clone(), answers: arx, cancel: c2, wake: wake.clone() };
            let r = run_job(&spec, &guard, &mut ctl);
            let _ = tx.send(JobMsg::Done(r));
            wake();
        })
        .expect("spawn a thread");
    Job { rx, answers: atx, cancel }
}
