// SPDX-License-Identifier: GPL-2.0-or-later
//! "Needs you": any app (or script) can ask for the person's attention,
//! e.g. Claude waiting for a tool approval. The shell shows a badge on the
//! app's window in Running, a dot on the cross's edge towards Running, the
//! Home "Needs you" widget, and one toast when it is raised (not again when
//! only the text changes).
//!
//! Apps raise and clear it through the control socket:
//!
//!   attention {"op":"set","app":"claude-pad","text":"Approve: run ls"}
//!   attention {"op":"clear","app":"claude-pad"}
//!
//! or `mortar-shell attention set|clear [--app ID] [TEXT]` (the app id
//! defaults to $MORTAR_APP, which the shell gives every app it starts).
//! An item for a launcher entry goes away when that app stops running;
//! items for other ids (a script, a service) stay until cleared.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Item {
    /// The launcher entry id (or any name a script picks).
    pub app: String,
    pub text: String,
    /// Raised in this order (oldest first).
    pub seq: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Attention {
    items: Vec<Item>,
    seq: u64,
}

/// One request from the socket.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cmd {
    pub op: String,
    pub app: String,
    #[serde(default)]
    pub text: String,
}

impl Cmd {
    pub fn parse(json: &str) -> Result<Cmd, String> {
        let c: Cmd = serde_json::from_str(json).map_err(|e| format!("attention: {e}"))?;
        if c.app.trim().is_empty() {
            return Err("attention: no app (--app ID, or run it from an app: $MORTAR_APP)".into());
        }
        if !matches!(c.op.as_str(), "set" | "clear") {
            return Err(format!("attention op {}: set | clear", c.op));
        }
        Ok(c)
    }
}

/// What a change did (the shell toasts only when something is raised).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Raised,
    Updated,
    Cleared,
    Nothing,
}

impl Attention {
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn get(&self, app: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.app == app)
    }

    pub fn set(&mut self, app: &str, text: &str) -> Change {
        let text = text.trim().chars().take(160).collect::<String>();
        let text = if text.is_empty() { "Waiting for you".to_string() } else { text };
        if let Some(i) = self.items.iter_mut().find(|i| i.app == app) {
            if i.text == text {
                return Change::Nothing;
            }
            i.text = text;
            return Change::Updated;
        }
        self.seq += 1;
        self.items.push(Item { app: app.to_string(), text, seq: self.seq });
        Change::Raised
    }

    pub fn clear(&mut self, app: &str) -> Change {
        let n = self.items.len();
        self.items.retain(|i| i.app != app);
        if self.items.len() != n { Change::Cleared } else { Change::Nothing }
    }

    pub fn apply(&mut self, c: &Cmd) -> Change {
        match c.op.as_str() {
            "set" => self.set(&c.app, &c.text),
            _ => self.clear(&c.app),
        }
    }

    /// Drop items of apps that stopped (`gone(app)`); true if any went.
    pub fn retain(&mut self, gone: impl Fn(&str) -> bool) -> bool {
        let n = self.items.len();
        self.items.retain(|i| !gone(&i.app));
        self.items.len() != n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raise_update_clear() {
        let mut a = Attention::default();
        assert_eq!(a.set("claude", "Approve: ls"), Change::Raised);
        assert_eq!(a.set("claude", "Approve: ls"), Change::Nothing, "the same text again: no second toast");
        assert_eq!(a.set("claude", "Approve: rm"), Change::Updated);
        assert_eq!(a.set("backup", ""), Change::Raised);
        assert_eq!(a.items().len(), 2);
        assert_eq!(a.get("backup").unwrap().text, "Waiting for you");
        assert!(a.items()[0].seq < a.items()[1].seq);
        assert_eq!(a.clear("claude"), Change::Cleared);
        assert_eq!(a.clear("claude"), Change::Nothing);
        assert!(a.retain(|app| app == "backup"));
        assert!(a.items().is_empty());
    }

    #[test]
    fn parses_requests() {
        let c = Cmd::parse(r#"{"op":"set","app":"claude-pad","text":"Approve: Bash"}"#).unwrap();
        assert_eq!((c.op.as_str(), c.app.as_str(), c.text.as_str()), ("set", "claude-pad", "Approve: Bash"));
        assert!(Cmd::parse(r#"{"op":"clear","app":"x"}"#).is_ok());
        assert!(Cmd::parse(r#"{"op":"toggle","app":"x"}"#).is_err());
        assert!(Cmd::parse(r#"{"op":"set","app":" "}"#).is_err());
        assert!(Cmd::parse(r#"{"op":"set","app":"x","colour":"red"}"#).is_err());
    }
}
