//! audio.rs — dB computation + Goertzel cry detection extracted from firmware audio.rs

const FULL_SCALE_24: f32 = 8_388_608.0; // 2^23

pub fn compute_db(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -96.0;
    }
    let mut sum: f32 = 0.0;
    for &s in samples {
        sum += s * s;
    }
    let rms = libm::sqrtf(sum / samples.len() as f32);
    if rms < 1.0 {
        return -96.0;
    }
    let db = 20.0 * libm::log10f(rms / FULL_SCALE_24);
    db.clamp(-96.0, 0.0)
}

// ── Goertzel algorithm for tone detection ─────────────────────────────────
//
// Computes DFT magnitude at a single frequency bin without a full FFT.
// Standard approach for tone detection on microcontrollers (DTMF decoders).
//
// Window: 1600 samples at 16 kHz = 100 ms → 10 Hz bin width.
// k = target_freq / bin_width = target_freq / 10.

pub const WINDOW_N: usize = 1_600;

/// Number of F0 bins covering the baby cry fundamental range (350–550 Hz).
pub const NUM_F0_BINS: usize = 5;

pub struct GoertzelBin {
    coeff: f32, // 2 * cos(2π * k / N), pre-computed
    s1: f32,    // v[n-1]
    s2: f32,    // v[n-2]
}

impl GoertzelBin {
    /// Create a new Goertzel bin for frequency index k.
    /// k = target_frequency / (sample_rate / N) = target_frequency / 10
    pub fn new(k: u32) -> Self {
        let coeff = 2.0 * libm::cosf(2.0 * core::f32::consts::PI * k as f32 / WINDOW_N as f32);
        Self { coeff, s1: 0.0, s2: 0.0 }
    }

    /// Create from a pre-computed coefficient (for firmware const init).
    pub fn from_coeff(coeff: f32) -> Self {
        Self { coeff, s1: 0.0, s2: 0.0 }
    }

    /// Feed one sample into the filter.
    #[inline]
    pub fn push(&mut self, sample: f32) {
        let s0 = sample + self.coeff * self.s1 - self.s2;
        self.s2 = self.s1;
        self.s1 = s0;
    }

    /// Compute the power (magnitude squared) at the target frequency.
    /// Call after processing all N samples in the window.
    pub fn power(&self) -> f32 {
        self.s1 * self.s1 + self.s2 * self.s2 - self.coeff * self.s1 * self.s2
    }

    /// Reset state for the next window.
    pub fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }
}

// ── Hanning window ────────────────────────────────────────────────────────
// Reduces spectral leakage from -13 dB (rectangular) to -31 dB sidelobes.

/// Compute Hanning window coefficient for sample index i in window of size n.
pub fn hanning(i: usize, n: usize) -> f32 {
    0.5 - 0.5 * libm::cosf(2.0 * core::f32::consts::PI * i as f32 / n as f32)
}

// ── Multi-bin cry detection ───────────────────────────────────────────────
//
// 5 F0 bins covering the real baby cry F0 range (350–550 Hz):
//   350 Hz (k=35), 400 Hz (k=40), 450 Hz (k=45), 500 Hz (k=50), 550 Hz (k=55)
//
// 5 adaptive harmonic bins at 2× each F0:
//   700 Hz (k=70), 800 Hz (k=80), 900 Hz (k=90), 1000 Hz (k=100), 1100 Hz (k=110)
//
// Additional features: ZCR (zero-crossing rate), spectral flatness,
// harmonic-to-total energy ratio.

/// Evaluate whether the current 100ms window looks like a baby cry.
///
/// Uses 6 checks:
/// 1. Loud enough (db >= tripwire)
/// 2. Strong F0 energy in cry band (best F0 bin > noise floor)
/// 3. Harmonic present at 2× the strongest F0 (≥5% of fundamental)
/// 4. ZCR consistent with 350–550 Hz tonal source (50–130 crossings per 100ms window)
/// 5. Tonal energy ratio: cry concentrates energy at F0+harmonic vs total
/// 6. Spectral peakedness: one F0 bin dominates (cry) vs flat (broadband)
pub fn is_cry_like(
    f0_powers: &[f32; NUM_F0_BINS],
    harm_powers: &[f32; NUM_F0_BINS],
    zc_count: u32,
    total_energy: f32,
    db: f32,
    tripwire: f32,
) -> bool {
    // 1. Must be loud enough
    if db < tripwire {
        return false;
    }

    // Find strongest F0 bin
    let mut best_idx = 0usize;
    let mut best_power = f0_powers[0];
    for i in 1..NUM_F0_BINS {
        if f0_powers[i] > best_power {
            best_power = f0_powers[i];
            best_idx = i;
        }
    }

    // 2. Noise threshold: best F0 bin must have meaningful energy
    if best_power < 1e6 {
        return false;
    }

    // 3. Adaptive harmonic check: harmonic at 2× the strongest F0
    if harm_powers[best_idx] < best_power * 0.05 {
        return false;
    }

    // 4. ZCR band check: baby cries at 350–550 Hz → 70–110 zero crossings
    // per 1600-sample (100ms) window. Allow wider margin (50–130) for harmonics.
    if zc_count < 50 || zc_count > 130 {
        return false;
    }

    // 5. Harmonic-to-total energy ratio: tonal sources concentrate energy
    // at F0 + harmonic; broadband noise spreads it everywhere.
    if total_energy > 0.0 {
        let tonal = best_power + harm_powers[best_idx];
        if tonal / total_energy < 0.005 {
            return false;
        }
    }

    // 6. Spectral peakedness across F0 bins.
    // Baby cries peak at one F0; broadband noise spreads equally across bins.
    // Ratio of best bin to average: cry > 2.0, broadband ≈ 1.0.
    let avg_f0 = f0_powers.iter().sum::<f32>() / NUM_F0_BINS as f32;
    if avg_f0 > 0.0 && best_power / avg_f0 < 1.8 {
        return false;
    }

    true
}

/// Count zero crossings in a sample buffer.
pub fn count_zero_crossings(samples: &[f32]) -> u32 {
    if samples.len() < 2 {
        return 0;
    }
    let mut count = 0u32;
    for i in 1..samples.len() {
        if (samples[i] > 0.0) != (samples[i - 1] > 0.0) {
            count += 1;
        }
    }
    count
}

/// Temporal cry pattern tracker.
/// Baby cries have a distinctive ~1 Hz rhythmic pattern:
/// 0.5–1.6s crying burst, 0.3–0.4s breath pause, repeating.
///
/// Detection requires 3+ burst-gap cycles to confirm crying,
/// filtering brief false positives from TV scenes with 450 Hz tones.
pub struct CryTracker {
    burst_windows: u8,     // consecutive cry-positive windows (100ms each)
    gap_windows: u8,       // consecutive cry-negative windows
    cycles: u8,            // completed burst-gap cycles
    pub crying: bool,      // confirmed crying (3+ cycles)
    cooldown_windows: u8,  // windows remaining before crying flag clears
}

impl CryTracker {
    pub fn new() -> Self {
        Self {
            burst_windows: 0,
            gap_windows: 0,
            cycles: 0,
            crying: false,
            cooldown_windows: 0,
        }
    }

    /// Call every 100ms with the is_cry_like result for the current window.
    /// Returns true when crying is first confirmed (for one-shot event).
    pub fn tick(&mut self, is_cry: bool) -> bool {
        let was_crying = self.crying;

        if is_cry {
            self.burst_windows = self.burst_windows.saturating_add(1);
            // If we were in a gap and it was valid length (2–5 windows = 200–500ms),
            // count it as a completed cycle
            if self.gap_windows >= 2 && self.gap_windows <= 5 && self.cycles < 10 {
                self.cycles += 1;
            }
            self.gap_windows = 0;
        } else {
            // Burst was valid length? (3–16 windows = 300ms–1.6s)
            if self.burst_windows >= 3 && self.burst_windows <= 16 {
                self.gap_windows = self.gap_windows.saturating_add(1);
            } else if self.burst_windows > 0 {
                // Burst too short or too long — reset pattern
                self.cycles = 0;
                self.gap_windows = 0;
            } else {
                // No burst was active, just accumulate gap
                self.gap_windows = self.gap_windows.saturating_add(1);
            }
            self.burst_windows = 0;

            // If gap gets too long (>500ms = 5 windows without cry), pattern broken
            if self.gap_windows > 5 {
                self.cycles = 0;
                self.gap_windows = 0;
            }
        }

        // Confirm crying after 3+ burst-gap cycles
        if self.cycles >= 3 {
            self.crying = true;
            self.cooldown_windows = 30; // 3 seconds after last confirmation
        }

        // Cooldown: keep crying flag active for a few seconds after pattern stops
        if self.crying && !is_cry {
            if self.cooldown_windows > 0 {
                self.cooldown_windows -= 1;
            } else {
                self.crying = false;
                self.cycles = 0;
            }
        }

        // Reset cooldown timer when cry pattern still active
        if self.crying && self.cycles >= 3 {
            self.cooldown_windows = 30;
        }

        // Return true only on the rising edge (first detection)
        self.crying && !was_crying
    }
}
