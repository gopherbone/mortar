// SPDX-License-Identifier: GPL-2.0-or-later
//! The user's Display choices (scaling filter + fit), chosen in the in-game
//! menu or the shell's game panel: kept per game or per system in
//! `<stateDir>/display.json`, over the system's default from the Nix
//! config. A game's own choice wins over its system's.
//!
//! ```json
//! {"systems": {"psx": "smooth/fit"}, "games": {"psx/Some Game (USA)": "integer/fit"}}
//! ```

use crate::config::{Config, System, expand};
use crate::video::View;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where the current choice comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The system's default (modules/packs/emulation.nix).
    Default,
    /// Kept for every game of the system.
    System,
    /// Kept for this game.
    Game,
    /// Changed for this session, not kept.
    Session,
}

impl Scope {
    pub fn name(self) -> &'static str {
        match self {
            Scope::Default => "default",
            Scope::System => "system",
            Scope::Game => "game",
            Scope::Session => "session",
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Prefs {
    #[serde(default)]
    pub systems: BTreeMap<String, String>,
    #[serde(default)]
    pub games: BTreeMap<String, String>,
}

/// The key of a game: `<system>/<file stem>` (as its saves are named).
pub fn game_key(sys: &System, game: &Path) -> String {
    format!("{}/{}", sys.id, game.file_stem().unwrap_or_default().to_string_lossy())
}

pub fn path(cfg: &Config) -> PathBuf {
    expand(&cfg.state_dir).join("display.json")
}

impl Prefs {
    pub fn load(file: &Path) -> Prefs {
        std::fs::read(file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save(&self, file: &Path) -> Result<(), String> {
        if let Some(d) = file.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default()).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, file).map_err(|e| format!("{}: {e}", file.display()))
    }

    /// What a game starts with.
    pub fn resolve(&self, sys: &System, game: Option<&Path>) -> (View, Scope) {
        if let Some(v) = game.and_then(|g| self.games.get(&game_key(sys, g))).and_then(|s| View::parse(s)) {
            return (v, Scope::Game);
        }
        if let Some(v) = self.systems.get(&sys.id).and_then(|s| View::parse(s)) {
            return (v, Scope::System);
        }
        (sys.view(), Scope::Default)
    }

    /// Keep `view` for the game (`for_game`, needs a game file) or for the
    /// whole system (the game's own choice is dropped so it follows). Read,
    /// change, write: other games may have written in the meantime.
    pub fn keep(file: &Path, sys: &System, game: Option<&Path>, view: View, for_game: bool) -> Result<Scope, String> {
        let mut p = Prefs::load(file);
        if for_game {
            let g = game.ok_or("this game has no file to keep it for")?;
            p.games.insert(game_key(sys, g), view.key());
        } else {
            if let Some(g) = game {
                p.games.remove(&game_key(sys, g));
            }
            if view == sys.view() {
                p.systems.remove(&sys.id);
            } else {
                p.systems.insert(sys.id.clone(), view.key());
            }
        }
        p.save(file)?;
        Ok(if for_game { Scope::Game } else { Scope::System })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::video::{Filter, Fit};

    #[test]
    fn keep_and_resolve() {
        let dir = std::env::temp_dir().join(format!("mp-display-{}", std::process::id()));
        let file = dir.join("display.json");
        let sys = System { id: "psx".into(), scaling: Filter::SharpBilinear, ..Default::default() };
        let game = Path::new("/roms/psx/A Game (USA).chd");
        let other = Path::new("/roms/psx/Other.chd");
        assert_eq!(Prefs::load(&file).resolve(&sys, Some(game)), (sys.view(), Scope::Default));
        let smooth = View { filter: Filter::Smooth, fit: Fit::Fit };
        let int = View { filter: Filter::Integer, fit: Fit::Fit };
        assert_eq!(Prefs::keep(&file, &sys, Some(game), int, true), Ok(Scope::Game));
        assert_eq!(Prefs::keep(&file, &sys, Some(other), smooth, false), Ok(Scope::System));
        let p = Prefs::load(&file);
        assert_eq!(p.resolve(&sys, Some(game)), (int, Scope::Game));
        assert_eq!(p.resolve(&sys, Some(other)), (smooth, Scope::System));
        assert_eq!(p.games.keys().collect::<Vec<_>>(), vec!["psx/A Game (USA)"]);
        // keeping for the system drops the game's own choice
        assert_eq!(Prefs::keep(&file, &sys, Some(game), smooth, false), Ok(Scope::System));
        assert_eq!(Prefs::load(&file).resolve(&sys, Some(game)), (smooth, Scope::System));
        // back to the default: nothing kept
        Prefs::keep(&file, &sys, None, sys.view(), false).unwrap();
        assert!(Prefs::load(&file).systems.is_empty());
        assert!(Prefs::keep(&file, &sys, None, int, true).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
