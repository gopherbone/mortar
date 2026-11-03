// SPDX-License-Identifier: GPL-2.0-or-later
//! `mortar-ai preview`: render demo states to PNG on any OS (UI work, docs).
//! The demo states are built by feeding stream-json lines through the model,
//! the same path the real session takes.

use crate::keys;
use crate::model::Model;
use crate::view::View;
use mortar_ui::gfx::{Fonts, Ui};
use mortar_ui::status::Sys;
use serde_json::{Value, json};
use std::path::Path;

pub const SCREENS: &[&str] = &["empty", "chat", "bash", "edit", "busy", "error", "exited", "pair", "pair-direct"];

fn init() -> Value {
    json!({"type":"system","subtype":"init","model":"claude-sonnet-5","cwd":"/root/ai","permissionMode":"default"})
}

fn text(id: &str, t: &str) -> Vec<Value> {
    vec![
        json!({"type":"stream_event","event":{"type":"message_start","message":{"id":id}}}),
        json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":t}}}),
    ]
}

fn tool(id: &str, name: &str, input: Value) -> Value {
    json!({"type":"assistant","message":{"id":"m","content":[{"type":"tool_use","id":id,"name":name,"input":input}]}})
}

fn result(id: &str, out: &str, err: bool) -> Value {
    json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":id,"content":out,"is_error":err}]}})
}

fn ask(m: &mut Model, s: &str) {
    m.input = s.into();
    m.key(keys::ENTER, true, 20);
}

pub fn demo(screen: &str) -> Result<Model, String> {
    let mut m = Model::new();
    let feed = |m: &mut Model, v: Vec<Value>| {
        for l in v {
            m.event(&l);
        }
    };
    m.event(&init());
    if screen == "empty" {
        return Ok(m);
    }
    ask(&mut m, "What's using the most memory on this thing?");
    feed(&mut m, text("m1", "I'll check the processes sorted by resident memory."));
    feed(&mut m, vec![
        tool("t1", "Bash", json!({"command":"ps -eo rss,comm --sort=-rss | head -6","description":"List the biggest processes"})),
        result("t1", "   RSS COMMAND\n201344 claude\n 58212 mortar-shell\n 41020 m8c-brick\n 15872 foot\n  9216 systemd-journal", false),
    ]);
    feed(&mut m, text("m2", "Claude Code itself is the biggest at about 197 MB, then the shell (57 MB) and m8c (40 MB). Together that is well under the 970 MB, and zram holds the rest.\n\nWant me to log this every minute while you play?"));
    feed(&mut m, vec![json!({"type":"result","subtype":"success","duration_ms":8400,"is_error":false})]);
    match screen {
        "chat" => {}
        "bash" => {
            ask(&mut m, "Yes, log it to ~/mem.log");
            feed(&mut m, text("m3", "I'll start a small logger in the background with systemd-run."));
            feed(&mut m, vec![
                tool("t2", "Bash", json!({"command":"systemd-run --user --unit=memlog --on-unit-active=60 sh -c 'date; free -m' >> ~/mem.log"})),
                json!({"type":"control_request","request_id":"r1","request":{
                    "subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"t2",
                    "input":{"command":"systemd-run --user --unit=memlog --on-unit-active=60 sh -c 'date; free -m' >> ~/mem.log"},
                    "description":"Start a timer that logs free memory every minute",
                    "permission_suggestions":[{"type":"addRules","behavior":"allow","destination":"localSettings","rules":[{"toolName":"Bash","ruleContent":"systemd-run *"}]}]}}),
            ]);
        }
        "edit" => {
            ask(&mut m, "Make the terminal font a bit bigger");
            feed(&mut m, vec![
                tool("t3", "Read", json!({"file_path":"/root/mortar/modules/core/shell.nix"})),
                result("t3", "(412 lines)", false),
                tool("t4", "Edit", json!({"file_path":"/root/mortar/modules/core/shell.nix","old_string":"default = 13;","new_string":"default = 15;"})),
                json!({"type":"control_request","request_id":"r2","request":{
                    "subtype":"can_use_tool","tool_name":"Edit","tool_use_id":"t4",
                    "input":{"file_path":"/root/mortar/modules/core/shell.nix","old_string":"        default = 13;","new_string":"        default = 15;"},
                    "permission_suggestions":[{"type":"setMode","mode":"acceptEdits","destination":"session"}]}}),
                json!({"type":"control_request","request_id":"r3","request":{
                    "subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"t5",
                    "input":{"command":"nix run .#mortar -- deploy"},"description":"Deploy the change"}}),
            ]);
        }
        "busy" => {
            ask(&mut m, "Summarise the shell's DESIGN.md");
            feed(&mut m, vec![tool("t6", "Read", json!({"file_path":"/root/mortar/pkgs/mortar-shell/DESIGN.md"}))]);
            m.input = "and the open quest".into();
        }
        "error" => {
            feed(&mut m, vec![json!({"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"})]);
        }
        "exited" => {
            m.stderr("Error: connect ENETUNREACH".into());
            m.exited(Some(1));
        }
        s => return Err(format!("unknown screen '{s}' ({})", SCREENS.join(", "))),
    }
    Ok(m)
}

/// A made-up sign-in link of the real length (the OAuth URL is ~400 chars).
pub const DEMO_LOGIN_URL: &str = "https://claude.ai/oauth/authorize?code=true&client_id=00000000-0000-4000-8000-000000000000&response_type=code&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&scope=org%3Acreate_api_key+user%3Aprofile+user%3Ainference+user%3Asessions%3Aclaude_code&code_challenge=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA&code_challenge_method=S256&state=BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";

pub fn run(screen: &str, out: &Path, size: (u32, u32), scroll: usize) -> Result<(), String> {
    let fonts = Fonts::load(&mortar_ui::gfx::font_dir())?;
    let mut ui = Ui::new(size.0, size.1, fonts);
    if screen.starts_with("pair") {
        use crate::screen::Screen;
        let mut p = if screen == "pair" {
            crate::pair::Pair::demo(DEMO_LOGIN_URL, "http://192.168.1.42:41873/8f3a0c1d2e4b5a69788796a5b4c3d2e1", true)
        } else {
            crate::pair::Pair::new(DEMO_LOGIN_URL, false)
        };
        p.draw(&mut ui);
        return ui.save_png(out);
    }
    let mut m = demo(screen)?;
    m.scroll = scroll;
    let sys = Sys::demo(Some("21:40".into()));
    let mut v = View::default();
    v.draw(&mut ui, &mut m, &sys.bar());
    ui.save_png(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn demos_build() {
        for s in super::SCREENS.iter().filter(|s| !s.starts_with("pair")) {
            super::demo(s).unwrap();
        }
    }
}
