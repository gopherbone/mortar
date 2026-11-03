// SPDX-License-Identifier: GPL-2.0-or-later
//! Background work and how it reaches the app: threads send `Bg` messages on
//! one channel, then wake the window loop through a pipe (`wake_pipe`). The
//! loop sleeps in poll() until then, so nothing here costs a timer.

use crate::listing::Listing;
use crate::preview::Preview;
use crate::storage::ScanUpdate;
use std::path::PathBuf;
use std::sync::Arc;

pub type Wake = Arc<dyn Fn() + Send + Sync>;

pub fn no_wake() -> Wake {
    Arc::new(|| {})
}

/// What a worker sends.
pub enum Bg {
    Listing { generation: u64, dir: PathBuf, result: Result<Listing, String> },
    Preview { key: PathBuf, generation: u64, preview: Preview },
    Scan { generation: u64, update: ScanUpdate },
    PlaceFacts(Vec<(String, String)>),
    /// A toast from a thread.
    Note(String),
    Image { generation: u64, result: Result<Arc<resvg::tiny_skia::Pixmap>, String> },
    Text { generation: u64, result: Result<(Vec<String>, bool), String> },
}

/// A pipe for waking poll(): the read end for the loop, a Wake for threads.
pub fn wake_pipe() -> Result<(i32, Wake), String> {
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err("pipe failed".into());
    }
    for fd in fds {
        unsafe {
            libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK);
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
    let w = fds[1];
    Ok((fds[0], Arc::new(move || {
        // a full pipe is awake enough
        let _ = unsafe { libc::write(w, b"x".as_ptr() as *const libc::c_void, 1) };
    })))
}

/// Empty the pipe after waking.
pub fn drain(fd: i32) {
    let mut b = [0u8; 64];
    while unsafe { libc::read(fd, b.as_mut_ptr() as *mut libc::c_void, b.len()) } > 0 {}
}
