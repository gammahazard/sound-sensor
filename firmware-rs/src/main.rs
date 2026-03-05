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
#![feature(impl_trait_in_assoc_type)]
#![allow(dead_code)]

// ── RP2350 IMAGE_DEF ────────────────────────────────────────────────────────
// The RP2350 boot ROM scans the first 4 KB of flash for this block.
// Without it, the chip falls back to BOOTSEL mode.
// Format: picobin block with IMAGE_TYPE = EXE, chip = RP2350, cpu = ARM, security = S
#[link_section = ".start_block"]
#[used]
pub static IMAGE_DEF: [u32; 5] = [
    0xffff_ded3, // PICOBIN_BLOCK_MARKER_START
    0x1021_0142, // IMAGE_TYPE: EXE | chip=RP2350 | cpu=ARM | security=S
    0x0000_01ff, // LAST item
    0x0000_0000, // next block link (0 = self-loop)
    0xab12_3579, // PICOBIN_BLOCK_MARKER_END
];

use defmt::*;
use defmt_rtt as _;
use panic_probe as _;

use embassy_executor::Spawner;
use embassy_rp::{
    bind_interrupts,
    peripherals::{PIO0, PIO1, USB, TRNG},
};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel, mutex::Mutex};
use portable_atomic::AtomicBool;
use static_cell::StaticCell;

/// Structured dev log macro — forwards to WebSocket when `dev-mode` feature is on.
/// Compiles to nothing in production builds.
#[macro_export]
macro_rules! dev_log {
    ($cat:expr, $lvl:expr, $($arg:tt)*) => {{
        #[cfg(feature = "dev-mode")]
        {
            let mut _msg: heapless::String<128> = heapless::String::new();
            let _ = core::fmt::Write::write_fmt(&mut _msg, format_args!($($arg)*));
            let _entry = $crate::dev_log::LogEntry {
                level: $lvl,
                cat: $cat,
                msg: _msg,
            };
            let _ = $crate::dev_log::DEV_LOG_CH.try_send(_entry);
        }
    }};
}

mod ap_services;
mod audio;
#[cfg(feature = "dev-mode")]
mod dev_log;
mod ducking;
mod flash_fs;
mod http;
mod net;
mod ota;
mod pwa_assets;
mod setup_html;
mod tv;
mod ws;

// ── AP mode flag (true = device is in setup mode) ────────────────────────────
pub static AP_MODE: AtomicBool = AtomicBool::new(false);

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
    OtaDownload { tls_seed: u64 },
    SaveCalibration { floor: f32, tripwire: f32 },
}

// ── WiFi events (wifi_task → ws_task) ────────────────────────────────────────
pub enum WifiEvent {
    ScanResults(heapless::Vec<NetworkInfo, 16>),
    OtaComplete { success: bool, version: heapless::String<16> },
}

#[derive(Clone, defmt::Format)]
pub struct NetworkInfo {
    pub ssid: heapless::String<32>,
    pub rssi: i16,
}

// ── Shared channel: audio_task → ducking_task ─────────────────────────────────
pub static DB_CHANNEL: Channel<ThreadModeRawMutex, f32, 4> = Channel::new();

// ── Shared telemetry: ducking_task → websocket_task ──────────────────────────
/// Latest dB + engine state, updated every 100ms by ducking_task.
/// ws.rs reads this to build telemetry JSON; no channel needed.
pub struct TelemetrySnapshot {
    pub db: f32,
    pub armed: bool,
    pub tripwire: f32,
    pub ducking: bool,
}
pub static TELEMETRY: Mutex<ThreadModeRawMutex, TelemetrySnapshot> =
    Mutex::new(TelemetrySnapshot { db: -60.0, armed: false, tripwire: -20.0, ducking: false });

/// Signal channel: ducking_task notifies ws.rs that new telemetry is ready.
pub static TELEM_SIGNAL: Channel<ThreadModeRawMutex, (), 1> = Channel::new();

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
    TRNG_IRQ => embassy_rp::trng::InterruptHandler<TRNG>;
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
    // PIN_25 = WL_CS (SPI chip select), PIN_29 = WL_CLK (SPI clock)
    // LED is on CYW43 GPIO_0, driven via control.gpio_set(0,…) inside wifi_task.
    spawner
        .spawn(net::wifi_task(
            p.PIO1, p.PIN_23, p.PIN_24, p.PIN_25, p.PIN_29, p.DMA_CH1,
            p.FLASH,
            p.TRNG,
            spawner,
            engine,
            tv_config,
        ))
        .unwrap();

    info!("All tasks spawned — entering idle");
}
