// SPDX-License-Identifier: GPL-2.0-or-later
//! Who the app asks. All it can do to the system is send these requests: on a device
//! over the service's socket, in the preview and tests to a made-up service.

use crate::protocol::{Conn, JobView, Reply, Request, ask};
use std::path::PathBuf;

pub trait Backend: Send + Sync {
    fn ask(&self, req: &Request) -> Result<Reply, String>;
    /// The job as it changes, until it ends or the connection goes.
    fn watch(&self, on: &mut dyn FnMut(JobView)) -> Result<(), String>;
}

/// The service, over its socket. A connection per request: none stays open at rest, so
/// the service can exit.
pub struct Socket(pub PathBuf);

impl Socket {
    pub fn detect() -> Socket {
        Socket(std::env::var_os("MORTAR_PACKAGES_SOCKET").map(PathBuf::from).unwrap_or_else(|| crate::protocol::SOCKET.into()))
    }
}

impl Backend for Socket {
    fn ask(&self, req: &Request) -> Result<Reply, String> {
        ask(&self.0, req)
    }

    fn watch(&self, on: &mut dyn FnMut(JobView)) -> Result<(), String> {
        let req = Request::new(crate::protocol::Op::Watch);
        for r in Conn::open(&self.0, &req)? {
            if let Reply::Job(j) = r {
                on(j);
            }
        }
        Ok(())
    }
}
