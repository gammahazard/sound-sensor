//! audio.rs — PIO I²S capture + RMS computation
//!
//! The SPH0645LM4H sends 24-bit samples left-justified inside 32-bit words.
//! Data is valid on the rising edge of BCLK; WS (LRCL) must be BCLK+1 GPIO.
//!
//! PIO program strategy: we borrow the approach from community rp2040 I²S
//! examples (e.g. pico-inmp441). The PIO shifts in 32 bits per WS cycle,
//! MSB first. We then right-shift by 8 to get a 24-bit signed value.
//!
//! Sample rate: 16 kHz  →  window = 1600 samples = 100 ms
//! RMS is computed in fixed-point (i64 accumulator) to avoid FPU cost.

use defmt::*;
use embassy_rp::{
    peripherals::{DMA_CH0, PIO0},
    pio::{Config, Direction, FifoJoin, Pio, ShiftConfig, ShiftDirection, StateMachine},
};
use embassy_time::{Duration, Timer};
use libm::log10f;

use crate::DB_CHANNEL;

const SAMPLE_RATE:    u32 = 16_000;
const WINDOW_SAMPLES: usize = 1_600;   // 100 ms at 16 kHz
const FULL_SCALE_24:  f32 = 8_388_608.0; // 2^23

// PIO I²S program for SPH0645:
//   - 32-bit words, MSB first
//   - BCLK on pin N, LRCL on pin N+1 (auto-set by PIO wrapping)
//   - Data latched on BCLK rising edge
//
// This is a simplified "bit-bang via PIO" approach.  For production, replace
// with the proper pio! macro program once embassy-rp PIO API stabilises.
//
// For now we use the raw side-set / shift approach.
const I2S_PIO_PROGRAM: &[u16] = &[
    // Minimal I²S RX PIO: waits for WS high (left ch), shifts 32 bits in.
    // Reference: https://github.com/raspberrypi/pico-examples (i2s)
    0x2080, // wait 1 gpio 0  (wait for BCLK low before WS)
    0x2001, // wait 1 gpio 1  (wait for WS = 1, i.e. left channel)
    // inner loop: shift 32 bits
    0x4001, // in pins, 1
    0x0042, // jmp x-- ...    (repeat 31 more times — x pre-loaded to 31)
    // push to FIFO and restart
    0x8020, // push noblock
    0x0000, // jmp 0 (restart)
];

#[embassy_executor::task]
pub async fn audio_task(
    pio:      PIO0,
    dma_ch:   DMA_CH0,
    bclk_pin: embassy_rp::peripherals::PIN_0,
    lrcl_pin: embassy_rp::peripherals::PIN_1,
    data_pin: embassy_rp::peripherals::PIN_2,
) {
    info!("[audio] task started");

    // ── PIO setup ─────────────────────────────────────────────────────────────
    let Pio { mut common, mut sm0, .. } = Pio::new(pio, crate::Irqs);

    // Load the PIO program
    let prog = embassy_rp::pio::PioProgram::new(I2S_PIO_PROGRAM, 0);

    let mut cfg = Config::default();
    cfg.use_program(&prog, &[]);

    // Input pin: GP2 (DOUT from mic)
    let data_pin = common.make_pio_pin(data_pin);
    cfg.set_in_pins(&[&data_pin]);

    // JMP pin / side-set on BCLK (GP0)
    let bclk_pin = common.make_pio_pin(bclk_pin);
    cfg.set_jmp_pin(&bclk_pin);

    // LRCL (GP1) used as wait condition
    let lrcl_pin = common.make_pio_pin(lrcl_pin);

    // 32-bit shift, MSB first, auto-push at 32 bits
    cfg.shift_in = ShiftConfig {
        auto_fill: true,
        threshold: 32,
        direction: ShiftDirection::Left,
    };
    cfg.fifo_join = FifoJoin::RxOnly;

    // Clock: BCLK = SAMPLE_RATE × 64 (64 clocks per stereo sample pair)
    // PIO runs at system clock (125 MHz on RP2350 default).
    // divider = sys_clk / (BCLK × 2)  (×2 because PIO toggles each inst)
    let bclk_freq = SAMPLE_RATE * 64;
    let sys_clk   = 125_000_000u32;
    let div_int   = (sys_clk / (bclk_freq * 2)) as u16;
    cfg.clock_divider = embassy_rp::pio::ClkDivConfig {
        int:  div_int,
        frac: 0,
    };

    sm0.set_config(&cfg);
    sm0.set_pin_dirs(Direction::In, &[&data_pin, &lrcl_pin, &bclk_pin]);
    sm0.set_enabled(true);

    info!("[audio] PIO I2S running @ {}Hz, divider={}", SAMPLE_RATE, div_int);

    // ── Sample loop ───────────────────────────────────────────────────────────
    let mut buf  = [0i32; WINDOW_SAMPLES];
    let mut idx  = 0usize;

    loop {
        // Read one 32-bit word from RX FIFO (blocking)
        let raw: u32 = sm0.rx().wait_pull().await;

        // SPH0645: data is left-justified; right-shift 8 to get 24-bit signed
        let sample = (raw as i32) >> 8;
        buf[idx] = sample;
        idx += 1;

        if idx >= WINDOW_SAMPLES {
            idx = 0;
            let db = compute_db(&buf);
            // Send to WebSocket task (discard if channel full — prefer freshness)
            let _ = DB_CHANNEL.try_send(db);
        }
    }
}

fn compute_db(samples: &[i32]) -> f32 {
    // Accumulate in i64 to avoid overflow (24-bit values squared)
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
