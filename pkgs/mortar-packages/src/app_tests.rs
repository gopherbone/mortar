// SPDX-License-Identifier: GPL-2.0-or-later
//! The app driven with key events, against the made-up service. It does nothing but ask:
//! what it does to the system is what ends up in the service's log.

use crate::app::*;
use crate::demo::{self, Demo, DemoBackend};
use crate::protocol::*;
use mortar_files::bg;
use mortar_files::keys::{self, Input};
use mortar_files::shell::Shell;
use mortar_ui::input::Button;
use std::sync::{Arc, Mutex};

struct T {
    app: App,
    d: Arc<Demo>,
    home: std::path::PathBuf,
    shell: Arc<Mutex<Vec<String>>>,
}

impl T {
    fn new(name: &str) -> T {
        let home = std::env::temp_dir().join(format!("mortar-packages-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let d = Demo::new();
        let shell = Arc::new(Mutex::new(Vec::new()));
        let app = App::new(Setup { backend: Arc::new(DemoBackend(d.clone())), shell: Shell::Record(shell.clone()), now_secs: demo::NOW, home: home.clone(), reduce_motion: true, wake: bg::no_wake() });
        let mut t = T { app, d, home, shell };
        t.settle();
        t
    }

    fn settle(&mut self) {
        for _ in 0..300 {
            std::thread::sleep(std::time::Duration::from_millis(1));
            self.app.poll();
            self.app.tick(self.app.now + 20);
            self.app.dirty = false;
            if self.app.idle() {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
        self.app.poll();
    }

    fn press(&mut self, b: Button) {
        for (c, d) in keys::pad_events(b) {
            self.app.key(c, d, false);
        }
        self.settle();
    }

    fn sel(&mut self, name: &str) {
        let i = self.app.entries.iter().position(|e| match e {
            Entry::Package(r) => r.name == name,
            Entry::Tools { .. } => name == "Terminal tools",
        });
        self.app.home_list.sel = i.unwrap_or_else(|| panic!("no {name}"));
    }

    fn log(&self) -> Vec<String> {
        self.d.log.lock().unwrap().clone()
    }

    fn asked(&self, what: &str) -> bool {
        self.log().iter().any(|l| l == what)
    }
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

#[test]
fn lists_what_there_is_with_terminal_tools_last() {
    let t = T::new("list");
    assert_eq!(t.app.screen, Screen::Home);
    assert!(matches!(t.app.entries.last(), Some(Entry::Tools { count: 93_663, installed: 1 })));
    assert!(t.app.entries.iter().all(|e| !matches!(e, Entry::Package(r) if r.kind == mortar_index::Kind::Tool)), "tools aren't apps");
    assert!(t.asked("Overview"));
    // nothing else has been asked: a passing row doesn't start Nix
    assert!(!t.log().iter().any(|l| l.starts_with("Details")));
}

#[test]
fn details_ask_for_the_size_of_what_is_missing() {
    let mut t = T::new("details");
    t.sel("Harbour Browser");
    t.press(Button::A);
    assert_eq!(t.app.screen, Screen::Details);
    assert!(t.asked("Details mortar harbour-browser"));
    assert_eq!(t.app.details.as_ref().unwrap().size.as_ref().unwrap().unpacked, 209 * 1048576);
    t.press(Button::B);
    assert_eq!(t.app.screen, Screen::Home);
}

#[test]
fn a_bundle_cannot_be_installed_and_nothing_is_asked() {
    let mut t = T::new("bundle");
    t.sel("Sky Forge");
    t.press(Button::A);
    t.press(Button::A);
    assert_eq!(t.app.screen, Screen::Details);
    assert!(!t.log().iter().any(|l| l.starts_with("Install")));
}

#[test]
fn what_came_from_elsewhere_is_left_alone() {
    let mut t = T::new("elsewhere");
    t.sel("Moon Miner Deluxe");
    t.press(Button::A);
    for b in [Button::A, Button::X, Button::Y] {
        t.press(b);
    }
    assert!(t.app.overlay.is_none());
    assert!(!t.log().iter().any(|l| l.starts_with("Install") || l.starts_with("Remove") || l.starts_with("Rollback")));
}

#[test]
fn install_sends_only_a_repository_and_an_id() {
    let mut t = T::new("install");
    t.sel("Harbour Browser");
    t.press(Button::A);
    t.press(Button::A);
    assert_eq!(t.app.screen, Screen::Progress);
    assert!(t.asked("Install mortar harbour-browser"));
    assert_eq!(t.app.job.as_ref().unwrap().id, "harbour-browser");
}

#[test]
fn leaving_keeps_the_job_going_and_coming_back_finds_it() {
    let mut t = T::new("leave");
    t.sel("Harbour Browser");
    t.press(Button::A);
    t.press(Button::A);
    t.d.with(|s| {
        let j = s.job.as_mut().unwrap();
        j.phase = Phase::Downloading;
        j.done = j.total / 2;
    });
    t.press(Button::B);
    assert_eq!(t.app.screen, Screen::Home);
    assert!(t.app.job.is_some());
    assert!(!t.log().iter().any(|l| l.starts_with("Cancel")), "leaving isn't cancelling");
    // selecting it goes to its progress, not to Install again
    t.press(Button::A);
    assert_eq!(t.app.screen, Screen::Progress);
    assert_eq!(t.log().iter().filter(|l| l.starts_with("Install")).count(), 1);
}

#[test]
fn cancel_asks_the_service() {
    let mut t = T::new("cancel");
    t.sel("Harbour Browser");
    t.press(Button::A);
    t.press(Button::A);
    t.press(Button::X);
    assert!(t.asked("Cancel"));
    assert!(t.app.job.as_ref().unwrap().cancelling);
    // a second press doesn't ask again
    t.press(Button::X);
    assert_eq!(t.log().iter().filter(|l| *l == "Cancel").count(), 1);
}

/// A job on a timeline, from the button to the end.
fn run_job(t: &mut T, name: &str, setup: impl FnOnce(&mut crate::demo::DemoState)) {
    t.d.with(|s| {
        s.timeline = true;
        s.job_ms = 60;
        setup(s);
    });
    t.sel(name);
    t.press(Button::A);
    t.press(Button::A);
}

fn until(t: &mut T, what: &str, f: impl Fn(&App) -> bool) {
    for _ in 0..2000 {
        t.settle();
        if f(&t.app) {
            return;
        }
    }
    panic!("never: {what}");
}

#[test]
fn a_job_that_works_goes_back_to_details_and_says_so() {
    let mut t = T::new("ends");
    run_job(&mut t, "Harbour Browser", |_| {});
    until(&mut t, "it ends", |a| a.job.is_none() && a.toast.is_some());
    assert_eq!(t.app.screen, Screen::Details);
    assert!(t.app.toast.as_ref().unwrap().0.contains("is ready"));
    until(&mut t, "the list catches up", |a| a.row.as_ref().is_some_and(|r| r.state == RowState::Installed));
}

#[test]
fn a_job_that_fails_says_why_and_shows_nothing_as_installed() {
    let mut t = T::new("fails");
    run_job(&mut t, "Harbour Browser", |s| s.fails = Some(Problem::new("no-room", "Not enough room", "It needs more.")));
    until(&mut t, "it ends", |a| a.job.is_none());
    assert!(matches!(&t.app.overlay, Some(Overlay::Report(p)) if p.code == "no-room"));
    assert_eq!(t.app.screen, Screen::Details);
    t.press(Button::A);
    assert!(t.app.overlay.is_none());
    assert_eq!(t.app.row.as_ref().unwrap().state, RowState::Available);
}

#[test]
fn cancelling_leaves_it_available() {
    let mut t = T::new("cancels");
    run_job(&mut t, "Harbour Browser", |s| s.job_ms = 3000);
    t.press(Button::X);
    until(&mut t, "it stops", |a| a.job.is_none());
    assert_eq!(t.app.screen, Screen::Details);
    assert!(t.app.toast.as_ref().unwrap().0.contains("wasn't installed"));
    assert_eq!(t.app.row.as_ref().unwrap().state, RowState::Available);
}

#[test]
fn a_refused_install_comes_straight_back() {
    let mut t = T::new("refused");
    t.sel("Harbour Browser");
    t.press(Button::A);
    // another client got in first
    t.d.with(|s| s.job = Some(JobView { id: "other".into(), phase: Phase::Downloading, ..JobView::default() }));
    t.press(Button::A);
    assert!(t.app.job.is_none() || t.app.job.as_ref().is_some_and(|j| j.id == "other"));
    assert!(matches!(&t.app.overlay, Some(Overlay::Report(p)) if p.code == "busy"));
    assert_eq!(t.app.screen, Screen::Details);
}

#[test]
fn a_job_the_service_forgot_is_not_waited_for() {
    let mut t = T::new("forgot");
    run_job(&mut t, "Harbour Browser", |s| s.job_ms = 1_000_000);
    assert_eq!(t.app.screen, Screen::Progress);
    // the service went away and came back knowing nothing, and the watcher noticed
    t.d.with(|s| s.job = None);
    t.app.forget_watcher();
    t.app.focus(true);
    until(&mut t, "it notices", |a| a.job.is_none());
    assert_eq!(t.app.screen, Screen::Details);
}

#[test]
fn a_job_going_while_the_app_opens_is_found() {
    let t = T::new("open-a");
    t.d.with(|s| {
        let mut j = JobView { repo: "mortar".into(), id: "cave-runner".into(), name: "Cave Runner II".into(), ..JobView::default() };
        j.phase = Phase::Downloading;
        s.job = Some(j);
    });
    let d = t.d.clone();
    let mut again = T { app: App::new(Setup { backend: Arc::new(DemoBackend(d.clone())), shell: Shell::None, now_secs: demo::NOW, home: t.home.clone(), reduce_motion: true, wake: bg::no_wake() }), d, home: t.home.clone(), shell: t.shell.clone() };
    again.settle();
    assert_eq!(again.app.job.as_ref().unwrap().id, "cave-runner");
    again.sel("Cave Runner II");
    again.press(Button::A);
    assert_eq!(again.app.screen, Screen::Progress);
}

#[test]
fn another_job_while_one_runs_is_refused_here() {
    let mut t = T::new("busy");
    t.sel("Harbour Browser");
    t.press(Button::A);
    t.press(Button::A);
    t.press(Button::B);
    t.sel("Cave Runner II");
    t.press(Button::A);
    t.press(Button::A);
    assert!(matches!(&t.app.overlay, Some(Overlay::Report(p)) if p.code == "busy"));
    assert_eq!(t.log().iter().filter(|l| l.starts_with("Install")).count(), 1);
}

#[test]
fn remove_takes_y_and_a_does_nothing() {
    let mut t = T::new("remove");
    t.sel("Tracker Nine");
    t.press(Button::A);
    t.press(Button::X);
    assert!(matches!(&t.app.overlay, Some(Overlay::Confirm(c)) if c.kind == ConfirmKind::Remove));
    t.press(Button::A);
    t.press(Button::A);
    assert!(matches!(&t.app.overlay, Some(Overlay::Confirm(c)) if c.nudge));
    assert!(!t.log().iter().any(|l| l.starts_with("Remove")));
    t.press(Button::B);
    assert!(t.app.overlay.is_none() && !t.log().iter().any(|l| l.starts_with("Remove")));
    t.press(Button::X);
    t.press(Button::Y);
    assert!(t.asked("Remove mortar tracker-nine"));
    assert_eq!(t.app.screen, Screen::Home);
}

#[test]
fn rollback_is_deliberate_too() {
    let mut t = T::new("rollback");
    t.sel("Tracker Nine");
    t.press(Button::A);
    t.press(Button::Y);
    assert!(matches!(&t.app.overlay, Some(Overlay::Confirm(c)) if c.kind == ConfirmKind::Rollback));
    t.press(Button::A);
    assert!(!t.log().iter().any(|l| l.starts_with("Rollback")));
    t.press(Button::Y);
    assert!(t.asked("Rollback mortar tracker-nine"));
    // no previous version: nothing to confirm
    let mut t = T::new("rollback2");
    t.sel("Harbour Browser");
    t.press(Button::A);
    t.press(Button::Y);
    assert!(t.app.overlay.is_none());
}

#[test]
fn an_update_is_an_update() {
    let mut t = T::new("update");
    t.sel("Pond Radio");
    t.press(Button::A);
    t.press(Button::A);
    assert!(t.asked("Update mortar pond-radio"));
    assert_eq!(t.app.job.as_ref().unwrap().action, Action::Update);
}

#[test]
fn needs_your_files_follows_the_files() {
    let mut t = T::new("files");
    t.sel("Cave Runner II");
    let r = t.app.selected_row().unwrap().clone();
    assert!(t.app.needs_files(&r));
    let d = t.home.join(".local/share/cave-runner");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("CAVES.DAT"), "x").unwrap();
    assert!(t.app.needs_files(&r), "one of two");
    std::fs::write(d.join("SOUND.DAT"), "x").unwrap();
    assert!(!t.app.needs_files(&r));
}

#[test]
fn open_goes_through_the_shell() {
    let mut t = T::new("open");
    t.sel("Tracker Nine");
    t.press(Button::A);
    t.press(Button::A);
    assert_eq!(t.shell.lock().unwrap().as_slice(), ["launch tracker-nine"]);
}

#[test]
fn terminal_tools_and_search_over_the_socket() {
    let mut t = T::new("tools");
    t.sel("Terminal tools");
    t.press(Button::A);
    assert_eq!(t.app.screen, Screen::Tools);
    assert_eq!(t.app.tool_rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["ledger-line"]);
    // START is the shell's: the window gets short
    t.app.resized(358);
    assert!(matches!(t.app.overlay, Some(Overlay::Find(_))));
    for c in "line".chars() {
        t.app.input(Input::Char(c));
    }
    // a letter at a time doesn't search; a pause does
    assert!(!t.log().iter().any(|l| l.starts_with("Search")));
    t.app.tick(t.app.now + 300);
    t.settle();
    assert!(t.asked("Search   line"), "{:?}", t.log());
    assert!(t.app.tool_rows.iter().any(|r| r.id == "ripline") && t.app.tool_rows.iter().any(|r| r.id == "treeline"));
    assert_eq!(t.log().iter().filter(|l| l.starts_with("Search")).count(), 1);
    // done: the keyboard goes, and the results stay as a list
    t.press(Button::A);
    assert_eq!(t.shell.lock().unwrap().as_slice(), ["press start"]);
    t.app.resized(768);
    assert!(t.app.overlay.is_none());
    assert_eq!(t.app.query, "line");
    t.press(Button::A);
    assert_eq!(t.app.screen, Screen::Details);
    assert_eq!(t.app.row.as_ref().unwrap().kind, mortar_index::Kind::Tool);
    t.press(Button::B);
    assert_eq!(t.app.screen, Screen::Tools);
    // B clears the search, then goes back
    t.press(Button::B);
    assert!(t.app.query.is_empty());
    assert_eq!(t.app.tool_rows.len(), 1);
    t.press(Button::B);
    assert_eq!(t.app.screen, Screen::Home);
}

#[test]
fn a_repository_problem_keeps_the_list_and_says_so() {
    let mut t = T::new("problem");
    t.d.with(|s| s.repos[0].problem = Some(mortar_index::Refusal::NewerFormat(2).into()));
    t.press(Button::Y);
    assert!(t.asked("Overview"));
    assert!(t.app.overview.repos[0].problem.is_some());
    assert!(!t.app.entries.is_empty());
}

#[test]
fn at_rest_there_is_nothing_to_do() {
    let mut t = T::new("rest");
    t.settle();
    t.app.toast = None;
    t.app.tick(t.app.now + 10_000);
    t.app.dirty = false;
    assert_eq!(t.app.next_deadline(), None);
    assert!(!t.app.animating());
    let before = t.log().len();
    for _ in 0..50 {
        t.app.poll();
        t.app.tick(t.app.now + 1000);
    }
    assert!(!t.app.dirty);
    assert_eq!(t.log().len(), before, "no polling the service");
}

#[test]
fn every_screen_draws() {
    // the preview's list is everything a person can see: none of them may panic
    let dir = std::env::temp_dir().join(format!("mortar-packages-screens-{}", std::process::id()));
    if mortar_ui::gfx::font_dir().join("Rubik-Medium.ttf").exists() || std::env::var_os("MORTAR_UI_FONTS").is_some() {
        crate::preview_cli::main(&["--all".to_string(), dir.to_string_lossy().into_owned()]).unwrap();
        assert!(dir.join("details.png").exists());
    }
    let _ = std::fs::remove_dir_all(&dir);
}
