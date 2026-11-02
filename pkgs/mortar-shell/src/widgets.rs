// SPDX-License-Identifier: GPL-2.0-or-later
//! Home widgets: what the dashboard shows, declared in the NixOS module
//! (`mortar.shell.home.widgets`, config `home.widgets`), laid out on a
//! 4 x 3 grid of tiles (1x1 or 2x1), and the worker that runs "command"
//! widgets off the event loop.
//!
//! Built-in kinds: `clock` (the big clock and date), `continue` (the last
//! app or game; A resumes it), `needs-you` (apps asking for attention,
//! attention.rs), `battery`, `wifi`, `storage`, `now-playing` (which app is
//! making sound, from the Mixer's PipeWire streams) and `command`: a script
//! run every `interval` seconds while Home shows, whose output is shown as
//! text, or, if it prints a JSON object, as
//! `{"title": "...", "value": "...", "accent": "lime"}`.
//! Any widget can have `open = "<app id>"`: A starts (or switches to) it.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Clock,
    Continue,
    NeedsYou,
    Battery,
    Wifi,
    Storage,
    NowPlaying,
    Command,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Size {
    #[serde(rename = "1x1")]
    One,
    #[serde(rename = "2x1")]
    Two,
}

impl Size {
    pub fn cols(self) -> usize {
        match self {
            Size::One => 1,
            Size::Two => 2,
        }
    }
}

/// A tile colour (the Pond accents; `ground`: a plain tile).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accent {
    Lime,
    Lavender,
    Yellow,
    Orange,
    Pink,
    Ground,
}

impl Accent {
    pub fn parse(s: &str) -> Option<Accent> {
        serde_json::from_value(serde_json::Value::String(s.to_ascii_lowercase())).ok()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Widget {
    pub kind: Kind,
    /// 1x1 or 2x1 (default: 2x1 for clock, continue, needs-you,
    /// now-playing; 1x1 for the rest).
    #[serde(default)]
    pub size: Option<Size>,
    /// The tile's label (default: from the kind).
    #[serde(default)]
    pub title: Option<String>,
    /// `command`: the argv to run.
    #[serde(default)]
    pub command: Vec<String>,
    /// `command`: seconds between runs (at least 2).
    #[serde(default = "d_interval")]
    pub interval: u32,
    #[serde(default)]
    pub accent: Option<Accent>,
    /// A on the tile opens this launcher entry.
    #[serde(default)]
    pub open: Option<String>,
}

fn d_interval() -> u32 {
    60
}

impl Widget {
    pub fn new(kind: Kind) -> Widget {
        Widget { kind, size: None, title: None, command: Vec::new(), interval: d_interval(), accent: None, open: None }
    }

    pub fn size(&self) -> Size {
        self.size.unwrap_or(match self.kind {
            Kind::Clock | Kind::Continue | Kind::NeedsYou | Kind::NowPlaying => Size::Two,
            _ => Size::One,
        })
    }

    pub fn label(&self) -> String {
        if let Some(t) = &self.title {
            return t.clone();
        }
        match self.kind {
            Kind::Clock => "Clock",
            Kind::Continue => "Continue",
            Kind::NeedsYou => "Needs you",
            Kind::Battery => "Battery",
            Kind::Wifi => "Wi-Fi",
            Kind::Storage => "Storage",
            Kind::NowPlaying => "Now playing",
            Kind::Command => "Script",
        }
        .to_string()
    }
}

/// The dashboard without any configuration.
pub fn defaults() -> Vec<Widget> {
    [Kind::Clock, Kind::Continue, Kind::NeedsYou, Kind::NowPlaying, Kind::Battery, Kind::Wifi, Kind::Storage].into_iter().map(Widget::new).collect()
}

pub const COLS: usize = 4;
pub const ROWS: usize = 3;

/// A widget's place: (widget index, column, row, columns wide).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub i: usize,
    pub col: usize,
    pub row: usize,
    pub w: usize,
}

/// First fit in reading order (a 2x1 that does not fit at the end of a row
/// goes to the next one, and a later 1x1 fills the gap). Widgets that do
/// not fit are left out.
pub fn layout(ws: &[Widget]) -> Vec<Slot> {
    let mut used = [[false; COLS]; ROWS];
    let mut out = Vec::new();
    for (i, w) in ws.iter().enumerate() {
        let n = w.size().cols();
        'find: for row in 0..ROWS {
            for col in 0..=(COLS - n) {
                if (col..col + n).all(|c| !used[row][c]) {
                    for c in col..col + n {
                        used[row][c] = true;
                    }
                    out.push(Slot { i, col, row, w: n });
                    break 'find;
                }
            }
        }
    }
    out
}

/// A command widget's last result.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Output {
    pub title: Option<String>,
    pub value: String,
    pub accent: Option<Accent>,
    /// It failed (exit status, timeout, could not start).
    pub error: bool,
}

/// What a script printed: a JSON object {title, value, accent}, else the
/// text (its first two lines).
pub fn parse_output(stdout: &str) -> Output {
    let s = stdout.trim();
    if s.starts_with('{')
        && let Ok(serde_json::Value::Object(m)) = serde_json::from_str::<serde_json::Value>(s)
    {
        let st = |k: &str| m.get(k).map(|v| match v {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        });
        return Output {
            title: st("title").filter(|t| !t.is_empty()),
            value: clip(&st("value").unwrap_or_default(), 2),
            accent: st("accent").and_then(|a| Accent::parse(&a)),
            error: false,
        };
    }
    Output { value: clip(s, 2), ..Default::default() }
}

fn clip(s: &str, lines: usize) -> String {
    s.lines().map(str::trim).filter(|l| !l.is_empty()).take(lines).collect::<Vec<_>>().join("\n").chars().take(120).collect()
}

/// Runs the command widgets, each every `interval` seconds while active
/// (Home on screen), on a thread of its own; a run that takes more than
/// 10 s is killed (its whole process group).
pub struct Worker {
    shared: Arc<Shared>,
    seen: u64,
}

struct Shared {
    out: Mutex<Vec<Option<Output>>>,
    seq: AtomicU64,
    active: AtomicBool,
    stop: AtomicBool,
}

const RUN_LIMIT: Duration = Duration::from_secs(10);

impl Worker {
    pub fn new(ws: &[Widget]) -> Worker {
        let jobs: Vec<(usize, Vec<String>, Duration)> = ws
            .iter()
            .enumerate()
            .filter(|(_, w)| w.kind == Kind::Command && !w.command.is_empty())
            .map(|(i, w)| (i, w.command.clone(), Duration::from_secs(w.interval.max(2) as u64)))
            .collect();
        let shared = Arc::new(Shared { out: Mutex::new(vec![None; ws.len()]), seq: AtomicU64::new(0), active: AtomicBool::new(false), stop: AtomicBool::new(false) });
        if !jobs.is_empty() {
            let sh = shared.clone();
            let spawned = std::thread::Builder::new().name("widgets".into()).spawn(move || {
                let mut last: Vec<Option<Instant>> = vec![None; jobs.len()];
                while !sh.stop.load(Ordering::Acquire) {
                    if sh.active.load(Ordering::Acquire) {
                        for (j, (i, argv, every)) in jobs.iter().enumerate() {
                            if last[j].is_some_and(|t| t.elapsed() < *every) {
                                continue;
                            }
                            last[j] = Some(Instant::now());
                            let o = run(argv);
                            if let Ok(mut g) = sh.out.lock()
                                && let Some(slot) = g.get_mut(*i)
                                && slot.as_ref() != Some(&o)
                            {
                                *slot = Some(o);
                                sh.seq.fetch_add(1, Ordering::Release);
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            });
            if let Err(e) = spawned {
                tracing::warn!("widgets thread: {e}");
            }
        }
        Worker { shared, seen: 0 }
    }

    pub fn set_active(&self, on: bool) {
        self.shared.active.store(on, Ordering::Release);
    }

    /// The newest results, when they changed.
    pub fn take(&mut self) -> Option<Vec<Option<Output>>> {
        let g = self.shared.seq.load(Ordering::Acquire);
        if g == self.seen {
            return None;
        }
        self.seen = g;
        self.shared.out.lock().ok().map(|v| v.clone())
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
    }
}

/// One run of a script, with the time limit.
pub fn run(argv: &[String]) -> Output {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    let Some((cmd, args)) = argv.split_first() else { return Output { value: "no command".into(), error: true, ..Default::default() } };
    let child = std::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(e) => return Output { value: format!("{cmd}: {e}"), error: true, ..Default::default() },
    };
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = &mut stdout {
            let _ = o.take(64 * 1024).read_to_string(&mut s);
        }
        s
    });
    let t0 = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if t0.elapsed() < RUN_LIMIT => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                // SAFETY: plain syscall on our own child's process group
                unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                let _ = child.wait();
                break None;
            }
        }
    };
    let text = reader.join().unwrap_or_default();
    match status {
        Some(s) if s.success() => parse_output(&text),
        Some(s) => {
            let t = clip(&text, 2);
            Output { value: if t.is_empty() { format!("exit {}", s.code().unwrap_or(-1)) } else { t }, error: true, ..Default::default() }
        }
        None => Output { value: "timed out".into(), error: true, ..Default::default() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_and_defaults() {
        let w: Vec<Widget> = serde_json::from_str(
            r#"[{"kind":"clock"},{"kind":"command","size":"2x1","title":"Disk","command":["df","-h"],"interval":30,"accent":"lime","open":"terminal"},{"kind":"now-playing"}]"#,
        )
        .unwrap();
        assert_eq!(w[0].size(), Size::Two);
        assert_eq!(w[1].size(), Size::Two);
        assert_eq!(w[1].interval, 30);
        assert_eq!(w[1].accent, Some(Accent::Lime));
        assert_eq!(w[2].label(), "Now playing");
        assert!(serde_json::from_str::<Widget>(r#"{"kind":"weather"}"#).is_err());
        assert!(serde_json::from_str::<Widget>(r#"{"kind":"clock","colour":1}"#).is_err());
        assert_eq!(defaults().len(), 7);
    }

    #[test]
    fn layout_fills_the_grid() {
        // clock 2, continue 2 | needs-you 2, now-playing 2 | three 1x1
        let s = layout(&defaults());
        assert_eq!(s.len(), 7);
        assert_eq!(s[0], Slot { i: 0, col: 0, row: 0, w: 2 });
        assert_eq!(s[1], Slot { i: 1, col: 2, row: 0, w: 2 });
        assert_eq!(s[3], Slot { i: 3, col: 2, row: 1, w: 2 });
        assert_eq!(s[6], Slot { i: 6, col: 2, row: 2, w: 1 });
        // 1x1, 2x1, 2x1: the second 2x1 goes to the next row, a 1x1 fills the gap
        let w = |k: Kind, s: Size| Widget { size: Some(s), ..Widget::new(k) };
        let ws = vec![w(Kind::Battery, Size::One), w(Kind::Clock, Size::Two), w(Kind::Continue, Size::Two), w(Kind::Wifi, Size::One)];
        let s = layout(&ws);
        assert_eq!(s[2], Slot { i: 2, col: 0, row: 1, w: 2 });
        assert_eq!(s[3], Slot { i: 3, col: 3, row: 0, w: 1 });
        // more than fits: left out
        let many: Vec<Widget> = (0..20).map(|_| w(Kind::Battery, Size::One)).collect();
        assert_eq!(layout(&many).len(), COLS * ROWS);
    }

    #[test]
    fn script_output() {
        let o = parse_output("{\"title\":\"Disk\",\"value\":\"42%\",\"accent\":\"Lime\"}\n");
        assert_eq!(o, Output { title: Some("Disk".into()), value: "42%".into(), accent: Some(Accent::Lime), error: false });
        let o = parse_output("{\"value\": 7}");
        assert_eq!(o.value, "7");
        let o = parse_output("\n  up 3 days  \nload 0.4\nmore\n");
        assert_eq!(o.value, "up 3 days\nload 0.4");
        assert_eq!(o.title, None);
    }

    #[test]
    fn runs_scripts_with_a_limit() {
        let sh = |s: &str| vec!["sh".to_string(), "-c".to_string(), s.to_string()];
        assert_eq!(run(&sh("echo '{\"value\":\"ok\"}'")).value, "ok");
        let bad = run(&sh("echo nope; exit 3"));
        assert!(bad.error && bad.value == "nope");
        assert!(run(&["/nonexistent/widget".to_string()]).error);
        // the worker: runs while active, results by widget index
        let ws = vec![Widget::new(Kind::Clock), Widget { command: sh("echo hello"), ..Widget::new(Kind::Command) }];
        let mut wk = Worker::new(&ws);
        assert!(wk.take().is_none());
        wk.set_active(true);
        let t0 = Instant::now();
        let got = loop {
            if let Some(v) = wk.take() {
                break v;
            }
            assert!(t0.elapsed() < Duration::from_secs(5), "no result");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(got[0], None);
        assert_eq!(got[1].as_ref().unwrap().value, "hello");
    }
}
