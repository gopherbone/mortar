// SPDX-License-Identifier: GPL-2.0-or-later
//! The socket between the app and the service: one JSON line each way (a stream of
//! them for `watch`), then the connection closes, so an app at rest holds nothing open
//! and the service can exit.
//!
//! A request names a repository and a catalog ID, and that's all. No path, URL or key
//! travels here: the fields are fixed, and anything else is refused. So a misbehaving
//! app could ask for the browser to be installed or removed, which is a nuisance and
//! nothing more. It can't add a source or install anything unsigned.

use mortar_index::Kind;
use serde::{Deserialize, Serialize};

pub const SOCKET: &str = "/run/mortar-packages/socket";
/// Far beyond any real request.
pub const MAX_REQUEST: u64 = 4 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Op {
    /// Repositories, what they offer here, what's installed, what's running.
    Overview,
    /// What installing `repo`/`id` would need: a dry run.
    Details,
    /// Terminal tools in a large index.
    Search,
    Install,
    Update,
    Remove,
    Rollback,
    Cancel,
    /// The job now, once.
    Status,
    /// The job now and each change, until it ends.
    Watch,
    /// The daily check: official packages, if Auto-check is on.
    Auto,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub op: Op,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// Overview: read the repositories' indexes again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub refresh: bool,
}

impl Request {
    pub fn new(op: Op) -> Request {
        Request { op, repo: None, id: None, query: None, refresh: false }
    }
    pub fn package(op: Op, repo: &str, id: &str) -> Request {
        Request { op, repo: Some(repo.into()), id: Some(id.into()), query: None, refresh: false }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RowState {
    #[default]
    Available,
    /// A bundle: listed, but this version can't install one.
    NotYet,
    Installed,
    /// Installed, and the catalog's build is another path.
    Update,
    /// In its profile, and not by this service (`mortar xcom`, say), or from
    /// another repository. Left alone.
    Elsewhere,
}

/// One package, as a screen wants it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub repo: String,
    pub repo_name: String,
    pub official: bool,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub licence: String,
    #[serde(default)]
    pub source: String,
    #[serde(default = "nix_kind")]
    pub kind: Kind,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub program: Option<String>,
    /// The catalog's date.
    #[serde(default)]
    pub published: u64,
    pub state: RowState,
    #[serde(default)]
    pub installed_version: Option<String>,
    /// There's a previous version to go back to.
    #[serde(default)]
    pub can_roll_back: bool,
}

fn nix_kind() -> Kind {
    Kind::Nix
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoView {
    pub id: String,
    pub name: String,
    pub official: bool,
    /// The index's date; 0: nothing read from it yet.
    pub published: u64,
    /// When it was last read.
    pub checked: u64,
    /// Why the newest index wasn't taken: what's shown is the last good one.
    #[serde(default)]
    pub problem: Option<Problem>,
    /// Lines in its large index of terminal tools; 0: it has none.
    #[serde(default)]
    pub tools: u64,
}

/// What went wrong, for a person, with a word tests can match on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    pub code: String,
    pub headline: String,
    #[serde(default)]
    pub detail: String,
}

impl Problem {
    pub fn new(code: &str, headline: impl Into<String>, detail: impl Into<String>) -> Problem {
        Problem { code: code.into(), headline: headline.into(), detail: detail.into() }
    }
}

impl From<mortar_index::Refusal> for Problem {
    fn from(r: mortar_index::Refusal) -> Problem {
        Problem { code: r.code().into(), headline: r.headline(), detail: r.detail() }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Overview {
    pub repos: Vec<RepoView>,
    pub rows: Vec<Row>,
    #[serde(default)]
    pub job: Option<JobView>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Size {
    pub download: u64,
    pub unpacked: u64,
    pub paths: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Details {
    pub repo: String,
    pub id: String,
    /// What this device is missing: a dry run, not the whole package. None: couldn't
    /// ask, with why in `problem`.
    #[serde(default)]
    pub size: Option<Size>,
    pub free: u64,
    /// `size.unpacked` plus the room that has to stay free.
    #[serde(default)]
    pub need: u64,
    #[serde(default)]
    pub battery: Option<u8>,
    #[serde(default)]
    pub charging: bool,
    /// Enough charge to start.
    #[serde(default)]
    pub power_ok: bool,
    #[serde(default)]
    pub problem: Option<Problem>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    #[default]
    Checking,
    Downloading,
    Installing,
    Done,
    Failed,
    Cancelled,
}

impl Phase {
    pub fn ended(self) -> bool {
        matches!(self, Phase::Done | Phase::Failed | Phase::Cancelled)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    #[default]
    Install,
    Update,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
    pub repo: String,
    pub id: String,
    pub name: String,
    pub action: Action,
    pub phase: Phase,
    /// Bytes unpacked so far, of `total`.
    pub done: u64,
    pub total: u64,
    /// Cancel was asked for and it hasn't stopped yet.
    #[serde(default)]
    pub cancelling: bool,
    /// Why it failed.
    #[serde(default)]
    pub problem: Option<Problem>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Found {
    pub rows: Vec<Row>,
    pub total: u64,
    /// How many tools there are to search.
    pub searched: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "kebab-case")]
pub enum Reply {
    Overview(Overview),
    Details(Details),
    Found(Found),
    Job(JobView),
    /// No job.
    NoJob,
    /// Done at once: remove, rollback, cancel.
    Ok { message: String },
    Refused(Problem),
}

/// Ask once, over a new connection, and read one line.
pub fn ask(socket: &std::path::Path, req: &Request) -> Result<Reply, String> {
    let mut conn = Conn::open(socket, req)?;
    conn.next().ok_or_else(|| "the service closed the connection".to_string())
}

/// Replies as they come.
pub struct Conn(std::io::BufReader<std::os::unix::net::UnixStream>);

impl Conn {
    pub fn open(socket: &std::path::Path, req: &Request) -> Result<Conn, String> {
        use std::io::Write;
        let mut s = std::os::unix::net::UnixStream::connect(socket).map_err(|e| format!("{}: {e}", socket.display()))?;
        let mut line = serde_json::to_vec(req).expect("request serializes");
        line.push(b'\n');
        s.write_all(&line).map_err(|e| e.to_string())?;
        Ok(Conn(std::io::BufReader::new(s)))
    }
}

impl Iterator for Conn {
    type Item = Reply;
    fn next(&mut self) -> Option<Reply> {
        use std::io::BufRead;
        let mut line = String::new();
        match self.0.read_line(&mut line) {
            Ok(n) if n > 0 => serde_json::from_str(&line).ok(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_can_carry_a_repository_and_an_id_and_no_more() {
        let good = r#"{"op":"install","repo":"mortar","id":"wpe-browser"}"#;
        let r: Request = serde_json::from_str(good).unwrap();
        assert_eq!(r, Request::package(Op::Install, "mortar", "wpe-browser"));
        for bad in [
            r#"{"op":"install","repo":"mortar","id":"x","path":"/nix/store/evil"}"#,
            r#"{"op":"install","repo":"mortar","id":"x","url":"http://evil"}"#,
            r#"{"op":"install","repo":"mortar","id":"x","publicKey":"evil:AA=="}"#,
            r#"{"op":"add-repository","url":"http://evil"}"#,
            r#"{"id":"x"}"#,
        ] {
            assert!(serde_json::from_str::<Request>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn replies_round_trip() {
        for r in [
            Reply::Overview(Overview::default()),
            Reply::Job(JobView { id: "a".into(), phase: Phase::Downloading, done: 5, total: 9, ..Default::default() }),
            Reply::NoJob,
            Reply::Ok { message: "gone".into() },
            Reply::Refused(Problem::new("older", "No", "because")),
            Reply::Found(Found::default()),
            Reply::Details(Details::default()),
        ] {
            let j = serde_json::to_string(&r).unwrap();
            assert_eq!(serde_json::from_str::<Reply>(&j).unwrap(), r, "{j}");
            assert!(!j.contains('\n'));
        }
    }
}
