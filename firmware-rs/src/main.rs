//! Guardian — Firmware (Phase 2)
//! Rust + Embassy-rp on Raspberry Pi Pico 2 W (RP2350)
//!
//! Task layout:
//!   audio_task      — PIO I²S → RMS → channel
//!   wifi_task       — CYW43 + embassy-net DHCP
//!   http_task       — HTTP/1.1 server, serves PWA from flash + LittleFS
//!   websocket_task  — TCP listener, WS framing, broadcast
//!   tv_task         — LG WebOS WS client, volume duck commands
//!   blink_task      — heartbeat LED
//!
//! Flash with:  cargo run --release
//! Logs via:    probe-rs attach + RTT (defmt-rtt)

#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
#![allow(dead_code)]

use defmt::*;
use defmt_rtt as _;
use panic_probe as _;

use embassy_executor::Spawner;
use embassy_rp::{
    bind_interrupts,
    gpio::{Level, Output},
    peripherals::{PIN_25, PIO0, PIO1, USB},
};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, mutex::Mutex};
use embassy_time::{Duration, Timer};
use static_cell::StaticCell;

mod audio;
mod ducking;
mod http;
mod net;
mod pwa_assets;
mod tv;
mod ws;

use ducking::DuckingEngine;
use tv::TvConfig;

// ── Version strings ───────────────────────────────────────────────────────────
/// Firmware version, broadcast in every WS message and returned by /api/ota.
pub const FW_VERSION: &str = "0.2.0";

/// PWA version currently embedded in flash (pwa_assets).
/// Reflects the version of the files in the pwa/ directory at build time.
/// Overridden at runtime when LittleFS holds a newer OTA copy.
pub const PWA_VERSION: &str = pwa_assets::EMBEDDED_PWA_VERSION;

// ── Shared channel: audio_task → websocket_task ───────────────────────────────
// Capacity of 4: if WS is slow, we drop old readings (non-blocking send).
pub static DB_CHANNEL: embassy_sync::channel::Channel<ThreadModeRawMutex, f32, 4> =
    embassy_sync::channel::Channel::new();

// ── Shared ducking engine ─────────────────────────────────────────────────────
static DUCKING_CELL: StaticCell<Mutex<ThreadModeRawMutex, DuckingEngine>> = StaticCell::new();

// ── Shared TV config (written by ws_task on set_tv command, read by tv_task) ──
static TV_CONFIG_CELL: StaticCell<Mutex<ThreadModeRawMutex, TvConfig>> = StaticCell::new();

// ── Interrupt bindings (required by embassy-rp) ───────────────────────────────
bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => embassy_rp::pio::InterruptHandler<PIO0>;
    PIO1_IRQ_0 => embassy_rp::pio::InterruptHandler<PIO1>;
    USBCTRL_IRQ => embassy_rp::usb::InterruptHandler<USB>;
});

// ── Entry point ───────────────────────────────────────────────────────────────
#[embassy_executor::main]
async fn main(spawner: Spawner) {
    info!("Guardian v{} starting (RP2350)", FW_VERSION);

    let p = embassy_rp::init(Default::default());

    // Onboard LED — slow heartbeat while running
    let led = Output::new(p.PIN_25, Level::Low);
    spawner.spawn(blink_task(led)).unwrap();

    // ── Ducking engine — initialise once, share via &'static ref ─────────────
    let engine: &'static Mutex<ThreadModeRawMutex, DuckingEngine> =
        DUCKING_CELL.init(Mutex::new(DuckingEngine::new(-20.0, -60.0)));

    // ── TV config — default uses compile-time GUARDIAN_TV_IP env var ──────────
    let tv_config: &'static Mutex<ThreadModeRawMutex, TvConfig> =
        TV_CONFIG_CELL.init(Mutex::new(TvConfig::default()));

    // ── Audio (PIO I²S) ───────────────────────────────────────────────────────
    spawner
        .spawn(audio::audio_task(
            p.PIO0, p.DMA_CH0,
            p.PIN_0,  // BCLK → GP0
            p.PIN_1,  // LRCL → GP1 (must be BCLK+1)
            p.PIN_2,  // DOUT → GP2
        ))
        .unwrap();

    // ── WiFi (CYW43439 chip on Pico 2 W) ──────────────────────────────────────
    // CYW43 SPI pins are hardwired on Pico W/2W:
    //   GP23 (WL_ON), GP24 (SPI DATA), GP25 (CLK), GP29 (CS/VBUS sense)
    spawner
        .spawn(net::wifi_task(
            p.PIO1, p.PIN_23, p.PIN_24, p.PIN_29, p.DMA_CH1,
            spawner,
            engine,
            tv_config,
        ))
        .unwrap();

    info!("All tasks spawned — entering idle");
}

// ── Heartbeat LED ─────────────────────────────────────────────────────────────
#[embassy_executor::task]
async fn blink_task(mut led: Output<'static, PIN_25>) {
    loop {
        led.set_high();
        Timer::after(Duration::from_millis(100)).await;
        led.set_low();
        Timer::after(Duration::from_millis(1900)).await;
    }
}
