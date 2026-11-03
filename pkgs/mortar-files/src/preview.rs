// SPDX-License-Identifier: GPL-2.0-or-later
//! What the panel shows for the selected thing, and what the viewers load.
//! All of this runs on worker threads; the UI only ever receives finished
//! pictures, already scaled to the size they'll be drawn at.

use crate::listing::{Entry, Kind, kind_of};
use resvg::tiny_skia::Pixmap;
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Larger pictures aren't decoded: a 40 MP PNG is 160 MB in a 970 MB handheld.
pub const MAX_PIXELS: u64 = 24_000_000;
/// How much of a text file the viewer holds.
pub const TEXT_LIMIT: u64 = 512 * 1024;
/// A byte of the file per tile.
pub const BYTE_TILES: usize = 121;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FolderSummary {
    pub count: usize,
    pub folders: usize,
    pub files: usize,
    pub hidden: usize,
    /// Children by kind, for the mosaic: most common first.
    pub by_kind: Vec<(Kind, usize)>,
    pub bytes: u64,
    /// Too big to add up in the time allowed: `bytes` is a lower bound.
    pub more: bool,
}

#[derive(Clone)]
pub enum Preview {
    Image(Arc<Pixmap>),
    Text(Arc<Vec<String>>),
    Bytes(Arc<Vec<u8>>),
    Folder(FolderSummary),
    /// Nothing to show, in plain words.
    Note(String),
}

/// Add up a folder within a budget, so a selection never waits on a big tree.
pub fn summarize(dir: &Path, games: &[String], stop: &dyn Fn() -> bool) -> Result<FolderSummary, String> {
    let rd = fs::read_dir(dir).map_err(|e| crate::ops::plain(&e))?;
    let mut s = FolderSummary::default();
    let mut kinds: Vec<(Kind, usize)> = Vec::new();
    let dev = fs::symlink_metadata(dir).map(|m| m.dev()).unwrap_or(0);
    let deadline = Instant::now() + Duration::from_millis(400);
    let mut subdirs = Vec::new();
    for e in rd.flatten() {
        if stop() {
            break;
        }
        let name = e.file_name();
        if name.as_bytes().first() == Some(&b'.') {
            s.hidden += 1;
            continue;
        }
        s.count += 1;
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let kind = if is_dir { Kind::Folder } else { kind_of(&name, games) };
        match kinds.iter_mut().find(|k| k.0 == kind) {
            Some(k) => k.1 += 1,
            None => kinds.push((kind, 1)),
        }
        if is_dir {
            s.folders += 1;
            subdirs.push(e.path());
        } else {
            s.files += 1;
            if Instant::now() < deadline
                && let Ok(m) = e.metadata()
            {
                s.bytes += m.len();
            }
        }
    }
    kinds.sort_by(|a, b| b.1.cmp(&a.1));
    s.by_kind = kinds;
    while let Some(d) = subdirs.pop() {
        if stop() || Instant::now() > deadline {
            s.more = true;
            break;
        }
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.dev() != dev {
                continue;
            }
            if m.is_dir() {
                subdirs.push(e.path());
            } else {
                s.bytes += m.len();
            }
        }
    }
    Ok(s)
}

/// Read the start of a file. Non-blocking open, so a FIFO can't hang a worker.
fn head(p: &Path, n: u64) -> std::io::Result<Vec<u8>> {
    use std::os::unix::fs::OpenOptionsExt;
    let f = fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(p)?;
    let mut b = Vec::new();
    f.take(n).read_to_end(&mut b)?;
    Ok(b)
}

/// Does this look like text a person could read?
pub fn is_text(b: &[u8]) -> bool {
    if b.contains(&0) {
        return false;
    }
    let s = String::from_utf8_lossy(b);
    let bad = s.chars().filter(|c| *c == '\u{fffd}' || (c.is_control() && !matches!(c, '\n' | '\r' | '\t'))).count();
    bad <= 1 || bad * 50 <= s.chars().count()
}

fn clean_line(l: &str) -> String {
    l.replace('\t', "  ").chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// Text split into lines, wrapped at `cols` columns (0: not wrapped).
pub fn text_lines(b: &[u8], cols: usize, max_lines: usize) -> Vec<String> {
    let s = String::from_utf8_lossy(b);
    let mut out = Vec::new();
    for l in s.lines() {
        let l = clean_line(l);
        if cols == 0 || l.chars().count() <= cols {
            out.push(l);
        } else {
            let c: Vec<char> = l.chars().collect();
            for w in c.chunks(cols) {
                out.push(w.iter().collect());
            }
        }
        if out.len() >= max_lines {
            break;
        }
    }
    out
}

fn fit(w: u32, h: u32, mw: u32, mh: u32, upscale: bool) -> (u32, u32) {
    let s = (mw as f32 / w as f32).min(mh as f32 / h as f32);
    let s = if upscale { if s >= 1.0 { s.floor() } else { s } } else { s.min(1.0) };
    (((w as f32 * s).round() as u32).max(1), ((h as f32 * s).round() as u32).max(1))
}

/// Scale premultiplied RGBA: box filter down, whole-number pixels up (which
/// is how a game screenshot wants to be seen).
pub fn scale(src: &Pixmap, w: u32, h: u32) -> Option<Pixmap> {
    let (sw, sh) = (src.width() as usize, src.height() as usize);
    let mut out = Pixmap::new(w, h)?;
    let s = src.data();
    let d = out.data_mut();
    let (w, h) = (w as usize, h as usize);
    for y in 0..h {
        let (y0, y1) = (y * sh / h, ((y + 1) * sh).div_ceil(h).min(sh).max(y * sh / h + 1));
        for x in 0..w {
            let (x0, x1) = (x * sw / w, ((x + 1) * sw).div_ceil(w).min(sw).max(x * sw / w + 1));
            let mut acc = [0u32; 4];
            for yy in y0..y1 {
                for px in s[(yy * sw + x0) * 4..(yy * sw + x1) * 4].chunks_exact(4) {
                    acc[0] += px[0] as u32;
                    acc[1] += px[1] as u32;
                    acc[2] += px[2] as u32;
                    acc[3] += px[3] as u32;
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as u32;
            let o = (y * w + x) * 4;
            for c in 0..4 {
                d[o + c] = (acc[c] / n) as u8;
            }
        }
    }
    Some(out)
}

fn png_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 24 || &b[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    Some((u32::from_be_bytes(b[16..20].try_into().ok()?), u32::from_be_bytes(b[20..24].try_into().ok()?)))
}

/// Decode a PNG or JPEG, scaled to fit (mw, mh).
pub fn load_image(p: &Path, mw: u32, mh: u32) -> Result<Pixmap, String> {
    let sniff = head(p, 32).map_err(|e| crate::ops::plain(&e))?;
    let too_big = || "It's too big to show.".to_string();
    if let Some((w, h)) = png_size(&sniff) {
        if w as u64 * h as u64 > MAX_PIXELS {
            return Err(too_big());
        }
        let data = fs::read(p).map_err(|e| crate::ops::plain(&e))?;
        let pm = Pixmap::decode_png(&data).map_err(|_| "This picture is damaged.".to_string())?;
        let (fw, fh) = fit(pm.width(), pm.height(), mw, mh, true);
        return scale(&pm, fw, fh).ok_or_else(too_big);
    }
    if sniff.starts_with(&[0xff, 0xd8]) {
        let f = File::open(p).map_err(|e| crate::ops::plain(&e))?;
        let mut dec = jpeg_decoder::Decoder::new(BufReader::new(f));
        dec.read_info().map_err(|_| "This picture is damaged.".to_string())?;
        let info = dec.info().ok_or("This picture is damaged.")?;
        if info.width as u64 * info.height as u64 > MAX_PIXELS * 4 {
            return Err(too_big());
        }
        // the decoder can halve (up to 1/8) as it goes: far less work
        let _ = dec.scale(mw.min(65535) as u16, mh.min(65535) as u16);
        let px = dec.decode().map_err(|_| "This picture is damaged.".to_string())?;
        let info = dec.info().ok_or("This picture is damaged.")?;
        let pm = jpeg_pixmap(&px, info)?;
        let (fw, fh) = fit(pm.width(), pm.height(), mw, mh, true);
        return scale(&pm, fw, fh).ok_or_else(too_big);
    }
    Err("Only PNG and JPEG pictures can be shown.".into())
}

fn jpeg_pixmap(px: &[u8], info: jpeg_decoder::ImageInfo) -> Result<Pixmap, String> {
    let mut pm = Pixmap::new(info.width as u32, info.height as u32).ok_or("This picture is damaged.")?;
    let d = pm.data_mut();
    let n = info.width as usize * info.height as usize;
    use jpeg_decoder::PixelFormat as F;
    let step = match info.pixel_format {
        F::L8 => 1,
        F::L16 => 2,
        F::RGB24 => 3,
        F::CMYK32 => 4,
    };
    if px.len() < n * step {
        return Err("This picture is damaged.".into());
    }
    for (o, s) in d.chunks_exact_mut(4).zip(px.chunks_exact(step)) {
        let (r, g, b) = match info.pixel_format {
            F::L8 | F::L16 => (s[0], s[0], s[0]),
            F::RGB24 => (s[0], s[1], s[2]),
            F::CMYK32 => {
                let k = s[3] as u32;
                ((s[0] as u32 * k / 255) as u8, (s[1] as u32 * k / 255) as u8, (s[2] as u32 * k / 255) as u8)
            }
        };
        o.copy_from_slice(&[r, g, b, 255]);
    }
    Ok(pm)
}

/// The panel's preview of one entry.
pub fn panel(dir: &Path, e: &Entry, games: &[String], size: u32, stop: &dyn Fn() -> bool) -> Preview {
    let p = dir.join(&e.name);
    if e.broken {
        return Preview::Note("This link points at nothing.".into());
    }
    if e.special {
        return Preview::Note("Not a regular file.".into());
    }
    if e.is_dir {
        return match summarize(&p, games, stop) {
            Ok(s) => Preview::Folder(s),
            Err(w) => Preview::Note(w),
        };
    }
    if e.kind == Kind::Image {
        return match std::panic::catch_unwind(|| load_image(&p, size, size)) {
            Ok(Ok(pm)) => Preview::Image(Arc::new(pm)),
            Ok(Err(w)) => Preview::Note(w),
            Err(_) => Preview::Note("This picture is damaged.".into()),
        };
    }
    let b = match head(&p, 4096) {
        Ok(b) => b,
        Err(er) => return Preview::Note(crate::ops::plain(&er)),
    };
    if b.is_empty() {
        return Preview::Note("Empty.".into());
    }
    if is_text(&b) && matches!(e.kind, Kind::Text | Kind::Other) {
        return Preview::Text(Arc::new(text_lines(&b, 0, 16)));
    }
    Preview::Bytes(Arc::new(b.into_iter().take(BYTE_TILES).collect()))
}

/// A text file for the viewer: wrapped lines, and whether it was cut short.
pub fn load_text(p: &Path, cols: usize) -> Result<(Vec<String>, bool), String> {
    let m = fs::metadata(p).map_err(|e| crate::ops::plain(&e))?;
    let b = head(p, TEXT_LIMIT).map_err(|e| crate::ops::plain(&e))?;
    Ok((text_lines(&b, cols, 40_000), m.len() > TEXT_LIMIT))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_png(p: &Path, w: u32, h: u32) {
        let mut pm = Pixmap::new(w, h).unwrap();
        pm.fill(resvg::tiny_skia::Color::from_rgba8(200, 30, 90, 255));
        pm.save_png(p).unwrap();
    }

    #[test]
    fn png_preview_is_scaled_and_keeps_its_shape() {
        let d = crate::ops_tests::temp("png");
        let p = d.join("a.png");
        test_png(&p, 1000, 500);
        let pm = load_image(&p, 320, 320).unwrap();
        assert_eq!((pm.width(), pm.height()), (320, 160));
        assert_eq!(pm.pixel(10, 10).unwrap().red(), 200);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn jpeg_preview() {
        let d = crate::ops_tests::temp("jpg");
        let p = d.join("a.jpg");
        fs::write(&p, include_bytes!("../tests/sample.jpg")).unwrap();
        let pm = load_image(&p, 32, 32).unwrap();
        assert_eq!(pm.width().max(pm.height()), 32);
        // the fixture is a mostly red picture
        let px = pm.pixel(pm.width() / 2, pm.height() / 2).unwrap();
        assert!(px.red() > px.blue(), "{px:?}");
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn pixel_art_scales_by_whole_numbers() {
        let d = crate::ops_tests::temp("art");
        let p = d.join("a.png");
        test_png(&p, 160, 144);
        let pm = load_image(&p, 1024, 768).unwrap();
        assert_eq!((pm.width(), pm.height()), (800, 720));
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn broken_pictures_are_messages() {
        let d = crate::ops_tests::temp("broken");
        let a = d.join("a.png");
        fs::write(&a, b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\0\x10\0\0\0\x10garbage").unwrap();
        assert_eq!(load_image(&a, 64, 64).unwrap_err(), "This picture is damaged.");
        let b = d.join("b.jpg");
        fs::write(&b, [0xff, 0xd8, 0xff, 0xe0, 0, 1, 2, 3]).unwrap();
        assert_eq!(load_image(&b, 64, 64).unwrap_err(), "This picture is damaged.");
        let c = d.join("c.png");
        // an IHDR that claims an absurd size is refused before any allocation
        fs::write(&c, b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\xff\xff\xff\xff\xff\xff\xff\xff").unwrap();
        assert_eq!(load_image(&c, 64, 64).unwrap_err(), "It's too big to show.");
        let e = d.join("e.gif");
        fs::write(&e, b"GIF89a").unwrap();
        assert!(load_image(&e, 64, 64).unwrap_err().starts_with("Only PNG"));
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn text_or_not() {
        assert!(is_text(b"hello\nworld\n"));
        assert!(is_text("h\u{e9}llo".as_bytes()));
        assert!(!is_text(b"PK\x03\x04\0\0"));
        assert!(!is_text(&[0xff; 100]));
        assert!(!is_text(b"\x01\x02\x03\x04"));
        // a cut in the middle of a character isn't binary
        assert!(is_text(&"longer text with \u{e9}".as_bytes()[..18]));
        assert_eq!(text_lines(b"abcdef\n\tx", 4, 10), ["abcd", "ef", "  x"]);
        assert_eq!(text_lines(b"a\nb\nc", 0, 2), ["a", "b"]);
    }

    #[test]
    fn folder_summary_counts_and_kinds() {
        let d = crate::ops_tests::temp("summary");
        fs::create_dir(d.join("sub")).unwrap();
        fs::write(d.join("sub/inner.bin"), vec![0u8; 5000]).unwrap();
        fs::write(d.join("a.png"), vec![0u8; 100]).unwrap();
        fs::write(d.join("b.png"), vec![0u8; 100]).unwrap();
        fs::write(d.join(".secret"), "x").unwrap();
        let s = summarize(&d, &[], &|| false).unwrap();
        assert_eq!((s.count, s.folders, s.files, s.hidden), (3, 1, 2, 1));
        assert_eq!(s.by_kind[0], (Kind::Image, 2));
        assert_eq!(s.bytes, 5200);
        assert!(!s.more);
        fs::remove_dir_all(d).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_cannot_hang_the_preview() {
        let d = crate::ops_tests::temp("fifo");
        let p = d.join("pipe");
        let c = std::ffi::CString::new(p.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        // returns at once: empty read, no writer needed
        let b = head(&p, 16).unwrap();
        assert!(b.is_empty());
        fs::remove_dir_all(d).unwrap();
    }
}
