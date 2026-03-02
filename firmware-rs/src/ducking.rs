//! ducking.rs — Adaptive ducking state machine
//!
//! Runs entirely on the Pico; makes ducking decisions and returns commands
//! for the tv_task to execute.
//!
//! Algorithm (every 100ms tick):
//!
//!   1. Accumulate / decay:
//!      if db > tripwire → sustained_ms += 100
//!      else             → sustained_ms = max(0, sustained_ms - 50)
//!
//!   2. Ducking trigger (sustained_ms >= 3000):
//!      excess = db - tripwire
//!      rate = >15 dB → 500ms (crisis)  |  5-15 dB → 1000ms  |  <5 dB → 2000ms (gentle)
//!      emit VolumeDown at rate
//!
//!   3. Restore (two paths):
//!      Path A: db < floor + 2  →  restore immediately (room is nearly silent)
//!      Path B: sustained_ms decayed to 0 AND 30s since last VolumeDown
//!              →  restore (loud scene ended, ducked TV still audible)
//!
//! The 30-second hold prevents oscillation: after ducking the TV, the lower
//! volume causes dB to drop below tripwire. Without the hold, we'd restore
//! immediately, the TV goes loud again, and we re-duck 3 seconds later.
//! The hold ensures the loud content has actually ended.
//!
//! Baby wake timing:
//!   - Detection: 3 seconds (filters brief sounds: doors, coughs, footsteps)
//!   - Ramp-down: 2-10 seconds depending on how loud (crisis=0.5s/step, gentle=2s/step)
//!   - Total loud exposure: 5-13 seconds before volume is significantly reduced
//!   - Research suggests babies in light sleep tolerate ~10-20s of moderate noise

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

/// Minimum seconds after the last VolumeDown before restore via Path B.
/// Prevents duck/restore oscillation when the sensor hears its own ducked output.
const RESTORE_HOLD_SECS: u64 = 30;

pub struct DuckingEngine {
    pub tripwire_db:  f32,
    pub floor_db:     f32,
    pub armed:        bool,

    sustained_ms:     u32,
    state:            DuckingState,
    last_duck_at:     Option<Instant>,
    duck_interval_ms: u32,

    /// How many VolumeDown commands have been sent since the last restore.
    pub duck_steps_taken: u8,

    /// TV volume captured just before the first VolumeDown command.
    /// Used on Restore to call setVolume instead of replaying VolumeUp presses.
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

        // ── Accumulate / decay ──────────────────────────────────────────────
        if db > self.tripwire_db {
            self.sustained_ms = self.sustained_ms.saturating_add(100);
        } else {
            self.sustained_ms = self.sustained_ms.saturating_sub(50);
        }

        // ── Restore checks (only while actively ducking) ────────────────────
        if self.state == DuckingState::Ducking {
            // Path A: Room is near-silent → restore immediately.
            // Handles: TV turned off, commercial break, user muted TV, etc.
            if db < self.floor_db + 2.0 {
                self.state = DuckingState::Restoring;
                self.sustained_ms = 0;
                return DuckCommand::Restore;
            }

            // Path B: Noise has been below tripwire long enough for sustained_ms
            // to fully decay, AND the hold timer has elapsed.
            // Handles: loud scene ended but ducked TV still audible above floor.
            if self.sustained_ms == 0 {
                let hold_elapsed = match self.last_duck_at {
                    Some(t) => Instant::now().duration_since(t).as_secs() >= RESTORE_HOLD_SECS,
                    None => true,
                };
                if hold_elapsed {
                    self.state = DuckingState::Restoring;
                    return DuckCommand::Restore;
                }
                // Hold not elapsed yet — stay in Ducking state, wait.
            }
        }

        // Transition from Watching → Quiet when counter fully decays
        // (NOT from Ducking — that's handled above with restore logic)
        if self.sustained_ms == 0 && self.state == DuckingState::Watching {
            self.state = DuckingState::Quiet;
        }

        // ── Ducking trigger ─────────────────────────────────────────────────
        if self.sustained_ms >= 3000 {
            self.state = DuckingState::Ducking;

            let excess = db - self.tripwire_db;
            self.duck_interval_ms = if excess > 15.0 {
                500     // crisis: baby may wake fast, drop volume quickly
            } else if excess < 5.0 {
                2000    // gentle: barely over threshold, nudge slowly
            } else {
                1000    // standard: moderate excess
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

    /// Set tripwire with validation: must be at least floor + 6 dB.
    /// Prevents false-positive ducking from trivially small gaps.
    pub fn set_tripwire(&mut self, db: f32) {
        let min = self.floor_db + 6.0;
        self.tripwire_db = if db < min { min } else { db };
    }

    /// Set floor with validation: reject < -80 dB (dead mic), clamp to -60.
    /// Re-clamp tripwire if floor changed so tripwire >= floor + 6.
    pub fn set_floor(&mut self, db: f32) {
        let db = if db < -80.0 { -60.0 } else { db };
        self.floor_db = db;
        // Ensure tripwire is at least floor + 6 dB
        if self.tripwire_db < self.floor_db + 6.0 {
            self.tripwire_db = self.floor_db + 6.0;
        }
    }

    pub fn arm(&mut self)   { self.armed = true; }
    pub fn disarm(&mut self) {
        self.armed = false;
        self.sustained_ms = 0;
        self.clear_duck_state();
    }
    pub fn state(&self) -> DuckingState     { self.state }
    pub fn sustained_ms(&self) -> u32       { self.sustained_ms }
}
