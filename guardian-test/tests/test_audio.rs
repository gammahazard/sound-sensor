use guardian_test::audio::compute_db;

#[test]
fn silence_returns_minus_96() {
    let samples = [0.0f32; 1600];
    assert_eq!(compute_db(&samples), -96.0);
}

#[test]
fn full_scale_returns_near_zero() {
    // Max 24-bit value = 2^23 - 1 = 8388607
    let samples = [8_388_607.0f32; 1600];
    let db = compute_db(&samples);
    // Should be very close to 0 dBFS (within 0.01 dB)
    assert!(db > -0.01, "Expected ~0 dBFS, got {}", db);
    assert!(db <= 0.0, "Expected <= 0 dBFS, got {}", db);
}

#[test]
fn known_rms() {
    // If all samples are 838861 (roughly 1/10 of full scale = -20 dBFS)
    // RMS = 838861, dB = 20*log10(838861/8388608) ≈ -20.0
    let val = 838_861.0f32;
    let samples = [val; 1600];
    let db = compute_db(&samples);
    assert!(
        (db - (-20.0)).abs() < 0.1,
        "Expected ~-20 dBFS, got {}",
        db
    );
}

#[test]
fn negative_samples() {
    // Negative samples should give same RMS as positive (squared)
    let pos = [100_000.0f32; 1600];
    let neg = [-100_000.0f32; 1600];
    let db_pos = compute_db(&pos);
    let db_neg = compute_db(&neg);
    assert!(
        (db_pos - db_neg).abs() < 0.001,
        "Positive ({}) and negative ({}) should match",
        db_pos,
        db_neg
    );
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
    // Very small signal — rms=10, 20*log10(10/8388608) ≈ -118 dB
    // But clamp means result is -96.0
    let samples = [10.0f32; 1600];
    let db = compute_db(&samples);
    assert_eq!(db, -96.0, "Expected clamped to -96 dBFS, got {}", db);
}

#[test]
fn sub_one_rms_returns_minus_96() {
    // RMS < 1.0 early-returns -96
    let samples = [0.5f32; 1600];
    assert_eq!(compute_db(&samples), -96.0);
}

#[test]
fn clamp_above_zero() {
    // Samples larger than full scale would compute positive dBFS,
    // but should be clamped to 0.0
    let samples = [16_000_000.0f32; 1600];
    let db = compute_db(&samples);
    assert_eq!(db, 0.0, "Expected clamped to 0.0 dBFS, got {}", db);
}
