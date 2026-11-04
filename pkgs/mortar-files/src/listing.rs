// SPDX-License-Identifier: GPL-2.0-or-later
//! Reading a folder, sorting it, and saying sizes and dates in few words.
//! Entries keep the name as the OS gives it (it may not be UTF-8); `display`
//! is only what's shown.

use std::cmp::Ordering;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// A folder with more than this shows the first ones and says so.
pub const MAX_ENTRIES: usize = 200_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Folder,
    Image,
    Video,
    Audio,
    Game,
    Save,
    Archive,
    Text,
    Other,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Folder => "Folder",
            Kind::Image => "Picture",
            Kind::Video => "Video",
            Kind::Audio => "Sound",
            Kind::Game => "Game",
            Kind::Save => "Save",
            Kind::Archive => "Archive",
            Kind::Text => "Text",
            Kind::Other => "File",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: OsString,
    /// Safe to draw: no control characters, invalid bytes as U+FFFD.
    pub display: String,
    pub kind: Kind,
    pub size: u64,
    pub mtime: i64,
    /// It opens as a folder (a real one, or a link to one).
    pub is_dir: bool,
    pub is_link: bool,
    /// A link whose target is missing.
    pub broken: bool,
    pub hidden: bool,
    /// A FIFO, socket or device: leave it alone.
    pub special: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    Name,
    Size,
    Date,
}

impl Sort {
    pub fn word(self) -> &'static str {
        match self {
            Sort::Name => "Name",
            Sort::Size => "Size",
            Sort::Date => "Date",
        }
    }
    pub fn next(self) -> Sort {
        match self {
            Sort::Name => Sort::Size,
            Sort::Size => Sort::Date,
            Sort::Date => Sort::Name,
        }
    }
    pub fn prev(self) -> Sort {
        self.next().next()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Listing {
    pub entries: Vec<Entry>,
    /// Names beginning with a dot that aren't shown.
    pub hidden_count: usize,
    pub truncated: bool,
}

/// Make a name safe to draw.
pub fn display(n: &OsStr) -> String {
    n.to_string_lossy().chars().map(|c| if c.is_control() { '\u{fffd}' } else { c }).collect()
}

fn ext_of(n: &OsStr) -> String {
    let b = n.as_bytes();
    match b.iter().rposition(|c| *c == b'.') {
        Some(i) if i > 0 && b.len() - i <= 12 => String::from_utf8_lossy(&b[i + 1..]).to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// What a name says it is. `games` is what the emulator config claims.
pub fn kind_of(n: &OsStr, games: &[String]) -> Kind {
    let e = ext_of(n);
    let e = e.as_str();
    if !e.is_empty() && games.iter().any(|g| g == e) {
        return Kind::Game;
    }
    match e {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "tga" | "tiff" => Kind::Image,
        "mp4" | "mkv" | "webm" | "avi" | "mov" | "m4v" => Kind::Video,
        "mp3" | "ogg" | "flac" | "wav" | "m4a" | "opus" | "aac" | "it" | "xm" | "mod" | "s3m" | "sid" | "m8s" | "ctm" => Kind::Audio,
        "gb" | "gbc" | "gba" | "nes" | "fds" | "sfc" | "smc" | "gen" | "smd" | "sms" | "gg" | "pce" | "n64" | "z64" | "nds" | "cue" | "chd" | "iso" | "p8" | "ngp" | "ngc" | "ws" | "wsc" | "a26" | "a78" | "lnx" | "m3u" => Kind::Game,
        "sav" | "srm" | "state" | "auto" | "st0" | "st1" | "st2" | "st3" | "mcr" | "eep" | "fla" => Kind::Save,
        "zip" | "7z" | "tar" | "gz" | "tgz" | "xz" | "zst" | "bz2" | "rar" | "lz4" => Kind::Archive,
        "txt" | "md" | "log" | "json" | "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "nix" | "rs" | "py" | "sh" | "zsh" | "bash" | "c" | "h" | "cpp" | "hpp" | "js" | "ts" | "html" | "css" | "xml" | "csv" | "lua" | "desktop" | "srt" | "nfo" | "diz" | "pub" | "trashinfo" => Kind::Text,
        _ => Kind::Other,
    }
}

fn entry(dir: &Path, name: OsString, games: &[String]) -> Entry {
    let p = dir.join(&name);
    let hidden = name.as_bytes().first() == Some(&b'.');
    let shown = display(&name);
    let lm = fs::symlink_metadata(&p);
    let (is_link, mut is_dir, mut size, mut mtime, mut special) = match &lm {
        Ok(m) => {
            let t = m.file_type();
            (t.is_symlink(), t.is_dir(), m.len(), m.mtime(), !(t.is_symlink() || t.is_dir() || t.is_file()))
        }
        Err(_) => (false, false, 0, 0, false),
    };
    let mut broken = false;
    if is_link {
        match fs::metadata(&p) {
            Ok(m) => {
                is_dir = m.is_dir();
                size = m.len();
                mtime = m.mtime();
                special = !(m.is_dir() || m.is_file());
            }
            Err(_) => broken = true,
        }
    }
    let kind = if is_dir { Kind::Folder } else { kind_of(&name, games) };
    Entry { name, display: shown, kind, size, mtime, is_dir, is_link, broken, hidden, special }
}

/// Read a folder. `stop` is polled, so a stale read in a huge folder ends
/// early (and the result is thrown away).
pub fn read(dir: &Path, hidden: bool, sort: Sort, games: &[String], stop: &dyn Fn() -> bool) -> Result<Listing, String> {
    let rd = fs::read_dir(dir).map_err(|e| crate::ops::plain(&e))?;
    let mut l = Listing::default();
    for (i, e) in rd.enumerate() {
        if i % 64 == 0 && stop() {
            break;
        }
        let Ok(e) = e else { continue };
        let name = e.file_name();
        if !hidden && name.as_bytes().first() == Some(&b'.') {
            l.hidden_count += 1;
            continue;
        }
        if l.entries.len() >= MAX_ENTRIES {
            l.truncated = true;
            break;
        }
        l.entries.push(entry(dir, name, games));
    }
    sort_entries(&mut l.entries, sort);
    Ok(l)
}

/// Compare the way people read: case folded, "track 9" before "track 10".
pub fn natural_cmp(a: &[u8], b: &[u8]) -> Ordering {
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (si, sj) = (i, j);
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let strip = |s: &'_ [u8]| -> usize { s.iter().take_while(|c| **c == b'0').count() };
            let (x, y) = (&a[si..i], &b[sj..j]);
            let (x, y) = (&x[strip(x)..], &y[strip(y)..]);
            let c = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
            if c != Ordering::Equal {
                return c;
            }
            continue;
        }
        let c = a[i].to_ascii_lowercase().cmp(&b[j].to_ascii_lowercase());
        if c != Ordering::Equal {
            return c;
        }
        i += 1;
        j += 1;
    }
    (a.len() - i).cmp(&(b.len() - j)).then_with(|| a.cmp(b))
}

/// Folders first, always; then by the chosen key.
pub fn sort_entries(v: &mut [Entry], sort: Sort) {
    v.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then_with(|| {
            let by_name = || natural_cmp(a.name.as_bytes(), b.name.as_bytes());
            match sort {
                Sort::Name => by_name(),
                Sort::Size => b.size.cmp(&a.size).then_with(by_name),
                Sort::Date => b.mtime.cmp(&a.mtime).then_with(by_name),
            }
        })
    });
}

// --------------------------------------------------------------- saying ----

pub fn human_size(n: u64) -> String {
    const U: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64 / 1024.0;
    let mut i = 0;
    while v >= 1024.0 && i + 1 < U.len() {
        v /= 1024.0;
        i += 1;
    }
    if v < 10.0 { format!("{v:.1} {}", U[i]) } else { format!("{v:.0} {}", U[i]) }
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// How the app reads the time: the system's zone, or a fixed one for
/// previews and tests.
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    pub fixed_now: Option<i64>,
}

fn broken_down(t: i64, utc: bool) -> libc::tm {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let t = t as libc::time_t;
    unsafe {
        if utc {
            libc::gmtime_r(&t, &mut tm);
        } else {
            libc::localtime_r(&t, &mut tm);
        }
    }
    tm
}

impl Clock {
    pub fn system() -> Clock {
        Clock { fixed_now: None }
    }
    fn now(&self) -> i64 {
        self.fixed_now.unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0))
    }
    fn tm(&self, t: i64) -> libc::tm {
        broken_down(t, self.fixed_now.is_some())
    }
    /// Short, for a row: "14:02", "12 Mar", "12 Mar 2023".
    pub fn short(&self, t: i64) -> String {
        if t <= 0 {
            return "–".into();
        }
        let (a, n) = (self.tm(t), self.tm(self.now()));
        if a.tm_year == n.tm_year && a.tm_yday == n.tm_yday {
            return format!("{:02}:{:02}", a.tm_hour, a.tm_min);
        }
        let m = MONTHS[a.tm_mon.clamp(0, 11) as usize];
        if a.tm_year == n.tm_year { format!("{} {m}", a.tm_mday) } else { format!("{} {m} {}", a.tm_mday, a.tm_year + 1900) }
    }
    /// With the time: "12 Mar 2026, 14:02".
    pub fn long(&self, t: i64) -> String {
        if t <= 0 {
            return "–".into();
        }
        let a = self.tm(t);
        format!("{} {} {}, {:02}:{:02}", a.tm_mday, MONTHS[a.tm_mon.clamp(0, 11) as usize], a.tm_year + 1900, a.tm_hour, a.tm_min)
    }
}

pub fn plural(n: usize, one: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {one}s") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn natural_order() {
        let mut v = vec!["track 10", "Track 9", "track 1", "a", "B", "007", "7"];
        v.sort_by(|a, b| natural_cmp(a.as_bytes(), b.as_bytes()));
        assert_eq!(v, ["007", "7", "a", "B", "track 1", "Track 9", "track 10"]);
    }

    #[test]
    fn sizes() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(12 * 1024 * 1024), "12 MB");
        assert_eq!(human_size(2_500_000_000), "2.3 GB");
    }

    #[test]
    fn dates() {
        // 2026-11-02 12:00 UTC
        let c = Clock { fixed_now: Some(1_793_620_800) };
        assert_eq!(c.short(1_793_620_800 - 3600), "11:00");
        assert_eq!(c.short(1_793_620_800 - 86400 * 2), "31 Oct");
        assert_eq!(c.short(1_793_620_800 - 86400 * 400), "28 Sep 2025");
        assert_eq!(c.long(1_793_620_800), "2 Nov 2026, 12:00");
        assert_eq!(c.short(0), "–");
    }

    #[test]
    fn weird_names_display_safely() {
        let bad = OsString::from_vec(vec![b'a', 0xff, b'\n', b'b']);
        assert_eq!(display(&bad), "a\u{fffd}\u{fffd}b");
        assert_eq!(kind_of(OsStr::new(".bashrc"), &[]), Kind::Other);
        assert_eq!(kind_of(OsStr::new("Pic.JPG"), &[]), Kind::Image);
        assert_eq!(kind_of(OsStr::new("x.rom"), &["rom".to_string()]), Kind::Game);
        assert_eq!(kind_of(&bad, &[]), Kind::Other);
    }

    #[test]
    fn folders_first_and_big_folders_are_fine() {
        let d = crate::ops_tests::temp("listing");
        fs::create_dir(d.join("z folder")).unwrap();
        for i in 0..5000 {
            fs::write(d.join(format!("file {i}.txt")), "x").unwrap();
        }
        fs::write(d.join(".hidden"), "").unwrap();
        let t = std::time::Instant::now();
        let l = read(&d, false, Sort::Name, &[], &|| false).unwrap();
        assert_eq!(l.entries.len(), 5001);
        assert_eq!(l.hidden_count, 1);
        assert_eq!(l.entries[0].display, "z folder");
        assert_eq!(l.entries[1].display, "file 0.txt");
        assert_eq!(l.entries[2].display, "file 1.txt");
        assert_eq!(l.entries[5000].display, "file 4999.txt");
        assert!(t.elapsed().as_secs() < 5);
        let l2 = read(&d, true, Sort::Name, &[], &|| false).unwrap();
        assert_eq!(l2.entries.len(), 5002);
        assert_eq!(l2.hidden_count, 0);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn unreadable_folder_is_a_plain_message() {
        let e = read(Path::new("/definitely/not/here"), false, Sort::Name, &[], &|| false).unwrap_err();
        assert_eq!(e, "It isn't there any more.");
    }

    #[test]
    fn links_are_marked() {
        let d = crate::ops_tests::temp("links");
        fs::create_dir(d.join("real")).unwrap();
        std::os::unix::fs::symlink(d.join("real"), d.join("to-dir")).unwrap();
        std::os::unix::fs::symlink(d.join("nope"), d.join("broken")).unwrap();
        let l = read(&d, false, Sort::Name, &[], &|| false).unwrap();
        let find = |n: &str| l.entries.iter().find(|e| e.display == n).unwrap();
        assert!(find("to-dir").is_dir && find("to-dir").is_link);
        assert!(find("broken").broken && !find("broken").is_dir);
        fs::remove_dir_all(d).unwrap();
    }
}
