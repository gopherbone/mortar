// SPDX-License-Identifier: GPL-2.0-or-later
//! mortar-packagesd: the root service behind the Packages app.
//!
//!   mortar-packagesd                 serve the socket until idle (systemd starts it)
//!   mortar-packagesd ask JSON        send one request, print what comes back
//!   mortar-packagesd auto            the daily check, as mortar-packages-auto runs it
//!
//! Config: /etc/mortar-packages/config.json, or $MORTAR_PACKAGES_CONFIG. Requests:
//! src/protocol.rs. Design: ../../DESIGN.md.

use mortar_packages::protocol::{self, Conn, Op, Reply, Request};
use mortar_packages::service::{config, daemon::Daemon, server};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn socket() -> PathBuf {
    std::env::var_os("MORTAR_PACKAGES_SOCKET").map(PathBuf::from).unwrap_or_else(|| protocol::SOCKET.into())
}

fn ask(line: &str) -> Result<(), String> {
    let req: Request = serde_json::from_str(line).map_err(|e| format!("request: {e}"))?;
    let s = socket();
    let mut any = false;
    // as they come: a watch goes on for as long as the job does
    for r in Conn::open(Path::new(&s), &req)? {
        any = true;
        println!("{}", serde_json::to_string(&r).expect("reply serializes"));
    }
    if any { Ok(()) } else { Err("no reply".into()) }
}

fn auto() -> Result<(), String> {
    match protocol::ask(Path::new(&socket()), &Request::new(Op::Auto))? {
        Reply::Ok { message } => {
            println!("{message}");
            Ok(())
        }
        Reply::Refused(p) => Err(format!("{}: {}", p.headline, p.detail)),
        other => Err(format!("{other:?}")),
    }
}

fn serve() -> Result<(), String> {
    let path = std::env::var_os("MORTAR_PACKAGES_CONFIG").map(PathBuf::from).unwrap_or_else(|| config::DEFAULT_PATH.into());
    let cfg = config::Config::load(&path)?;
    server::run(Daemon::new(cfg), std::env::var_os("MORTAR_PACKAGES_SOCKET").as_deref().map(Path::new))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => serve(),
        ["ask", line] => ask(line),
        ["auto"] => auto(),
        _ => Err("usage: mortar-packagesd [ask JSON | auto]".into()),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mortar-packagesd: {e}");
            ExitCode::FAILURE
        }
    }
}
