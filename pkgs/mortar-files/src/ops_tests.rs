// SPDX-License-Identifier: GPL-2.0-or-later
//! The operations against real temp folders.

use crate::ops::*;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);

pub fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mortar-files-test-{}-{}-{tag}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir_all(&d).unwrap();
    fs::canonicalize(d).unwrap()
}

/// Running as root (a Nix sandbox on some setups) permission tests prove nothing.
fn root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// Answers from a script; records what it was asked; cancels on request.
#[derive(Default)]
struct Script {
    answers: Vec<Answer>,
    asked: Vec<Clash>,
    /// Cancel after this many progress callbacks.
    cancel_after: Option<usize>,
    progress: usize,
    last: Progress,
}

impl Ctl for Script {
    fn progress(&mut self, p: &Progress) {
        self.progress += 1;
        self.last = p.clone();
    }
    fn ask(&mut self, c: &Clash) -> Answer {
        self.asked.push(c.clone());
        if self.answers.is_empty() { Answer { choice: None, for_rest: false } } else { self.answers.remove(0) }
    }
    fn cancelled(&self) -> bool {
        self.cancel_after.is_some_and(|n| self.progress >= n)
    }
}

fn answer(c: Choice) -> Answer {
    Answer { choice: Some(c), for_rest: false }
}

fn job(kind: JobKind, sources: Vec<PathBuf>, dest: &Path) -> JobSpec {
    JobSpec { kind, sources, dest: Some(dest.to_path_buf()), trash: None }
}

fn guard() -> Guard {
    Guard::system(None, vec![])
}

fn names(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

fn no_parts(d: &Path) {
    for e in fs::read_dir(d).unwrap().flatten() {
        assert!(!e.file_name().to_string_lossy().starts_with(".mortar-part"), "a part left behind: {:?}", e.path());
    }
}

// ------------------------------------------------------------------ copy ----

#[test]
fn copy_a_tree_keeps_links_modes_and_times() {
    let d = temp("copy");
    let (a, b) = (d.join("a"), d.join("b"));
    fs::create_dir_all(a.join("tree/sub")).unwrap();
    fs::create_dir(&b).unwrap();
    fs::write(a.join("tree/sub/data.bin"), vec![7u8; 300_000]).unwrap();
    fs::set_permissions(a.join("tree/sub/data.bin"), fs::Permissions::from_mode(0o640)).unwrap();
    symlink("sub/data.bin", a.join("tree/link")).unwrap();
    let f = fs::File::options().write(true).open(a.join("tree/sub/data.bin")).unwrap();
    let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_500_000_000);
    f.set_modified(old).unwrap();
    drop(f);

    let mut s = Script::default();
    let r = run_job(&job(JobKind::Copy, vec![a.join("tree")], &b), &guard(), &mut s);
    assert!(r.clean(), "{r:?}");
    assert_eq!((r.done, r.skipped), (1, 0));
    assert_eq!(r.headline(), "Copied 1 item.");
    let copied = b.join("tree/sub/data.bin");
    assert_eq!(fs::read(&copied).unwrap(), vec![7u8; 300_000]);
    assert_eq!(fs::metadata(&copied).unwrap().mode() & 0o777, 0o640);
    assert_eq!(fs::metadata(&copied).unwrap().mtime(), 1_500_000_000);
    assert_eq!(fs::read_link(b.join("tree/link")).unwrap(), Path::new("sub/data.bin"));
    assert!(a.join("tree/sub/data.bin").exists(), "a copy leaves the original");
    assert_eq!(s.last.bytes_done, 300_000);
    assert_eq!(s.last.bytes_total, 300_000);
    assert_eq!(s.last.files_done, s.last.files_total);
    no_parts(&b);
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn a_clash_is_asked_and_never_overwritten_silently() {
    let d = temp("clash");
    let (a, b) = (d.join("a"), d.join("b"));
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    fs::write(a.join("save.srm"), "new").unwrap();
    fs::write(b.join("save.srm"), "old").unwrap();

    // no answer means cancel: nothing changes
    let mut s = Script::default();
    let r = run_job(&job(JobKind::Copy, vec![a.join("save.srm")], &b), &guard(), &mut s);
    assert!(r.cancelled);
    assert_eq!(s.asked.len(), 1);
    assert_eq!(s.asked[0].name, "save.srm");
    assert_eq!(s.asked[0].options, [Choice::Skip, Choice::KeepBoth, Choice::Replace]);
    assert_eq!(fs::read(b.join("save.srm")).unwrap(), b"old");

    // skip
    let mut s = Script { answers: vec![answer(Choice::Skip)], ..Default::default() };
    let r = run_job(&job(JobKind::Copy, vec![a.join("save.srm")], &b), &guard(), &mut s);
    assert_eq!((r.done, r.skipped), (0, 1));
    assert_eq!(r.headline(), "Nothing changed.");
    assert_eq!(fs::read(b.join("save.srm")).unwrap(), b"old");

    // keep both
    let mut s = Script { answers: vec![answer(Choice::KeepBoth)], ..Default::default() };
    let r = run_job(&job(JobKind::Copy, vec![a.join("save.srm")], &b), &guard(), &mut s);
    assert_eq!(r.done, 1);
    assert_eq!(r.landed, [OsString::from("save (2).srm")]);
    assert_eq!(fs::read(b.join("save.srm")).unwrap(), b"old");
    assert_eq!(fs::read(b.join("save (2).srm")).unwrap(), b"new");

    // replace, only when told to
    let mut s = Script { answers: vec![answer(Choice::Replace)], ..Default::default() };
    let r = run_job(&job(JobKind::Copy, vec![a.join("save.srm")], &b), &guard(), &mut s);
    assert_eq!(r.done, 1);
    assert_eq!(fs::read(b.join("save.srm")).unwrap(), b"new");
    no_parts(&b);
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn answer_for_the_rest_is_remembered_by_sort() {
    let d = temp("sticky");
    let (a, b) = (d.join("a"), d.join("b"));
    fs::create_dir_all(a.join("x")).unwrap();
    fs::create_dir(&b).unwrap();
    for n in ["1", "2", "3"] {
        fs::write(a.join(n), "new").unwrap();
        fs::write(b.join(n), "old").unwrap();
    }
    fs::create_dir(b.join("x")).unwrap();
    let mut s = Script { answers: vec![Answer { choice: Some(Choice::Replace), for_rest: true }, answer(Choice::Skip)], ..Default::default() };
    let src: Vec<_> = ["1", "2", "3", "x"].iter().map(|n| a.join(n)).collect();
    let r = run_job(&job(JobKind::Copy, src, &b), &guard(), &mut s);
    // one question for the files, one for the folder: Replace can't apply to it
    assert_eq!(s.asked.len(), 2);
    assert_eq!(r.done, 3);
    assert_eq!(fs::read(b.join("3")).unwrap(), b"new");
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn folders_merge_with_each_clash_asked() {
    let d = temp("merge");
    let (a, b) = (d.join("a"), d.join("b"));
    fs::create_dir_all(a.join("music/deep")).unwrap();
    fs::create_dir_all(b.join("music/deep")).unwrap();
    fs::write(a.join("music/deep/same.txt"), "new").unwrap();
    fs::write(a.join("music/only-in-a.txt"), "a").unwrap();
    fs::write(b.join("music/deep/same.txt"), "old").unwrap();
    fs::write(b.join("music/only-in-b.txt"), "b").unwrap();
    let mut s = Script { answers: vec![answer(Choice::Merge), answer(Choice::Merge), answer(Choice::Skip)], ..Default::default() };
    let r = run_job(&job(JobKind::Copy, vec![a.join("music")], &b), &guard(), &mut s);
    assert_eq!(s.asked.len(), 3);
    assert_eq!(s.asked[0].options, [Choice::Skip, Choice::KeepBoth, Choice::Merge]);
    assert_eq!(r.done, 1);
    assert_eq!(fs::read(b.join("music/deep/same.txt")).unwrap(), b"old");
    assert_eq!(names(&b.join("music")), ["deep", "only-in-a.txt", "only-in-b.txt"]);
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn a_file_and_a_folder_with_one_name_cannot_replace() {
    let d = temp("mismatch");
    let (a, b) = (d.join("a"), d.join("b"));
    fs::create_dir(&a).unwrap();
    fs::create_dir_all(b.join("thing/inside")).unwrap();
    fs::write(a.join("thing"), "file").unwrap();
    let mut s = Script { answers: vec![answer(Choice::KeepBoth)], ..Default::default() };
    run_job(&job(JobKind::Copy, vec![a.join("thing")], &b), &guard(), &mut s);
    assert_eq!(s.asked[0].options, [Choice::Skip, Choice::KeepBoth]);
    assert!(b.join("thing/inside").is_dir());
    assert_eq!(fs::read(b.join("thing (2)")).unwrap(), b"file");
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn copying_onto_itself_makes_a_copy() {
    let d = temp("self");
    fs::write(d.join("a.txt"), "x").unwrap();
    let mut s = Script::default();
    let r = run_job(&job(JobKind::Copy, vec![d.join("a.txt")], &d), &guard(), &mut s);
    assert!(s.asked.is_empty());
    assert_eq!(r.landed, [OsString::from("a (2).txt")]);
    assert_eq!(names(&d), ["a (2).txt", "a.txt"]);
    // moving onto itself does nothing
    let r = run_job(&job(JobKind::Move, vec![d.join("a.txt")], &d), &guard(), &mut s);
    assert_eq!((r.done, r.skipped), (0, 1));
    assert_eq!(names(&d), ["a (2).txt", "a.txt"]);
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn a_folder_cannot_go_inside_itself() {
    let d = temp("inside");
    fs::create_dir_all(d.join("a/b")).unwrap();
    fs::write(d.join("a/f"), "x").unwrap();
    for kind in [JobKind::Copy, JobKind::Move] {
        let mut s = Script::default();
        let r = run_job(&job(kind, vec![d.join("a")], &d.join("a/b")), &guard(), &mut s);
        assert_eq!(r.failed.len(), 1);
        assert_eq!(r.failed[0].why, "A folder can't go inside itself.");
        assert!(d.join("a/f").exists());
        assert_eq!(names(&d.join("a/b")), Vec::<String>::new());
    }
    fs::remove_dir_all(d).unwrap();
}

// ---------------------------------------------------------------- cancel ----

#[test]
fn cancel_leaves_originals_and_no_partial_copy() {
    let d = temp("cancel");
    let (a, b) = (d.join("a"), d.join("b"));
    fs::create_dir_all(a.join("big")).unwrap();
    fs::create_dir(&b).unwrap();
    for i in 0..30 {
        fs::write(a.join(format!("big/{i}")), vec![1u8; 1 << 20]).unwrap();
    }
    // the first progress call goes through, then cancel is seen at the next check
    let mut s = Script { cancel_after: Some(1), ..Default::default() };
    let r = run_job(&job(JobKind::Copy, vec![a.join("big")], &b), &guard(), &mut s);
    assert!(r.cancelled);
    assert_eq!(r.done, 0);
    assert_eq!(names(&b), Vec::<String>::new(), "no folder under its final name, no parts");
    assert_eq!(names(&a.join("big")).len(), 30);
    assert_eq!(r.headline(), "Stopped. Copied 0 items first.");
    assert!(r.details().iter().any(|l| l == "Your originals are untouched."));

    let mut s = Script { cancel_after: Some(1), ..Default::default() };
    let r = run_job(&job(JobKind::Move, vec![a.join("big")], &b), &guard(), &mut s);
    // a rename is one step on one disk, so it either happened or it didn't;
    // either way nothing is half-way
    if r.cancelled && r.done == 0 && r.landed.is_empty() {
        assert_eq!(names(&a.join("big")).len(), 30);
    } else {
        assert_eq!(names(&b.join("big")).len(), 30);
    }
    no_parts(&b);
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn cancel_from_a_clash_keeps_whats_done() {
    let d = temp("cancel2");
    let (a, b) = (d.join("a"), d.join("b"));
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    fs::write(a.join("1"), "a").unwrap();
    fs::write(a.join("2"), "a").unwrap();
    fs::write(b.join("2"), "b").unwrap();
    let mut s = Script::default();
    let r = run_job(&job(JobKind::Copy, vec![a.join("1"), a.join("2")], &b), &guard(), &mut s);
    assert!(r.cancelled);
    assert_eq!(r.done, 1);
    assert_eq!(r.headline(), "Stopped. Copied 1 item first.");
    assert_eq!(fs::read(b.join("2")).unwrap(), b"b");
    fs::remove_dir_all(d).unwrap();
}

// -------------------------------------------------------------- failures ----

#[test]
fn a_failure_partway_says_so_and_leaves_originals() {
    if root() {
        return;
    }
    let d = temp("fail");
    let (a, b) = (d.join("a"), d.join("b"));
    fs::create_dir_all(a.join("tree/locked")).unwrap();
    fs::create_dir(&b).unwrap();
    fs::write(a.join("tree/fine"), "x").unwrap();
    fs::write(a.join("tree/locked/secret"), "x").unwrap();
    fs::write(a.join("after"), "y").unwrap();
    fs::set_permissions(a.join("tree/locked"), fs::Permissions::from_mode(0o000)).unwrap();
    let mut s = Script::default();
    let r = run_job(&job(JobKind::Copy, vec![a.join("tree"), a.join("after")], &b), &guard(), &mut s);
    fs::set_permissions(a.join("tree/locked"), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(r.failed.len(), 1);
    assert_eq!(r.failed[0].name, "tree");
    assert_eq!(r.failed[0].why, "You don't have permission.");
    assert_eq!(r.done, 1, "it carries on with the next thing");
    assert_eq!(r.headline(), "Copied 1 of 2.");
    assert!(r.details().iter().any(|l| l == "Your originals are untouched."));
    assert_eq!(names(&b), ["after"], "no partial tree under any name");
    assert!(a.join("tree/fine").exists());
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn writing_to_a_folder_without_permission_is_a_plain_message() {
    if root() {
        return;
    }
    let d = temp("deny");
    let (a, b) = (d.join("a"), d.join("b"));
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    fs::write(a.join("f"), "x").unwrap();
    fs::set_permissions(&b, fs::Permissions::from_mode(0o555)).unwrap();
    let mut s = Script::default();
    let r = run_job(&job(JobKind::Copy, vec![a.join("f")], &b), &guard(), &mut s);
    assert_eq!(r.failed[0].why, "You don't have permission.");
    let g = guard();
    assert_eq!(new_folder(&g, &b, "x").unwrap_err(), "You don't have permission.");
    fs::write(a.join("g"), "y").unwrap();
    fs::set_permissions(&a, fs::Permissions::from_mode(0o555)).unwrap();
    assert_eq!(rename(&g, &a.join("g"), "h").unwrap_err(), "You don't have permission.");
    assert_eq!(names(&a), ["f", "g"]);
    fs::set_permissions(&a, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&b, fs::Permissions::from_mode(0o755)).unwrap();
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn not_enough_room_is_refused_before_anything_happens() {
    let d = temp("room");
    let (a, b) = (d.join("a"), d.join("b"));
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    // a sparse file bigger than the free space here
    let (free, _) = free_space(&d).unwrap();
    let f = fs::File::create(a.join("huge")).unwrap();
    if f.set_len(free + (1 << 30)).is_err() {
        // this filesystem can't hold a file that size at all
        fs::remove_dir_all(d).unwrap();
        return;
    }
    drop(f);
    let mut s = Script::default();
    let r = run_job(&job(JobKind::Copy, vec![a.join("huge")], &b), &guard(), &mut s);
    assert!(r.refused.as_ref().is_some_and(|x| x.starts_with("Not enough room")), "{r:?}");
    assert_eq!(r.details(), ["Nothing was changed."]);
    assert_eq!(names(&b), Vec::<String>::new());
    // a move on one disk needs no room
    let r = run_job(&job(JobKind::Move, vec![a.join("huge")], &b), &guard(), &mut s);
    assert!(r.clean(), "{r:?}");
    assert_eq!(names(&b), ["huge"]);
    fs::remove_dir_all(d).unwrap();
}

// ------------------------------------------------------------------ move ----

#[test]
fn move_on_one_disk_renames() {
    let d = temp("move");
    let (a, b) = (d.join("a"), d.join("b"));
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    fs::write(a.join("f"), "x").unwrap();
    let ino = fs::metadata(a.join("f")).unwrap().ino();
    let mut s = Script::default();
    let r = run_job(&job(JobKind::Move, vec![a.join("f")], &b), &guard(), &mut s);
    assert!(r.clean());
    assert_eq!(fs::metadata(b.join("f")).unwrap().ino(), ino, "the same file: nothing was copied");
    assert!(!a.join("f").exists());
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn move_clash_keeps_both_sides_until_told() {
    let d = temp("moveclash");
    let (a, b) = (d.join("a"), d.join("b"));
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    fs::write(a.join("f"), "new").unwrap();
    fs::write(b.join("f"), "old").unwrap();
    let mut s = Script { answers: vec![answer(Choice::Skip)], ..Default::default() };
    run_job(&job(JobKind::Move, vec![a.join("f")], &b), &guard(), &mut s);
    assert_eq!(fs::read(a.join("f")).unwrap(), b"new");
    assert_eq!(fs::read(b.join("f")).unwrap(), b"old");
    let mut s = Script { answers: vec![answer(Choice::Replace)], ..Default::default() };
    run_job(&job(JobKind::Move, vec![a.join("f")], &b), &guard(), &mut s);
    assert!(!a.join("f").exists());
    assert_eq!(fs::read(b.join("f")).unwrap(), b"new");
    fs::remove_dir_all(d).unwrap();
}

#[cfg(target_os = "linux")]
mod cross_disk {
    //! A move across disks, with a real second filesystem when the test can
    //! get one: /dev/shm is tmpfs on any Linux with a /dev.
    use super::*;

    fn shm() -> Option<PathBuf> {
        let base = Path::new("/dev/shm");
        if !base.is_dir() {
            return None;
        }
        let d = base.join(format!("mortar-files-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&d).ok()?;
        Some(d)
    }

    #[test]
    fn copies_then_removes() {
        let d = temp("xdev");
        let Some(far) = shm() else { return };
        if fs::metadata(&d).unwrap().dev() == fs::metadata(&far).unwrap().dev() {
            eprintln!("SKIPPED: no second filesystem here");
            return;
        }
        fs::create_dir_all(d.join("tree/sub")).unwrap();
        fs::write(d.join("tree/sub/f"), vec![3u8; 100_000]).unwrap();
        symlink("/etc/hostname", d.join("tree/link")).unwrap();
        let mut s = Script::default();
        let mut g = guard();
        g.read_only.retain(|p| p != Path::new("/dev"));
        let r = run_job(&job(JobKind::Move, vec![d.join("tree")], &far), &g, &mut s);
        assert!(r.clean(), "{r:?}");
        assert!(!d.join("tree").exists(), "the original goes after the copy");
        assert_eq!(fs::read(far.join("tree/sub/f")).unwrap(), vec![3u8; 100_000]);
        assert_eq!(fs::read_link(far.join("tree/link")).unwrap(), Path::new("/etc/hostname"));
        no_parts(&far);
        fs::remove_dir_all(d).unwrap();
        fs::remove_dir_all(far).unwrap();
    }

    #[test]
    fn a_failed_copy_keeps_the_original() {
        if root() {
            return;
        }
        let d = temp("xdevfail");
        let Some(far) = shm() else { return };
        if fs::metadata(&d).unwrap().dev() == fs::metadata(&far).unwrap().dev() {
            eprintln!("SKIPPED: no second filesystem here");
            return;
        }
        fs::create_dir_all(d.join("tree/locked")).unwrap();
        fs::write(d.join("tree/ok"), "x").unwrap();
        fs::write(d.join("tree/locked/f"), "x").unwrap();
        fs::set_permissions(d.join("tree/locked"), fs::Permissions::from_mode(0)).unwrap();
        let mut g = guard();
        g.read_only.retain(|p| p != Path::new("/dev"));
        let mut s = Script::default();
        let r = run_job(&job(JobKind::Move, vec![d.join("tree")], &far), &g, &mut s);
        fs::set_permissions(d.join("tree/locked"), fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(r.failed.len(), 1);
        assert_eq!(r.done, 0);
        assert!(d.join("tree/ok").exists() && d.join("tree/locked/f").exists(), "originals intact");
        assert_eq!(names(&far), Vec::<String>::new());
        assert!(r.details().iter().any(|l| l.contains("still where it was")));
        fs::remove_dir_all(d).unwrap();
        fs::remove_dir_all(far).unwrap();
    }

    #[test]
    fn what_cant_come_along_keeps_the_original() {
        let d = temp("xdevfifo");
        let Some(far) = shm() else { return };
        if fs::metadata(&d).unwrap().dev() == fs::metadata(&far).unwrap().dev() {
            eprintln!("SKIPPED: no second filesystem here");
            return;
        }
        fs::create_dir(d.join("tree")).unwrap();
        fs::write(d.join("tree/f"), "x").unwrap();
        let c = std::ffi::CString::new(d.join("tree/pipe").as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let mut g = guard();
        g.read_only.retain(|p| p != Path::new("/dev"));
        let mut s = Script::default();
        let r = run_job(&job(JobKind::Move, vec![d.join("tree")], &far), &g, &mut s);
        assert_eq!(r.kept_originals, 1);
        assert_eq!(r.details(), ["1 item is now in both places."]);
        assert!(d.join("tree/f").exists() && d.join("tree/pipe").exists(), "the original stays whole");
        assert!(far.join("tree/f").exists());
        fs::remove_dir_all(d).unwrap();
        fs::remove_dir_all(far).unwrap();
    }

    #[test]
    fn trash_refuses_another_disk_and_leaves_it() {
        let d = temp("xdevtrash");
        let Some(far) = shm() else { return };
        if fs::metadata(&d).unwrap().dev() == fs::metadata(&far).unwrap().dev() {
            eprintln!("SKIPPED: no second filesystem here");
            return;
        }
        fs::write(far.join("f"), "x").unwrap();
        let t = Trash { root: d.join("Trash") };
        let mut g = guard();
        g.read_only.clear();
        assert!(!t.can_take(&far.join("f")));
        assert_eq!(t.put(&g, &far.join("f")).unwrap_err(), "It's on another disk, so it can't go in the Trash.");
        assert!(far.join("f").exists());
        fs::remove_dir_all(d).unwrap();
        fs::remove_dir_all(far).unwrap();
    }
}

// -------------------------------------------------------------- symlinks ----

#[test]
fn delete_follows_no_links() {
    let d = temp("del");
    let outside = temp("del-outside");
    fs::write(outside.join("precious"), "keep me").unwrap();
    fs::create_dir(outside.join("folder")).unwrap();
    fs::write(outside.join("folder/also"), "keep me").unwrap();
    // a link as the thing deleted, and links inside a deleted tree
    symlink(&outside, d.join("top-link")).unwrap();
    fs::create_dir_all(d.join("tree/deep/deeper")).unwrap();
    symlink(&outside, d.join("tree/link")).unwrap();
    symlink(outside.join("folder"), d.join("tree/deep/deeper/link")).unwrap();
    symlink(outside.join("precious"), d.join("tree/deep/file-link")).unwrap();

    let mut s = Script::default();
    let spec = JobSpec { kind: JobKind::Delete, sources: vec![d.join("top-link"), d.join("tree")], dest: None, trash: None };
    let r = run_job(&spec, &guard(), &mut s);
    assert!(r.clean(), "{r:?}");
    assert_eq!(r.done, 2);
    assert_eq!(names(&d), Vec::<String>::new());
    assert_eq!(fs::read(outside.join("precious")).unwrap(), b"keep me");
    assert_eq!(fs::read(outside.join("folder/also")).unwrap(), b"keep me");
    fs::remove_dir_all(d).unwrap();
    fs::remove_dir_all(outside).unwrap();
}

#[test]
fn copy_keeps_a_link_a_link() {
    let d = temp("linkcopy");
    let outside = temp("linkcopy-outside");
    fs::write(outside.join("big"), vec![0u8; 1 << 20]).unwrap();
    symlink(&outside, d.join("link")).unwrap();
    let b = d.join("b");
    fs::create_dir(&b).unwrap();
    let mut s = Script::default();
    let r = run_job(&job(JobKind::Copy, vec![d.join("link")], &b), &guard(), &mut s);
    assert!(r.clean());
    assert!(fs::symlink_metadata(b.join("link")).unwrap().file_type().is_symlink());
    assert_eq!(fs::read_link(b.join("link")).unwrap(), outside);
    fs::remove_dir_all(d).unwrap();
    fs::remove_dir_all(outside).unwrap();
}

#[test]
fn replacing_a_link_replaces_the_link_not_its_target() {
    let d = temp("replink");
    let outside = temp("replink-outside");
    fs::write(outside.join("target"), "keep me").unwrap();
    let (a, b) = (d.join("a"), d.join("b"));
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    fs::write(a.join("f"), "new").unwrap();
    symlink(outside.join("target"), b.join("f")).unwrap();
    let mut s = Script { answers: vec![answer(Choice::Replace)], ..Default::default() };
    let r = run_job(&job(JobKind::Copy, vec![a.join("f")], &b), &guard(), &mut s);
    assert!(r.clean(), "{r:?}");
    assert_eq!(fs::read(outside.join("target")).unwrap(), b"keep me");
    assert!(fs::symlink_metadata(b.join("f")).unwrap().is_file());
    fs::remove_dir_all(d).unwrap();
    fs::remove_dir_all(outside).unwrap();
}

#[test]
fn trash_takes_the_link_not_its_target() {
    let d = temp("trashlink");
    let outside = temp("trashlink-outside");
    fs::write(outside.join("precious"), "keep").unwrap();
    symlink(outside.join("precious"), d.join("link")).unwrap();
    let t = Trash { root: d.join("Trash") };
    t.put(&guard(), &d.join("link")).unwrap();
    assert!(fs::symlink_metadata(t.files().join("link")).unwrap().file_type().is_symlink());
    assert_eq!(fs::read(outside.join("precious")).unwrap(), b"keep");
    fs::remove_dir_all(d).unwrap();
    fs::remove_dir_all(outside).unwrap();
}

#[test]
fn a_fifo_in_a_tree_is_left_out_of_a_copy_without_hanging() {
    let d = temp("fifo");
    fs::create_dir(d.join("tree")).unwrap();
    fs::write(d.join("tree/f"), "x").unwrap();
    let c = std::ffi::CString::new(d.join("tree/pipe").as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let b = d.join("b");
    fs::create_dir(&b).unwrap();
    let mut s = Script::default();
    let r = run_job(&job(JobKind::Copy, vec![d.join("tree")], &b), &guard(), &mut s);
    assert_eq!(r.done, 1);
    assert_eq!(names(&b.join("tree")), ["f"]);
    fs::remove_dir_all(d).unwrap();
}

// ----------------------------------------------------------------- guard ----

#[test]
fn read_only_places_are_refused_by_the_operations() {
    let d = temp("guard");
    let locked = d.join("store");
    fs::create_dir(&locked).unwrap();
    fs::write(locked.join("f"), "x").unwrap();
    let outside = d.join("home");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("mine"), "m").unwrap();
    // a link in home that points into the read-only tree
    symlink(&locked, outside.join("into-store")).unwrap();
    let g = Guard { read_only: vec![locked.clone()], pinned: vec![], home: None };

    let mut s = Script::default();
    let r = run_job(&job(JobKind::Copy, vec![outside.join("mine")], &locked), &g, &mut s);
    assert_eq!(r.refused.as_deref(), Some("This place is read-only."));
    let r = run_job(&job(JobKind::Copy, vec![outside.join("mine")], &outside.join("into-store")), &g, &mut s);
    assert_eq!(r.refused.as_deref(), Some("This place is read-only."), "through a link too");
    let r = run_job(&job(JobKind::Move, vec![locked.join("f")], &outside), &g, &mut s);
    assert_eq!(r.failed[0].why, "This place is read-only.");
    assert!(locked.join("f").exists());
    let spec = JobSpec { kind: JobKind::Delete, sources: vec![locked.join("f")], dest: None, trash: None };
    let r = run_job(&spec, &g, &mut s);
    assert_eq!(r.failed[0].why, "This place is read-only.");
    assert!(locked.join("f").exists());
    let t = Trash { root: d.join("Trash") };
    assert_eq!(t.put(&g, &locked.join("f")).unwrap_err(), "This place is read-only.");
    assert_eq!(rename(&g, &locked.join("f"), "g").unwrap_err(), "This place is read-only.");
    assert_eq!(new_folder(&g, &locked, "x").unwrap_err(), "This place is read-only.");
    // copying out is fine
    let r = run_job(&job(JobKind::Copy, vec![locked.join("f")], &outside), &g, &mut s);
    assert!(r.clean());
    // and the link itself, which lives in home, can go
    g.check_removable(&outside.join("into-store")).unwrap();
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn the_system_trees_are_read_only() {
    let g = guard();
    for p in ["/nix/store/abc-foo/bin", "/proc/1", "/sys/class", "/dev/null", "/nix/store", "/boot/extlinux"] {
        assert!(g.is_read_only(Path::new(p)), "{p}");
    }
    assert!(!g.is_read_only(Path::new("/home/brick")));
    assert!(!g.is_read_only(Path::new("/nixos")), "by path parts, not by prefix");
    assert_eq!(g.check_removable(Path::new("/nix/store/abc")).unwrap_err(), "This place is read-only.");
}

#[test]
fn vital_folders_cannot_be_removed() {
    let d = temp("vital");
    let home = d.join("home/brick");
    fs::create_dir_all(home.join("roms")).unwrap();
    let g = Guard { read_only: vec![], pinned: vec![home.join("roms")], home: Some(home.clone()) };
    assert_eq!(g.check_removable(Path::new("/")).unwrap_err(), "That's part of the system.");
    assert_eq!(g.check_removable(Path::new("/etc")).unwrap_err(), "That's part of the system.");
    assert_eq!(g.check_removable(&home).unwrap_err(), "That holds your home folder.");
    assert_eq!(g.check_removable(&d.join("home")).unwrap_err(), "That holds your home folder.");
    assert_eq!(g.check_removable(&home.join("roms")).unwrap_err(), "Mortar needs that folder.");
    fs::write(home.join("ok"), "x").unwrap();
    g.check_removable(&home.join("ok")).unwrap();
    fs::remove_dir_all(d).unwrap();
}

// ----------------------------------------------------------------- trash ----

#[test]
fn trash_and_restore_round_trip() {
    let d = temp("trash");
    let t = Trash { root: d.join("home/.local/share/Trash") };
    fs::create_dir_all(d.join("home/games")).unwrap();
    let f = d.join("home/games/Star Courier (USA) [100%].nes");
    fs::write(&f, "rom").unwrap();
    let g = guard();
    let tr = t.put(&g, &f).unwrap();
    assert!(!f.exists());
    assert!(t.files().join(&tr.id).exists());
    assert_eq!(t.count(), 1);
    let info = fs::read_to_string(t.info().join(format!("{}.trashinfo", tr.id.to_string_lossy()))).unwrap();
    assert!(info.starts_with("[Trash Info]\nPath=/"), "{info}");
    assert!(info.contains("Star%20Courier%20%28USA%29%20%5B100%25%5D.nes"), "{info}");
    assert!(info.contains("DeletionDate=20"), "{info}");
    assert_eq!(t.read_info(&tr.id).unwrap().origin, f);

    let spec = JobSpec { kind: JobKind::Restore, sources: vec![t.files().join(&tr.id)], dest: None, trash: Some(t.root.clone()) };
    let mut s = Script::default();
    let r = run_job(&spec, &g, &mut s);
    assert!(r.clean(), "{r:?}");
    assert_eq!(fs::read(&f).unwrap(), b"rom");
    assert_eq!(t.count(), 0);
    assert_eq!(names(&t.info()), Vec::<String>::new());
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn same_name_twice_in_the_trash_restores_both() {
    let d = temp("trash2");
    let t = Trash { root: d.join("Trash") };
    let g = guard();
    fs::write(d.join("x.txt"), "first").unwrap();
    let a = t.put(&g, &d.join("x.txt")).unwrap();
    fs::write(d.join("x.txt"), "second").unwrap();
    let b = t.put(&g, &d.join("x.txt")).unwrap();
    assert_ne!(a.id, b.id);
    // restore the second under its own name, then the first asks
    let spec = |id: &OsStr| JobSpec { kind: JobKind::Restore, sources: vec![t.files().join(id)], dest: None, trash: Some(t.root.clone()) };
    let mut s = Script::default();
    run_job(&spec(&b.id), &g, &mut s);
    assert_eq!(fs::read(d.join("x.txt")).unwrap(), b"second");
    let mut s = Script { answers: vec![answer(Choice::KeepBoth)], ..Default::default() };
    let r = run_job(&spec(&a.id), &g, &mut s);
    assert_eq!(s.asked.len(), 1);
    assert!(r.clean());
    assert_eq!(fs::read(d.join("x.txt")).unwrap(), b"second");
    assert_eq!(fs::read(d.join("x (2).txt")).unwrap(), b"first");
    assert_eq!(t.count(), 0);
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn restore_recreates_a_missing_folder_and_a_skip_stays_in_the_trash() {
    let d = temp("trash3");
    let t = Trash { root: d.join("Trash") };
    let g = guard();
    fs::create_dir(d.join("gone")).unwrap();
    fs::write(d.join("gone/f"), "x").unwrap();
    let a = t.put(&g, &d.join("gone/f")).unwrap();
    fs::remove_dir(d.join("gone")).unwrap();
    let spec = JobSpec { kind: JobKind::Restore, sources: vec![t.files().join(&a.id)], dest: None, trash: Some(t.root.clone()) };
    let mut s = Script::default();
    assert!(run_job(&spec, &g, &mut s).clean());
    assert_eq!(fs::read(d.join("gone/f")).unwrap(), b"x");

    let a = t.put(&g, &d.join("gone/f")).unwrap();
    fs::write(d.join("gone/f"), "new one").unwrap();
    let spec = JobSpec { kind: JobKind::Restore, sources: vec![t.files().join(&a.id)], dest: None, trash: Some(t.root.clone()) };
    let mut s = Script { answers: vec![answer(Choice::Skip)], ..Default::default() };
    let r = run_job(&spec, &g, &mut s);
    assert_eq!(r.skipped, 1);
    assert_eq!(t.count(), 1, "still in the Trash");
    assert!(t.read_info(&a.id).is_some());
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn purge_removes_the_item_and_its_info() {
    let d = temp("purge");
    let t = Trash { root: d.join("Trash") };
    fs::write(d.join("f"), "x").unwrap();
    let a = t.put(&guard(), &d.join("f")).unwrap();
    let spec = JobSpec { kind: JobKind::Delete, sources: vec![t.files().join(&a.id)], dest: None, trash: Some(t.root.clone()) };
    let mut s = Script::default();
    assert!(run_job(&spec, &guard(), &mut s).clean());
    assert_eq!(t.count(), 0);
    assert_eq!(names(&t.info()), Vec::<String>::new());
    assert_eq!(t.purge(OsStr::new("../f"), &|| false).unwrap_err(), "That isn't in the Trash.");
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn the_trash_cannot_go_in_itself() {
    let d = temp("trash4");
    let t = Trash { root: d.join("Trash") };
    fs::create_dir_all(t.files()).unwrap();
    fs::write(t.files().join("x"), "").unwrap();
    assert_eq!(t.put(&guard(), &t.files().join("x")).unwrap_err(), "That's already in the Trash.");
    assert_eq!(t.put(&guard(), &t.files()).unwrap_err(), "That's already in the Trash.");
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn encoding_is_reversible_for_any_bytes() {
    let p = PathBuf::from(OsString::from_vec(b"/a b/\xff\xfe%20/\xc3\xa9.txt".to_vec()));
    let e = encode_path(&p);
    assert!(e.is_ascii());
    assert_eq!(decode_path(&e), p);
    assert_eq!(decode_path("/plain/%zz/%4"), PathBuf::from("/plain/%zz/%4"));
}

// ----------------------------------------------------------- weird names ----

#[test]
fn names_that_are_not_utf8_copy_move_trash_and_restore() {
    let d = temp("weird");
    let bad = OsString::from_vec(vec![b'b', b'a', 0xff, 0xfe, b'd', b'.', b't', b'x', b't']);
    let (a, b) = (d.join("a"), d.join("b"));
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    if fs::write(a.join(&bad), "x").is_err() {
        // some filesystems (APFS) refuse invalid UTF-8 outright
        fs::remove_dir_all(d).unwrap();
        return;
    }
    let mut s = Script::default();
    let r = run_job(&job(JobKind::Copy, vec![a.join(&bad)], &b), &guard(), &mut s);
    assert!(r.clean(), "{r:?}");
    assert_eq!(fs::read(b.join(&bad)).unwrap(), b"x");
    let mut s = Script { answers: vec![answer(Choice::KeepBoth)], ..Default::default() };
    let r = run_job(&job(JobKind::Copy, vec![a.join(&bad)], &b), &guard(), &mut s);
    assert!(s.asked[0].name.contains('\u{fffd}'));
    assert!(r.clean());
    assert_eq!(fs::read_dir(&b).unwrap().count(), 2);
    let t = Trash { root: d.join("Trash") };
    let tr = t.put(&guard(), &a.join(&bad)).unwrap();
    let spec = JobSpec { kind: JobKind::Restore, sources: vec![t.files().join(&tr.id)], dest: None, trash: Some(t.root.clone()) };
    let mut s = Script::default();
    assert!(run_job(&spec, &guard(), &mut s).clean());
    assert_eq!(fs::read(a.join(&bad)).unwrap(), b"x");
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn very_long_names_work_all_the_way() {
    let d = temp("long");
    let long = format!("{}.gba", "x".repeat(251));
    assert_eq!(long.len(), 255);
    let (a, b) = (d.join("a"), d.join("b"));
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    fs::write(a.join(&long), "x").unwrap();
    fs::write(b.join(&long), "y").unwrap();
    let mut s = Script { answers: vec![answer(Choice::KeepBoth)], ..Default::default() };
    let r = run_job(&job(JobKind::Copy, vec![a.join(&long)], &b), &guard(), &mut s);
    assert!(r.clean(), "{r:?}");
    let landed = r.landed[0].to_string_lossy().into_owned();
    assert_eq!(landed.len(), 255);
    assert!(landed.ends_with(" (2).gba"));
    let t = Trash { root: d.join("Trash") };
    let tr = t.put(&guard(), &a.join(&long)).unwrap();
    assert!(tr.id.len() < 240, "room for .trashinfo");
    let spec = JobSpec { kind: JobKind::Restore, sources: vec![t.files().join(&tr.id)], dest: None, trash: Some(t.root.clone()) };
    let mut s = Script::default();
    assert!(run_job(&spec, &guard(), &mut s).clean());
    assert_eq!(fs::read(a.join(&long)).unwrap(), b"x", "back under the whole name");
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn names_people_can_give() {
    for bad in ["", ".", "..", "a/b", "a\nb", "a\0b"] {
        assert!(validate_name(bad).is_err(), "{bad:?}");
    }
    assert!(validate_name(&"x".repeat(256)).is_err());
    for ok in ["a", ".hidden", "with space", "ünï", "a.b.c", "..."] {
        assert!(validate_name(ok).is_ok(), "{ok:?}");
    }
}

#[test]
fn rename_and_new_folder_never_replace() {
    let d = temp("rename");
    fs::write(d.join("a"), "a").unwrap();
    fs::write(d.join("b"), "b").unwrap();
    let g = guard();
    assert_eq!(rename(&g, &d.join("a"), "b").unwrap_err(), "Something with that name is already there.");
    assert_eq!(fs::read(d.join("b")).unwrap(), b"b");
    let to = rename(&g, &d.join("a"), "c").unwrap();
    assert_eq!(to, d.join("c"));
    assert_eq!(new_folder(&g, &d, "b").unwrap_err(), "Something with that name is already there.");
    assert!(new_folder(&g, &d, "new").unwrap().is_dir());
    assert_eq!(rename(&g, &d.join("c"), "x/y").unwrap_err(), "A name can't have / in it.");
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn no_replace_rename_is_atomic_about_it() {
    let d = temp("noreplace");
    fs::write(d.join("a"), "a").unwrap();
    fs::write(d.join("b"), "b").unwrap();
    let e = rename_noreplace(&d.join("a"), &d.join("b")).unwrap_err();
    assert_eq!(e.raw_os_error(), Some(libc::EEXIST));
    assert_eq!(fs::read(d.join("b")).unwrap(), b"b");
    rename_noreplace(&d.join("a"), &d.join("c")).unwrap();
    assert_eq!(names(&d), ["b", "c"]);
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn threaded_job_asks_and_can_be_cancelled_mid_copy() {
    let d = temp("thread");
    let (a, b) = (d.join("a"), d.join("b"));
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    fs::write(a.join("f"), "new").unwrap();
    fs::write(b.join("f"), "old").unwrap();
    let job1 = spawn(job(JobKind::Copy, vec![a.join("f")], &b), guard(), crate::bg::no_wake());
    let mut report = None;
    for m in job1.rx.iter() {
        match m {
            JobMsg::Clash(c) => {
                assert_eq!(c.name, "f");
                job1.answers.send(Answer { choice: Some(Choice::Replace), for_rest: false }).unwrap();
            }
            JobMsg::Done(r) => report = Some(r),
            JobMsg::Progress(_) => {}
        }
    }
    assert!(report.unwrap().clean());
    assert_eq!(fs::read(b.join("f")).unwrap(), b"new");

    // dropping the app (the answer channel) is a cancel, not a hang
    fs::write(a.join("f"), "newer").unwrap();
    let job2 = spawn(job(JobKind::Copy, vec![a.join("f")], &b), guard(), crate::bg::no_wake());
    drop(job2.answers);
    let mut last = None;
    for m in job2.rx.iter() {
        if let JobMsg::Done(r) = m {
            last = Some(r);
        }
    }
    assert!(last.unwrap().cancelled);
    assert_eq!(fs::read(b.join("f")).unwrap(), b"new");
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn deleting_stops_at_a_folder_it_may_not_touch() {
    if root() {
        return;
    }
    let d = temp("delfail");
    fs::create_dir_all(d.join("tree/locked")).unwrap();
    fs::write(d.join("tree/locked/f"), "x").unwrap();
    fs::set_permissions(d.join("tree/locked"), fs::Permissions::from_mode(0o555)).unwrap();
    let mut s = Script::default();
    let spec = JobSpec { kind: JobKind::Delete, sources: vec![d.join("tree")], dest: None, trash: None };
    let r = run_job(&spec, &guard(), &mut s);
    assert_eq!(r.failed.len(), 1);
    assert!(r.failed[0].why.contains("permission"), "{:?}", r.failed);
    assert!(d.join("tree/locked/f").exists());
    fs::set_permissions(d.join("tree/locked"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::remove_dir_all(d).unwrap();
}
