// SPDX-License-Identifier: GPL-2.0-or-later
//! `mortar-ai ask PROMPT`: the same session and model in a plain terminal
//! (for SSH and for checking the protocol against a real claude off-device).
//! Permission prompts are answered on stdin: y allow, a always, else deny.

use crate::model::{Entry, Model, Out};
use crate::proto::{Claude, Decision, Msg, Spawn};
use std::io::{BufRead, Write};

pub fn ask(spawn: Spawn, prompt: &str) -> Result<(), String> {
    let mut c = Claude::spawn(&spawn).map_err(|e| format!("{}: {e}", spawn.program))?;
    let mut m = Model::new();
    m.input = prompt.to_string();
    for o in m.key(crate::keys::ENTER, true, 20) {
        if let Out::Send(v) = o {
            c.send(&v);
        }
    }
    let mut shown = 0usize; // entries already printed (the last may grow)
    let mut shown_len = 0usize;
    let stdin = std::io::stdin();
    loop {
        let msg = c.rx.recv().map_err(|e| e.to_string())?;
        let mut outs = Vec::new();
        match msg {
            Msg::Json(v) => outs = m.event(&v),
            Msg::Text(t) => eprintln!("{t}"),
            Msg::Stderr(t) => eprintln!("[stderr] {t}"),
            Msg::Exit => {
                let code = c.reap();
                println!("\n[claude exited: {code:?}]");
                return Ok(());
            }
        }
        // print new entries / streamed text
        while shown < m.entries.len() {
            let e = &m.entries[shown];
            let last = shown + 1 == m.entries.len();
            match e {
                Entry::Assistant { text, .. } => {
                    print!("{}", &text[shown_len.min(text.len())..]);
                    shown_len = text.len();
                    if last {
                        break;
                    }
                    println!();
                }
                Entry::User(t) => println!("> {t}"),
                Entry::Tool { name, summary, output, .. } => {
                    if last && output.is_empty() {
                        break;
                    }
                    println!("[{name}] {summary}\n{output}");
                }
                Entry::Notice(_, t) | Entry::Turn(t) => println!("[{t}]"),
            }
            shown += 1;
            shown_len = 0;
        }
        std::io::stdout().flush().ok();
        for o in outs {
            match o {
                Out::Send(v) => {
                    c.send(&v);
                }
                Out::Attention => {
                    let p = m.pending.front().cloned().unwrap();
                    print!("\n[permission] {}\n{}\nallow? [y]es / [a]lways ({}) / [N]o: ", p.title, p.detail, p.always.as_deref().unwrap_or("-"));
                    std::io::stdout().flush().ok();
                    let mut line = String::new();
                    stdin.lock().read_line(&mut line).ok();
                    let d = match line.trim() {
                        "y" | "Y" => Decision::Allow,
                        "a" | "A" if p.always.is_some() => Decision::AllowAlways,
                        _ => Decision::Deny("The user denied this.".into()),
                    };
                    for o in m.decide(d) {
                        if let Out::Send(v) = o {
                            c.send(&v);
                        }
                    }
                }
                _ => {}
            }
        }
        if !m.busy && m.turns > 0 {
            println!();
            return Ok(()); // one turn; dropping `c` closes stdin and ends claude
        }
    }
}
