//! Guardian — Firmware (Phase 3)
//! Rust + Embassy-rp on Raspberry Pi Pico 2 W (RP2350)
//!
//! Task layout:
//!   audio_task      — PIO I²S → RMS → channel
//!   wifi_task       — CYW43 + embassy-net DHCP + LED loop
//!   http_task       — HTTP/1.1 server, serves PWA
//!   websocket_task  — TCP listener, WS framing, broadcast
//!   tv_task         — Multi-brand TV control
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
    peripherals::{PIO0, PIO1, USB},
};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel, mutex::Mutex};
use static_cell::StaticCell;

mod audio;
mod ducking;
mod flash_fs;
mod http;
mod net;
mod ota;
mod pwa_assets;
mod tv;
mod ws;

use ducking::DuckingEngine;
use tv::TvConfig;

// ── Version strings ───────────────────────────────────────────────────────────
pub const FW_VERSION: &str = "0.3.0";

pub const PWA_VERSION: &str = pwa_assets::EMBEDDED_PWA_VERSION;

// ── LED patterns (driven by wifi_task's 100ms loop) ──────────────────────────
#[derive(Clone, Copy, PartialEq, defmt::Format)]
pub enum LedPattern {
    WifiConnecting,  // 200ms on/off fast blink
    Idle,            // 100ms on / 2s off slow pulse
    Armed,           // Double-flash every 2.9s
    Ducking,         // Solid on
    Error,           // 3 rapid blinks then off
}

// ── WiFi commands (ws_task / tv_task → wifi_task) ────────────────────────────
#[derive(defmt::Format)]
pub enum WifiCmd {
    Scan,
    Reconfigure { ssid: heapless::String<64>, pass: heapless::String<64> },
    SaveTvConfig(TvConfig),
}

// ── WiFi events (wifi_task → ws_task) ────────────────────────────────────────
pub enum WifiEvent {
    ScanResults(heapless::Vec<NetworkInfo, 16>),
}

#[derive(Clone, defmt::Format)]
pub struct NetworkInfo {
    pub ssid: heapless::String<32>,
    pub rssi: i16,
}

// ── Shared channel: audio_task → websocket_task ───────────────────────────────
pub static DB_CHANNEL: Channel<ThreadModeRawMutex, f32, 4> = Channel::new();

// ── Inter-task channels ──────────────────────────────────────────────────────
pub static LED_CHANNEL:  Channel<ThreadModeRawMutex, LedPattern, 4> = Channel::new();
pub static WIFI_CMD_CH:  Channel<ThreadModeRawMutex, WifiCmd, 4>    = Channel::new();
pub static WIFI_EVT_CH:  Channel<ThreadModeRawMutex, WifiEvent, 2>  = Channel::new();

// ── Shared ducking engine ───────────────────────────────────────────────────
static DUCKING_CELL: StaticCell<Mutex<ThreadModeRawMutex, DuckingEngine>> = StaticCell::new();

// ── Shared TV config ────────────────────────────────────────────────────────
static TV_CONFIG_CELL: StaticCell<Mutex<ThreadModeRawMutex, TvConfig>> = StaticCell::new();

// ── Interrupt bindings ──────────────────────────────────────────────────────
bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => embassy_rp::pio::InterruptHandler<PIO0>;
    PIO1_IRQ_0 => embassy_rp::pio::InterruptHandler<PIO1>;
    USBCTRL_IRQ => embassy_rp::usb::InterruptHandler<USB>;
});

// ── Entry point ─────────────────────────────────────────────────────────────
#[embassy_executor::main]
async fn main(spawner: Spawner) {
    info!("Guardian v{} starting (RP2350)", FW_VERSION);

    let p = embassy_rp::init(Default::default());

    // ── Ducking engine ──────────────────────────────────────────────────────
    let engine: &'static Mutex<ThreadModeRawMutex, DuckingEngine> =
        DUCKING_CELL.init(Mutex::new(DuckingEngine::new(-20.0, -60.0)));

    // ── TV config — default uses compile-time env var ───────────────────────
    let tv_config: &'static Mutex<ThreadModeRawMutex, TvConfig> =
        TV_CONFIG_CELL.init(Mutex::new(TvConfig::default()));

    // ── Audio (PIO I²S) ─────────────────────────────────────────────────────
    spawner
        .spawn(audio::audio_task(
            p.PIO0, p.DMA_CH0,
            p.PIN_0,  // BCLK → GP0
            p.PIN_1,  // LRCL → GP1 (must be BCLK+1)
            p.PIN_2,  // DOUT → GP2
        ))
        .unwrap();

    // ── WiFi (CYW43439 chip on Pico 2 W) ────────────────────────────────────
    // PIN_25 NOT passed — it's CYW43 SPI CLK, conflicts with GPIO Output.
    // LED is on CYW43 GPIO_0, driven via control.gpio_set(0,…) inside wifi_task.
    spawner
        .spawn(net::wifi_task(
            p.PIO1, p.PIN_23, p.PIN_24, p.PIN_29, p.DMA_CH1,
            p.FLASH,
            spawner,
            engine,
            tv_config,
        ))
        .unwrap();

    info!("All tasks spawned — entering idle");
}
