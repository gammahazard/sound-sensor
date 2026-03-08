use guardian_test::audio::{
    compute_db, GoertzelBin, is_cry_like, CryTracker,
    count_zero_crossings, hanning, NUM_F0_BINS, WINDOW_N,
};

// ── compute_db tests ────────────────────────────────────────────────────────

#[test]
fn silence_returns_minus_96() {
    let samples = [0.0f32; 1600];
    assert_eq!(compute_db(&samples), -96.0);
}

#[test]
fn full_scale_returns_near_zero() {
    let samples = [8_388_607.0f32; 1600];
    let db = compute_db(&samples);
    assert!(db > -0.01, "Expected ~0 dBFS, got {}", db);
    assert!(db <= 0.0, "Expected <= 0 dBFS, got {}", db);
}

#[test]
fn known_rms() {
    let val = 838_861.0f32;
    let samples = [val; 1600];
    let db = compute_db(&samples);
    assert!((db - (-20.0)).abs() < 0.1, "Expected ~-20 dBFS, got {}", db);
}

#[test]
fn negative_samples() {
    let pos = [100_000.0f32; 1600];
    let neg = [-100_000.0f32; 1600];
    assert!((compute_db(&pos) - compute_db(&neg)).abs() < 0.001);
}

#[test]
fn single_sample() {
    let samples = [8_388_607.0f32; 1];
    let db = compute_db(&samples);
    assert!(db > -0.01 && db <= 0.0);
}

#[test]
fn compute_db_empty_slice() {
    assert_eq!(compute_db(&[]), -96.0);
}

#[test]
fn very_quiet() {
    let samples = [10.0f32; 1600];
    assert_eq!(compute_db(&samples), -96.0);
}

#[test]
fn sub_one_rms_returns_minus_96() {
    let samples = [0.5f32; 1600];
    assert_eq!(compute_db(&samples), -96.0);
}

#[test]
fn clamp_above_zero() {
    let samples = [16_000_000.0f32; 1600];
    assert_eq!(compute_db(&samples), 0.0);
}

// ── Goertzel tests ──────────────────────────────────────────────────────────

fn sine_wave(freq_hz: f32, amplitude: f32) -> Vec<f32> {
    (0..1600)
        .map(|i| amplitude * libm::sinf(2.0 * core::f32::consts::PI * freq_hz * i as f32 / 16000.0))
        .collect()
}

/// Generate a windowed sine wave (Hanning applied).
fn sine_wave_windowed(freq_hz: f32, amplitude: f32) -> Vec<f32> {
    (0..1600)
        .map(|i| {
            let s = amplitude * libm::sinf(2.0 * core::f32::consts::PI * freq_hz * i as f32 / 16000.0);
            s * hanning(i, WINDOW_N)
        })
        .collect()
}

#[test]
fn goertzel_detects_450hz() {
    let samples = sine_wave(450.0, 100_000.0);
    let mut bin = GoertzelBin::new(45);
    for &s in &samples { bin.push(s); }
    assert!(bin.power() > 1e9, "Expected high power at 450 Hz, got {}", bin.power());
}

#[test]
fn goertzel_rejects_wrong_frequency() {
    let samples = sine_wave(200.0, 100_000.0);
    let mut bin_450 = GoertzelBin::new(45);
    let mut bin_200 = GoertzelBin::new(20);
    for &s in &samples { bin_450.push(s); bin_200.push(s); }
    assert!(bin_200.power() > bin_450.power() * 100.0);
}

#[test]
fn goertzel_detects_harmonic() {
    let samples: Vec<f32> = (0..1600)
        .map(|i| {
            let t = i as f32 / 16000.0;
            80_000.0 * libm::sinf(2.0 * core::f32::consts::PI * 450.0 * t)
          + 40_000.0 * libm::sinf(2.0 * core::f32::consts::PI * 900.0 * t)
        })
        .collect();
    let mut bin_f0 = GoertzelBin::new(45);
    let mut bin_h2 = GoertzelBin::new(90);
    for &s in &samples { bin_f0.push(s); bin_h2.push(s); }
    assert!(bin_f0.power() > 1e9);
    assert!(bin_h2.power() > bin_f0.power() * 0.05);
}

#[test]
fn goertzel_reset_clears_state() {
    let mut bin = GoertzelBin::new(45);
    for &s in &sine_wave(450.0, 100_000.0) { bin.push(s); }
    assert!(bin.power() > 1e9);
    bin.reset();
    assert_eq!(bin.power(), 0.0);
}

// ── Hanning window tests ────────────────────────────────────────────────────

#[test]
fn hanning_endpoints_near_zero() {
    // Hanning window is 0 at endpoints
    assert!(hanning(0, 1600) < 0.001);
    assert!(hanning(1599, 1600) < 0.01); // near-zero, not exactly 0
}

#[test]
fn hanning_center_is_one() {
    let mid = hanning(800, 1600);
    assert!((mid - 1.0).abs() < 0.001, "Hanning center should be ~1.0, got {}", mid);
}

#[test]
fn hanning_reduces_leakage() {
    // A 420 Hz tone should leak heavily into the 450 Hz bin WITHOUT windowing
    // but much less WITH windowing. (420 Hz is only 3 bins away from 450 Hz)
    let samples = sine_wave(420.0, 100_000.0);
    let windowed: Vec<f32> = samples.iter().enumerate()
        .map(|(i, &s)| s * hanning(i, 1600))
        .collect();

    let mut bin_no_win = GoertzelBin::new(45);
    let mut bin_win = GoertzelBin::new(45);
    for (&s, &w) in samples.iter().zip(windowed.iter()) {
        bin_no_win.push(s);
        bin_win.push(w);
    }
    let leak_no_win = bin_no_win.power();
    let leak_win = bin_win.power();
    // Windowed version should have at least 10× less leakage
    assert!(
        leak_win < leak_no_win / 10.0,
        "Hanning should reduce leakage: no_win={:.2e}, win={:.2e}",
        leak_no_win, leak_win
    );
}

// ── Multi-bin F0 coverage tests ─────────────────────────────────────────────

#[test]
fn multi_bin_catches_500hz_cry() {
    // Simulate a cry at 500 Hz (older baby / pain cry) — missed by single 450 Hz bin
    let samples = sine_wave_windowed(500.0, 100_000.0);
    let mut bin_450 = GoertzelBin::new(45);
    let mut bin_500 = GoertzelBin::new(50);
    for &s in &samples { bin_450.push(s); bin_500.push(s); }
    // The 500 Hz bin should catch it even though 450 Hz doesn't
    assert!(bin_500.power() > bin_450.power() * 10.0,
        "500 Hz bin should dominate: p500={:.2e}, p450={:.2e}", bin_500.power(), bin_450.power());
}

#[test]
fn multi_bin_catches_350hz_cry() {
    // Young newborn with low F0
    let samples = sine_wave_windowed(350.0, 100_000.0);
    let mut bin_350 = GoertzelBin::new(35);
    let mut bin_450 = GoertzelBin::new(45);
    for &s in &samples { bin_350.push(s); bin_450.push(s); }
    assert!(bin_350.power() > bin_450.power() * 10.0);
}

// ── Zero-crossing rate tests ────────────────────────────────────────────────

#[test]
fn zcr_sine_450hz() {
    let samples = sine_wave(450.0, 100_000.0);
    let zc = count_zero_crossings(&samples);
    // 450 Hz at 16kHz in 1600 samples = 45 cycles × 2 crossings = 90
    assert!(zc >= 85 && zc <= 95, "Expected ~90 ZC for 450 Hz, got {}", zc);
}

#[test]
fn zcr_sine_200hz() {
    let samples = sine_wave(200.0, 100_000.0);
    let zc = count_zero_crossings(&samples);
    // 200 Hz at 16kHz in 1600 samples = 20 cycles × 2 = 40 — below cry range
    assert!(zc < 50, "Expected ~40 ZC for 200 Hz, got {}", zc);
}

#[test]
fn zcr_silence() {
    let samples = [0.0f32; 1600];
    assert_eq!(count_zero_crossings(&samples), 0);
}

// ── is_cry_like v2 (multi-bin) tests ────────────────────────────────────────

/// Helper: build cry-like F0 and harmonic power arrays with one dominant F0 bin.
fn cry_powers(dominant_idx: usize, f0_power: f32, harm_ratio: f32) -> ([f32; 5], [f32; 5]) {
    let mut f0 = [1e3; NUM_F0_BINS]; // low baseline
    f0[dominant_idx] = f0_power;
    let mut harm = [1e2; NUM_F0_BINS];
    harm[dominant_idx] = f0_power * harm_ratio;
    (f0, harm)
}

#[test]
fn cry_like_v2_true_for_cry_at_450() {
    let (f0, harm) = cry_powers(2, 1e8, 0.10); // 450 Hz dominant, 10% harmonic
    assert!(is_cry_like(&f0, &harm, 90, 1e10));
}

#[test]
fn cry_like_v2_true_for_cry_at_500() {
    // Older baby — F0 at 500 Hz (bin index 3)
    let (f0, harm) = cry_powers(3, 1e8, 0.10);
    assert!(is_cry_like(&f0, &harm, 100, 1e10));
}

#[test]
fn cry_like_v2_true_for_cry_at_350() {
    // Newborn — F0 at 350 Hz (bin index 0)
    let (f0, harm) = cry_powers(0, 1e8, 0.10);
    assert!(is_cry_like(&f0, &harm, 70, 1e10));
}

#[test]
fn cry_like_v2_false_no_harmonic() {
    let (f0, mut harm) = cry_powers(2, 1e8, 0.10);
    harm[2] = 100.0; // kill the harmonic
    assert!(!is_cry_like(&f0, &harm, 90, 1e10));
}

#[test]
fn cry_like_v2_false_zcr_too_low() {
    // ZCR too low — sub-200 Hz range, adult speech
    let (f0, harm) = cry_powers(2, 1e8, 0.10);
    assert!(!is_cry_like(&f0, &harm, 30, 1e10));
}

#[test]
fn cry_like_v2_false_zcr_too_high() {
    // ZCR too high (broadband noise / very high freq)
    let (f0, harm) = cry_powers(2, 1e8, 0.10);
    assert!(!is_cry_like(&f0, &harm, 200, 1e10));
}

#[test]
fn cry_like_v2_false_flat_spectrum() {
    // All F0 bins have equal power → flat spectrum (broadband noise, not cry)
    let f0 = [1e8; NUM_F0_BINS];
    let harm = [1e7; NUM_F0_BINS];
    // peakedness = 1.0 (equal) < 1.8 threshold → rejected
    assert!(!is_cry_like(&f0, &harm, 90, 1e10));
}

#[test]
fn cry_like_v2_false_silence() {
    let f0 = [100.0; NUM_F0_BINS];
    let harm = [50.0; NUM_F0_BINS];
    assert!(!is_cry_like(&f0, &harm, 0, 1e3));
}

#[test]
fn cry_like_v2_false_low_tonal_ratio() {
    // Tonal bins have some energy but total energy is vastly higher (broadband)
    let (f0, harm) = cry_powers(2, 1e6, 0.10);
    assert!(!is_cry_like(&f0, &harm, 90, 1e15));
}

// ── CryTracker temporal pattern tests ─────────────────────────────────────

#[test]
fn cry_tracker_three_cycles_confirms() {
    let mut tracker = CryTracker::new();
    let mut first_detected = false;
    // 4 burst-gap sequences = 3 gap→burst transitions = 3 cycles
    for _cycle in 0..4 {
        for _ in 0..5 { if tracker.tick(true) { first_detected = true; } }
        for _ in 0..3 { if tracker.tick(false) { first_detected = true; } }
    }
    assert!(tracker.crying, "Should be crying after 3 cycles");
    assert!(first_detected, "Should have returned true on first detection");
}

#[test]
fn cry_tracker_two_cycles_not_enough() {
    let mut tracker = CryTracker::new();
    for _cycle in 0..2 {
        for _ in 0..5 { tracker.tick(true); }
        for _ in 0..3 { tracker.tick(false); }
    }
    assert!(!tracker.crying);
}

#[test]
fn cry_tracker_single_burst_no_detection() {
    let mut tracker = CryTracker::new();
    for _ in 0..30 { tracker.tick(true); }
    assert!(!tracker.crying);
}

#[test]
fn cry_tracker_cooldown_clears() {
    let mut tracker = CryTracker::new();
    for _cycle in 0..4 {
        for _ in 0..5 { tracker.tick(true); }
        for _ in 0..3 { tracker.tick(false); }
    }
    assert!(tracker.crying);
    for _ in 0..40 { tracker.tick(false); }
    assert!(!tracker.crying);
}

#[test]
fn cry_tracker_brief_burst_rejected() {
    let mut tracker = CryTracker::new();
    for _cycle in 0..5 {
        for _ in 0..2 { tracker.tick(true); } // 200ms — too short
        for _ in 0..3 { tracker.tick(false); }
    }
    assert!(!tracker.crying);
}

// ── Integration: full pipeline (synthetic waveform → Goertzel → is_cry_like) ──

/// Generate a 450 Hz + 900 Hz cry-like waveform, run through 10 Goertzel bins
/// with Hanning window, compute ZCR and energy, and verify is_cry_like returns true.
#[test]
fn full_pipeline_synthetic_cry_detected() {
    use core::f32::consts::PI;
    const N: usize = 1600;

    // Synthetic cry: 450 Hz fundamental + 900 Hz harmonic (25% amplitude)
    // Power ratio = 0.25² = 6.25%, above 5% harmonic threshold
    let samples: Vec<f32> = (0..N)
        .map(|i| {
            let t = i as f32 / 16000.0;
            100_000.0 * libm::sinf(2.0 * PI * 450.0 * t)
          + 25_000.0 * libm::sinf(2.0 * PI * 900.0 * t)
        })
        .collect();

    // 5 F0 bins: 350, 400, 450, 500, 550 Hz
    let mut g_f0 = [
        GoertzelBin::new(35), GoertzelBin::new(40), GoertzelBin::new(45),
        GoertzelBin::new(50), GoertzelBin::new(55),
    ];
    // 5 harmonic bins: 700, 800, 900, 1000, 1100 Hz
    let mut g_harm = [
        GoertzelBin::new(70), GoertzelBin::new(80), GoertzelBin::new(90),
        GoertzelBin::new(100), GoertzelBin::new(110),
    ];

    let mut energy_sum: f32 = 0.0;
    for (i, &s) in samples.iter().enumerate() {
        let w = hanning(i, N);
        let windowed = s * w;
        for bin in g_f0.iter_mut() { bin.push(windowed); }
        for bin in g_harm.iter_mut() { bin.push(windowed); }
        energy_sum += s * s;
    }

    let f0_powers = [
        g_f0[0].power(), g_f0[1].power(), g_f0[2].power(),
        g_f0[3].power(), g_f0[4].power(),
    ];
    let harm_powers = [
        g_harm[0].power(), g_harm[1].power(), g_harm[2].power(),
        g_harm[3].power(), g_harm[4].power(),
    ];

    let zc = count_zero_crossings(&samples);
    let db = compute_db(&samples);

    assert!(
        is_cry_like(&f0_powers, &harm_powers, zc, energy_sum),
        "Synthetic 450+900 Hz cry should be detected: db={:.1}, zc={}, \
         f0_max={:.2e}, harm_at_best={:.2e}, energy={:.2e}",
        db, zc, f0_powers[2], harm_powers[2], energy_sum
    );
}

/// White-ish noise (random-ish pattern) should NOT trigger cry detection.
#[test]
fn full_pipeline_broadband_rejected() {
    const N: usize = 1600;

    // Pseudo-broadband: sum of many frequencies at equal amplitude
    let samples: Vec<f32> = (0..N)
        .map(|i| {
            let t = i as f32 / 16000.0;
            let mut v = 0.0f32;
            for freq in (100..2000).step_by(50) {
                v += 10_000.0 * libm::sinf(2.0 * core::f32::consts::PI * freq as f32 * t);
            }
            v
        })
        .collect();

    let mut g_f0 = [
        GoertzelBin::new(35), GoertzelBin::new(40), GoertzelBin::new(45),
        GoertzelBin::new(50), GoertzelBin::new(55),
    ];
    let mut g_harm = [
        GoertzelBin::new(70), GoertzelBin::new(80), GoertzelBin::new(90),
        GoertzelBin::new(100), GoertzelBin::new(110),
    ];

    let mut energy_sum: f32 = 0.0;
    for (i, &s) in samples.iter().enumerate() {
        let w = hanning(i, N);
        let windowed = s * w;
        for bin in g_f0.iter_mut() { bin.push(windowed); }
        for bin in g_harm.iter_mut() { bin.push(windowed); }
        energy_sum += s * s;
    }

    let f0_powers = [
        g_f0[0].power(), g_f0[1].power(), g_f0[2].power(),
        g_f0[3].power(), g_f0[4].power(),
    ];
    let harm_powers = [
        g_harm[0].power(), g_harm[1].power(), g_harm[2].power(),
        g_harm[3].power(), g_harm[4].power(),
    ];

    let zc = count_zero_crossings(&samples);
    assert!(
        !is_cry_like(&f0_powers, &harm_powers, zc, energy_sum),
        "Broadband noise should NOT be detected as cry"
    );
}
