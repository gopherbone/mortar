// SPDX-License-Identifier: GPL-2.0-or-later
//! KMS dumb buffers on the display device (sun4i-drm on the Brick), shared
//! with mortar-shell as linear XRGB8888 dma-bufs. They are CMA (physically
//! contiguous), so the shell can put them straight on a display plane, and
//! the display engine's scaler can scale them: no copy and no CPU in the
//! shell. The same allocator mortar-wlegl uses for GPU apps
//! (pkgs/mortar-wlegl), in a few ioctls.
//!
//! The mapping is write-combined: write it sequentially, never read it.

use std::ffi::c_void;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

const fn iowr(nr: u32, size: usize) -> u64 {
    (3 << 30) | ((size as u64) << 16) | (0x64 << 8) | nr as u64
}

#[repr(C)]
struct Version {
    major: i32,
    minor: i32,
    patch: i32,
    name_len: usize,
    name: *mut u8,
    date_len: usize,
    date: *mut u8,
    desc_len: usize,
    desc: *mut u8,
}
#[repr(C)]
struct GetCap {
    capability: u64,
    value: u64,
}
#[repr(C)]
struct CreateDumb {
    height: u32,
    width: u32,
    bpp: u32,
    flags: u32,
    handle: u32,
    pitch: u32,
    size: u64,
}
#[repr(C)]
struct MapDumb {
    handle: u32,
    pad: u32,
    offset: u64,
}
#[repr(C)]
struct Handle {
    handle: u32,
}
#[repr(C)]
struct Prime {
    handle: u32,
    flags: u32,
    fd: i32,
}

const VERSION: u64 = iowr(0x00, size_of::<Version>());
const GET_CAP: u64 = iowr(0x0c, size_of::<GetCap>());
const PRIME_HANDLE_TO_FD: u64 = iowr(0x2d, size_of::<Prime>());
const CREATE_DUMB: u64 = iowr(0xb2, size_of::<CreateDumb>());
const MAP_DUMB: u64 = iowr(0xb3, size_of::<MapDumb>());
const DESTROY_DUMB: u64 = iowr(0xb4, size_of::<Handle>());
const CAP_DUMB_BUFFER: u64 = 1;

fn ioctl<T>(fd: &OwnedFd, req: u64, arg: &mut T) -> std::io::Result<()> {
    loop {
        let r = unsafe { libc::ioctl(fd.as_raw_fd(), req as _, arg as *mut T as *mut c_void) };
        if r == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        if !matches!(e.raw_os_error(), Some(libc::EINTR) | Some(libc::EAGAIN)) {
            return Err(e);
        }
    }
}

pub struct Card {
    fd: OwnedFd,
}

/// One buffer: the dma-buf to hand to the compositor and its mapping.
pub struct DumbBuf {
    pub fd: OwnedFd,
    pub ptr: *mut u32,
    pub len: usize,
    /// Row pitch in bytes.
    pub pitch: u32,
}

impl Drop for DumbBuf {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.ptr as *mut c_void, self.len) };
    }
}

impl Card {
    /// The display device: `MORTAR_PLAY_DRM`, else the first card that is
    /// sun4i-drm (or failing that, any card with dumb buffers).
    pub fn open() -> Option<Card> {
        let open = |p: &str| -> Option<OwnedFd> {
            let c = std::ffi::CString::new(p).ok()?;
            let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
            (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) })
        };
        if let Some(p) = std::env::var_os("MORTAR_PLAY_DRM") {
            return open(&p.to_string_lossy()).map(|fd| Card { fd });
        }
        let mut fallback = None;
        for i in 0..8 {
            let Some(fd) = open(&format!("/dev/dri/card{i}")) else { continue };
            let mut cap = GetCap { capability: CAP_DUMB_BUFFER, value: 0 };
            if ioctl(&fd, GET_CAP, &mut cap).is_err() || cap.value == 0 {
                continue;
            }
            let mut name = [0u8; 64];
            let mut v = Version { major: 0, minor: 0, patch: 0, name_len: name.len(), name: name.as_mut_ptr(), date_len: 0, date: std::ptr::null_mut(), desc_len: 0, desc: std::ptr::null_mut() };
            let sun4i = ioctl(&fd, VERSION, &mut v).is_ok() && name[..v.name_len.min(name.len())] == *b"sun4i-drm";
            if sun4i {
                return Some(Card { fd });
            }
            if fallback.is_none() {
                fallback = Some(Card { fd });
            }
        }
        fallback
    }

    /// A `w` x `h` XRGB8888 buffer, mapped, exported as a dma-buf.
    pub fn alloc(&self, w: u32, h: u32) -> Result<DumbBuf, String> {
        let mut c = CreateDumb { height: h, width: w, bpp: 32, flags: 0, handle: 0, pitch: 0, size: 0 };
        ioctl(&self.fd, CREATE_DUMB, &mut c).map_err(|e| format!("dumb buffer {w}x{h}: {e}"))?;
        let destroy = |h: u32| {
            let mut d = Handle { handle: h };
            let _ = ioctl(&self.fd, DESTROY_DUMB, &mut d);
        };
        let mut m = MapDumb { handle: c.handle, pad: 0, offset: 0 };
        if let Err(e) = ioctl(&self.fd, MAP_DUMB, &mut m) {
            destroy(c.handle);
            return Err(format!("map dumb: {e}"));
        }
        let len = c.size as usize;
        let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, self.fd.as_raw_fd(), m.offset as libc::off_t) };
        if ptr == libc::MAP_FAILED {
            let e = std::io::Error::last_os_error();
            destroy(c.handle);
            return Err(format!("mmap dumb: {e}"));
        }
        let mut p = Prime { handle: c.handle, flags: (libc::O_CLOEXEC | libc::O_RDWR) as u32, fd: -1 };
        let r = ioctl(&self.fd, PRIME_HANDLE_TO_FD, &mut p);
        // the dma-buf and the mapping hold their own references to the object
        destroy(c.handle);
        if let Err(e) = r {
            unsafe { libc::munmap(ptr, len) };
            return Err(format!("PRIME export: {e}"));
        }
        Ok(DumbBuf { fd: unsafe { OwnedFd::from_raw_fd(p.fd) }, ptr: ptr as *mut u32, len, pitch: c.pitch })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ioctl_numbers() {
        // from <drm/drm.h> on 64-bit Linux
        assert_eq!(super::CREATE_DUMB, 0xc02064b2);
        assert_eq!(super::MAP_DUMB, 0xc01064b3);
        assert_eq!(super::DESTROY_DUMB, 0xc00464b4);
        assert_eq!(super::PRIME_HANDLE_TO_FD, 0xc00c642d);
        assert_eq!(super::GET_CAP, 0xc010640c);
        assert_eq!(super::VERSION, 0xc0406400);
    }
}
