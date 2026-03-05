//! audio.rs — dB computation extracted from firmware audio.rs

const FULL_SCALE_24: f32 = 8_388_608.0; // 2^23

pub fn compute_db(samples: &[i32]) -> f32 {
    if samples.is_empty() {
        return -96.0;
    }
    let mut sum: i64 = 0;
    for &s in samples {
        let v = s as i64;
        sum += v * v;
    }
    let mean = (sum / samples.len() as i64) as f32;
    let rms = libm::sqrtf(mean);
    if rms < 1.0 {
        return -96.0;
    }
    20.0 * libm::log10f(rms / FULL_SCALE_24)
}
