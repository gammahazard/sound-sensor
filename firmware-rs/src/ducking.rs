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
    VolumeUp,
    /// Restore TV volume. Carries the saved restore parameters so tv_task
    /// doesn't need to re-read the engine (avoids race on disarm).
    Restore { original_volume: Option<u8>, steps: u8 },
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

/// Maximum VolumeDown steps before we stop counting.
/// Prevents Samsung/Roku restore overshoot (they use relative key presses,
/// so phantom steps at volume 0 would cause too many VolumeUp presses).
const MAX_DUCK_STEPS: u8 = 30;

/// Baby cry temporal pattern tracker.
/// Tracks rhythmic burst-gap cycles (~1 Hz) characteristic of infant crying.
/// Runs independently of arm/disarm state — always active.
pub struct CryTracker {
    burst_windows: u8,
    gap_windows: u8,
    cycles: u8,
    pub crying: bool,
    cooldown_windows: u8,
}

impl CryTracker {
    pub const fn new() -> Self {
        Self { burst_windows: 0, gap_windows: 0, cycles: 0, crying: false, cooldown_windows: 0 }
    }

    /// Call every 100ms. Returns true only on the rising edge (first detection).
    pub fn tick(&mut self, is_cry: bool) -> bool {
        let was_crying = self.crying;

        if is_cry {
            self.burst_windows = self.burst_windows.saturating_add(1);
            if self.gap_windows >= 2 && self.gap_windows <= 5 && self.cycles < 10 {
                self.cycles += 1;
            }
            self.gap_windows = 0;
        } else {
            if self.burst_windows >= 3 && self.burst_windows <= 16 {
                self.gap_windows = self.gap_windows.saturating_add(1);
            } else if self.burst_windows > 0 {
                self.cycles = 0;
                self.gap_windows = 0;
            } else {
                self.gap_windows = self.gap_windows.saturating_add(1);
            }
            self.burst_windows = 0;
            if self.gap_windows > 5 {
                self.cycles = 0;
                self.gap_windows = 0;
            }
        }

        if self.cycles >= 3 {
            self.crying = true;
            self.cooldown_windows = 30;
        }

        if self.crying && !is_cry {
            if self.cooldown_windows > 0 {
                self.cooldown_windows -= 1;
            } else {
                self.crying = false;
                self.cycles = 0;
            }
        }

        if self.crying && self.cycles >= 3 {
            self.cooldown_windows = 30;
        }

        self.crying && !was_crying
    }
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
    pub duck_steps_taken: u8,

    /// TV volume captured just before the first VolumeDown command.
    /// Used on Restore to call setVolume instead of replaying VolumeUp presses.
    pub original_volume: Option<u8>,

    /// Baby cry temporal pattern tracker (always active, independent of armed).
    pub cry_tracker: CryTracker,
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
            cry_tracker: CryTracker::new(),
        }
    }

    /// Call every 100 ms with the latest dBFS reading.
    /// Returns what action (if any) the TV task should take.
    pub fn tick(&mut self, db: f32) -> DuckCommand {
        if !db.is_finite() {
            return DuckCommand::None;
        }
        if !self.armed {
            self.sustained_ms = 0;
            self.state = DuckingState::Quiet;
            return DuckCommand::None;
        }

        #[allow(unused)]
        let prev_state = self.state;
        #[allow(unused)]
        let prev_sustained = self.sustained_ms;

        // ── Accumulate / decay ──────────────────────────────────────────────
        if db > self.tripwire_db {
            self.sustained_ms = self.sustained_ms.saturating_add(100);
        } else {
            self.sustained_ms = self.sustained_ms.saturating_sub(50);
        }

        // Log sustained_ms milestones (1s, 2s, 3s crossings)
        #[cfg(feature = "dev-mode")]
        {
            use crate::dev_log::{LogCat, LogLevel};
            for &ms in &[1000u32, 2000, 3000] {
                if prev_sustained < ms && self.sustained_ms >= ms {
                    dev_log!(LogCat::Ducking, LogLevel::Info,
                        "sustained={}ms db={:.1}", ms, db);
                }
            }
        }

        // ── Restore checks (only while actively ducking) ────────────────────
        if self.state == DuckingState::Ducking {
            // Path A: Room is near-silent → restore immediately.
            // Handles: TV turned off, commercial break, user muted TV, etc.
            if db < self.floor_db + 2.0 {
                dev_log!(crate::dev_log::LogCat::Ducking, crate::dev_log::LogLevel::Info,
                    "restore_A: db={:.1} < floor+2={:.1}", db, self.floor_db + 2.0);
                let cmd = DuckCommand::Restore {
                    original_volume: self.original_volume,
                    steps: self.duck_steps_taken,
                };
                self.state = DuckingState::Restoring;
                self.sustained_ms = 0;
                return cmd;
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
                    dev_log!(crate::dev_log::LogCat::Ducking, crate::dev_log::LogLevel::Info,
                        "restore_B: hold_elapsed steps={}", self.duck_steps_taken);
                    let cmd = DuckCommand::Restore {
                        original_volume: self.original_volume,
                        steps: self.duck_steps_taken,
                    };
                    self.state = DuckingState::Restoring;
                    return cmd;
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
        // Skip if Restoring — tv_task is ramping volume back up; re-entering
        // Ducking would send VolumeDown while the ramp is still running.
        if self.sustained_ms >= 3000 && self.state != DuckingState::Restoring {
            self.state = DuckingState::Ducking;

            let excess = db - self.tripwire_db;
            #[allow(unused)]
            let prev_interval = self.duck_interval_ms;
            self.duck_interval_ms = if excess > 15.0 {
                500     // crisis: baby may wake fast, drop volume quickly
            } else if excess < 5.0 {
                2000    // gentle: barely over threshold, nudge slowly
            } else {
                1000    // standard: moderate excess
            };

            // Log rate tier changes
            #[cfg(feature = "dev-mode")]
            if prev_interval != self.duck_interval_ms {
                use crate::dev_log::{LogCat, LogLevel};
                let tier = match self.duck_interval_ms {
                    500  => "crisis",
                    2000 => "gentle",
                    _    => "standard",
                };
                dev_log!(LogCat::Ducking, LogLevel::Info,
                    "rate: {}({}ms) excess={:.1}dB", tier, self.duck_interval_ms, excess);
            }

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
                if self.duck_steps_taken >= MAX_DUCK_STEPS {
                    // Stop ducking further — TV is likely at 0.
                    // Prevents Samsung/Roku overshoot on restore.
                    dev_log!(crate::dev_log::LogCat::Ducking, crate::dev_log::LogLevel::Info,
                        "vol_down CAPPED at {} steps", MAX_DUCK_STEPS);
                } else {
                    self.duck_steps_taken += 1;
                    dev_log!(crate::dev_log::LogCat::Ducking, crate::dev_log::LogLevel::Info,
                        "vol_down step={} interval={}ms", self.duck_steps_taken, self.duck_interval_ms);
                    return DuckCommand::VolumeDown;
                }
            }
        } else if self.sustained_ms > 0 && self.state != DuckingState::Ducking && self.state != DuckingState::Restoring {
            self.state = DuckingState::Watching;
        }

        // Log state transitions
        #[cfg(feature = "dev-mode")]
        if self.state != prev_state {
            use crate::dev_log::{LogCat, LogLevel};
            let s = |st: DuckingState| match st {
                DuckingState::Quiet     => "quiet",
                DuckingState::Watching  => "watching",
                DuckingState::Ducking   => "ducking",
                DuckingState::Restoring => "restoring",
            };
            dev_log!(LogCat::Ducking, LogLevel::Info,
                "{}->{} db={:.1}", s(prev_state), s(self.state), db);
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
        self.state = DuckingState::Quiet;
    }

    /// Set tripwire with validation: must be at least floor + 6 dB.
    /// Prevents false-positive ducking from trivially small gaps.
    pub fn set_tripwire(&mut self, db: f32) {
        let min = self.floor_db + 6.0;
        if db < min {
            dev_log!(crate::dev_log::LogCat::Ducking, crate::dev_log::LogLevel::Warn,
                "tripwire {:.1} < floor+6={:.1}, clamped", db, min);
        }
        self.tripwire_db = if db < min { min } else { db };
    }

    /// Set floor with validation: reject < -80 dB (dead mic), clamp to -60.
    /// Re-clamp tripwire if floor changed so tripwire >= floor + 6.
    pub fn set_floor(&mut self, db: f32) {
        if db < -80.0 {
            dev_log!(crate::dev_log::LogCat::Ducking, crate::dev_log::LogLevel::Warn,
                "floor {:.1} < -80dB (dead mic?), clamped to -60", db);
        }
        let db = if db < -80.0 { -60.0 } else { db };
        self.floor_db = db;
        // Ensure tripwire is at least floor + 6 dB
        if self.tripwire_db < self.floor_db + 6.0 {
            dev_log!(crate::dev_log::LogCat::Ducking, crate::dev_log::LogLevel::Info,
                "tripwire bumped {:.1}->{:.1} (floor+6)", self.tripwire_db, self.floor_db + 6.0);
            self.tripwire_db = self.floor_db + 6.0;
        }
    }

    pub fn arm(&mut self)   { self.armed = true; }
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
    pub fn state(&self) -> DuckingState     { self.state }
    pub fn sustained_ms(&self) -> u32       { self.sustained_ms }
}

// ── Ducking task (always runs, independent of WebSocket) ─────────────────────

use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, mutex::Mutex};

/// Ducking tick task: consumes audio dB readings and drives the ducking engine.
/// Also consumes cry detection results for baby cry notification.
/// Runs independently of any WebSocket client — baby protection works even
/// with the browser closed or WiFi momentarily down.
#[embassy_executor::task]
pub async fn ducking_task(
    engine: &'static Mutex<ThreadModeRawMutex, DuckingEngine>,
) {
    let mut prev_led = crate::LedPattern::Idle;

    loop {
        let db = crate::DB_CHANNEL.receive().await;

        // Drain cry detection result (non-blocking, matches audio window rate)
        let is_cry = crate::CRY_CHANNEL.try_receive().unwrap_or(false);

        // Tick the ducking engine + cry tracker
        let (duck_cmd, armed, tripwire, ducking, crying, cry_onset) = {
            let mut eng = engine.lock().await;
            let cmd = eng.tick(db);
            let ducking = eng.state() == DuckingState::Ducking;
            let cry_onset = eng.cry_tracker.tick(is_cry);
            let crying = eng.cry_tracker.crying;
            if cry_onset {
                dev_log!(crate::dev_log::LogCat::Audio, crate::dev_log::LogLevel::Info,
                    "baby cry detected");
            }
            (cmd, eng.armed, eng.tripwire_db, ducking, crying, cry_onset)
        };

        // Update LED pattern ONLY when state changes (prevents led_step reset spam)
        let new_led = if ducking {
            crate::LedPattern::Ducking
        } else if armed {
            crate::LedPattern::Armed
        } else {
            crate::LedPattern::Idle
        };
        if new_led != prev_led {
            if crate::LED_CHANNEL.try_send(new_led).is_err() {
                dev_log!(crate::dev_log::LogCat::Ducking, crate::dev_log::LogLevel::Warn,
                    "LED_CHANNEL full");
            }
            prev_led = new_led;
        }

        // Dispatch duck commands to TV task
        if duck_cmd != DuckCommand::None {
            crate::tv::send_duck_command(duck_cmd).await;
        }

        // Update shared telemetry snapshot for ws.rs to read
        {
            let mut t = crate::TELEMETRY.lock().await;
            t.db = db;
            t.armed = armed;
            t.tripwire = tripwire;
            t.ducking = ducking;
            t.crying = crying;
        }
        // Update atomic tripwire for audio_task (lock-free read)
        crate::set_tripwire_db(tripwire);

        // Send one-shot baby_cry event (ws.rs reads via channel)
        if cry_onset {
            let _ = crate::CRY_EVENT_CH.try_send(());
        }

        // Notify ws.rs that new telemetry is available (non-blocking)
        let _ = crate::TELEM_SIGNAL.try_send(());
    }
}
