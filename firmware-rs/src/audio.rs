//! audio.rs — PIO I²S master-mode capture + RMS computation + cry detection
//!
//! The SPH0645LM4H sends 24-bit samples left-justified inside 32-bit words.
//! The mic is an I2S slave — the Pico must generate BCLK and WS clocks.
//!
//! PIO program: master-mode — generates BCLK + WS via side-set, reads DOUT.
//! BCLK=GP0, LRCL=GP1 (must be consecutive for side-set), DOUT=GP2.
//!
//! Sample rate: 16 kHz  →  window = 1600 samples = 100 ms
//!
//! Cry detection: 10-bin Goertzel (5 F0 + 5 harmonic) with Hanning window,
//! ZCR check, spectral peakedness, and harmonic-to-total ratio.

use defmt::*;
use embassy_rp::{
    peripherals::{DMA_CH0, PIO0},
    pio::{Config, Direction, FifoJoin, Pio, ShiftConfig, ShiftDirection},
};
use fixed::traits::ToFixed;
use libm::log10f;

use crate::{DB_CHANNEL, CRY_CHANNEL};

const SAMPLE_RATE:    u32 = 16_000;
const WINDOW_SAMPLES: usize = 1_600;   // 100 ms at 16 kHz
const FULL_SCALE_24:  f32 = 8_388_608.0; // 2^23
const NUM_F0_BINS:    usize = 5;

// ── Goertzel algorithm for tone detection ─────────────────────────────────
// Computes DFT magnitude at specific frequencies without a full FFT.
//
// 5 F0 bins covering baby cry fundamental range (350–550 Hz):
//   350 Hz (k=35), 400 Hz (k=40), 450 Hz (k=45), 500 Hz (k=50), 550 Hz (k=55)
//
// 5 adaptive harmonic bins at 2× each F0:
//   700 Hz (k=70), 800 Hz (k=80), 900 Hz (k=90), 1000 Hz (k=100), 1100 Hz (k=110)

struct GoertzelBin {
    coeff: f32,
    s1: f32,
    s2: f32,
}

impl GoertzelBin {
    const fn from_coeff(coeff: f32) -> Self {
        Self { coeff, s1: 0.0, s2: 0.0 }
    }

    #[inline]
    fn push(&mut self, sample: f32) {
        let s0 = sample + self.coeff * self.s1 - self.s2;
        self.s2 = self.s1;
        self.s1 = s0;
    }

    fn power(&self) -> f32 {
        self.s1 * self.s1 + self.s2 * self.s2 - self.coeff * self.s1 * self.s2
    }

    fn reset(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }
}

// Pre-computed Goertzel coefficients: 2 * cos(2π * k / 1600)
// F0 bins (baby cry fundamental range)
const COEFF_350:  f32 = 1.981139;  // k=35
const COEFF_400:  f32 = 1.975377;  // k=40
const COEFF_450:  f32 = 1.968853;  // k=45
const COEFF_500:  f32 = 1.961571;  // k=50
const COEFF_550:  f32 = 1.953532;  // k=55
// Harmonic bins (2× each F0)
const COEFF_700:  f32 = 1.924910;  // k=70
const COEFF_800:  f32 = 1.902113;  // k=80
const COEFF_900:  f32 = 1.876383;  // k=90
const COEFF_1000: f32 = 1.847759;  // k=100
const COEFF_1100: f32 = 1.816286;  // k=110

// Hanning window recursive oscillator: w[n] = cos(2π·n/N)
// alpha = 2·cos(2π/N), w[n] = alpha·w[n-1] − w[n-2]
const HANN_ALPHA: f32 = 1.999985;  // 2·cos(2π/1600)

// 2nd-order Butterworth high-pass filter at 200 Hz (fs = 16 kHz)
const HPF_B0: f32 =  0.9460;
const HPF_B1: f32 = -1.8920;
const HPF_B2: f32 =  0.9460;
const HPF_A1: f32 = -1.8890;
const HPF_A2: f32 =  0.8949;

#[embassy_executor::task]
pub async fn audio_task(
    pio:      PIO0,
    _dma_ch:  DMA_CH0,
    bclk_pin: embassy_rp::peripherals::PIN_0,
    lrcl_pin: embassy_rp::peripherals::PIN_1,
    data_pin: embassy_rp::peripherals::PIN_2,
) {
    info!("[audio] task started");

    let Pio { mut common, mut sm0, .. } = Pio::new(pio, crate::Irqs);

    // PIO I²S master-mode receive program
    // Side-set 2 bits: bit0 = BCLK, bit1 = WS/LRCL
    let prog = pio_proc::pio_asm!(
        ".side_set 2",
        "    set x, 30          side 0b00",
        "left_data:",
        "    in pins, 1         side 0b01",
        "    jmp x-- left_data  side 0b00",
        "    in pins, 1         side 0b11",
        "    set x, 30          side 0b10",
        "right_data:",
        "    in pins, 1         side 0b11",
        "    jmp x-- right_data side 0b10",
        "    in pins, 1         side 0b01",
    );

    let mut cfg = Config::default();

    let bclk_pin = common.make_pio_pin(bclk_pin);
    let lrcl_pin = common.make_pio_pin(lrcl_pin);
    cfg.use_program(&common.load_program(&prog.program), &[&bclk_pin, &lrcl_pin]);

    let data_pin = common.make_pio_pin(data_pin);
    cfg.set_in_pins(&[&data_pin]);

    cfg.shift_in = ShiftConfig {
        auto_fill: true,
        threshold: 32,
        direction: ShiftDirection::Left,
    };
    cfg.fifo_join = FifoJoin::RxOnly;

    cfg.clock_divider = 73u8.to_fixed();

    sm0.set_config(&cfg);
    sm0.set_pin_dirs(Direction::Out, &[&bclk_pin, &lrcl_pin]);
    sm0.set_pin_dirs(Direction::In, &[&data_pin]);
    sm0.set_enable(true);

    info!("[audio] PIO I2S master @ {}Hz, div=73", SAMPLE_RATE);

    // ── HPF state (2nd-order biquad) ─────────────────────────────────────
    let mut hpf_x1: f32 = 0.0;
    let mut hpf_x2: f32 = 0.0;
    let mut hpf_y1: f32 = 0.0;
    let mut hpf_y2: f32 = 0.0;

    // Warmup: 200ms — mic PDM stabilization + HPF settling
    for _ in 0..(SAMPLE_RATE as usize / 5) {
        let raw: u32 = sm0.rx().wait_pull().await;
        let _right: u32 = sm0.rx().wait_pull().await;
        let sample = ((raw as i32) >> 8) as f32;
        let y = HPF_B0 * sample + HPF_B1 * hpf_x1 + HPF_B2 * hpf_x2
              - HPF_A1 * hpf_y1 - HPF_A2 * hpf_y2;
        hpf_x2 = hpf_x1;
        hpf_x1 = sample;
        hpf_y2 = hpf_y1;
        hpf_y1 = y;
    }
    info!("[audio] Mic warmup complete");

    // ── Sample loop ───────────────────────────────────────────────────────
    let mut buf  = [0f32; WINDOW_SAMPLES];
    let mut idx  = 0usize;
    let mut smoothed: f32 = -96.0;
    const EMA_ATTACK: f32 = 0.3;
    const EMA_DECAY:  f32 = 0.08;

    // 10 Goertzel bins: 5 F0 (350–550 Hz) + 5 harmonic (700–1100 Hz)
    let mut g_f0 = [
        GoertzelBin::from_coeff(COEFF_350),
        GoertzelBin::from_coeff(COEFF_400),
        GoertzelBin::from_coeff(COEFF_450),
        GoertzelBin::from_coeff(COEFF_500),
        GoertzelBin::from_coeff(COEFF_550),
    ];
    let mut g_harm = [
        GoertzelBin::from_coeff(COEFF_700),
        GoertzelBin::from_coeff(COEFF_800),
        GoertzelBin::from_coeff(COEFF_900),
        GoertzelBin::from_coeff(COEFF_1000),
        GoertzelBin::from_coeff(COEFF_1100),
    ];

    // Hanning window via recursive cosine oscillator
    // w[n] = cos(2π·n/N), computed as w[n] = alpha·w[n-1] − w[n-2]
    let hann_w2_init: f32 = libm::cosf(2.0 * core::f32::consts::PI * (WINDOW_SAMPLES - 1) as f32 / WINDOW_SAMPLES as f32);
    let mut hann_w1: f32 = 1.0;  // cos(0) = 1
    let mut hann_w2: f32 = hann_w2_init;

    // Zero-crossing rate state
    let mut zc_count: u32 = 0;
    let mut prev_sign_positive: bool = false;

    // Total energy accumulator (sum of filtered² for harmonic-to-total ratio)
    let mut energy_sum: f32 = 0.0;

    loop {
        let raw: u32 = sm0.rx().wait_pull().await;
        let _right: u32 = sm0.rx().wait_pull().await;

        let sample = ((raw as i32) >> 8) as f32;

        // 2nd-order Butterworth HPF at 200 Hz
        let filtered = HPF_B0 * sample + HPF_B1 * hpf_x1 + HPF_B2 * hpf_x2
                     - HPF_A1 * hpf_y1 - HPF_A2 * hpf_y2;
        hpf_x2 = hpf_x1;
        hpf_x1 = sample;
        hpf_y2 = hpf_y1;
        hpf_y1 = filtered;

        buf[idx] = filtered;

        // Hanning window: h[n] = 0.5 − 0.5·cos(2π·n/N)
        // Recursive cosine: w0 = alpha·w1 − w2
        let hann_w0 = HANN_ALPHA * hann_w1 - hann_w2;
        let hann_coeff = 0.5 - 0.5 * hann_w1;
        hann_w2 = hann_w1;
        hann_w1 = hann_w0;
        let windowed = filtered * hann_coeff;

        // Feed all 10 Goertzel bins with windowed sample
        for bin in g_f0.iter_mut() { bin.push(windowed); }
        for bin in g_harm.iter_mut() { bin.push(windowed); }

        // Zero-crossing count (on unwindowed filtered signal)
        let sign_pos = filtered > 0.0;
        if idx > 0 && sign_pos != prev_sign_positive {
            zc_count += 1;
        }
        prev_sign_positive = sign_pos;

        // Accumulate total energy
        energy_sum += filtered * filtered;

        idx += 1;

        if idx >= WINDOW_SAMPLES {
            idx = 0;

            // ── dB computation ───────────────────────────────────────────
            let db = compute_db(&buf);
            let alpha = if db > smoothed { EMA_ATTACK } else { EMA_DECAY };
            smoothed = alpha * db + (1.0 - alpha) * smoothed;
            if DB_CHANNEL.try_send(smoothed).is_err() {
                dev_log!(crate::dev_log::LogCat::Audio, crate::dev_log::LogLevel::Warn,
                    "DB_CHANNEL full, dropped db={:.1}", smoothed);
            }

            // ── Cry detection (6-check pipeline) ─────────────────────────
            let tripwire = crate::TELEMETRY.try_lock().map(|t| t.tripwire).unwrap_or(-20.0);

            // Gather Goertzel bin powers
            let f0_powers = [
                g_f0[0].power(), g_f0[1].power(), g_f0[2].power(),
                g_f0[3].power(), g_f0[4].power(),
            ];
            let harm_powers = [
                g_harm[0].power(), g_harm[1].power(), g_harm[2].power(),
                g_harm[3].power(), g_harm[4].power(),
            ];

            let cry_like = evaluate_cry(
                &f0_powers, &harm_powers, zc_count, energy_sum, smoothed, tripwire,
            );

            if CRY_CHANNEL.try_send(cry_like).is_err() {
                dev_log!(crate::dev_log::LogCat::Audio, crate::dev_log::LogLevel::Warn,
                    "CRY_CHANNEL full");
            }

            // Reset all bins + counters for next window
            for bin in g_f0.iter_mut() { bin.reset(); }
            for bin in g_harm.iter_mut() { bin.reset(); }
            zc_count = 0;
            prev_sign_positive = false;
            energy_sum = 0.0;

            // Reset Hanning oscillator for next window
            hann_w1 = 1.0;
            hann_w2 = hann_w2_init;
        }
    }
}

/// Six-check cry detection pipeline.
/// Matches the logic in guardian-test/src/audio.rs::is_cry_like.
fn evaluate_cry(
    f0_powers: &[f32; NUM_F0_BINS],
    harm_powers: &[f32; NUM_F0_BINS],
    zc_count: u32,
    total_energy: f32,
    db: f32,
    tripwire: f32,
) -> bool {
    // 1. Loud enough
    if db < tripwire { return false; }

    // 2. Find strongest F0 bin + noise threshold
    let mut best_idx = 0usize;
    let mut best_power = f0_powers[0];
    for i in 1..NUM_F0_BINS {
        if f0_powers[i] > best_power {
            best_power = f0_powers[i];
            best_idx = i;
        }
    }
    if best_power < 1e6 { return false; }

    // 3. Adaptive harmonic at 2× the strongest F0
    if harm_powers[best_idx] < best_power * 0.05 { return false; }

    // 4. ZCR band check (350–550 Hz → 70–110 crossings per 100ms window)
    if zc_count < 50 || zc_count > 130 { return false; }

    // 5. Harmonic-to-total energy ratio
    if total_energy > 0.0 {
        let tonal = best_power + harm_powers[best_idx];
        if tonal / total_energy < 0.005 { return false; }
    }

    // 6. Spectral peakedness: best F0 bin vs average of all F0 bins
    let avg_f0 = f0_powers.iter().sum::<f32>() / NUM_F0_BINS as f32;
    if avg_f0 > 0.0 && best_power / avg_f0 < 1.8 { return false; }

    true
}

fn compute_db(samples: &[f32]) -> f32 {
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
    let db = 20.0 * log10f(rms / FULL_SCALE_24);
    db.clamp(-96.0, 0.0)
}
