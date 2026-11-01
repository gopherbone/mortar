// SPDX-License-Identifier: GPL-2.0-or-later
//! The pairing code's life cycle (pure, unit-tested; the daemon wraps it).
//!
//! - With no SSH keys on the Brick, a code is always on offer (renewed when
//!   it expires) so the first-boot screen has one to show.
//! - With keys, a code exists only after a button press on the Brick
//!   ("Pair a new computer" / Y on the code screen).
//! - A code dies after `CODE_TTL`, when a key is added, or after
//!   `MAX_TRIES` failed tries; after that last case pairing stays locked
//!   until a button press makes a new code.

use crate::control::PairingView;
use mortar_pair::{Code, ErrorCode, CODE_TTL, MAX_TRIES};
use std::time::{Duration, Instant};

/// Minimum gap between two pairing connections.
pub const MIN_GAP: Duration = Duration::from_millis(1000);

#[derive(Debug, Default)]
pub struct Pairing {
    code: Option<(Code, Instant)>,
    tries: u32,
    locked: bool,
    last_try: Option<Instant>,
}

impl Pairing {
    pub fn active(&self, now: Instant) -> Option<&Code> {
        match &self.code {
            Some((c, at)) if !self.locked && now.duration_since(*at) < CODE_TTL => Some(c),
            _ => None,
        }
    }

    pub fn locked(&self) -> bool {
        self.locked
    }

    pub fn tries_left(&self) -> u32 {
        if self.locked { 0 } else { MAX_TRIES.saturating_sub(self.tries) }
    }

    pub fn view(&self, now: Instant) -> Option<PairingView> {
        let (c, at) = self.code.as_ref()?;
        self.active(now)?;
        Some(PairingView {
            code: c.to_string(),
            expires_in_s: CODE_TTL.saturating_sub(now.duration_since(*at)).as_secs(),
            tries_left: self.tries_left(),
        })
    }

    /// A button press on the Brick: fresh code, fresh tries, unlocked.
    pub fn new_code(&mut self, now: Instant) {
        self.code = Some((Code::random(), now));
        self.tries = 0;
        self.locked = false;
    }

    pub fn end(&mut self) {
        self.code = None;
        self.tries = 0;
    }

    /// Housekeeping. Returns true if what the screen shows changed.
    pub fn tick(&mut self, now: Instant, keys_present: bool) -> bool {
        let before = self.code.as_ref().map(|c| c.0.clone());
        if self.code.as_ref().is_some_and(|(_, at)| now.duration_since(*at) >= CODE_TTL) {
            self.code = None;
            self.tries = 0;
        }
        if !keys_present && self.code.is_none() && !self.locked {
            self.new_code(now);
        }
        before != self.code.as_ref().map(|c| c.0.clone())
    }

    /// Start one try (a pairing connection or an SSH password). Counted
    /// before the outcome is known: SPAKE2 allows one guess per exchange.
    pub fn begin_try(&mut self, now: Instant) -> Result<Code, (ErrorCode, &'static str)> {
        if self.locked {
            return Err((ErrorCode::Locked, "Too many wrong codes. Press Y on the Brick for a new one."));
        }
        let Some(code) = self.active(now).cloned() else {
            return Err((ErrorCode::NotPairing, "The Brick is not showing a pairing code. Open Settings > Remote access > Pair a new computer on it."));
        };
        if self.last_try.is_some_and(|t| now.duration_since(t) < MIN_GAP) {
            return Err((ErrorCode::Busy, "Too fast; try again in a second."));
        }
        self.last_try = Some(now);
        self.tries += 1;
        if self.tries >= MAX_TRIES {
            // this was the last try the code gets
            self.locked = true;
        }
        Ok(code)
    }

    /// The try succeeded and a key was added: the code is used up.
    pub fn succeeded(&mut self) {
        self.code = None;
        self.tries = 0;
        self.locked = false;
    }

    /// A key arrived some other way (ssh-copy-id with the code as password,
    /// the card): the code is used up too.
    pub fn consumed(&mut self) {
        self.succeeded();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_code_without_keys() {
        let t0 = Instant::now();
        let mut p = Pairing::default();
        assert!(p.active(t0).is_none());
        assert!(p.tick(t0, false));
        let c = p.active(t0).unwrap().clone();
        assert!(!p.tick(t0 + Duration::from_secs(60), false));
        assert_eq!(p.view(t0 + Duration::from_secs(28)).unwrap().expires_in_s, 15 * 60 - 28);
        // expires and renews
        let later = t0 + CODE_TTL;
        assert!(p.tick(later, false));
        assert_ne!(p.active(later).unwrap(), &c);
    }

    #[test]
    fn no_code_with_keys_until_asked() {
        let t0 = Instant::now();
        let mut p = Pairing::default();
        p.tick(t0, true);
        assert!(p.active(t0).is_none());
        assert_eq!(p.begin_try(t0).unwrap_err().0, ErrorCode::NotPairing);
        p.new_code(t0);
        assert!(p.active(t0).is_some());
        p.begin_try(t0).unwrap();
        p.succeeded();
        assert!(p.active(t0).is_none(), "first key ends the code");
        p.tick(t0, true);
        assert!(p.active(t0).is_none());
        // and it expires
        p.new_code(t0);
        p.tick(t0 + CODE_TTL, true);
        assert!(p.active(t0 + CODE_TTL).is_none());
    }

    #[test]
    fn tries_are_limited_and_paced() {
        let t0 = Instant::now();
        let mut p = Pairing::default();
        p.new_code(t0);
        let mut t = t0;
        for i in 0..MAX_TRIES {
            assert_eq!(p.tries_left(), MAX_TRIES - i);
            p.begin_try(t).unwrap();
            if i + 1 < MAX_TRIES {
                assert_eq!(p.begin_try(t).unwrap_err().0, ErrorCode::Busy);
            }
            t += MIN_GAP;
        }
        assert!(p.locked());
        assert_eq!(p.begin_try(t).unwrap_err().0, ErrorCode::Locked);
        // locked stays locked: no auto renewal, even without keys
        p.tick(t + CODE_TTL, false);
        assert!(p.active(t + CODE_TTL).is_none());
        // a button press unlocks
        p.new_code(t);
        assert!(!p.locked() && p.active(t).is_some());
    }
}
