//! audio.rs — PIO I²S capture + RMS computation
//!
//! The SPH0645LM4H sends 24-bit samples left-justified inside 32-bit words.
//! Data is valid on the rising edge of BCLK; WS (LRCL) must be BCLK+1 GPIO.
//!
//! PIO program: slave-mode receive using pio_proc::pio_asm!
//! Clock div=4 (31.25 MHz), X initialized to 31 via `set x, 31`, auto-push at 32 bits.
//!
//! Sample rate: 16 kHz  →  window = 1600 samples = 100 ms

use defmt::*;
use embassy_rp::{
    peripherals::{DMA_CH0, PIO0},
    pio::{Config, Direction, FifoJoin, Pio, ShiftConfig, ShiftDirection},
};
use libm::log10f;

use crate::DB_CHANNEL;

const SAMPLE_RATE:    u32 = 16_000;
const WINDOW_SAMPLES: usize = 1_600;   // 100 ms at 16 kHz
const FULL_SCALE_24:  f32 = 8_388_608.0; // 2^23

#[embassy_executor::task]
pub async fn audio_task(
    pio:      PIO0,
    dma_ch:   DMA_CH0,
    bclk_pin: embassy_rp::peripherals::PIN_0,
    lrcl_pin: embassy_rp::peripherals::PIN_1,
    data_pin: embassy_rp::peripherals::PIN_2,
) {
    info!("[audio] task started");

    let Pio { mut common, mut sm0, .. } = Pio::new(pio, crate::Irqs);

    // PIO I²S slave-mode receive program
    let prog = pio_proc::pio_asm!(
        ".wrap_target",
        "wait 0 pin 0",       // wait for BCLK low
        "wait 1 pin 1",       // wait for WS high (left channel)
        "set x, 31",          // 32 bits to shift
        "bitloop:",
        "wait 1 pin 0",       // wait BCLK high (data valid)
        "in pins, 1",         // shift in data bit
        "wait 0 pin 0",       // wait BCLK low
        "jmp x-- bitloop",    // loop 32 times
        ".wrap",
    );

    let mut cfg = Config::default();
    cfg.use_program(&common.load_program(&prog.program), &[]);

    // Input pin: GP2 (DOUT from mic)
    let data_pin = common.make_pio_pin(data_pin);
    cfg.set_in_pins(&[&data_pin]);

    // Wait/JMP pins: GP0 (BCLK), GP1 (LRCL)
    let bclk_pin = common.make_pio_pin(bclk_pin);
    let lrcl_pin = common.make_pio_pin(lrcl_pin);

    // 32-bit shift, MSB first, auto-push at 32 bits
    cfg.shift_in = ShiftConfig {
        auto_fill: true,
        threshold: 32,
        direction: ShiftDirection::Left,
    };
    cfg.fifo_join = FifoJoin::RxOnly;

    // Clock divider = 4 → PIO runs at 31.25 MHz (125/4)
    cfg.clock_divider = embassy_rp::pio::ClkDivConfig {
        int:  4,
        frac: 0,
    };

    sm0.set_config(&cfg);
    sm0.set_pin_dirs(Direction::In, &[&data_pin, &lrcl_pin, &bclk_pin]);
    sm0.set_enabled(true);

    info!("[audio] PIO I2S running @ {}Hz, div=4", SAMPLE_RATE);

    // ── Sample loop ───────────────────────────────────────────────────────
    let mut buf  = [0i32; WINDOW_SAMPLES];
    let mut idx  = 0usize;

    loop {
        let raw: u32 = sm0.rx().wait_pull().await;

        // SPH0645: data is left-justified; right-shift 8 to get 24-bit signed
        let sample = (raw as i32) >> 8;
        buf[idx] = sample;
        idx += 1;

        if idx >= WINDOW_SAMPLES {
            idx = 0;
            let db = compute_db(&buf);
            let _ = DB_CHANNEL.try_send(db);
        }
    }
}

fn compute_db(samples: &[i32]) -> f32 {
    let mut sum: i64 = 0;
    for &s in samples {
        let v = s as i64;
        sum += v * v;
    }
    let rms = libm::sqrtf((sum as f32) / (samples.len() as f32));
    if rms < 1.0 {
        return -96.0;
    }
    20.0 * log10f(rms / FULL_SCALE_24)
}
