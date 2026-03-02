//! ducking.rs — Adaptive ducking state machine
//!
//! Runs entirely on the Pico; makes ducking decisions and returns commands
//! for the tv_task to execute.
//!
//! Algorithm (from the plan):
//!   Every 100ms tick:
//!     if db > tripwire → sustained_ms += 100
//!     else             → sustained_ms = max(0, sustained_ms - 50)  (decay)
//!
//!   if sustained_ms >= 3000:
//!     excess = db - tripwire
//!     rate = match excess {
//!         > 15 → 500 ms   (crisis: fast drops)
//!         < 5  → 2000 ms  (fine: gentle nudge)
//!         _    → 1000 ms  (standard)
//!     }
//!     emit DuckCommand::VolumeDown  if interval elapsed
//!
//!   if db < floor + 2:
//!     emit DuckCommand::Restore

use embassy_time::Instant;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DuckCommand {
    VolumeDown,
    Restore,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DuckingState {
    Quiet,
    Watching,   // sustained_ms accumulating but < 3000
    Ducking,    // actively ducking
    Restoring,  // level dropped; restoring volume
}

pub struct DuckingEngine {
    pub tripwire_db:  f32,
    pub floor_db:     f32,
    pub armed:        bool,

    sustained_ms:     u32,
    state:            DuckingState,
    last_duck_at:     Option<Instant>,
    duck_interval_ms: u32,

    /// How many VolumeDown commands have been sent since the last restore.
    /// Reset to 0 on disarm or restore.
    pub duck_steps_taken: u8,

    /// TV volume captured just before the first VolumeDown command.
    /// Set by tv_task after querying ssap://audio/getVolume.
    /// Used on Restore to call setVolume instead of a single volumeUp.
    pub original_volume: Option<u8>,
}

impl DuckingEngine {
    pub fn new(tripwire_db: f32, floor_db: f32) -> Self {
        Self {
            tripwire_db,
            floor_db,
            armed: false,
            sustained_ms: 0,
            state: DuckingState::Quiet,
            last_duck_at: None,
            duck_interval_ms: 1000,
            duck_steps_taken: 0,
            original_volume: None,
        }
    }

    /// Call every 100 ms with the latest dBFS reading.
    /// Returns what action (if any) the TV task should take.
    pub fn tick(&mut self, db: f32) -> DuckCommand {
        if !self.armed {
            self.sustained_ms = 0;
            self.state = DuckingState::Quiet;
            return DuckCommand::None;
        }

        // ── Accumulate / decay ────────────────────────────────────────────────
        if db > self.tripwire_db {
            self.sustained_ms = self.sustained_ms.saturating_add(100);
        } else {
            self.sustained_ms = self.sustained_ms.saturating_sub(50);
        }

        // ── Restore check ─────────────────────────────────────────────────────
        if db < self.floor_db + 2.0 && self.state == DuckingState::Ducking {
            self.state = DuckingState::Restoring;
            self.sustained_ms = 0;
            return DuckCommand::Restore;
        }

        if self.sustained_ms == 0 && self.state != DuckingState::Quiet {
            self.state = DuckingState::Quiet;
        }

        // ── Ducking trigger ───────────────────────────────────────────────────
        if self.sustained_ms >= 3000 {
            self.state = DuckingState::Ducking;

            let excess = db - self.tripwire_db;
            self.duck_interval_ms = if excess > 15.0 {
                500
            } else if excess < 5.0 {
                2000
            } else {
                1000
            };

            let now = Instant::now();
            let should_duck = match self.last_duck_at {
                None => true,
                Some(t) => {
                    now.duration_since(t).as_millis()
                        >= self.duck_interval_ms as u64
                }
            };

            if should_duck {
                self.last_duck_at = Some(now);
                self.duck_steps_taken = self.duck_steps_taken.saturating_add(1);
                return DuckCommand::VolumeDown;
            }
        } else if self.sustained_ms > 0 {
            self.state = DuckingState::Watching;
        }

        DuckCommand::None
    }

    /// Called by tv_task after it successfully queries the TV's current volume.
    /// Only stores the value if we haven't captured it yet (first duck in a session).
    pub fn set_original_volume(&mut self, v: u8) {
        if self.original_volume.is_none() {
            self.original_volume = Some(v);
        }
    }

    /// Reset ducking counters. Called on disarm and after a successful restore.
    pub fn clear_duck_state(&mut self) {
        self.duck_steps_taken = 0;
        self.original_volume = None;
        self.last_duck_at = None;
    }

    pub fn set_tripwire(&mut self, db: f32) { self.tripwire_db = db; }
    pub fn set_floor(&mut self, db: f32)    { self.floor_db = db; }
    pub fn arm(&mut self)                   { self.armed = true; }
    pub fn disarm(&mut self) {
        self.armed = false;
        self.sustained_ms = 0;
        self.clear_duck_state();
    }
    pub fn state(&self) -> DuckingState     { self.state }
    pub fn sustained_ms(&self) -> u32       { self.sustained_ms }
}
