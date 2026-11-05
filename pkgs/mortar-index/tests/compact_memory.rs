// SPDX-License-Identifier: GPL-2.0-or-later
//! A search of the whole index, the size the prototype measured, counting every
//! allocation: what 969 MB has to spare for it.

use mortar_index::compact::{HEADER, find, search};
use std::alloc::{GlobalAlloc, Layout, System};
use std::fs::File;
use std::io::{BufReader, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;
static NOW: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let n = NOW.fetch_add(l.size(), Ordering::Relaxed) + l.size();
        PEAK.fetch_max(n, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        NOW.fetch_sub(l.size(), Ordering::Relaxed);
        unsafe { System.dealloc(p, l) }
    }
}

#[global_allocator]
static A: Counting = Counting;

#[test]
fn searching_the_whole_index_takes_a_fixed_buffer() {
    let path = std::env::temp_dir().join(format!("mortar-compact-{}.tsv", std::process::id()));
    {
        let mut f = std::io::BufWriter::new(File::create(&path).unwrap());
        writeln!(f, "{HEADER}").unwrap();
        for i in 0..93_663u32 {
            writeln!(
                f,
                "package{i:05}\tpackage{i:05}-1.{}.0\t0c7f8ms0x6r4n3pq7x2a9w1kz5lbv8yd\tprog{i}\tSomething useful, described in about as many words as nixpkgs tends to use",
                i % 40
            )
            .unwrap();
        }
    }
    let size = std::fs::metadata(&path).unwrap().len();
    assert!(size > 12_000_000, "the prototype's index unpacks to 12.1 MiB; this is {size}");

    let start = NOW.load(Ordering::Relaxed);
    PEAK.store(start, Ordering::Relaxed);
    let t = std::time::Instant::now();
    let found = search(BufReader::with_capacity(64 * 1024, File::open(&path).unwrap()), "package9 useful", 50).unwrap();
    let took = t.elapsed();
    let peak = PEAK.load(Ordering::Relaxed) - start;
    eprintln!("{size} bytes, {} matches, {took:?}, peak {peak} bytes allocated", found.total);
    assert_eq!(found.hits.len(), 50);
    assert_eq!(found.total, 3_663);
    assert!(peak < 256 * 1024, "peak {peak}");

    PEAK.store(NOW.load(Ordering::Relaxed), Ordering::Relaxed);
    let start = NOW.load(Ordering::Relaxed);
    let t = find(BufReader::with_capacity(64 * 1024, File::open(&path).unwrap()), "package93662").unwrap().unwrap();
    assert_eq!(t.program, "prog93662");
    assert!(PEAK.load(Ordering::Relaxed) - start < 128 * 1024);
    std::fs::remove_file(&path).unwrap();
}
