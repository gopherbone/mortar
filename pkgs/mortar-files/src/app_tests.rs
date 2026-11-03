// SPDX-License-Identifier: GPL-2.0-or-later
//! The app driven the way the shell drives it: key events on the demo tree,
//! with real files changing underneath.

use crate::app::*;
use crate::keys::{self, Input};
use crate::listing::Sort;
use crate::ops_tests::temp;
use crate::shell::Shell;
use crate::{demo, view};
use mortar_ui::gfx::{Fonts, Ui, font_dir};
use mortar_ui::input::Button::{self, *};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

struct T {
    app: App,
    root: PathBuf,
    sent: Arc<Mutex<Vec<String>>>,
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn start() -> T {
    let root = temp("app");
    let env = demo::build(&root);
    let guard = demo::guard(&env);
    let sent = Arc::new(Mutex::new(Vec::new()));
    let play = Some(demo::play_config(&root));
    let mut app = App::new(Setup { env, guard, clock: demo::clock(), play, shell: Shell::Record(sent.clone()), reduce_motion: true, wake: crate::bg::no_wake() });
    let mut t = T { app: { app.dirty = false; app }, root, sent };
    t.rest();
    t
}

impl T {
    /// Until the background is done; no drawing, no slides.
    fn rest(&mut self) {
        for _ in 0..3000 {
            std::thread::sleep(Duration::from_millis(1));
            self.app.poll();
            self.app.tick(self.app.now + 5);
            self.app.slide_req = None;
            let due = self.app.next_deadline().is_some_and(|d| self.app.toast.as_ref().is_none_or(|t| d < t.1));
            if self.app.idle() && !due {
                self.app.poll();
                return;
            }
        }
        panic!("the app never came to rest");
    }
    fn press(&mut self, b: Button) {
        for (c, d) in keys::pad_events(b) {
            self.app.key(c, d, false);
        }
        self.rest();
    }
    fn presses(&mut self, bs: &[Button]) {
        for b in bs {
            self.press(*b);
        }
    }
    fn place(&mut self, name: &str) {
        self.app.place_sel = self.app.places.iter().position(|p| p.name == name).unwrap();
        self.press(A);
    }
    fn sel(&mut self, name: &str) {
        let i = self.app.entries.iter().position(|e| e.display == name).unwrap_or_else(|| panic!("no {name}"));
        while self.app.sel > i {
            self.press(Up);
        }
        while self.app.sel < i {
            self.press(Down);
        }
    }
    fn act(&mut self, a: Act) {
        self.press(X);
        let Some(Overlay::Actions { rows, .. }) = &self.app.overlay else { panic!("no actions") };
        let i = rows.iter().position(|r| *r == a).unwrap_or_else(|| panic!("{a:?} isn't offered: {rows:?}"));
        for _ in 0..i {
            self.press(Down);
        }
        self.press(A);
    }
    fn toast(&self) -> String {
        self.app.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default()
    }
    fn exists(&self, p: &str) -> bool {
        fs::symlink_metadata(self.root.join(p)).is_ok()
    }
}

#[test]
fn starts_on_places_and_b_comes_back() {
    let mut t = start();
    assert!(matches!(t.app.screen, Screen::Places));
    assert_eq!(t.app.places[0].name, "Home");
    t.press(A);
    assert!(matches!(t.app.screen, Screen::Browse));
    assert_eq!(t.app.dir, t.root.join("home"));
    t.sel("Documents");
    t.press(A);
    assert_eq!(t.app.dir, t.root.join("home/Documents"));
    t.press(B);
    assert_eq!(t.app.dir, t.root.join("home"));
    assert_eq!(t.app.selected().unwrap().display, "Documents", "the cursor comes back to where you were");
    t.press(B);
    assert!(matches!(t.app.screen, Screen::Places));
    t.press(A);
    assert_eq!(t.app.selected().unwrap().display, "Documents", "and remembers across visits");
}

#[test]
fn nothing_runs_at_rest() {
    let mut t = start();
    t.place("Home");
    t.sel("Pictures");
    t.app.tick(t.app.now + 10_000);
    t.app.dirty = false;
    assert!(!t.app.animating());
    assert_eq!(t.app.next_deadline(), None, "no timer, so poll() sleeps until something happens");
    assert!(!t.app.poll());
    assert!(!t.app.dirty, "no frame");
}

// ---------------------------------------------------------------- delete ----

#[test]
fn delete_needs_y_not_a_second_a() {
    let mut t = start();
    t.place("Home");
    t.sel("today.txt");
    t.act(Act::Delete);
    assert!(matches!(t.app.overlay, Some(Overlay::Confirm(_))));
    // the same button that got us here, many times
    t.presses(&[A, A, A, X, L1, R1, Up, Down, Select]);
    assert!(t.exists("home/today.txt"), "nothing happened");
    assert!(matches!(t.app.overlay, Some(Overlay::Confirm(_))));
    t.press(B);
    assert!(t.app.overlay.is_none());
    assert!(t.exists("home/today.txt"));
    // and with Y, it goes to the Trash, and can come back
    t.act(Act::Delete);
    t.press(Y);
    assert!(!t.exists("home/today.txt"));
    assert_eq!(t.toast(), "In the Trash: 1 item.");
    assert!(t.exists("home/.local/share/Trash/files/today.txt"));
    assert!(t.exists("home/.local/share/Trash/info/today.txt.trashinfo"));
    t.press(L2);
    t.place("Trash");
    assert_eq!(t.app.selected().unwrap().display, "today.txt");
    t.act(Act::Restore);
    t.rest();
    assert!(t.exists("home/today.txt"));
    assert!(!t.exists("home/.local/share/Trash/files/today.txt"));
}

#[test]
fn delete_says_what_and_how_many() {
    let mut t = start();
    t.place("Games");
    t.sel("nes");
    t.press(A);
    t.presses(&[Y, Y, Y]);
    assert_eq!(t.app.marks.len(), 3);
    t.act(Act::Delete);
    let Some(Overlay::Confirm(c)) = &t.app.overlay else { panic!() };
    assert_eq!(c.sources.len(), 3);
    assert_eq!(c.names, ["Cave Runner II.nes", "Hexagon Harvest.nes", "Lantern Road.nes"]);
    assert!(!c.forever);
}

#[test]
fn forever_takes_both_shoulders_held() {
    let mut t = start();
    t.place("Home");
    t.sel("today.txt");
    t.act(Act::Delete);
    t.press(Y);
    t.press(L2);
    t.place("Trash");
    let f = "home/.local/share/Trash/files/today.txt";
    t.act(Act::DeleteForever);
    let down = |t: &mut T, c: u16| t.app.key(c, true, false);
    let up = |t: &mut T, c: u16| t.app.key(c, false, false);
    // Y, and one shoulder, do nothing
    t.press(Y);
    down(&mut t, keys::PAGEUP);
    t.app.tick(t.app.now + HOLD_MS * 2);
    assert!(t.exists(f));
    // both, but let go early
    down(&mut t, keys::PAGEDOWN);
    t.app.tick(t.app.now + HOLD_MS / 2);
    up(&mut t, keys::PAGEDOWN);
    t.app.tick(t.app.now + HOLD_MS * 2);
    t.rest();
    assert!(t.exists(f), "let go too soon");
    // losing focus forgets a hold (the MENU opened)
    down(&mut t, keys::PAGEDOWN);
    t.app.focus(false);
    t.app.focus(true);
    t.app.tick(t.app.now + HOLD_MS * 2);
    t.rest();
    assert!(t.exists(f));
    // held for the whole time
    down(&mut t, keys::PAGEUP);
    down(&mut t, keys::PAGEDOWN);
    assert!(t.app.animating(), "the hold draws its tiles");
    t.app.tick(t.app.now + HOLD_MS - 10);
    assert!(t.exists(f));
    t.app.tick(t.app.now + 20);
    up(&mut t, keys::PAGEUP);
    up(&mut t, keys::PAGEDOWN);
    t.rest();
    assert!(!t.exists(f));
    assert!(!t.exists("home/.local/share/Trash/info/today.txt.trashinfo"));
    assert_eq!(t.toast(), "Deleted 1 item.");
}

#[test]
fn the_places_themselves_are_refused_before_any_confirm() {
    let mut t = start();
    t.place("Everything");
    t.sel("play");
    t.press(A);
    t.sel("roms");
    t.act(Act::Delete);
    assert!(t.app.overlay.is_none());
    assert_eq!(t.toast(), "Mortar needs that folder.");
    assert!(t.exists("play/roms"));
}

// ------------------------------------------------------------ copy, move ----

#[test]
fn copy_from_one_place_put_down_in_another() {
    let mut t = start();
    t.place("Saves");
    t.sel("gba");
    t.press(A);
    t.press(Y);
    t.press(Y);
    t.act(Act::Copy);
    assert!(t.app.carry.is_some());
    assert!(t.app.marks.is_empty());
    t.press(L2);
    t.place("Home");
    t.sel("Downloads");
    t.press(A);
    t.press(Y);
    t.rest();
    assert!(t.app.carry.is_none());
    assert!(t.exists("home/Downloads/Crystal Orchard.sav"));
    assert!(t.exists("home/Downloads/Ember Knights.sav"));
    assert!(t.exists("play/saves/gba/Crystal Orchard.sav"));
    assert_eq!(t.app.selected().unwrap().display, "Crystal Orchard.sav", "the new thing is selected");
    assert_eq!(t.toast(), "Copied 2 items.");
}

#[test]
fn a_clash_stops_for_a_choice_and_a_stray_a_changes_nothing() {
    let mut t = start();
    fs::write(t.root.join("home/Downloads/today.txt"), "other").unwrap();
    t.place("Home");
    t.sel("today.txt");
    t.act(Act::Copy);
    t.sel("Downloads");
    t.press(A);
    // don't rest: the job waits for the answer
    for (c, d) in keys::pad_events(Y) {
        t.app.key(c, d, false);
    }
    for _ in 0..2000 {
        std::thread::sleep(Duration::from_millis(1));
        t.app.poll();
        if t.app.job.as_ref().is_some_and(|j| j.clash.is_some()) {
            break;
        }
    }
    let j = t.app.job.as_ref().unwrap();
    assert_eq!(j.clash.as_ref().unwrap().clash.name, "today.txt");
    assert!(t.app.job_visible());
    // the first choice is Skip
    t.press(A);
    t.rest();
    assert!(t.app.job.is_none());
    assert_eq!(fs::read(t.root.join("home/Downloads/today.txt")).unwrap(), b"other");
    assert_eq!(t.toast(), "Nothing changed.");
}

#[test]
fn a_job_can_be_stopped_with_b() {
    let mut t = start();
    let big = t.root.join("home/Big");
    fs::create_dir(&big).unwrap();
    for i in 0..400 {
        fs::write(big.join(format!("{i}")), vec![1u8; 200_000]).unwrap();
    }
    t.place("Home");
    t.sel("Big");
    t.act(Act::Copy);
    t.sel("Downloads");
    t.press(A);
    for (c, d) in keys::pad_events(Y) {
        t.app.key(c, d, false);
    }
    assert!(t.app.job.is_some());
    assert!(!t.app.job_visible(), "a quick job never shows the screen");
    t.app.tick(t.app.now + 400);
    assert!(t.app.job_visible());
    for (c, d) in keys::pad_events(B) {
        t.app.key(c, d, false);
    }
    t.rest();
    assert!(t.app.job.is_none());
    // whichever way the race went, nothing is half-way
    let parts = fs::read_dir(t.root.join("home/Downloads")).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with(".mortar-part")).count();
    assert_eq!(parts, 0);
    assert_eq!(fs::read_dir(&big).unwrap().count(), 400);
    if t.exists("home/Downloads/Big") {
        assert_eq!(fs::read_dir(t.root.join("home/Downloads/Big")).unwrap().count(), 400);
    }
}

#[test]
fn move_is_put_down_too() {
    let mut t = start();
    t.place("Home");
    t.sel("today.txt");
    t.act(Act::Move);
    t.sel("Documents");
    t.press(A);
    t.press(Y);
    t.rest();
    assert!(!t.exists("home/today.txt"));
    assert!(t.exists("home/Documents/today.txt"));
    assert_eq!(t.toast(), "Moved 1 item.");
}

#[test]
fn leaving_a_folder_clears_marks_and_says_so() {
    let mut t = start();
    t.place("Home");
    t.presses(&[Y, Y]);
    assert_eq!(t.app.marks.len(), 2);
    t.press(B);
    t.place("Home");
    assert!(t.app.marks.is_empty());
    t.presses(&[Y]);
    t.sel("Music");
    t.press(A);
    assert!(t.app.marks.is_empty());
    assert_eq!(t.toast(), "Cleared 1 mark");
}

// ------------------------------------------------------------- read-only ----

#[test]
fn read_only_folders_offer_only_what_they_can_do() {
    let mut t = start();
    t.place("Everything");
    t.sel("nix");
    t.press(A);
    assert!(t.app.dir_read_only());
    t.press(X);
    let Some(Overlay::Actions { rows, .. }) = &t.app.overlay else { panic!() };
    assert!(rows.contains(&Act::Copy));
    for no in [Act::Move, Act::Rename, Act::Delete, Act::NewFolder] {
        assert!(!rows.contains(&no), "{no:?}");
    }
    t.press(B);
    // something in hand can't be put down here
    t.press(L2);
    t.place("Home");
    t.sel("today.txt");
    t.act(Act::Copy);
    t.press(L2);
    t.place("Everything");
    t.sel("nix");
    t.press(A);
    t.press(Y);
    assert_eq!(t.toast(), "This place is read-only.");
    assert!(!t.exists("nix/today.txt"));
    assert!(t.app.carry.is_some(), "still in hand");
}

// ---------------------------------------------------------------- naming ----

#[test]
fn rename_opens_the_keyboard_types_and_closes_it() {
    let mut t = start();
    t.place("Home");
    t.sel("today.txt");
    t.act(Act::Rename);
    assert_eq!(*t.sent.lock().unwrap(), ["press start"], "the shell's keyboard, opened for you");
    t.app.resized(358);
    let Some(Overlay::Naming(n)) = &t.app.overlay else { panic!() };
    assert_eq!(n.field.cursor, 5, "before the extension");
    // the OSK types: two more letters, and delete one
    for c in ['-', 'x', 'y'] {
        t.app.input(Input::Char(c));
    }
    t.press(B);
    // the OSK's Y is Enter
    t.press(A);
    assert_eq!(t.sent.lock().unwrap().len(), 2, "and closed again");
    t.app.resized(768);
    t.rest();
    assert!(t.exists("home/today-x.txt"));
    assert_eq!(t.app.selected().unwrap().display, "today-x.txt");
}

#[test]
fn a_taken_name_is_said_in_place() {
    let mut t = start();
    t.place("Home");
    t.sel("today.txt");
    t.act(Act::Rename);
    t.app.resized(358);
    t.app.input(Input::End);
    for _ in 0.."today.txt".len() {
        t.press(B);
    }
    for c in "Music".chars() {
        t.app.input(Input::Char(c));
    }
    t.press(A);
    let Some(Overlay::Naming(n)) = &t.app.overlay else { panic!("should still be open") };
    assert_eq!(n.error.as_deref(), Some("Something with that name is already there."));
    assert!(t.exists("home/today.txt"));
    // L2 walks away
    t.press(L2);
    assert!(t.app.overlay.is_none());
    assert!(t.exists("home/today.txt"));
}

#[test]
fn an_unchanged_name_does_nothing() {
    let mut t = start();
    t.place("Home");
    t.sel("today.txt");
    t.act(Act::Rename);
    t.press(A);
    assert!(t.app.overlay.is_none());
    assert!(t.exists("home/today.txt"));
}

#[test]
fn new_folder() {
    let mut t = start();
    t.place("Home");
    t.act(Act::NewFolder);
    t.press(A);
    t.rest();
    assert!(t.exists("home/New folder"));
    assert_eq!(t.app.selected().unwrap().display, "New folder");
    // a second one: told, not replaced
    t.act(Act::NewFolder);
    t.press(A);
    assert!(matches!(t.app.overlay, Some(Overlay::Naming(_))));
}

#[test]
fn start_from_the_list_finds_by_name() {
    let mut t = start();
    t.place("Games");
    t.sel("nes");
    t.press(A);
    t.app.resized(358);
    assert!(matches!(t.app.overlay, Some(Overlay::Find(_))));
    for c in "qu".chars() {
        t.app.input(Input::Char(c));
    }
    assert_eq!(t.app.selected().unwrap().display, "Quiet Station.nes");
    t.press(A);
    assert!(t.app.overlay.is_none());
    assert!(t.sent.lock().unwrap().contains(&"press start".to_string()));
}

// ------------------------------------------------------------------ open ----

#[test]
fn a_game_goes_to_the_shell() {
    let mut t = start();
    t.place("Games");
    t.sel("nes");
    t.press(A);
    t.sel("Moon Miner.nes");
    t.press(A);
    let want = format!("game nes {}", t.root.join("play/roms/nes/Moon Miner.nes").display());
    assert_eq!(*t.sent.lock().unwrap(), [want]);
}

#[test]
fn what_cant_be_opened_says_so() {
    let mut t = start();
    t.place("Home");
    t.sel("Music");
    t.press(A);
    t.sel("mixdown.wav");
    t.press(A);
    assert_eq!(t.toast(), "Nothing here can open .wav files.");
    assert!(matches!(t.app.screen, Screen::Browse));
}

#[test]
fn a_picture_and_text_open_full_screen() {
    let mut t = start();
    t.place("Home");
    t.sel("Pictures");
    t.press(A);
    t.sel("Lime hills.png");
    t.press(A);
    assert!(matches!(&t.app.screen, Screen::Image { pic: Some(Ok(_)), .. }));
    t.press(Right);
    assert_eq!(t.app.selected().unwrap().display, "Moon over water.png");
    t.press(B);
    assert!(matches!(t.app.screen, Screen::Browse));
    t.press(L2);
    t.place("Home");
    t.press(B);
    t.place("Home");
    t.sel("today.txt");
    t.press(A);
    let Screen::Text { lines: Some(Ok(l)), .. } = &t.app.screen else { panic!("no text") };
    assert_eq!(l[0], "Back up the saves.");
    t.press(B);
    assert!(matches!(t.app.screen, Screen::Browse));
}

#[test]
fn a_folder_that_cant_be_read_is_a_message() {
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let mut t = start();
    let locked = t.root.join("home/Locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0)).unwrap();
    t.place("Home");
    t.sel("Locked");
    t.press(A);
    assert_eq!(t.app.dir, t.root.join("home"), "you stay where you are");
    assert_eq!(t.toast(), "You don't have permission.");
    fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
}

#[test]
fn a_folder_removed_underneath_goes_up() {
    let mut t = start();
    t.place("Home");
    t.sel("Downloads");
    t.press(A);
    fs::remove_dir_all(t.root.join("home/Downloads")).unwrap();
    t.app.focus(false);
    t.app.focus(true);
    t.rest();
    assert_eq!(t.app.dir, t.root.join("home"));
}

// ------------------------------------------------------------- big lists ----

#[test]
fn thousands_of_entries() {
    let mut t = start();
    let big = t.root.join("home/Big");
    fs::create_dir(&big).unwrap();
    for i in 0..5000 {
        fs::write(big.join(format!("track {i}.flac")), "").unwrap();
    }
    t.place("Home");
    t.sel("Big");
    t.press(A);
    assert_eq!(t.app.entries.len(), 5000);
    t.press(R1);
    assert_eq!(t.app.sel, 6);
    t.press(L1);
    t.press(L1);
    assert_eq!(t.app.sel, 0, "stops at the top");
    for _ in 0..3 {
        t.press(Button::R2);
    }
    assert_eq!(t.app.sort, Sort::Name, "R2 goes round");
    assert_eq!(t.app.entries[9].display, "track 9.flac");
    assert_eq!(t.app.entries[10].display, "track 10.flac");
}

#[test]
fn hidden_files_are_a_toggle() {
    let mut t = start();
    t.place("Home");
    assert!(!t.app.entries.iter().any(|e| e.display == ".profile"));
    t.press(X);
    let Some(Overlay::Actions { rows, sel }) = &mut t.app.overlay else { panic!() };
    *sel = rows.iter().position(|r| *r == Act::Hidden).unwrap();
    t.press(A);
    assert!(t.app.entries.iter().any(|e| e.display == ".profile"));
    assert!(matches!(t.app.overlay, Some(Overlay::Actions { .. })), "stays open to see it change");
}

#[test]
fn storage_from_the_places_and_of_a_folder() {
    let mut t = start();
    t.place("Storage");
    assert!(matches!(t.app.screen, Screen::Storage));
    assert!(t.app.levels.last().unwrap().tiles.iter().any(|x| x.name == "Free"));
    t.press(B);
    assert!(matches!(t.app.screen, Screen::Places));
    // SELECT: what's using the space in this folder; A goes in, B comes out
    t.place("Home");
    t.press(Select);
    assert!(matches!(t.app.screen, Screen::Storage));
    let l = t.app.levels.last().unwrap();
    let i = l.tiles.iter().position(|x| x.name == "Pictures").unwrap();
    assert!(l.tiles.iter().any(|x| x.name == "today.txt"));
    assert_eq!(l.tiles[i].tint, Some(crate::listing::Kind::Image), "mostly pictures, so it looks it");
    t.app.levels.last_mut().unwrap().sel = i;
    t.press(A);
    assert_eq!(t.app.levels.len(), 2);
    t.press(B);
    t.press(B);
    assert!(matches!(t.app.screen, Screen::Browse));
    assert_eq!(t.app.dir, t.root.join("home"));
}

// --------------------------------------------------------------- drawing ----

#[test]
fn every_screen_draws_with_odd_names_and_in_both_palettes() {
    let Ok(fonts) = Fonts::load(&font_dir()) else {
        eprintln!("no fonts here: not drawing");
        return;
    };
    let mut ui = Ui::new(1024, 768, fonts);
    let mut t = start();
    let dir = t.root.join("home/Odd");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join(format!("{}.gba", "long name ".repeat(24).trim())), "x").unwrap();
    fs::write(dir.join("with\nnewline"), "x").unwrap();
    fs::write(dir.join("ünï çødé 日本語 🎮.txt"), "héllo").unwrap();
    use std::os::unix::ffi::OsStringExt;
    let _ = fs::write(dir.join(std::ffi::OsString::from_vec(vec![b'x', 0xff, 0xfe])), "x");
    let draw = |t: &mut T, ui: &mut Ui| {
        t.app.dirty = false;
        view::draw(&mut t.app, ui);
    };
    t.place("Home");
    t.sel("Odd");
    t.press(A);
    for (i, _) in t.app.entries.clone().iter().enumerate() {
        t.app.sel = i;
        t.app.refresh_preview();
        t.rest();
        draw(&mut t, &mut ui);
    }
    t.press(X);
    draw(&mut t, &mut ui);
    t.press(Down);
    t.press(B);
    t.press(L2);
    draw(&mut t, &mut ui);
    t.place("Storage");
    draw(&mut t, &mut ui);
    // a slide: the first frame is the old picture, the last the new one
    let mut small = Ui::new(1024, 358, Fonts::load(&font_dir()).unwrap());
    t.press(L2);
    t.place("Home");
    t.app.resized(358);
    draw(&mut t, &mut small);
    let before = ui.pm.clone();
    t.app.resized(768);
    t.press(L2);
    t.app.reduce_motion = false;
    t.app.place_sel = 0;
    t.press(A);
    t.app.slide_req = Some(SlideDir::Forward);
    draw(&mut t, &mut ui);
    assert!(t.app.animating());
    assert_eq!(ui.pm.data(), before.data(), "starts from what was on screen");
    t.app.tick(t.app.now + 1000);
    draw(&mut t, &mut ui);
    assert!(t.app.slide.is_none());
    assert!(!t.app.animating());
}

#[test]
fn long_names_keep_their_ends() {
    let Ok(fonts) = Fonts::load(&font_dir()) else { return };
    let ui = Ui::new(1024, 768, fonts);
    let st = mortar_ui::gfx::TextStyle::new(mortar_ui::gfx::Face::Body600, 29.0, crate::mosaic::DARK);
    let s = "The Long Way Round (Europe) (Rev 1) (Translated) (Extended Edition).nes";
    let f = view::fit_name(&ui, s, &st, 400.0);
    assert!(f.starts_with("The Long"), "{f}");
    assert!(f.ends_with("on).nes"), "{f}");
    assert!(ui.measure(&f, &st) <= 400.0);
    assert_eq!(view::fit_name(&ui, "short", &st, 400.0), "short");
    let lines = view::wrap(&ui, &"x".repeat(400), &st, 300.0, 3);
    assert_eq!(lines.len(), 3);
}
