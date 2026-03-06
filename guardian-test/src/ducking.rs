//! ducking.rs — Adaptive ducking state machine (host-testable version)
//!
//! Identical logic to firmware-rs/src/ducking.rs but with `Instant` replaced
//! by an injectable `u64` millisecond timestamp via `tick_at()`.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DuckCommand {
    VolumeDown,
    VolumeUp,
    Restore { original_volume: Option<u8>, steps: u8 },
    None,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DuckingState {
    Quiet,
    Watching,
    Ducking,
    Restoring,
}

const RESTORE_HOLD_SECS: u64 = 30;
const MAX_DUCK_STEPS: u8 = 30;

pub struct DuckingEngine {
    pub tripwire_db: f32,
    pub floor_db: f32,
    pub armed: bool,

    sustained_ms: u32,
    state: DuckingState,
    last_duck_at_ms: Option<u64>,
    duck_interval_ms: u32,

    pub duck_steps_taken: u8,
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
            last_duck_at_ms: None,
            duck_interval_ms: 1000,
            duck_steps_taken: 0,
            original_volume: None,
        }
    }

    /// Testable tick: takes current time as milliseconds instead of Instant::now().
    pub fn tick_at(&mut self, db: f32, now_ms: u64) -> DuckCommand {
        if !db.is_finite() {
            return DuckCommand::None;
        }
        if !self.armed {
            self.sustained_ms = 0;
            self.state = DuckingState::Quiet;
            return DuckCommand::None;
        }

        // Accumulate / decay
        if db > self.tripwire_db {
            self.sustained_ms = self.sustained_ms.saturating_add(100);
        } else {
            self.sustained_ms = self.sustained_ms.saturating_sub(50);
        }

        // Restore checks (only while actively ducking)
        if self.state == DuckingState::Ducking {
            // Path A: Room is near-silent
            if db < self.floor_db + 2.0 {
                let cmd = DuckCommand::Restore {
                    original_volume: self.original_volume,
                    steps: self.duck_steps_taken,
                };
                self.state = DuckingState::Restoring;
                self.sustained_ms = 0;
                return cmd;
            }

            // Path B: sustained_ms decayed to 0 + hold timer elapsed
            if self.sustained_ms == 0 {
                let hold_elapsed = match self.last_duck_at_ms {
                    Some(t) => (now_ms.saturating_sub(t)) / 1000 >= RESTORE_HOLD_SECS,
                    None => true,
                };
                if hold_elapsed {
                    let cmd = DuckCommand::Restore {
                        original_volume: self.original_volume,
                        steps: self.duck_steps_taken,
                    };
                    self.state = DuckingState::Restoring;
                    return cmd;
                }
            }
        }

        // Watching → Quiet when counter fully decays
        if self.sustained_ms == 0 && self.state == DuckingState::Watching {
            self.state = DuckingState::Quiet;
        }

        // Ducking trigger
        // Skip if Restoring — tv_task is ramping volume back up.
        if self.sustained_ms >= 3000 && self.state != DuckingState::Restoring {
            self.state = DuckingState::Ducking;

            let excess = db - self.tripwire_db;
            self.duck_interval_ms = if excess > 15.0 {
                500
            } else if excess < 5.0 {
                2000
            } else {
                1000
            };

            let should_duck = match self.last_duck_at_ms {
                None => true,
                Some(t) => now_ms.saturating_sub(t) >= self.duck_interval_ms as u64,
            };

            if should_duck {
                self.last_duck_at_ms = Some(now_ms);
                if self.duck_steps_taken >= MAX_DUCK_STEPS {
                    // Stop ducking further — TV is likely at 0.
                } else {
                    self.duck_steps_taken += 1;
                    return DuckCommand::VolumeDown;
                }
            }
        } else if self.sustained_ms > 0 && self.state != DuckingState::Ducking && self.state != DuckingState::Restoring {
            self.state = DuckingState::Watching;
        }

        DuckCommand::None
    }

    pub fn set_original_volume(&mut self, v: u8) {
        if self.original_volume.is_none() {
            self.original_volume = Some(v);
        }
    }

    pub fn clear_duck_state(&mut self) {
        self.duck_steps_taken = 0;
        self.original_volume = None;
        self.last_duck_at_ms = None;
        self.state = DuckingState::Quiet;
    }

    pub fn set_tripwire(&mut self, db: f32) {
        let min = self.floor_db + 6.0;
        self.tripwire_db = if db < min { min } else { db };
    }

    pub fn set_floor(&mut self, db: f32) {
        let db = if db < -80.0 { -60.0 } else { db };
        self.floor_db = db;
        if self.tripwire_db < self.floor_db + 6.0 {
            self.tripwire_db = self.floor_db + 6.0;
        }
    }

    pub fn arm(&mut self) {
        self.armed = true;
    }
    /// Returns a Restore command with captured params if actively ducking, else None.
    pub fn disarm(&mut self) -> DuckCommand {
        let cmd = if self.state == DuckingState::Ducking {
            DuckCommand::Restore {
                original_volume: self.original_volume,
                steps: self.duck_steps_taken,
            }
        } else {
            DuckCommand::None
        };
        self.armed = false;
        self.sustained_ms = 0;
        self.state = DuckingState::Quiet;
        self.clear_duck_state();
        cmd
    }
    pub fn state(&self) -> DuckingState {
        self.state
    }
    pub fn sustained_ms(&self) -> u32 {
        self.sustained_ms
    }
}
