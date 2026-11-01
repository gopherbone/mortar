// SPDX-License-Identifier: GPL-2.0-or-later
//! Games in the shell (mortar-play, pkgs/mortar-play): pure data and
//! policy, unit-tested on any OS.
//!
//! - The library: `mortar-play list --json`, run by a worker thread
//!   (comp/gamectl.rs) and cached by the modification times of the folders
//!   it names (`watch`).
//! - One game, one app: a game runs as `<games.run> SYSTEM ROM`, its own
//!   app titled "Pocket King · GBC", keyed by `game:SYSTEM:PATH` so starting
//!   it again switches to it.
//! - The game panel (MENU > Running > Y): requests to the game's control
//!   socket (`$XDG_RUNTIME_DIR/mortar-play-<pid>.sock`, one JSON line each
//!   way) and what its `list_states` answer says.
//! - Hibernation: which background game goes to sleep when memory runs
//!   low or too many are open.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// State slots in mortar-play (1-4 in the protocol).
pub const SLOTS: usize = 4;

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Library {
    pub systems: Vec<LibSystem>,
    pub recent: Vec<LibRecent>,
    /// Folders whose modification times change when the answer would.
    pub watch: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LibSystem {
    pub id: String,
    pub name: String,
    pub tag: String,
    pub order: i32,
    /// Runs without a game file (2048): its row starts it.
    pub no_content: bool,
    pub count: usize,
    pub games: Vec<LibGame>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LibGame {
    pub path: PathBuf,
    /// Cleaned of "(USA)" and the like.
    pub name: String,
    /// The first tag of the file name ("USA").
    pub region: Option<String>,
    /// A resume state exists (the game continues where it was left).
    pub resume: bool,
    /// The resume state's picture.
    pub thumb: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LibRecent {
    pub system: String,
    pub path: PathBuf,
    pub name: String,
    pub region: Option<String>,
    pub tag: String,
    pub resume: bool,
    pub thumb: Option<PathBuf>,
}

impl Library {
    pub fn parse(s: &str) -> Result<Library, String> {
        serde_json::from_str(s).map_err(|e| format!("mortar-play list: {e}"))
    }
    pub fn system(&self, id: &str) -> Option<&LibSystem> {
        self.systems.iter().find(|s| s.id == id)
    }
    /// Systems that have something to play, in the module's order.
    pub fn shelves(&self) -> Vec<&LibSystem> {
        let mut v: Vec<&LibSystem> = self.systems.iter().filter(|s| s.no_content || s.count > 0).collect();
        v.sort_by_key(|s| s.order);
        v
    }
    /// A game by system and file (for recents and the system page).
    pub fn game_ref(&self, system: &str, path: &Path) -> Option<GameRef> {
        let sys = self.system(system)?;
        let name = sys
            .games
            .iter()
            .find(|g| g.path == path)
            .map(|g| g.name.clone())
            .or_else(|| self.recent.iter().find(|r| r.path == path).map(|r| r.name.clone()))?;
        Some(GameRef { system: system.into(), path: Some(path.into()), name, tag: sys.tag.clone(), system_name: sys.name.clone() })
    }
    pub fn recent_ref(&self, r: &LibRecent) -> GameRef {
        let system_name = self.system(&r.system).map(|s| s.name.clone()).unwrap_or_default();
        GameRef { system: r.system.clone(), path: Some(r.path.clone()), name: r.name.clone(), tag: r.tag.clone(), system_name }
    }
    /// A no-content system (2048) as a game.
    pub fn system_ref(&self, sys: &LibSystem) -> GameRef {
        GameRef { system: sys.id.clone(), path: None, name: sys.name.clone(), tag: sys.tag.clone(), system_name: sys.name.clone() }
    }
}

/// A game to start (or switch to).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameRef {
    pub system: String,
    /// None: a core that runs without a game (2048).
    pub path: Option<PathBuf>,
    pub name: String,
    pub tag: String,
    pub system_name: String,
}

impl GameRef {
    /// The app's entry id: one app per game.
    pub fn key(&self) -> String {
        match &self.path {
            Some(p) => format!("game:{}:{}", self.system, p.display()),
            None => format!("game:{}", self.system),
        }
    }
    /// The window's title in Running ("Pocket King · GBC").
    pub fn title(&self) -> String {
        if self.path.is_none() || self.tag.is_empty() || self.name == self.tag {
            return self.name.clone();
        }
        format!("{} · {}", self.name, self.tag)
    }
    /// `<run> SYSTEM [ROM]`.
    pub fn argv(&self, run: &[String]) -> Vec<String> {
        let mut v = run.to_vec();
        v.push(self.system.clone());
        if let Some(p) = &self.path {
            v.push(p.to_string_lossy().into_owned());
        }
        v
    }
}

/// An app entry id names a game.
pub fn is_game_key(id: &str) -> bool {
    id.starts_with("game:")
}

/// A request to a game's control socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameCmd {
    ListStates,
    /// Slot 0-3.
    Save(u8),
    Load(u8),
    /// The resume state (sleep safety).
    SaveAuto,
    Reset,
    /// Fast-forward on / off (None: toggle).
    FastForward(Option<bool>),
    NextDisc,
    /// Write the resume state and exit (`quiet`: no "Saved" toast; the
    /// shell is putting the game to sleep).
    Quit { quiet: bool },
}

impl GameCmd {
    pub fn json(&self) -> String {
        use serde_json::json;
        match self {
            GameCmd::ListStates => json!({"cmd": "list_states"}),
            GameCmd::Save(n) => json!({"cmd": "save_state", "slot": n + 1}),
            GameCmd::Load(n) => json!({"cmd": "load_state", "slot": n + 1}),
            GameCmd::SaveAuto => json!({"cmd": "save_state", "slot": "auto"}),
            GameCmd::Reset => json!({"cmd": "reset"}),
            GameCmd::FastForward(None) => json!({"cmd": "fast_forward", "on": "toggle"}),
            GameCmd::FastForward(Some(on)) => json!({"cmd": "fast_forward", "on": on}),
            GameCmd::NextDisc => json!({"cmd": "change_disc", "to": "next"}),
            GameCmd::Quit { quiet } => json!({"cmd": "quit", "quiet": quiet}),
        }
        .to_string()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlotInfo {
    pub age_s: u64,
    pub thumb: Option<PathBuf>,
}

/// A game's states and switches, from its `list_states` answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameStatus {
    pub slots: [Option<SlotInfo>; SLOTS],
    /// (current, count), 1-based.
    pub discs: Option<(u32, u32)>,
    pub fast_forward: bool,
    pub can_save: bool,
}

impl GameStatus {
    pub fn parse(v: &serde_json::Value) -> Option<GameStatus> {
        if v.get("ok")?.as_bool() != Some(true) {
            return None;
        }
        let slot = |s: &serde_json::Value| -> Option<SlotInfo> {
            Some(SlotInfo { age_s: s.get("age_s")?.as_u64()?, thumb: s.get("thumb").and_then(|t| t.as_str()).map(PathBuf::from) })
        };
        let mut st = GameStatus::default();
        if let Some(a) = v.get("slots").and_then(|s| s.as_array()) {
            for (i, s) in a.iter().take(SLOTS).enumerate() {
                st.slots[i] = slot(s);
            }
        }
        st.discs = v.get("discs").and_then(|d| d.as_array()).and_then(|d| Some((d.first()?.as_u64()? as u32, d.get(1)?.as_u64()? as u32)));
        st.fast_forward = v.get("fast_forward").and_then(|f| f.as_bool()).unwrap_or(false);
        st.can_save = v.get("can_save").and_then(|f| f.as_bool()).unwrap_or(true);
        Some(st)
    }
    /// More than one disc: the panel offers Change disc.
    pub fn multi_disc(&self) -> bool {
        self.discs.is_some_and(|(_, n)| n > 1)
    }
}

/// What the UI is told about games (shellui's `View::games`).
#[derive(Clone, Debug, Default)]
pub struct GamesView {
    /// The library (None until the first `list` answer).
    pub lib: Option<Library>,
    /// Decoded pictures by file.
    pub thumbs: std::collections::HashMap<PathBuf, Pic>,
    /// Each game app's states and switches (the game panel), by uid.
    pub status: std::collections::HashMap<u32, GameStatus>,
    /// The launcher entry of mortar-play's own library ("All systems").
    pub library_app: Option<String>,
}

/// A decoded picture: its `Src::Pic` id and size in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pic {
    pub id: u32,
    pub w: u32,
    pub h: u32,
}

impl Pic {
    /// The largest rectangle of the picture's shape centred in a box.
    pub fn fit(&self, x: f32, y: f32, w: f32, h: f32) -> [f32; 4] {
        if self.w == 0 || self.h == 0 {
            return [x, y, w, h];
        }
        let s = (w / self.w as f32).min(h / self.h as f32);
        let (pw, ph) = (self.w as f32 * s, self.h as f32 * s);
        [x + (w - pw) / 2.0, y + (h - ph) / 2.0, pw, ph]
    }
}

impl GamesView {
    pub fn pic(&self, p: Option<&Path>) -> Option<Pic> {
        self.thumbs.get(p?).copied()
    }
}

/// "just now", "5 min ago", "3 h ago", "2 days ago" (as mortar-play says it).
pub fn ago(s: u64) -> String {
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86400 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86400),
    }
}

/// MemAvailable from /proc/meminfo, in MB.
pub fn mem_available_mb(meminfo: &str) -> Option<u64> {
    let l = meminfo.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kb: u64 = l.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024)
}

/// A running game as the hibernation policy sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Awake {
    pub uid: u32,
    /// Position in the MRU list (0: most recent).
    pub rank: usize,
    /// In front (or about to be): never put to sleep.
    pub front: bool,
    /// Still starting (no picture yet, or just launched): never.
    pub starting: bool,
}

/// Which background game goes to sleep: when more than `max_games` are
/// awake, or memory is below `low_mb`, the least recently used one that is
/// neither in front nor starting. None: nothing to do.
pub fn hibernate_pick(awake: &[Awake], max_games: usize, mem_mb: Option<u64>, low_mb: u64) -> Option<u32> {
    let over = awake.len() > max_games.max(1);
    let low = mem_mb.is_some_and(|m| m < low_mb);
    if !over && !low {
        return None;
    }
    awake.iter().filter(|a| !a.front && !a.starting).max_by_key(|a| a.rank).map(|a| a.uid)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"{"version":1,
      "systems":[
        {"id":"gbc","name":"Game Boy Color","tag":"GBC","order":11,"noContent":false,"count":2,
         "games":[{"path":"/r/gbc/Another.gbc","name":"Another","region":null,"resume":false,"thumb":null},
                  {"path":"/r/gbc/Pocket King (World).gbc","name":"Pocket King","region":"World","resume":true,"thumb":"/s/gbc/Pocket King (World).state.auto.png"}]},
        {"id":"nes","name":"NES","tag":"NES","order":20,"noContent":false,"count":0,"games":[]},
        {"id":"2048","name":"2048","tag":"2048","order":90,"noContent":true,"count":0,"games":[]}],
      "recent":[{"system":"gbc","path":"/r/gbc/Pocket King (World).gbc","name":"Pocket King","region":"World","tag":"GBC","resume":true,"thumb":null}],
      "watch":["/r/gbc","/s/gbc"]}"#;

    #[test]
    fn library_and_titles() {
        let lib = Library::parse(LIST).unwrap();
        let shelves: Vec<&str> = lib.shelves().iter().map(|s| s.id.as_str()).collect();
        assert_eq!(shelves, vec!["gbc", "2048"], "empty systems are left out");
        let g = lib.recent_ref(&lib.recent[0]);
        assert_eq!(g.title(), "Pocket King · GBC");
        assert_eq!(g.system_name, "Game Boy Color");
        assert_eq!(g.key(), "game:gbc:/r/gbc/Pocket King (World).gbc");
        assert!(is_game_key(&g.key()));
        assert!(!is_game_key("play"));
        assert_eq!(g.argv(&["mortar-play".into(), "run".into()]), vec!["mortar-play", "run", "gbc", "/r/gbc/Pocket King (World).gbc"]);
        assert_eq!(lib.game_ref("gbc", Path::new("/r/gbc/Another.gbc")).unwrap().title(), "Another · GBC");
        let t = lib.system_ref(lib.system("2048").unwrap());
        assert_eq!((t.title(), t.key(), t.argv(&["p".into()]).len()), ("2048".into(), "game:2048".into(), 2));
        assert!(Library::parse("{nope").is_err());
    }

    #[test]
    fn commands_and_status() {
        assert_eq!(GameCmd::Save(0).json(), r#"{"cmd":"save_state","slot":1}"#);
        assert_eq!(GameCmd::SaveAuto.json(), r#"{"cmd":"save_state","slot":"auto"}"#);
        assert_eq!(GameCmd::Quit { quiet: true }.json(), r#"{"cmd":"quit","quiet":true}"#);
        assert_eq!(GameCmd::FastForward(None).json(), r#"{"cmd":"fast_forward","on":"toggle"}"#);
        let v: serde_json::Value = serde_json::from_str(
            r#"{"ok":true,"game":"G","tag":"PS1","slots":[{"age_s":30,"thumb":"/s/g.state.1.png"},null,{"age_s":7200,"thumb":null},null],
                "auto":null,"discs":[1,3],"fast_forward":true,"can_save":true}"#,
        )
        .unwrap();
        let st = GameStatus::parse(&v).unwrap();
        assert_eq!(st.slots[0].as_ref().unwrap().age_s, 30);
        assert!(st.slots[1].is_none());
        assert!(st.slots[2].as_ref().unwrap().thumb.is_none());
        assert!(st.multi_disc() && st.fast_forward);
        assert!(GameStatus::parse(&serde_json::json!({"ok": false})).is_none());
        assert_eq!(ago(30), "just now");
        assert_eq!(ago(7200), "2 h ago");
    }

    #[test]
    fn meminfo() {
        assert_eq!(mem_available_mb("MemTotal:  993000 kB\nMemFree:  10000 kB\nMemAvailable:  184320 kB\n"), Some(180));
        assert_eq!(mem_available_mb("nothing"), None);
    }

    #[test]
    fn who_goes_to_sleep() {
        let a = |uid, rank, front, starting| Awake { uid, rank, front, starting };
        // three open, max 3, plenty of memory: nothing
        let g = [a(1, 0, true, false), a(2, 1, false, false), a(3, 2, false, false)];
        assert_eq!(hibernate_pick(&g, 3, Some(400), 180), None);
        // a fourth: the least recently used background one
        let g4 = [a(1, 0, true, false), a(2, 1, false, false), a(3, 2, false, false), a(4, 3, false, false)];
        assert_eq!(hibernate_pick(&g4, 3, Some(400), 180), Some(4));
        // low memory with two open: the one behind
        assert_eq!(hibernate_pick(&g[..2], 3, Some(120), 180), Some(2));
        // never the one in front, never one that is starting
        assert_eq!(hibernate_pick(&[a(1, 0, true, false)], 3, Some(50), 180), None);
        assert_eq!(hibernate_pick(&[a(1, 0, true, false), a(2, 1, false, true)], 3, Some(50), 180), None);
        let starting_last = [a(1, 0, false, false), a(2, 1, false, false), a(3, 2, false, true), a(4, 3, true, false)];
        assert_eq!(hibernate_pick(&starting_last, 3, None, 180), Some(2));
    }
}
