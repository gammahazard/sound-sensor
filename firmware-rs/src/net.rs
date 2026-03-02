//! net.rs — WiFi bring-up + embassy-net stack
//!
//! Uses the CYW43439 chip that is soldered on the Pico W / Pico 2 W.
//! The CYW43 SPI bus is on hardwired GPIO:
//!   GP23 → WL_ON (power-on)
//!   GP24 → SPI data
//!   GP25 → SPI CLK  (shared with LED on Pico W — Pico 2 W has dedicated LED)
//!   GP29 → CS / VSYS sense
//!
//! Credentials are read from flash (LittleFS / raw flash sector).
//! For Phase 2, credentials are compiled in via env vars set in .cargo/config.toml:
//!   GUARDIAN_SSID  and  GUARDIAN_PASS
//! (Replace with runtime flash read in Phase 3.)

use defmt::*;
use embassy_executor::Spawner;
use embassy_net::{Config as NetConfig, Stack, StackResources};
use embassy_rp::{
    peripherals::{DMA_CH1, PIN_23, PIN_24, PIN_29, PIO1},
    pio::Pio,
};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, mutex::Mutex};
use embassy_time::{Duration, Timer};
use static_cell::StaticCell;
use cyw43::JoinOptions;
use cyw43_pio::PioSpi;

use crate::ducking::DuckingEngine;
use crate::tv::TvConfig;

// ── Credentials (compile-time, override with env vars) ────────────────────────
const SSID: &str = env!("GUARDIAN_SSID", "MyHomeNetwork");
const PASS: &str = env!("GUARDIAN_PASS", "password");

// ── Static allocations ────────────────────────────────────────────────────────
static STATE:     StaticCell<cyw43::State>                           = StaticCell::new();
static RESOURCES: StaticCell<StackResources<4>>                      = StaticCell::new();
static STACK:     StaticCell<Stack<cyw43_pio::NetDriver<'static>>>   = StaticCell::new();

// The CYW43 firmware must be included at build time.
// Download from: https://github.com/embassy-rs/embassy/tree/main/cyw43-firmware
// Place in firmware-rs/cyw43-firmware/ directory.
const CYW43_FIRMWARE: &[u8] = include_bytes!("../cyw43-firmware/43439A0.bin");
const CYW43_CLM:      &[u8] = include_bytes!("../cyw43-firmware/43439A0_clm.bin");

/// Main WiFi task — bring up the CYW43, join the network, then spawn
/// networking sub-tasks (http_task, websocket_task, tv_task).
///
/// Note: PIN_25 (CYW43 SPI CLK on Pico W/2W) is managed internally by
/// cyw43-pio and not passed separately here. On Pico 2W the LED is
/// on CYW43 GPIO_0, not directly on RP2350 GPIO — use control.gpio_set(0,…)
/// for LED blinking in Phase 3.
#[embassy_executor::task]
pub async fn wifi_task(
    pio:      PIO1,
    pwr:      PIN_23,
    data:     PIN_24,
    cs:       PIN_29,
    dma:      DMA_CH1,
    spawner:  Spawner,
    engine:    &'static Mutex<ThreadModeRawMutex, DuckingEngine>,
    tv_config: &'static Mutex<ThreadModeRawMutex, TvConfig>,
) {
    info!("[net] Starting WiFi…");

    let Pio { mut common, sm0, .. } = embassy_rp::pio::Pio::new(pio, crate::Irqs);

    let pwr  = embassy_rp::gpio::Output::new(pwr,  embassy_rp::gpio::Level::Low);
    let cs   = embassy_rp::gpio::Output::new(cs,   embassy_rp::gpio::Level::High);
    let spi  = PioSpi::new(&mut common, sm0, crate::Irqs, cs, dma);

    let state = STATE.init(cyw43::State::new());
    let (net_device, mut control, runner) =
        cyw43::new(state, pwr, spi, CYW43_FIRMWARE).await;

    spawner.spawn(cyw43_task(runner)).unwrap();

    control.init(CYW43_CLM).await;
    control.set_power_management(cyw43::PowerManagementMode::PowerSave).await;

    // ── Join home WiFi ────────────────────────────────────────────────────────
    loop {
        info!("[net] Joining SSID: {}", SSID);
        match control.join(SSID, JoinOptions::new(PASS.as_bytes())).await {
            Ok(_)  => { info!("[net] Joined!"); break; }
            Err(e) => {
                warn!("[net] Join failed: {:?} — retrying in 5s", e);
                Timer::after(Duration::from_secs(5)).await;
            }
        }
    }

    // ── DHCP ─────────────────────────────────────────────────────────────────
    let cfg = NetConfig::dhcpv4(Default::default());
    let seed = 0x4242_4242_4242_4242u64; // TODO: use hardware RNG (rp2350 TRNG)

    let resources = RESOURCES.init(StackResources::new());
    let stack = STACK.init(Stack::new(net_device, cfg, resources, seed));
    spawner.spawn(net_stack_task(stack)).unwrap();

    // Wait for IP
    info!("[net] Waiting for IP…");
    stack.wait_config_up().await;
    let cfg = stack.config_v4().unwrap();
    info!("[net] IP: {}", cfg.address);

    // ── Spawn application-layer tasks ─────────────────────────────────────────
    spawner.spawn(crate::http::http_task(stack)).unwrap();
    spawner.spawn(crate::ws::websocket_task(stack, engine, tv_config)).unwrap();
    spawner.spawn(crate::tv::tv_task(stack, engine, tv_config)).unwrap();
}

#[embassy_executor::task]
async fn cyw43_task(
    runner: cyw43::Runner<'static,
        embassy_rp::gpio::Output<'static, PIN_23>,
        PioSpi<'static, PIN_29, PIO1, 0, DMA_CH1>,
    >,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_stack_task(
    stack: &'static Stack<cyw43_pio::NetDriver<'static>>,
) -> ! {
    stack.run().await
}
