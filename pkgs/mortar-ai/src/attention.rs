// SPDX-License-Identifier: GPL-2.0-or-later
//! Tell the Mortar shell when Claude waits for an approval ("Needs you":
//! a badge on this app's window in Running, the Home widget, one toast),
//! and when it no longer does. Over the shell's control socket
//! ($XDG_RUNTIME_DIR/mortar-shell.sock, the same as `mortar-shell
//! attention set|clear`), from a short-lived thread so the window never
//! waits on it. Outside the shell (no $MORTAR_APP) nothing is sent.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::Duration;

/// What the shell was last told (the text, or None: cleared).
#[derive(Default)]
pub struct Notifier {
    sent: Option<Option<String>>,
}

impl Notifier {
    /// The newest state: Some(text) while an approval waits. Sends only
    /// changes.
    pub fn update(&mut self, want: Option<String>) {
        if self.sent.as_ref() == Some(&want) || (self.sent.is_none() && want.is_none()) {
            return;
        }
        self.sent = Some(want.clone());
        let Some(line) = request(std::env::var("MORTAR_APP").ok().as_deref(), want.as_deref()) else { return };
        let Some(path) = socket() else { return };
        std::thread::spawn(move || {
            if let Err(e) = send(&path, &line) {
                eprintln!("mortar-ai: telling the shell: {e}");
            }
        });
    }
}

/// The control line: `attention {"op":"set","app":ID,"text":...}`.
pub fn request(app: Option<&str>, text: Option<&str>) -> Option<String> {
    let app = app.filter(|a| !a.is_empty())?;
    let j = match text {
        Some(t) => serde_json::json!({ "op": "set", "app": app, "text": t }),
        None => serde_json::json!({ "op": "clear", "app": app }),
    };
    Some(format!("attention {j}"))
}

fn socket() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("mortar-shell.sock");
    p.exists().then_some(p)
}

fn send(path: &PathBuf, line: &str) -> Result<(), String> {
    let mut s = std::os::unix::net::UnixStream::connect(path).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(3))).ok();
    s.write_all(line.as_bytes()).and_then(|_| s.write_all(b"\n")).map_err(|e| e.to_string())?;
    let mut out = String::new();
    BufReader::new(&s).read_line(&mut out).map_err(|e| e.to_string())?;
    match out.trim().strip_prefix("error: ") {
        Some(e) => Err(e.to_string()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests() {
        assert_eq!(request(Some("claude-pad"), Some("Approve: ls")).unwrap(), r#"attention {"app":"claude-pad","op":"set","text":"Approve: ls"}"#);
        assert_eq!(request(Some("claude-pad"), None).unwrap(), r#"attention {"app":"claude-pad","op":"clear"}"#);
        assert_eq!(request(None, Some("x")), None, "outside the shell");
        assert_eq!(request(Some(""), None), None);
        // only changes go out
        let mut n = Notifier::default();
        n.update(None);
        assert!(n.sent.is_none(), "nothing to clear at first");
        n.update(Some("a".into()));
        assert_eq!(n.sent, Some(Some("a".into())));
        n.update(None);
        assert_eq!(n.sent, Some(None));
    }
}
