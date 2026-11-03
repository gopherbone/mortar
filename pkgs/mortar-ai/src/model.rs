// SPDX-License-Identifier: GPL-2.0-or-later
//! What the screen shows and what keys do, independent of Wayland and of the
//! claude process: stream-json lines in (`event`), keys in (`key`), and
//! `Out` actions back (lines to write to claude, restart, quit). Unit-tested.

use crate::keys::{self, Mods};
use crate::proto::{self, Decision};
use serde_json::{Value, json};
use std::collections::{HashSet, VecDeque};

#[derive(Clone, Debug, PartialEq)]
pub enum ToolState {
    /// Claude asked to run it; no permission request (yet).
    Running,
    /// Waiting for A/B.
    Waiting,
    Denied,
    Ok,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NoticeKind {
    Info,
    Error,
}

#[derive(Clone, Debug)]
pub enum Entry {
    User(String),
    Assistant { msg_id: String, text: String },
    Tool { id: String, name: String, summary: String, state: ToolState, output: String },
    Notice(NoticeKind, String),
    /// End of a turn: "Done · 4.2 s".
    Turn(String),
}

/// A pending tool-permission request.
#[derive(Clone, Debug)]
pub struct Permission {
    pub request_id: String,
    pub tool: String,
    pub tool_use_id: String,
    pub input: Value,
    pub suggestions: Value,
    /// One line: what it does ("Write \"hi\" into note.txt").
    pub title: String,
    /// The details shown in the card (command, file, diff excerpt).
    pub detail: String,
    /// What "always" would allow, if Claude Code suggested a rule.
    pub always: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    /// Write this JSON line to claude.
    Send(Value),
    Interrupt,
    /// Start claude again (it exited).
    Restart,
    /// Buzz: a permission request arrived.
    Attention,
}

#[derive(Default, Debug)]
pub struct Model {
    pub entries: Vec<Entry>,
    /// Bumped on every change to `entries` (layout cache key).
    pub rev: u64,
    pub pending: VecDeque<Permission>,
    pub input: String,
    pub busy: bool,
    pub alive: bool,
    pub model_name: String,
    pub cwd: String,
    pub mode: String,
    /// "thinking", "running Bash"...
    pub activity: Option<String>,
    /// Lines scrolled up from the bottom (0 = follow the output).
    pub scroll: usize,
    pub mods: Mods,
    pub turns: u32,
    streamed: HashSet<String>,
    cur_msg: String,
    stderr: VecDeque<String>,
    pub toast: Option<String>,
}

const MAX_ENTRIES: usize = 400;
const OUTPUT_LINES: usize = 6;

impl Model {
    pub fn new() -> Model {
        Model { alive: true, ..Default::default() }
    }

    fn push(&mut self, e: Entry) {
        self.entries.push(e);
        if self.entries.len() > MAX_ENTRIES {
            self.entries.drain(..self.entries.len() - MAX_ENTRIES);
        }
        self.rev += 1;
    }

    pub fn notice(&mut self, kind: NoticeKind, s: impl Into<String>) {
        self.push(Entry::Notice(kind, s.into()));
    }

    fn tool_mut(&mut self, id: &str) -> Option<&mut Entry> {
        self.entries.iter_mut().rev().find(|e| matches!(e, Entry::Tool { id: i, .. } if i == id))
    }

    // ------------------------------------------------------- claude side ----

    pub fn stderr(&mut self, line: String) {
        self.stderr.push_back(line);
        while self.stderr.len() > 8 {
            self.stderr.pop_front();
        }
    }

    pub fn exited(&mut self, code: Option<i32>) {
        self.alive = false;
        self.busy = false;
        self.activity = None;
        self.pending.clear();
        let tail: Vec<String> = self.stderr.drain(..).filter(|l| !l.trim().is_empty()).collect();
        let mut msg = match code {
            Some(0) | None => "Claude Code stopped.".to_string(),
            Some(c) => format!("Claude Code exited with status {c}."),
        };
        if !tail.is_empty() {
            msg.push('\n');
            msg.push_str(&tail.join("\n"));
        }
        msg.push_str("\nA starts it again.");
        self.notice(NoticeKind::Error, msg);
    }

    pub fn restarted(&mut self) {
        self.alive = true;
        self.rev += 1;
    }

    /// One stream-json line from claude. Returns actions (responses).
    pub fn event(&mut self, v: &Value) -> Vec<Out> {
        let mut out = Vec::new();
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("");
        match s("type") {
            "system" => match s("subtype") {
                "init" => {
                    self.model_name = s("model").to_string();
                    self.cwd = s("cwd").to_string();
                    self.mode = s("permissionMode").to_string();
                    self.rev += 1;
                }
                "status" => {
                    if s("status") == "requesting" {
                        self.activity = Some("thinking".into());
                    }
                }
                _ => {}
            },
            "stream_event" => self.stream_event(&v["event"]),
            "assistant" => self.assistant(&v["message"]),
            "user" => self.tool_results(&v["message"]),
            "control_request" => {
                let id = s("request_id").to_string();
                let req = &v["request"];
                if req["subtype"] == "can_use_tool" {
                    self.permission(&id, req);
                    out.push(Out::Attention);
                } else {
                    let st = req["subtype"].as_str().unwrap_or("?");
                    out.push(Out::Send(proto::control_error(&id, &format!("mortar-ai does not handle {st}"))));
                }
            }
            "control_cancel_request" => {
                let id = s("request_id");
                self.pending.retain(|p| p.request_id != id);
                self.rev += 1;
            }
            "result" => {
                self.busy = false;
                self.activity = None;
                self.turns += 1;
                let secs = v["duration_ms"].as_f64().unwrap_or(0.0) / 1000.0;
                if v["is_error"].as_bool() == Some(true) || s("subtype").starts_with("error") {
                    let text = v["result"]
                        .as_str()
                        .map(str::to_string)
                        .or_else(|| v["errors"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")))
                        .unwrap_or_else(|| s("subtype").to_string());
                    self.notice(NoticeKind::Error, explain_error(&text));
                } else {
                    self.push(Entry::Turn(format!("Done · {secs:.1} s")));
                }
            }
            _ => {}
        }
        out
    }

    fn stream_event(&mut self, ev: &Value) {
        match ev["type"].as_str().unwrap_or("") {
            "message_start" => {
                self.cur_msg = ev["message"]["id"].as_str().unwrap_or("").to_string();
            }
            "content_block_start" => {
                let b = &ev["content_block"];
                match b["type"].as_str() {
                    Some("thinking") => self.activity = Some("thinking".into()),
                    Some("text") => self.activity = Some("writing".into()),
                    Some("tool_use") => {
                        let name = b["name"].as_str().unwrap_or("tool").to_string();
                        self.activity = Some(format!("preparing {name}"));
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                if let Some(t) = ev["delta"]["text"].as_str() {
                    let id = self.cur_msg.clone();
                    self.streamed.insert(id.clone());
                    match self.entries.last_mut() {
                        Some(Entry::Assistant { msg_id, text }) if *msg_id == id => text.push_str(t),
                        _ => self.entries.push(Entry::Assistant { msg_id: id, text: t.to_string() }),
                    }
                    self.rev += 1;
                }
            }
            _ => {}
        }
    }

    fn assistant(&mut self, m: &Value) {
        let msg_id = m["id"].as_str().unwrap_or("").to_string();
        let Some(blocks) = m["content"].as_array() else { return };
        for b in blocks {
            match b["type"].as_str() {
                Some("text") if !self.streamed.contains(&msg_id) => {
                    let t = b["text"].as_str().unwrap_or("").to_string();
                    if !t.trim().is_empty() {
                        self.push(Entry::Assistant { msg_id: msg_id.clone(), text: t });
                    }
                }
                Some("tool_use") => {
                    let id = b["id"].as_str().unwrap_or("").to_string();
                    let name = b["name"].as_str().unwrap_or("tool").to_string();
                    let summary = summarize(&name, &b["input"]);
                    self.activity = Some(format!("running {name}"));
                    self.push(Entry::Tool { id, name, summary, state: ToolState::Running, output: String::new() });
                }
                _ => {}
            }
        }
    }

    fn tool_results(&mut self, m: &Value) {
        let Some(blocks) = m["content"].as_array() else { return };
        for b in blocks {
            if b["type"] != "tool_result" {
                continue;
            }
            let id = b["tool_use_id"].as_str().unwrap_or("").to_string();
            let err = b["is_error"].as_bool().unwrap_or(false);
            let text = match &b["content"] {
                Value::String(s) => s.clone(),
                Value::Array(a) => a.iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n"),
                _ => String::new(),
            };
            let short = first_lines(&text, OUTPUT_LINES);
            if let Some(Entry::Tool { state, output, .. }) = self.tool_mut(&id) {
                if *state != ToolState::Denied {
                    *state = if err { ToolState::Error } else { ToolState::Ok };
                }
                *output = short;
            }
            self.rev += 1;
        }
        self.activity = Some("thinking".into());
    }

    fn permission(&mut self, request_id: &str, req: &Value) {
        let tool = req["tool_name"].as_str().unwrap_or("tool").to_string();
        let input = req["input"].clone();
        let tool_use_id = req["tool_use_id"].as_str().unwrap_or("").to_string();
        let title = req["description"]
            .as_str()
            .filter(|d| !d.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| default_title(&tool, &input));
        let suggestions = req["permission_suggestions"].clone();
        let always = rule_text(&suggestions);
        let detail = detail(&tool, &input);
        if let Some(Entry::Tool { state, .. }) = self.tool_mut(&tool_use_id) {
            *state = ToolState::Waiting;
        }
        self.activity = Some("waiting for you".into());
        self.pending.push_back(Permission {
            request_id: request_id.to_string(),
            tool,
            tool_use_id,
            input,
            suggestions,
            title,
            detail,
            always,
        });
        self.rev += 1;
    }

    pub fn decide(&mut self, d: Decision) -> Vec<Out> {
        let Some(p) = self.pending.pop_front() else { return vec![] };
        if let Some(Entry::Tool { state, .. }) = self.tool_mut(&p.tool_use_id) {
            *state = match d {
                Decision::Deny(_) => ToolState::Denied,
                _ => ToolState::Running,
            };
        }
        if let Decision::Deny(_) = d {
            self.activity = Some("thinking".into());
        } else {
            self.activity = Some(format!("running {}", p.tool));
        }
        self.rev += 1;
        let suggestions = if p.always.is_some() { p.suggestions.clone() } else { json!([]) };
        vec![Out::Send(proto::permission_response(&p.request_id, &d, &p.input, &suggestions))]
    }

    // ---------------------------------------------------------- keys ----

    /// A key from wl_keyboard (evdev code). The shell maps the pad in
    /// keyboard mode: A Enter, B Backspace, X Tab, Y Space, D-pad arrows,
    /// L1/R1 PgUp/PgDn, L2 Esc, SELECT Ctrl+C, START the on-screen keyboard.
    pub fn key(&mut self, code: u16, down: bool, page_lines: usize) -> Vec<Out> {
        if self.mods.update(code, down) || !down {
            return vec![];
        }
        self.toast = None;
        let page = page_lines.max(3);
        match code {
            keys::UP => return self.scroll_by(3),
            keys::DOWN => return self.scroll_by(-3),
            keys::PAGEUP => return self.scroll_by(page as isize - 2),
            keys::PAGEDOWN => return self.scroll_by(-(page as isize - 2)),
            keys::HOME if self.mods.ctrl => {
                self.scroll = usize::MAX / 2;
                return vec![];
            }
            keys::END => {
                self.scroll = 0;
                return vec![];
            }
            _ => {}
        }
        if !self.pending.is_empty() {
            return match code {
                keys::ENTER | keys::KPENTER => self.decide(Decision::Allow),
                keys::BACKSPACE | keys::DELETE => self.decide(Decision::Deny("The user denied this on the Brick.".into())),
                keys::TAB if self.pending[0].always.is_some() => self.decide(Decision::AllowAlways),
                keys::ESC => {
                    let mut o = self.decide(Decision::Deny("The user stopped this.".into()));
                    o.push(Out::Interrupt);
                    o
                }
                _ => {
                    self.toast = Some("A allows · B denies".into());
                    vec![]
                }
            };
        }
        if !self.alive {
            return if matches!(code, keys::ENTER | keys::KPENTER) { vec![Out::Restart] } else { vec![] };
        }
        if self.mods.ctrl {
            return match code {
                keys::C => {
                    if self.busy {
                        vec![Out::Interrupt]
                    } else {
                        self.input.clear();
                        vec![]
                    }
                }
                keys::U => {
                    self.input.clear();
                    vec![]
                }
                keys::W => {
                    let t = self.input.trim_end().len();
                    let cut = self.input[..t].rfind(' ').map(|i| i + 1).unwrap_or(0);
                    self.input.truncate(cut);
                    vec![]
                }
                keys::L => {
                    self.entries.clear();
                    self.rev += 1;
                    vec![]
                }
                _ => vec![],
            };
        }
        match code {
            keys::ENTER | keys::KPENTER => {
                if self.mods.shift {
                    self.input.push('\n');
                    return vec![];
                }
                let text = self.input.trim().to_string();
                if text.is_empty() {
                    return vec![];
                }
                self.input.clear();
                self.scroll = 0;
                self.busy = true;
                self.activity = Some("sending".into());
                self.push(Entry::User(text.clone()));
                vec![Out::Send(proto::user_message(&text))]
            }
            keys::BACKSPACE => {
                self.input.pop();
                vec![]
            }
            keys::ESC => {
                if self.busy {
                    vec![Out::Interrupt]
                } else {
                    vec![]
                }
            }
            keys::TAB => vec![],
            c => {
                if let Some(ch) = self.mods.char_for(c) {
                    self.input.push(ch);
                }
                vec![]
            }
        }
    }

    fn scroll_by(&mut self, d: isize) -> Vec<Out> {
        self.scroll = (self.scroll as isize + d).max(0) as usize;
        vec![]
    }
}

// ----------------------------------------------------------- helpers ----

pub fn first_lines(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let mut out = lines.iter().take(n).copied().collect::<Vec<_>>().join("\n");
    if lines.len() > n {
        out.push_str(&format!("\n… {} more lines", lines.len() - n));
    }
    out
}

fn str_of<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// One line describing a tool call.
pub fn summarize(tool: &str, input: &Value) -> String {
    let s = match tool {
        "Bash" => str_of(input, "command").to_string(),
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => {
            let p = str_of(input, "file_path");
            if p.is_empty() { str_of(input, "notebook_path").to_string() } else { p.to_string() }
        }
        "Glob" | "Grep" => {
            let p = str_of(input, "pattern");
            let path = str_of(input, "path");
            if path.is_empty() { p.to_string() } else { format!("{p}  in {path}") }
        }
        "WebFetch" => str_of(input, "url").to_string(),
        "WebSearch" => str_of(input, "query").to_string(),
        "Task" | "Agent" => str_of(input, "description").to_string(),
        "TodoWrite" => "update the to-do list".to_string(),
        _ => {
            let j = input.to_string();
            if j == "{}" || j == "null" { String::new() } else { j }
        }
    };
    s.lines().next().unwrap_or("").chars().take(300).collect()
}

/// What the permission card shows below its title.
pub fn detail(tool: &str, input: &Value) -> String {
    match tool {
        "Bash" => format!("$ {}", str_of(input, "command")),
        "Edit" => {
            let mut s = str_of(input, "file_path").to_string();
            for l in first_lines(str_of(input, "old_string"), 4).lines() {
                s.push_str(&format!("\n- {l}"));
            }
            for l in first_lines(str_of(input, "new_string"), 4).lines() {
                s.push_str(&format!("\n+ {l}"));
            }
            s
        }
        "Write" => {
            let mut s = str_of(input, "file_path").to_string();
            for l in first_lines(str_of(input, "content"), 6).lines() {
                s.push_str(&format!("\n+ {l}"));
            }
            s
        }
        _ => {
            let one = summarize(tool, input);
            if one.is_empty() { tool.to_string() } else { one }
        }
    }
}

/// "Edit shell.nix" when Claude Code sends no description.
fn default_title(tool: &str, input: &Value) -> String {
    let file = |k: &str| str_of(input, k).rsplit('/').next().unwrap_or("").to_string();
    match tool {
        "Edit" | "MultiEdit" => format!("Edit {}", file("file_path")),
        "Write" => format!("Create or overwrite {}", file("file_path")),
        "Read" => format!("Read {}", file("file_path")),
        "WebFetch" => format!("Fetch {}", str_of(input, "url")),
        "WebSearch" => format!("Search the web for {}", str_of(input, "query")),
        "Bash" => "Run a command".to_string(),
        _ => format!("Use {tool}"),
    }
}

/// What "always" adds, from Claude Code's permission suggestions:
/// "Bash(git status *)" rules, or a mode switch ("accept edits").
pub fn rule_text(sugg: &Value) -> Option<String> {
    let mut rules = Vec::new();
    for s in sugg.as_array()? {
        if s["type"] == "setMode" && s["mode"] == "acceptEdits" {
            rules.push("accept edits this session".to_string());
        }
        if s["type"] == "addRules" && s["behavior"] == "allow" {
            for r in s["rules"].as_array().into_iter().flatten() {
                let t = str_of(r, "toolName");
                let c = str_of(r, "ruleContent");
                rules.push(if c.is_empty() { t.to_string() } else { format!("{t}({c})") });
            }
        }
    }
    if rules.is_empty() { None } else { Some(rules.join(", ")) }
}

/// Make the common failures actionable on the Brick.
pub fn explain_error(text: &str) -> String {
    let l = text.to_lowercase();
    if l.contains("not logged in") || l.contains("/login") || l.contains("invalid api key") || l.contains("oauth") {
        format!("{text}\nSign in first: open Claude Code from the launcher; it walks you through `claude auth login`.")
    } else if l.contains("enotfound") || l.contains("network") || l.contains("connection") {
        format!("{text}\nCheck Wi-Fi (MENU, Settings).")
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(m: &mut Model, lines: &[Value]) -> Vec<Out> {
        lines.iter().flat_map(|l| m.event(l)).collect()
    }

    #[test]
    fn streaming_text_and_tool_flow() {
        let mut m = Model::new();
        for c in "hi".chars() {
            m.mods = Mods::default();
            let code = match c { 'h' => 35, _ => 23 };
            m.key(code, true, 10);
        }
        let out = m.key(keys::ENTER, true, 10);
        assert_eq!(out, vec![Out::Send(proto::user_message("hi"))]);
        assert!(m.busy);
        feed(&mut m, &[
            json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"m1"}}}),
            json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hel"}}}),
            json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"lo"}}}),
            json!({"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"Hello"}]}}),
            json!({"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls -la"}}]}}),
        ]);
        assert!(matches!(&m.entries[1], Entry::Assistant { text, .. } if text == "Hello"));
        assert!(matches!(&m.entries[2], Entry::Tool { summary, state: ToolState::Running, .. } if summary == "ls -la"));
        let out = m.event(&json!({"type":"control_request","request_id":"r1","request":{
            "subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls -la"},"tool_use_id":"t1",
            "description":"List files","permission_suggestions":[{"type":"addRules","behavior":"allow","rules":[{"toolName":"Bash","ruleContent":"ls *"}]}]}}));
        assert_eq!(out, vec![Out::Attention]);
        assert_eq!(m.pending.len(), 1);
        assert_eq!(m.pending[0].always.as_deref(), Some("Bash(ls *)"));
        assert!(matches!(&m.entries[2], Entry::Tool { state: ToolState::Waiting, .. }));
        // typing does nothing while a prompt is up
        assert!(m.key(30, true, 10).is_empty());
        assert!(m.input.is_empty());
        let out = m.key(keys::ENTER, true, 10);
        match &out[0] {
            Out::Send(v) => {
                assert_eq!(v["response"]["request_id"], "r1");
                assert_eq!(v["response"]["response"]["behavior"], "allow");
            }
            o => panic!("{o:?}"),
        }
        feed(&mut m, &[
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"a\nb","is_error":false}]}}),
            json!({"type":"result","subtype":"success","duration_ms":4200,"is_error":false}),
        ]);
        assert!(matches!(&m.entries[2], Entry::Tool { state: ToolState::Ok, output, .. } if output == "a\nb"));
        assert!(matches!(m.entries.last(), Some(Entry::Turn(t)) if t == "Done · 4.2 s"));
        assert!(!m.busy);
    }

    #[test]
    fn deny_and_always() {
        let mut m = Model::new();
        let req = |id: &str| json!({"type":"control_request","request_id":id,"request":{
            "subtype":"can_use_tool","tool_name":"Write","input":{"file_path":"/x","content":"1\n2"},
            "permission_suggestions":[{"type":"addRules","behavior":"allow","rules":[{"toolName":"Write"}]}]}});
        m.event(&req("a"));
        m.event(&req("b"));
        assert_eq!(m.pending[0].detail, "/x\n+ 1\n+ 2");
        let o = m.key(keys::BACKSPACE, true, 10);
        let Out::Send(v) = &o[0] else { panic!() };
        assert_eq!(v["response"]["response"]["behavior"], "deny");
        let o = m.key(keys::TAB, true, 10);
        let Out::Send(v) = &o[0] else { panic!() };
        assert_eq!(v["response"]["request_id"], "b");
        assert_eq!(v["response"]["response"]["updatedPermissions"][0]["type"], "addRules");
        assert!(m.pending.is_empty());
    }

    #[test]
    fn unknown_control_requests_get_errors() {
        let mut m = Model::new();
        let o = m.event(&json!({"type":"control_request","request_id":"h","request":{"subtype":"hook_callback"}}));
        let Out::Send(v) = &o[0] else { panic!() };
        assert_eq!(v["response"]["subtype"], "error");
    }

    #[test]
    fn errors_and_restart() {
        let mut m = Model::new();
        m.event(&json!({"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}));
        assert!(matches!(m.entries.last(), Some(Entry::Notice(NoticeKind::Error, t)) if t.contains("claude auth login")));
        m.exited(Some(1));
        assert!(!m.alive);
        assert_eq!(m.key(keys::ENTER, true, 10), vec![Out::Restart]);
    }

    #[test]
    fn scroll_and_ctrl_keys() {
        let mut m = Model::new();
        m.key(keys::PAGEUP, true, 20);
        assert_eq!(m.scroll, 18);
        m.key(keys::DOWN, true, 20);
        assert_eq!(m.scroll, 15);
        m.key(keys::END, true, 20);
        assert_eq!(m.scroll, 0);
        m.input = "git status now".into();
        m.key(keys::LEFTCTRL, true, 20);
        m.key(keys::W, true, 20);
        assert_eq!(m.input, "git status ");
        m.key(keys::C, true, 20);
        assert_eq!(m.input, "");
        m.busy = true;
        assert_eq!(m.key(keys::C, true, 20), vec![Out::Interrupt]);
    }
}
