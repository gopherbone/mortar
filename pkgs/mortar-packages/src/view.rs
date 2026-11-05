// SPDX-License-Identifier: GPL-2.0-or-later
//! Drawing, in Files' language and with its helpers. One function per screen, all from
//! `App`'s state: solid blocks on the ground, a band for what's selected, the panel
//! beside it in the same colour.

use crate::app::*;
use crate::protocol::*;
use mortar_files::app::{H, HINT_H, Slide, W};
use mortar_files::listing::Kind as FileKind;
use mortar_files::mosaic::{self, DARK, Glyph, PINK, kind_colors};
use mortar_files::view::{
    ART, ART_X, ART_Y, BAND_GAP, BAND_H, BAND_SHADOW, BAND_Y, LIST_BOTTOM, PANEL_X, ROW_H, TITLE_H, body, cap, cap_ink, compose, draw_field, fit_name, hints, mono, regular, row_y, serif, tile, wrap,
};
use mortar_index::Kind;
use mortar_ui::gfx::{Color, Rect, Ui};
use mortar_ui::icons::Icon;
use mortar_ui::theme;

/// The Mortar UI icon a package names, or the pad that apps default to.
fn icon_of(r: &Row) -> Icon {
    serde_json::from_value(serde_json::Value::String(r.icon.clone())).unwrap_or(if r.kind == Kind::Tool { Icon::Term } else { Icon::Pad })
}

/// What a package is: the colour of its band, its panel and its tiles.
fn colors(r: &Row) -> (Color, Color) {
    let t = theme::get();
    if r.state == RowState::Update {
        return kind_colors(FileKind::Save);
    }
    if r.state == RowState::NotYet {
        return (t.ground_2, t.line);
    }
    match (r.kind, r.category.as_str()) {
        (Kind::Tool, _) => (PINK, DARK),
        (_, "games") => kind_colors(FileKind::Game),
        (_, "music") => kind_colors(FileKind::Audio),
        _ => kind_colors(FileKind::Folder),
    }
}

/// The colour of its art, in Files' kinds.
fn art_kinds(r: &Row) -> Vec<(FileKind, usize)> {
    let seed = r.id.bytes().fold(7usize, |a, b| a.wrapping_mul(31).wrapping_add(b as usize));
    let main = match (r.kind, r.category.as_str()) {
        (Kind::Tool, _) => FileKind::Image,
        (_, "games") => FileKind::Game,
        (_, "music") => FileKind::Audio,
        _ => FileKind::Folder,
    };
    vec![(main, 70 + seed % 90), (FileKind::Save, 8 + seed % 30), (FileKind::Other, 6 + seed % 24)]
}

fn gib_or_mib(done: u64, total: u64) -> String {
    const MIB: f64 = 1048576.0;
    if total as f64 >= 1024.0 * MIB {
        format!("{:.1} of {:.1} GB", done as f64 / (1024.0 * MIB), total as f64 / (1024.0 * MIB))
    } else {
        format!("{} of {} MB", (done as f64 / MIB).round() as u64, (total as f64 / MIB).ceil().max(1.0) as u64)
    }
}

fn size_word(b: u64) -> String {
    const MIB: f64 = 1048576.0;
    if b as f64 >= 1024.0 * MIB {
        format!("{:.1} GB", b as f64 / (1024.0 * MIB))
    } else {
        format!("{} MB", (b as f64 / MIB).ceil().max(1.0) as u64)
    }
}

/// A row's right-hand word: where it stands.
fn row_value(app: &App, r: &Row) -> String {
    if let Some(j) = app.job_for(r) {
        return match j.phase {
            Phase::Downloading if j.total > 0 => gib_or_mib(j.done, j.total),
            Phase::Installing => "Nearly there".into(),
            _ => "Starting".into(),
        };
    }
    match r.state {
        RowState::Update => "Update".into(),
        RowState::NotYet => "Not yet".into(),
        RowState::Installed | RowState::Elsewhere if app.needs_files(r) => "needs your files".into(),
        RowState::Installed | RowState::Elsewhere => "Installed".into(),
        RowState::Available if !r.requires.is_empty() && app.needs_files(r) => "needs your files".into(),
        RowState::Available => r.version.clone(),
    }
}

// ------------------------------------------------------------------ top ----

pub fn draw(app: &mut App, ui: &mut Ui) {
    if let Some(dir) = app.slide_req.take() {
        let from = ui.pm.clone();
        render(app, ui);
        let to = ui.pm.clone();
        app.slide = Some(Slide { dir, start: app.now, from, to });
    } else if app.slide_done() {
        app.slide = None;
        render(app, ui);
        return;
    } else if app.slide.is_none() {
        render(app, ui);
        return;
    }
    let p = app.slide_progress().unwrap_or(1.0);
    if let Some(s) = &app.slide {
        compose(ui, s, p);
    }
}

fn render(app: &App, ui: &mut Ui) {
    let t = theme::get();
    ui.clear(t.ground);
    match &app.overlay {
        Some(Overlay::Confirm(c)) => return draw_confirm(ui, c),
        Some(Overlay::Report(p)) => return draw_report(ui, p),
        Some(Overlay::Find(f)) => return draw_find(app, ui, f),
        None => {}
    }
    match app.screen {
        Screen::Home => draw_home(app, ui),
        Screen::Tools => draw_tools(app, ui),
        Screen::Details => draw_details(app, ui),
        Screen::Progress => draw_progress(app, ui),
    }
    if let Some((s, _)) = &app.toast {
        draw_toast(ui, s);
    }
}

fn draw_toast(ui: &mut Ui, s: &str) {
    let t = theme::get();
    let st = body(26.0, DARK);
    let lines = wrap(ui, s, &st, 760.0, 2);
    let w = lines.iter().map(|l| ui.measure(l, &st)).fold(0.0, f32::max) + 52.0;
    let h = lines.len() as f32 * 36.0 + 36.0;
    let r = Rect::new(24.0, 20.0, w, h);
    ui.fill_block(Rect::new(r.x + 10.0, r.y + 10.0, r.w, r.h), DARK);
    ui.fill_block(r, t.yellow);
    ui.outline(r, 3.0, DARK);
    for (i, l) in lines.iter().enumerate() {
        ui.text(r.x + 26.0, r.y + 16.0 + i as f32 * 36.0, l, &st);
    }
}

// ----------------------------------------------------------------- lists ----

/// A small tile with the package's icon on it: as Files' kinds, in the shell's icons.
fn icon_tile(ui: &mut Ui, x: f32, y: f32, s: f32, bg: Color, ink: Color, icon: Icon) {
    ui.fill_block(Rect::new(x, y, s, s), bg);
    let pad = (s * 0.14).round();
    ui.icon(icon, x + pad, y + pad, s - 2.0 * pad, ink, 2.4, false);
}

/// A check on a tile. Lime on ink by day; at night ink is pale, and lime is the tile.
fn installed_tile(ui: &mut Ui, x: f32, y: f32, s: f32) {
    let t = theme::get();
    if t.name == "dark" {
        tile(ui, x, y, s, t.lime, DARK, Glyph::Check);
    } else {
        tile(ui, x, y, s, t.line, t.lime, Glyph::Check);
    }
}

fn draw_title(ui: &mut Ui, name: &str, parent: Option<&str>, max_right: f32) {
    let t = theme::get();
    let name_st = serif(60.0, t.line);
    let crumb_st = regular(26.0, t.line_soft);
    let cy = TITLE_H / 2.0 + 4.0;
    let mut x = 28.0;
    if let Some(p) = parent {
        x += ui.text_vcenter(x, cy, p, &crumb_st);
        mortar_files::view::triangle(ui, x + 13.0, cy, 6.0, '▶', t.line_soft);
        x += 28.0;
    }
    let fit = fit_name(ui, name, &name_st, max_right - x);
    ui.text_vcenter(x, cy - 2.0, &fit, &name_st);
}

/// A block by the title, as Files shows what's marked: something worth a glance.
fn status_tag(ui: &mut Ui, label: &str, fill: Color) -> f32 {
    let t = theme::get();
    let st = body(21.0, DARK);
    let w = ui.measure(label, &st).min(352.0) + 30.0;
    let r = Rect::new(PANEL_X - 30.0 - w, 24.0, w, 46.0);
    ui.fill_block(Rect::new(r.x + 6.0, r.y + 6.0, r.w, r.h), t.line);
    ui.fill_block(r, fill);
    ui.outline(r, 3.0, t.line);
    ui.text_fit(r.x + 15.0, r.y + 10.0, label, &st, 352.0);
    w + 30.0
}

fn problem_tag(app: &App) -> Option<(String, Color)> {
    let t = theme::get();
    if app.checking {
        return Some(("Checking…".into(), t.lime));
    }
    let p = app.overview.repos.iter().find_map(|r| r.problem.as_ref())?;
    let short = if p.code == "newer-format" { p.headline.clone() } else if p.code == "offline" { "Offline".to_string() } else { "List not updated".to_string() };
    Some((short, if p.code == "offline" { t.ground_2 } else { t.yellow }))
}

fn draw_row(app: &App, ui: &mut Ui, r: &Row, y: f32, right: f32) {
    let t = theme::get();
    let cy = y + ROW_H / 2.0;
    let s = 48.0;
    let (bg, ink) = colors(r);
    if r.state == RowState::Installed {
        installed_tile(ui, 28.0, cy - s / 2.0, s);
    } else {
        icon_tile(ui, 28.0, cy - s / 2.0, s, bg, ink, icon_of(r));
    }
    let val = row_value(app, r);
    let val_st = mono(22.0, t.line_soft);
    let vw = ui.measure(&val, &val_st);
    ui.text_vcenter(right - vw, cy, &val, &val_st);
    let st = body(29.0, t.line);
    let fit = fit_name(ui, &r.name, &st, right - vw - 26.0 - 96.0);
    ui.text_vcenter(96.0, cy, &fit, &st);
}

fn draw_tools_row(ui: &mut Ui, count: u64, y: f32, right: f32) {
    let t = theme::get();
    let cy = y + ROW_H / 2.0;
    icon_tile(ui, 28.0, cy - 24.0, 48.0, PINK, DARK, Icon::Term);
    let val = count.to_string();
    let val_st = mono(22.0, t.line_soft);
    let vw = ui.measure(&val, &val_st);
    ui.text_vcenter(right - 22.0 - vw, cy, &val, &val_st);
    mortar_files::view::triangle(ui, right - 8.0, cy, 7.0, '▶', t.line_soft);
    ui.text_vcenter(96.0, cy, "Terminal tools", &body(29.0, t.line));
}

/// The selected row: the shell's band, from the screen's edge into the panel.
fn draw_band(ui: &mut Ui, name: &str, icon: Icon, installed: bool, bg: Color, ink: Color) {
    let t = theme::get();
    ui.fill_block(Rect::new(0.0, BAND_Y + BAND_H, PANEL_X, BAND_SHADOW), t.line);
    ui.fill_block(Rect::new(0.0, BAND_Y, PANEL_X, BAND_H), bg);
    let s = 64.0;
    let (tx, ty) = (24.0, BAND_Y + (BAND_H - s) / 2.0);
    if installed {
        installed_tile(ui, tx, ty, s);
    } else {
        icon_tile(ui, tx, ty, s, t.ground, t.line, icon);
    }
    let st = serif(58.0, ink);
    let fit = fit_name(ui, name, &st, PANEL_X - 112.0 - 18.0);
    ui.text_vcenter(112.0, BAND_Y + BAND_H / 2.0 - 2.0, &fit, &st);
}

fn covers(ui: &mut Ui) {
    let t = theme::get();
    // the title and the hint row sit over rows on their way out
    ui.fill_block(Rect::new(0.0, 0.0, PANEL_X, BAND_Y - 2.0 * ROW_H - 2.0), t.ground);
    ui.fill_block(Rect::new(0.0, LIST_BOTTOM, PANEL_X, HINT_H), t.ground);
}

fn empty_body(ui: &mut Ui, title: &str, sub: &str) {
    let t = theme::get();
    let st = serif(58.0, t.line);
    let mut y = BAND_Y - 24.0;
    for l in wrap(ui, title, &st, PANEL_X - 56.0, 3) {
        ui.text(28.0, y, &l, &st);
        y += st.line_px();
    }
    y += 10.0;
    let d = regular(26.0, t.line_soft);
    for l in wrap(ui, sub, &d, PANEL_X - 60.0, 6) {
        ui.text(30.0, y, &l, &d);
        y += d.line_px();
    }
}

fn draw_home(app: &App, ui: &mut Ui) {
    let t = theme::get();
    let right = PANEL_X - 20.0;
    let pos = app.scroll_pos(&app.home_list);
    let n = app.entries.len();
    let first = (pos - 4.0).floor().max(0.0) as usize;
    let last = ((pos + 7.0).ceil() as usize).min(n);
    for i in first..last {
        let y = row_y(i as f32 - pos);
        if y + ROW_H < TITLE_H - 8.0 || y > LIST_BOTTOM {
            continue;
        }
        match &app.entries[i] {
            Entry::Package(r) => draw_row(app, ui, r, y, right),
            Entry::Tools { count, .. } => draw_tools_row(ui, *count, y, right),
        }
    }
    covers(ui);
    let tag_w = problem_tag(app).map(|(l, c)| status_tag(ui, &l, c)).unwrap_or(0.0);
    draw_title(ui, "Packages", None, PANEL_X - 24.0 - tag_w);

    let (ink_hint, on) = (cap_ink(), t.ground);
    match app.entries.get(app.home_list.sel) {
        Some(Entry::Package(r)) => {
            let (bg, ink) = colors(r);
            draw_band(ui, &r.name, icon_of(r), r.state == RowState::Installed, bg, ink);
            draw_panel(app, ui, r, app.home_list.sel, n);
            let a = if app.job_for(r).is_some() { "Progress" } else { "Details" };
            hints(ui, 28.0, &[("A", a), ("Y", "Check again"), ("START", "Search")], ink_hint, on);
        }
        Some(Entry::Tools { count, installed }) => {
            draw_band(ui, "Terminal tools", Icon::Term, false, PINK, DARK);
            draw_tools_panel(ui, *count, *installed, app.home_list.sel, n);
            hints(ui, 28.0, &[("A", "Open"), ("Y", "Check again"), ("START", "Search")], ink_hint, on);
        }
        None => {
            let (title, sub) = home_empty(app);
            empty_body(ui, &title, &sub);
            ui.fill_block(Rect::new(PANEL_X, 0.0, W - PANEL_X, H), t.ground_2);
            let (x, y) = (ART_X + 64.0, ART_Y + 64.0);
            ui.icon(Icon::Box, x, y, 192.0, t.line, 1.6, false);
            hints(ui, 28.0, &[("Y", "Check again")], ink_hint, on);
        }
    }
}

fn home_empty(app: &App) -> (String, String) {
    if !app.loaded {
        return ("Reading…".into(), String::new());
    }
    if let Some(e) = &app.unreachable {
        return ("Can't reach the service.".into(), e.clone());
    }
    if app.overview.repos.is_empty() {
        return ("Nothing to follow.".into(), "This system doesn't follow a repository, so there's nothing to list.".into());
    }
    if let Some(p) = app.overview.repos.iter().find_map(|r| r.problem.as_ref()) {
        return (p.headline.clone(), p.detail.clone());
    }
    if app.checking {
        return ("Checking…".into(), String::new());
    }
    ("Nothing here yet.".into(), "Nothing has been published for this device. Y looks again.".into())
}

fn art(ui: &mut Ui, r: &Row, box_r: Rect) {
    mosaic::folder_mosaic(ui, box_r, &art_kinds(r), r.id.as_bytes());
    // its icon on a plate, so a package is recognisably itself
    let s = 132.0;
    let (x, y) = (box_r.x + (box_r.w - s) / 2.0, box_r.y + (box_r.h - s) / 2.0);
    let (bg, ink) = colors(r);
    ui.fill_block(Rect::new(x - 8.0, y - 8.0, s + 16.0, s + 16.0), DARK);
    ui.fill_block(Rect::new(x, y, s, s), bg);
    ui.icon(icon_of(r), x + 18.0, y + 18.0, s - 36.0, ink, 2.0, false);
}

fn draw_panel(app: &App, ui: &mut Ui, r: &Row, sel: usize, n: usize) {
    let (bg, ink) = colors(r);
    ui.fill_block(Rect::new(PANEL_X, 0.0, W - PANEL_X, H), bg);
    art(ui, r, Rect::new(ART_X, ART_Y, ART as f32, ART as f32));
    let (x, w) = (ART_X, ART as f32);
    let mut y = ART_Y + ART as f32 + 14.0;
    let job = app.job_for(r);
    let title = match (r.state, r.kind) {
        _ if job.is_some() => "Getting it".to_string(),
        (RowState::NotYet, _) => "Not yet".to_string(),
        (RowState::Update, _) => "Update".to_string(),
        (RowState::Elsewhere, _) => "From elsewhere".to_string(),
        _ if app.needs_files(r) => "Needs your files".to_string(),
        (RowState::Installed, _) => "Installed".to_string(),
        (_, Kind::Tool) => "Terminal tool".to_string(),
        _ => category_word(&r.category).to_string(),
    };
    let tst = serif(if title.len() > 12 { 42.0 } else { 62.0 }, ink);
    for l in wrap(ui, &title, &tst, w, 2) {
        ui.text(x, y, &l, &tst);
        y += tst.line_px();
    }
    y += 2.0;
    if let Some(j) = job {
        let big = body(38.0, ink);
        let line = match j.phase {
            Phase::Downloading if j.total > 0 => gib_or_mib(j.done, j.total),
            Phase::Installing => "Nearly there".to_string(),
            _ => "Starting".to_string(),
        };
        ui.text(x, y, &line, &big);
        y += big.line_px();
    }
    // a size is only known once it's been asked for (Details: a dry run); it's not
    // worth starting Nix for a row that's only passing by
    let d = regular(24.0, ink);
    let room = ((H - 92.0 - y - 44.0) / d.line_px()).floor().max(1.0) as usize;
    for l in wrap(ui, &r.description, &d, w, room) {
        ui.text(x, y, &l, &d);
        y += d.line_px();
    }
    y += 4.0;
    let m = mono(22.0, ink);
    let v = if r.state == RowState::Update {
        format!("{} → {}", r.installed_version.as_deref().unwrap_or("?"), r.version)
    } else {
        r.version.clone()
    };
    let line = if v.is_empty() { format!("from {}", r.repo_name) } else { format!("{v} · from {}", r.repo_name) };
    let fit = fit_name(ui, &line, &m, w);
    ui.text(x, y.min(H - 88.0), &fit, &m);
    let pst = mono(23.0, ink);
    ui.text(x, H - 50.0, &format!("{} / {}", sel + 1, n), &pst);
}

fn category_word(c: &str) -> &'static str {
    match c {
        "games" => "Game",
        "music" => "Music",
        "tools" => "Terminal tool",
        _ => "App",
    }
}

fn draw_tools_panel(ui: &mut Ui, count: u64, installed: usize, sel: usize, n: usize) {
    ui.fill_block(Rect::new(PANEL_X, 0.0, W - PANEL_X, H), PINK);
    let (x, w) = (ART_X, ART as f32);
    let a = Rect::new(ART_X, ART_Y, ART as f32, ART as f32);
    mosaic::folder_mosaic(ui, a, &[(FileKind::Image, 300), (FileKind::Other, 60), (FileKind::Save, 40)], b"terminal tools");
    let s = 132.0;
    let (px, py) = (a.x + (a.w - s) / 2.0, a.y + (a.h - s) / 2.0);
    ui.fill_block(Rect::new(px - 8.0, py - 8.0, s + 16.0, s + 16.0), DARK);
    ui.fill_block(Rect::new(px, py, s, s), PINK);
    ui.icon(Icon::Term, px + 18.0, py + 18.0, s - 36.0, DARK, 2.0, false);
    let mut y = ART_Y + ART as f32 + 14.0;
    let tst = serif(62.0, DARK);
    ui.text(x, y, &format!("{count}"), &tst);
    y += tst.line_px() + 2.0;
    let d = regular(24.0, DARK);
    let line = if installed > 0 { format!("{installed} here. On Terminal's path, not under Apps.") } else { "For Terminal's path, not under Apps.".to_string() };
    for l in wrap(ui, &line, &d, w, 3) {
        ui.text(x, y, &l, &d);
        y += d.line_px();
    }
    ui.text(x, H - 50.0, &format!("{} / {}", sel + 1, n), &mono(23.0, DARK));
}

fn draw_tools(app: &App, ui: &mut Ui) {
    let t = theme::get();
    let right = PANEL_X - 20.0;
    let pos = app.scroll_pos(&app.tools_list);
    let n = app.tool_rows.len();
    let first = (pos - 4.0).floor().max(0.0) as usize;
    let last = ((pos + 7.0).ceil() as usize).min(n);
    for i in first..last {
        let y = row_y(i as f32 - pos);
        if y + ROW_H < TITLE_H - 8.0 || y > LIST_BOTTOM {
            continue;
        }
        draw_row(app, ui, &app.tool_rows[i], y, right);
    }
    covers(ui);
    let tag_w = if app.searching { status_tag(ui, "Searching…", t.lime) } else { problem_tag(app).map(|(l, c)| status_tag(ui, &l, c)).unwrap_or(0.0) };
    if app.query.is_empty() {
        draw_title(ui, "Terminal tools", Some("Packages"), PANEL_X - 24.0 - tag_w);
    } else {
        draw_title(ui, &format!("“{}”", app.query), Some("Search"), PANEL_X - 24.0 - tag_w);
    }
    let (ink_hint, on) = (cap_ink(), t.ground);
    match app.tool_rows.get(app.tools_list.sel) {
        Some(r) => {
            let (bg, ink) = colors(r);
            draw_band(ui, &r.name, icon_of(r), r.state == RowState::Installed, bg, ink);
            draw_panel(app, ui, r, app.tools_list.sel, n);
            hints(ui, 28.0, &[("A", "Details"), ("START", "Search"), ("B", "Back")], ink_hint, on);
        }
        None => {
            let (title, sub) = if app.searching {
                ("Searching…".to_string(), String::new())
            } else if app.query.is_empty() {
                ("None here yet.".to_string(), "START searches everything your repositories offer.".to_string())
            } else {
                ("Nothing by that name.".to_string(), if app.searched > 0 { format!("Looked through {} tools.", app.searched) } else { String::new() })
            };
            empty_body(ui, &title, &sub);
            ui.fill_block(Rect::new(PANEL_X, 0.0, W - PANEL_X, H), PINK);
            ui.icon(Icon::Term, ART_X + 64.0, ART_Y + 64.0, 192.0, DARK, 1.6, false);
            hints(ui, 28.0, &[("START", "Search"), ("B", "Back")], ink_hint, on);
        }
    }
}

// --------------------------------------------------------------- details ----

fn draw_details(app: &App, ui: &mut Ui) {
    let t = theme::get();
    let Some(r) = &app.row else { return };
    let (bg, ink) = colors(r);
    // the band up top, running into the same panel the list had
    let band_y = 28.0;
    ui.fill_block(Rect::new(0.0, band_y + BAND_H, PANEL_X, BAND_SHADOW), t.line);
    ui.fill_block(Rect::new(0.0, band_y, PANEL_X, BAND_H), bg);
    let s = 64.0;
    let st = serif(58.0, ink);
    icon_tile(ui, 24.0, band_y + (BAND_H - s) / 2.0, s, t.ground, t.line, icon_of(r));
    let fit = fit_name(ui, &r.name, &st, PANEL_X - 112.0 - 18.0);
    ui.text_vcenter(112.0, band_y + BAND_H / 2.0 - 2.0, &fit, &st);

    ui.fill_block(Rect::new(PANEL_X, 0.0, W - PANEL_X, H), bg);
    art(ui, r, Rect::new(ART_X, ART_Y, ART as f32, ART as f32));

    let (x, w) = (30.0, PANEL_X - 60.0);
    let mut y = band_y + BAND_H + BAND_SHADOW + BAND_GAP + 6.0;
    let line = t.line;
    let d = regular(29.0, line);
    for l in wrap(ui, &r.description, &d, w, 2) {
        ui.text(x, y, &l, &d);
        y += d.line_px();
    }
    y += 8.0;
    let facts = regular(25.0, t.line_soft);
    let mut parts: Vec<String> = Vec::new();
    if !r.version.is_empty() {
        parts.push(format!("Version {}", r.version));
    }
    if r.published > 0 {
        parts.push(format!("published {}", app.age(r.published)));
    }
    parts.push(format!("from {}", r.repo_name));
    for l in wrap(ui, &parts.join(" · "), &facts, w, 2) {
        ui.text(x, y, &l, &facts);
        y += facts.line_px();
    }
    y += 8.0;

    // the big fact
    let big = body(34.0, line);
    let mut tail: Vec<String> = Vec::new();
    if let Some(d) = &app.details {
        if d.free > 0 {
            tail.push(format!("{} free", size_word(d.free)));
        }
    }
    if !r.licence.is_empty() {
        tail.push(format!("licence: {}", r.licence));
    }
    let headline = match r.state {
        RowState::NotYet => Some("Not installable yet".to_string()),
        RowState::Elsewhere => Some("Already here from elsewhere".to_string()),
        RowState::Installed => Some(format!("Installed: {}", r.installed_version.as_deref().unwrap_or(&r.version))),
        _ => match &app.details {
            _ if app.sizing => Some("Working out the size…".to_string()),
            Some(Details { problem: Some(p), .. }) => Some(p.headline.clone()),
            Some(Details { size: Some(s), .. }) if s.unpacked == 0 => Some("Already here, nothing to download".to_string()),
            Some(Details { size: Some(s), .. }) => Some(format!("{} to download", size_word(s.unpacked))),
            _ => None,
        },
    };
    if let Some(h) = &headline {
        for l in wrap(ui, h, &big, w, 2) {
            ui.text(x, y, &l, &big);
            y += big.line_px();
        }
    }
    if !tail.is_empty() {
        for l in wrap(ui, &tail.join(" · "), &facts, w, 2) {
            ui.text(x, y, &l, &facts);
            y += facts.line_px();
        }
    }
    y += 10.0;

    // what stands in the way, and what it can reach
    let say = regular(25.0, line);
    let mut notes: Vec<String> = Vec::new();
    if let Some(d) = &app.details {
        if let Some(p) = &d.problem {
            notes.push(p.detail.clone());
        } else if !d.power_ok && matches!(r.state, RowState::Available | RowState::Update) {
            notes.push(format!("Charge first: the battery is at {}%.", d.battery.unwrap_or(0)));
        } else if d.need > d.free && matches!(r.state, RowState::Available | RowState::Update) {
            notes.push(format!("Not enough room: this needs {}.", size_word(d.need)));
        }
    }
    match r.state {
        RowState::NotYet => notes.push("It's a bundle, which a later Mortar will run in a sandbox.".into()),
        RowState::Elsewhere => notes.push("It was put here from a computer or another repository, and stays as it is.".into()),
        RowState::Update => notes.push(format!("You have {}. The old one stays until you've updated.", r.installed_version.as_deref().unwrap_or("an older one"))),
        _ => {}
    }
    if r.kind == Kind::Tool {
        let cmd = r.program.as_deref().unwrap_or(&r.id);
        notes.push(format!("It goes on Terminal's path as {cmd}, not under Apps."));
    } else if r.kind != Kind::Bundle {
        notes.push("It can see your saves and other files, like any app on Mortar.".into());
    }
    for n in notes.iter() {
        for l in wrap(ui, n, &say, w, 3) {
            ui.text(x, y, &l, &say);
            y += say.line_px();
        }
        y += 6.0;
    }

    // your own files
    if !r.requires.is_empty() {
        let need = app.needs_files(r);
        let head = if need { "Needs your files. It shows under Apps once they're here:" } else { "Your files are here." };
        y += 4.0;
        for l in wrap(ui, head, &body(25.0, line), w, 2) {
            ui.text(x, y, &l, &body(25.0, line));
            y += 34.0;
        }
        let m = mono(21.0, line);
        for p in r.requires.iter().take(3) {
            let here = !app.missing(p);
            let side = 22.0;
            if here {
                mosaic::glyph(ui, Glyph::Check, x, y + 4.0, 3.0, 0.0, line);
            } else {
                ui.fill_block(Rect::new(x, y + 4.0, 18.0, 18.0), t.orange);
            }
            let fit = fit_name(ui, p, &m, w - side - 10.0);
            ui.text(x + side + 8.0, y, &fit, &m);
            y += 30.0;
        }
    }

    // in the panel, under the art: where it stands
    let px = ART_X;
    let mut py = ART_Y + ART as f32 + 14.0;
    let word = match r.state {
        RowState::Installed => "Installed",
        RowState::Update => "Update",
        RowState::NotYet => "Not yet",
        RowState::Elsewhere => "From elsewhere",
        RowState::Available if app.needs_files(r) => "Needs your files",
        RowState::Available => "Available",
    };
    let tst = serif(if word.len() > 12 { 42.0 } else { 62.0 }, ink);
    for l in wrap(ui, word, &tst, ART as f32, 2) {
        ui.text(px, py, &l, &tst);
        py += tst.line_px();
    }
    if !r.source.is_empty() {
        let m = mono(19.0, ink);
        for l in wrap(ui, r.source.trim_start_matches("https://"), &m, ART as f32, 3) {
            ui.text(px, py + 8.0, &l, &m);
            py += m.line_px();
        }
    }

    let (ink_hint, on) = (cap_ink(), t.ground);
    let mut hs: Vec<(&str, &str)> = Vec::new();
    match r.state {
        RowState::Available => hs.push(("A", "Install")),
        RowState::Update => hs.push(("A", "Update")),
        RowState::Installed if r.kind != Kind::Tool && !app.needs_files(r) => hs.push(("A", "Open")),
        _ => {}
    }
    if matches!(r.state, RowState::Installed | RowState::Update) {
        hs.push(("X", "Remove"));
    }
    if r.can_roll_back {
        hs.push(("Y", "Roll back"));
    }
    hs.push(("B", "Back"));
    // over whatever ran long: it's the way out
    ui.fill_block(Rect::new(0.0, LIST_BOTTOM, PANEL_X, HINT_H), t.ground);
    hints(ui, 28.0, &hs, ink_hint, on);
}

// -------------------------------------------------------------- progress ----

/// The whole screen is the progress bar: tiles lighting from the top, in the colour of
/// what's coming. It's Files' copy screen.
fn draw_progress(app: &App, ui: &mut Ui) {
    let t = theme::get();
    let Some(r) = &app.row else { return };
    let job = app.job_for(r);
    let (done, total) = job.map(|j| (j.done, j.total)).unwrap_or((0, 0));
    let frac = if total > 0 { (done as f32 / total as f32).clamp(0.0, 1.0) } else { 0.0 };
    let (g, tile_s) = (28.0f32, 24.0f32);
    let (cols, rows) = (37usize, 27usize);
    let cells = cols * rows;
    let lit = (frac * cells as f32).round() as usize;
    let (bg, ink) = colors(r);
    let c = if bg == t.ground_2 { ink } else { bg };
    for i in 0..lit {
        let (cx, cy) = ((i % cols) as f32, (i / cols) as f32);
        ui.fill_block(Rect::new(-12.0 + cx * g + 2.0, 6.0 + cy * g + 2.0, tile_s, tile_s), c);
    }
    let plate = Rect::new(112.0, 216.0, 800.0, 336.0);
    ui.fill_block(Rect::new(plate.x + 10.0, plate.y + 10.0, plate.w, plate.h), t.line);
    ui.fill_block(plate, t.ground);
    ui.outline(plate, 3.0, t.line);
    let update = job.is_some_and(|j| j.action == Action::Update);
    let cancelling = job.is_some_and(|j| j.cancelling);
    let verb = if cancelling {
        "Stopping…".to_string()
    } else if update {
        format!("Updating {}", r.name)
    } else {
        format!("Getting {}", r.name)
    };
    let st = serif(60.0, t.line);
    let fit = fit_name(ui, &verb, &st, plate.w - 72.0);
    ui.text(plate.x + 36.0, plate.y + 20.0, &fit, &st);
    let nums = match job.map(|j| j.phase) {
        Some(Phase::Downloading) if total > 0 => gib_or_mib(done, total),
        Some(Phase::Installing) => "Putting it in place".to_string(),
        _ => "Checking…".to_string(),
    };
    ui.text(plate.x + 38.0, plate.y + 100.0, &nums, &body(38.0, t.line));
    let d = regular(26.0, t.line_soft);
    let mut y = plate.y + 158.0;
    let what = if cancelling {
        "It stops here, and leaves nothing behind."
    } else if r.kind == Kind::Tool {
        "You can leave this and use other apps. It carries on, and shows on Terminal's path when it's ready."
    } else {
        "You can leave this and use other apps. It carries on, and shows under Apps when it's ready."
    };
    for l in wrap(ui, what, &d, plate.w - 76.0, 3) {
        ui.text(plate.x + 38.0, y, &l, &d);
        y += d.line_px();
    }
    let cy = plate.bottom() - 42.0;
    let mut x = plate.x + 38.0;
    x += cap(ui, x, cy, "B", cap_ink(), t.ground) + 10.0;
    x += ui.text_vcenter(x, cy, "Back", &regular(24.0, cap_ink())) + 26.0;
    if !cancelling && job.is_none_or(|j| j.phase != Phase::Installing) {
        x += cap(ui, x, cy, "X", cap_ink(), t.ground) + 10.0;
        ui.text_vcenter(x, cy, "Cancel", &regular(24.0, cap_ink()));
    }
}

// --------------------------------------------------------------- overlays ----

/// The deliberate kind of screen, as Files' Trash: it fills the display in a colour
/// nothing else uses that big, and the button is a different one from the way in.
fn draw_confirm(ui: &mut Ui, c: &Confirm) {
    let t = theme::get();
    let bg = t.yellow;
    ui.clear(bg);
    ui.fill_block(Rect::new(W - 252.0, 0.0, 252.0, H), DARK);
    let (rbg, rink) = colors(&c.row);
    icon_tile(ui, W - 252.0 + 56.0, 40.0, 140.0, rbg, rink, icon_of(&c.row));
    let left = 48.0;
    let room = W - 252.0 - left - 24.0;
    let tst = serif(84.0, DARK);
    let title = match c.kind {
        ConfirmKind::Remove => format!("Remove {}?", c.row.name),
        ConfirmKind::Rollback => format!("Back to the last version of {}?", c.row.name),
    };
    let mut y = 40.0;
    for l in wrap(ui, &title, &tst, room, 3) {
        ui.text(left, y, &l, &tst);
        y += tst.line_px();
    }
    y += 16.0;
    let d = regular(28.0, DARK);
    let text: Vec<String> = match c.kind {
        ConfirmKind::Remove => {
            let mut v = vec!["It leaves Apps at once. If it's running, it keeps running until you close it.".to_string(), "Your saves and files stay where they are.".to_string()];
            if c.row.kind == Kind::Tool {
                v[0] = "It leaves Terminal's path at once.".to_string();
            }
            v
        }
        ConfirmKind::Rollback => vec![format!("You have {}. The one before it comes back, and the daily check leaves it that way.", c.row.installed_version.as_deref().unwrap_or("this version")), "Your saves and files stay where they are.".to_string()],
    };
    for s in &text {
        for l in wrap(ui, s, &d, room, 3) {
            ui.text(left, y, &l, &d);
            y += d.line_px();
        }
        y += 8.0;
    }
    let btn_y = H - 170.0;
    let cy = btn_y + 30.0;
    let w = cap(ui, left, cy, "Y", DARK, bg) + 14.0;
    let label = match c.kind {
        ConfirmKind::Remove => "Remove it",
        ConfirmKind::Rollback => "Go back",
    };
    ui.text_vcenter(left + w, cy, label, &body(32.0, DARK));
    if c.nudge {
        ui.text_vcenter(left, btn_y + 80.0, "A won't do it, on purpose. It's Y.", &regular(26.0, DARK));
    }
    let cy = H - HINT_H / 2.0;
    let w = cap(ui, left, cy, "B", DARK, bg) + 10.0;
    ui.text_vcenter(left + w, cy, "Keep it", &regular(26.0, DARK));
}

fn draw_report(ui: &mut Ui, p: &Problem) {
    let t = theme::get();
    // the polite one isn't an alarm
    let bg = if matches!(p.code.as_str(), "newer-format" | "offline" | "low-battery" | "busy") { t.yellow } else { t.orange };
    ui.clear(bg);
    let left = 48.0;
    let room = W - 2.0 * left;
    let mut y = 40.0;
    let st = serif(76.0, DARK);
    for l in wrap(ui, &p.headline, &st, room, 3) {
        ui.text(left, y, &l, &st);
        y += st.line_px();
    }
    y += 16.0;
    let d = regular(30.0, DARK);
    for l in wrap(ui, &p.detail, &d, room, 8) {
        ui.text(left, y, &l, &d);
        y += d.line_px();
    }
    let cy = H - HINT_H / 2.0;
    let w = cap(ui, left, cy, "A", DARK, bg) + 10.0;
    ui.text_vcenter(left + w, cy, "OK", &regular(26.0, DARK));
}

/// Search, above the shell's keyboard: Files' Find, lime, with the list beneath.
fn draw_find(app: &App, ui: &mut Ui, f: &mortar_files::app::Field) {
    let t = theme::get();
    let h = ui.height();
    ui.clear(t.lime);
    ui.text(36.0, 8.0, "Find", &serif(56.0, DARK));
    draw_field(ui, Rect::new(176.0, 14.0, W - 216.0, 72.0), f);
    let (mut y, step) = (112.0, 56.0);
    let from = app.tools_list.sel;
    for (i, r) in app.tool_rows.iter().skip(from).take(((h - 150.0) / step).max(1.0) as usize).enumerate() {
        let st = if i == 0 { serif(44.0, DARK) } else { body(27.0, DARK) };
        icon_tile(ui, 36.0, y + 2.0, 44.0, DARK, t.lime, icon_of(r));
        let fit = fit_name(ui, &r.name, &st, W - 330.0);
        ui.text_vcenter(96.0, y + 24.0, &fit, &st);
        let v = row_value(app, r);
        let vs = mono(22.0, DARK);
        let vw = ui.measure(&v, &vs);
        ui.text_vcenter(W - 36.0 - vw, y + 24.0, &v, &vs);
        y += step;
    }
    if app.tool_rows.is_empty() && !app.query.is_empty() {
        let s = if app.searching { "Searching…" } else { "Nothing by that name." };
        ui.text(96.0, 118.0, s, &body(27.0, DARK));
    }
    let cy = h - 30.0;
    let mut x = 36.0;
    x += cap(ui, x, cy, "A", DARK, t.lime) + 10.0;
    x += ui.text_vcenter(x, cy, "Done", &regular(24.0, DARK)) + 26.0;
    if app.found_total > 0 {
        ui.text_vcenter(x, cy, &format!("{} found", app.found_total), &regular(24.0, DARK));
    }
}
