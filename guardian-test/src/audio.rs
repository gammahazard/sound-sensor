//! audio.rs — dB computation extracted from firmware audio.rs

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
