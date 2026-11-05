// SPDX-License-Identifier: GPL-2.0-or-later
//! A large index: every prebuilt terminal tool in nixpkgs, about 94,000 lines.
//!
//! Far too much to hold as JSON on 969 MB, and a search has to look at all of it. So
//! the list is a plain, sorted TSV in a store path, and a search streams it through
//! one fixed buffer, keeping only the best few hits. It's never in memory as a whole,
//! and costs nothing until someone searches.
//!
//! ```text
//! #mortar-tools<TAB>1
//! ripgrep<TAB>ripgrep-14.1.1<TAB>0c7f8ms0x6r4n3pq7x2a9w1kz5lbv8yd<TAB>rg<TAB>Fast line-oriented search
//! ```
//!
//! Columns: attribute (the id), store path name, store path hash, main program,
//! description. No tabs or newlines inside a column (the generator's job). The
//! catalog names the store path ([`crate::catalog::ToolsIndex`]), and `nix-store
//! --realise` checks it against the repository's key like any package: compressed by
//! the binary cache at about 3.5 MiB, so no second download path or checksum of its
//! own, and the Worker needs no new kind of object.

use std::io::{self, BufRead, Read};

/// The first line.
pub const HEADER: &str = "#mortar-tools\t1";
pub const FILE: &str = "tools.tsv";
/// A line longer than this isn't a line of an index.
const MAX_LINE: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tool {
    pub attr: String,
    pub name: String,
    pub hash: String,
    pub program: String,
    pub description: String,
}

impl Tool {
    fn from_fields(f: [&[u8]; 5]) -> Option<Tool> {
        let s = |b: &[u8]| std::str::from_utf8(b).ok().map(str::to_string);
        let t = Tool { attr: s(f[0])?, name: s(f[1])?, hash: s(f[2])?, program: s(f[3])?, description: s(f[4])? };
        (crate::catalog::valid_tool_id(&t.attr) && crate::manifest::valid_store_path(&t.path())).then_some(t)
    }

    pub fn path(&self) -> String {
        format!("/nix/store/{}-{}", self.hash, self.name)
    }

    /// `ripgrep-14.1.1` -> `14.1.1`: what follows the last `-<digit>`. Empty when a
    /// name has no version.
    pub fn version(&self) -> &str {
        let n = &self.name;
        let mut i = n.len();
        while let Some(d) = n[..i].rfind('-') {
            if n[d + 1..].starts_with(|c: char| c.is_ascii_digit()) {
                return &n[d + 1..];
            }
            i = d;
        }
        ""
    }

    /// The word a person would type.
    pub fn title(&self) -> &str {
        if self.program.is_empty() { &self.attr } else { &self.program }
    }
}

/// Is it a file this code can read? `Err(n)`: a newer format, number `n`.
pub fn check_header(first_line: &str) -> Result<(), u32> {
    let mut f = first_line.trim_end().split('\t');
    match (f.next(), f.next().and_then(|n| n.parse::<u32>().ok())) {
        (Some("#mortar-tools"), Some(1)) => Ok(()),
        (Some("#mortar-tools"), Some(n)) => Err(n),
        _ => Err(0),
    }
}

pub struct Found {
    /// The best first, up to the limit.
    pub hits: Vec<Tool>,
    /// How many lines matched in all.
    pub total: u64,
}

/// How well a line matches, or not at all. Lower is better.
fn rank(fields: &[&[u8]; 5], words: &[Vec<u8>]) -> Option<u8> {
    let [attr, name, _, program, desc] = *fields;
    let mut worst = 0u8;
    for w in words {
        let r = if attr.eq_ignore_ascii_case(w) || program.eq_ignore_ascii_case(w) {
            0
        } else if starts_with_ci(attr, w) || starts_with_ci(program, w) {
            1
        } else if contains_ci(attr, w) || contains_ci(name, w) || contains_ci(program, w) {
            2
        } else if contains_ci(desc, w) {
            3
        } else {
            return None;
        };
        worst = worst.max(r);
    }
    Some(worst)
}

fn starts_with_ci(hay: &[u8], lower: &[u8]) -> bool {
    hay.len() >= lower.len() && hay[..lower.len()].eq_ignore_ascii_case(lower)
}

fn contains_ci(hay: &[u8], lower: &[u8]) -> bool {
    let Some((&first, rest)) = lower.split_first() else { return true };
    if hay.len() < lower.len() {
        return false;
    }
    let up = first.to_ascii_uppercase();
    let last = hay.len() - lower.len();
    let mut i = 0;
    while i <= last {
        let c = hay[i];
        if (c == first || c == up) && hay[i + 1..i + lower.len()].eq_ignore_ascii_case(rest) {
            return true;
        }
        i += 1;
    }
    false
}

/// Call `f` with each well-formed line's columns, through one buffer. Ends early when
/// `f` says so.
fn each_line<R: BufRead>(mut r: R, mut f: impl FnMut([&[u8]; 5]) -> bool) -> io::Result<()> {
    let mut line: Vec<u8> = Vec::with_capacity(1024);
    loop {
        line.clear();
        // bounded: a damaged file can't grow this without limit
        let n = r.by_ref().take(MAX_LINE as u64 + 1).read_until(b'\n', &mut line)?;
        if n == 0 {
            return Ok(());
        }
        if line.last() != Some(&b'\n') && n > MAX_LINE {
            // skip the rest of it
            loop {
                line.clear();
                if r.by_ref().take(MAX_LINE as u64).read_until(b'\n', &mut line)? == 0 || line.last() == Some(&b'\n') {
                    break;
                }
            }
            continue;
        }
        let l = line.strip_suffix(b"\n").unwrap_or(&line);
        if l.first() == Some(&b'#') {
            continue;
        }
        let mut it = l.split(|b| *b == b'\t');
        if let (Some(a), Some(b), Some(c), Some(d), Some(e), None) = (it.next(), it.next(), it.next(), it.next(), it.next(), it.next())
            && !f([a, b, c, d, e])
        {
            return Ok(());
        }
    }
}

/// Every word of `query` has to match; attribute and program names beat descriptions.
/// Memory: the reader's buffer, one line, and `limit` tools.
pub fn search<R: BufRead>(r: R, query: &str, limit: usize) -> io::Result<Found> {
    let words: Vec<Vec<u8>> = query.split_whitespace().map(|w| w.to_ascii_lowercase().into_bytes()).collect();
    let mut hits: Vec<(u8, Tool)> = Vec::with_capacity(limit + 1);
    let mut total = 0;
    each_line(r, |fields| {
        let Some(score) = rank(&fields, &words) else { return true };
        total += 1;
        // the file is sorted, so an equal score keeps the earlier line
        if (hits.len() < limit || hits.last().is_some_and(|(s, _)| score < *s))
            && let Some(tool) = Tool::from_fields(fields)
        {
            let at = hits.partition_point(|(s, _)| *s <= score);
            hits.insert(at, (score, tool));
            hits.truncate(limit);
        }
        true
    })?;
    Ok(Found { hits: hits.into_iter().map(|(_, t)| t).collect(), total })
}

/// One tool by its attribute.
pub fn find<R: BufRead>(r: R, attr: &str) -> io::Result<Option<Tool>> {
    let mut out = None;
    each_line(r, |fields| {
        if fields[0] == attr.as_bytes() {
            out = Tool::from_fields(fields);
            return false;
        }
        true
    })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const H: &str = "0c7f8ms0x6r4n3pq7x2a9w1kz5lbv8yd";

    fn sample() -> String {
        let mut s = format!("{HEADER}\n");
        for (a, n, p, d) in [
            ("bat", "bat-0.25.0", "bat", "Cat clone with wings"),
            ("fd", "fd-10.2.0", "fd", "A simple alternative to find"),
            ("findutils", "findutils-4.10.0", "find", "GNU find, xargs and locate"),
            ("htop", "htop-3.4.1", "htop", "Interactive process viewer"),
            ("python3Packages.yt-dlp", "python3.13-yt-dlp-2025.1.1", "yt-dlp", "Download videos"),
            ("ripgrep", "ripgrep-14.1.1", "rg", "Fast search; better than grep"),
            ("yt-dlp", "yt-dlp-2025.1.1", "yt-dlp", "Download videos"),
        ] {
            s.push_str(&format!("{a}\t{n}\t{H}\t{p}\t{d}\n"));
        }
        s
    }

    fn q(s: &str) -> Vec<String> {
        search(Cursor::new(sample()), s, 50).unwrap().hits.into_iter().map(|t| t.attr).collect()
    }

    #[test]
    fn the_likely_one_comes_first() {
        // a program's own name beats "xargs" in a description
        assert_eq!(q("rg"), ["ripgrep", "findutils"]);
        assert_eq!(q("RIPGREP"), ["ripgrep"]);
        assert_eq!(q("find")[0], "findutils");
        // the rest by how close, the description last
        assert_eq!(q("find"), ["findutils", "fd"]);
        assert_eq!(q("grep"), ["ripgrep"]);
        assert_eq!(q("yt-dlp"), ["python3Packages.yt-dlp", "yt-dlp"]);
        assert_eq!(q("video download"), ["python3Packages.yt-dlp", "yt-dlp"]);
        assert!(q("nothing-called-this").is_empty());
        // no words: everything, in order
        assert_eq!(q("").len(), 7);
    }

    #[test]
    fn stops_at_the_limit_and_counts_the_rest() {
        let f = search(Cursor::new(sample()), "", 2).unwrap();
        assert_eq!((f.hits.len(), f.total), (2, 7));
    }

    #[test]
    fn a_better_match_replaces_a_worse_one_past_the_limit() {
        let f = search(Cursor::new(sample()), "rg", 1).unwrap();
        assert_eq!(f.hits[0].attr, "ripgrep");
        let f = search(Cursor::new(sample()), "find", 1).unwrap();
        assert_eq!(f.hits[0].attr, "findutils");
    }

    #[test]
    fn tool_facts() {
        let t = find(Cursor::new(sample()), "ripgrep").unwrap().unwrap();
        assert_eq!(t.path(), format!("/nix/store/{H}-ripgrep-14.1.1"));
        assert_eq!(t.version(), "14.1.1");
        assert_eq!(t.title(), "rg");
        assert_eq!(find(Cursor::new(sample()), "python3Packages.yt-dlp").unwrap().unwrap().version(), "2025.1.1");
        assert!(find(Cursor::new(sample()), "absent").unwrap().is_none());
        // a prefix of an attribute is not that attribute
        assert!(find(Cursor::new(sample()), "rip").unwrap().is_none());
        let bare = Tool { attr: "x".into(), name: "x".into(), hash: H.into(), program: String::new(), description: String::new() };
        assert_eq!((bare.version(), bare.title()), ("", "x"));
    }

    #[test]
    fn damage_is_skipped_not_believed() {
        let long = "x".repeat(MAX_LINE * 3);
        let s = format!(
            "{HEADER}\nshort\tline\n{long}\tb\tc\td\te\n../evil\tn-1\t{H}\tp\td\nbad\tn-1\t/etc/passwd\tp\td\nnot-utf8\tn-1\t{H}\tp\t\u{FFFD}\ngood\tgood-1\t{H}\tgood\tfine\n"
        );
        let f = search(Cursor::new(s.clone()), "good", 50).unwrap();
        assert_eq!(f.hits.len(), 1);
        assert_eq!(find(Cursor::new(s.clone()), "../evil").unwrap(), None);
        assert_eq!(find(Cursor::new(s), "bad").unwrap(), None);
    }

    #[test]
    fn headers() {
        assert_eq!(check_header(HEADER), Ok(()));
        assert_eq!(check_header("#mortar-tools\t2\n"), Err(2));
        assert_eq!(check_header("ripgrep\tripgrep-1"), Err(0));
    }
}
