//! net.rs — WiFi bring-up + embassy-net stack + flash credentials + LED loop
//!
//! Flash config block (256 bytes at 0x1FF000):
//!   Bytes 0–3:     magic (0xBADC0FFE)
//!   Bytes 4–67:    WiFi SSID (null-terminated)
//!   Bytes 68–131:  WiFi pass (null-terminated)
//!   Bytes 132:     tv_enabled (0=none, 1=configured)
//!   Bytes 133–148: tv_ip (null-terminated)
//!   Bytes 149:     tv_brand (0=Lg, 1=Samsung, 2=Sony, 3=Roku)
//!   Bytes 150–165: samsung_token (null-terminated)
//!   Bytes 166–173: sony_psk (null-terminated)
//!   Bytes 174–251: reserved
//!   Bytes 252–255: CRC32 over bytes 0–251

use defmt::*;
use embassy_executor::Spawner;
use embassy_net::{Config as NetConfig, Stack, StackResources};
use embassy_rp::{
    flash::{Blocking, Flash},
    peripherals::{DMA_CH1, FLASH, PIN_23, PIN_24, PIN_29, PIO1},
    pio::Pio,
};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, mutex::Mutex};
use embassy_time::{Duration, Timer};
use static_cell::StaticCell;
use cyw43::JoinOptions;
use cyw43_pio::PioSpi;

use crate::ducking::DuckingEngine;
use crate::tv::{TvBrand, TvConfig};
use crate::{LedPattern, LED_CHANNEL, WifiCmd, WifiEvent, WIFI_CMD_CH, WIFI_EVT_CH, NetworkInfo};

// ── Flash layout ────────────────────────────────────────────────────────────
const FLASH_SIZE: usize = 4 * 1024 * 1024; // 4 MB
const CONFIG_OFFSET: u32 = 0x1F_F000;       // last 4 KB sector
const CONFIG_MAGIC: u32 = 0xBADC_0FFE;

// ── Credentials (compile-time defaults, override with env vars) ─────────────
const DEFAULT_SSID: &str = option_env!("GUARDIAN_SSID").unwrap_or("MyHomeNetwork");
const DEFAULT_PASS: &str = option_env!("GUARDIAN_PASS").unwrap_or("password");

// ── Static allocations ─────────────────────────────────────────────────────
static STATE:     StaticCell<cyw43::State>                           = StaticCell::new();
static RESOURCES: StaticCell<StackResources<5>>                      = StaticCell::new();
static STACK:     StaticCell<Stack<cyw43_pio::NetDriver<'static>>>   = StaticCell::new();

const CYW43_FIRMWARE: &[u8] = include_bytes!("../cyw43-firmware/43439A0.bin");
const CYW43_CLM:      &[u8] = include_bytes!("../cyw43-firmware/43439A0_clm.bin");

// ── CRC32 (for config block integrity) ──────────────────────────────────────
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

// ── Flash credential helpers ────────────────────────────────────────────────

fn read_null_terminated(buf: &[u8]) -> &str {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    core::str::from_utf8(&buf[..end]).unwrap_or("")
}

pub fn flash_load_creds(flash: &mut Flash<'_, FLASH, Blocking, FLASH_SIZE>) -> Option<(heapless::String<64>, heapless::String<64>)> {
    let mut buf = [0u8; 256];
    if flash.blocking_read(CONFIG_OFFSET, &mut buf).is_err() {
        return None;
    }
    let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if magic != CONFIG_MAGIC { return None; }
    let stored_crc = u32::from_le_bytes([buf[252], buf[253], buf[254], buf[255]]);
    if crc32(&buf[..252]) != stored_crc { return None; }
    let ssid_str = read_null_terminated(&buf[4..68]);
    let pass_str = read_null_terminated(&buf[68..132]);
    if ssid_str.is_empty() { return None; }
    let mut ssid = heapless::String::new();
    let _ = ssid.push_str(ssid_str);
    let mut pass = heapless::String::new();
    let _ = pass.push_str(pass_str);
    Some((ssid, pass))
}

pub fn flash_save_creds(flash: &mut Flash<'_, FLASH, Blocking, FLASH_SIZE>, ssid: &str, pass: &str) {
    // Read-modify-write to preserve TV config fields
    let mut buf = [0u8; 256];
    let _ = flash.blocking_read(CONFIG_OFFSET, &mut buf);
    // Check if existing block is valid; if not, start fresh
    let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let stored_crc = u32::from_le_bytes([buf[252], buf[253], buf[254], buf[255]]);
    if magic != CONFIG_MAGIC || crc32(&buf[..252]) != stored_crc {
        buf = [0u8; 256];
    }
    // Write magic
    buf[0..4].copy_from_slice(&CONFIG_MAGIC.to_le_bytes());
    // Write SSID (null-terminated)
    buf[4..68].fill(0);
    let ssid_bytes = ssid.as_bytes();
    buf[4..4 + ssid_bytes.len().min(63)].copy_from_slice(&ssid_bytes[..ssid_bytes.len().min(63)]);
    // Write pass (null-terminated)
    buf[68..132].fill(0);
    let pass_bytes = pass.as_bytes();
    buf[68..68 + pass_bytes.len().min(63)].copy_from_slice(&pass_bytes[..pass_bytes.len().min(63)]);
    // Recompute CRC
    let crc = crc32(&buf[..252]);
    buf[252..256].copy_from_slice(&crc.to_le_bytes());
    // Erase + write
    let _ = flash.blocking_erase(CONFIG_OFFSET, CONFIG_OFFSET + 4096);
    let _ = flash.blocking_write(CONFIG_OFFSET, &buf);
}

pub fn flash_load_tv_config(flash: &mut Flash<'_, FLASH, Blocking, FLASH_SIZE>) -> Option<TvConfig> {
    let mut buf = [0u8; 256];
    if flash.blocking_read(CONFIG_OFFSET, &mut buf).is_err() { return None; }
    let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if magic != CONFIG_MAGIC { return None; }
    let stored_crc = u32::from_le_bytes([buf[252], buf[253], buf[254], buf[255]]);
    if crc32(&buf[..252]) != stored_crc { return None; }
    if buf[132] == 0 { return None; } // tv_enabled = 0
    let ip_str = read_null_terminated(&buf[133..149]);
    if ip_str.is_empty() { return None; }
    let brand = TvBrand::from_u8(buf[149]);
    let token_str = read_null_terminated(&buf[150..166]);
    let psk_str = read_null_terminated(&buf[166..174]);
    let mut cfg = TvConfig::default();
    cfg.ip.clear();
    let _ = cfg.ip.push_str(ip_str);
    cfg.brand = brand;
    cfg.samsung_token.clear();
    let _ = cfg.samsung_token.push_str(token_str);
    cfg.sony_psk.clear();
    let _ = cfg.sony_psk.push_str(psk_str);
    Some(cfg)
}

pub fn flash_save_tv_config(flash: &mut Flash<'_, FLASH, Blocking, FLASH_SIZE>, tv: &TvConfig) {
    // Read-modify-write to preserve WiFi creds
    let mut buf = [0u8; 256];
    let _ = flash.blocking_read(CONFIG_OFFSET, &mut buf);
    let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let stored_crc = u32::from_le_bytes([buf[252], buf[253], buf[254], buf[255]]);
    if magic != CONFIG_MAGIC || crc32(&buf[..252]) != stored_crc {
        buf = [0u8; 256];
        buf[0..4].copy_from_slice(&CONFIG_MAGIC.to_le_bytes());
    }
    // TV enabled flag
    buf[132] = if tv.is_configured() { 1 } else { 0 };
    // TV IP
    buf[133..149].fill(0);
    let ip_bytes = tv.ip.as_bytes();
    buf[133..133 + ip_bytes.len().min(15)].copy_from_slice(&ip_bytes[..ip_bytes.len().min(15)]);
    // TV brand
    buf[149] = tv.brand.to_u8();
    // Samsung token
    buf[150..166].fill(0);
    let tok_bytes = tv.samsung_token.as_bytes();
    buf[150..150 + tok_bytes.len().min(15)].copy_from_slice(&tok_bytes[..tok_bytes.len().min(15)]);
    // Sony PSK
    buf[166..174].fill(0);
    let psk_bytes = tv.sony_psk.as_bytes();
    buf[166..166 + psk_bytes.len().min(8)].copy_from_slice(&psk_bytes[..psk_bytes.len().min(8)]);
    // CRC
    let crc = crc32(&buf[..252]);
    buf[252..256].copy_from_slice(&crc.to_le_bytes());
    let _ = flash.blocking_erase(CONFIG_OFFSET, CONFIG_OFFSET + 4096);
    let _ = flash.blocking_write(CONFIG_OFFSET, &buf);
}

// ── WiFi task ───────────────────────────────────────────────────────────────
#[embassy_executor::task]
pub async fn wifi_task(
    pio:       PIO1,
    pwr:       PIN_23,
    data:      PIN_24,
    cs:        PIN_29,
    dma:       DMA_CH1,
    flash_per: FLASH,
    spawner:   Spawner,
    engine:    &'static Mutex<ThreadModeRawMutex, DuckingEngine>,
    tv_config: &'static Mutex<ThreadModeRawMutex, TvConfig>,
) {
    info!("[net] Starting WiFi…");
    let _ = LED_CHANNEL.try_send(LedPattern::WifiConnecting);

    let Pio { mut common, sm0, .. } = embassy_rp::pio::Pio::new(pio, crate::Irqs);

    let pwr = embassy_rp::gpio::Output::new(pwr, embassy_rp::gpio::Level::Low);
    let cs  = embassy_rp::gpio::Output::new(cs,  embassy_rp::gpio::Level::High);
    let spi = PioSpi::new(&mut common, sm0, crate::Irqs, cs, dma);

    let state = STATE.init(cyw43::State::new());
    let (net_device, mut control, runner) =
        cyw43::new(state, pwr, spi, CYW43_FIRMWARE).await;

    spawner.spawn(cyw43_task(runner)).unwrap();

    control.init(CYW43_CLM).await;
    control.set_power_management(cyw43::PowerManagementMode::PowerSave).await;

    // ── Load credentials from flash ────────────────────────────────────────
    let mut flash = Flash::<_, Blocking, FLASH_SIZE>::new_blocking(flash_per);

    let (flash_ssid, flash_pass) = flash_load_creds(&mut flash)
        .unwrap_or_else(|| {
            info!("[net] No flash creds — using compile-time defaults");
            (heapless::String::new(), heapless::String::new())
        });

    // Load TV config from flash
    if let Some(tv_cfg) = flash_load_tv_config(&mut flash) {
        info!("[net] TV config loaded from flash: {}", tv_cfg.ip.as_str());
        let mut tc = tv_config.lock().await;
        *tc = tv_cfg;
    }

    // ── Join WiFi with fallback ────────────────────────────────────────────
    let have_flash_creds = !flash_ssid.is_empty();

    'wifi: loop {
        // Try flash creds first (5 attempts)
        if have_flash_creds {
            for attempt in 0..5 {
                info!("[net] Joining flash SSID: {} (attempt {})", flash_ssid.as_str(), attempt + 1);
                match control.join(flash_ssid.as_str(), JoinOptions::new(flash_pass.as_bytes())).await {
                    Ok(_) => { info!("[net] Joined flash SSID!"); break 'wifi; }
                    Err(e) => {
                        warn!("[net] Join failed: {:?}", e);
                        Timer::after(Duration::from_secs(3)).await;
                    }
                }
            }
            warn!("[net] Flash creds failed 5× — trying compile-time creds");
        }

        // Try compile-time creds (5 attempts)
        for attempt in 0..5 {
            info!("[net] Joining compile-time SSID: {} (attempt {})", DEFAULT_SSID, attempt + 1);
            match control.join(DEFAULT_SSID, JoinOptions::new(DEFAULT_PASS.as_bytes())).await {
                Ok(_) => { info!("[net] Joined compile-time SSID!"); break 'wifi; }
                Err(e) => {
                    warn!("[net] Join failed: {:?}", e);
                    Timer::after(Duration::from_secs(3)).await;
                }
            }
        }

        // Both sets of creds failed — set error LED and retry the whole cycle
        warn!("[net] All WiFi creds failed — retrying in 10s");
        let _ = LED_CHANNEL.try_send(LedPattern::Error);
        Timer::after(Duration::from_secs(10)).await;
    }

    // ── DHCP ───────────────────────────────────────────────────────────────
    let cfg = NetConfig::dhcpv4(Default::default());
    let seed = 0x4242_4242_4242_4242u64;

    let resources = RESOURCES.init(StackResources::new());
    let stack = STACK.init(Stack::new(net_device, cfg, resources, seed));
    spawner.spawn(net_stack_task(stack)).unwrap();

    info!("[net] Waiting for IP…");
    stack.wait_config_up().await;
    let ip_cfg = stack.config_v4().unwrap();
    info!("[net] IP: {}", ip_cfg.address);

    let _ = LED_CHANNEL.try_send(LedPattern::Idle);

    // ── Spawn application-layer tasks ──────────────────────────────────────
    spawner.spawn(crate::http::http_task(stack)).unwrap();
    spawner.spawn(crate::ws::websocket_task(stack, engine, tv_config)).unwrap();
    spawner.spawn(crate::tv::tv_task(stack, engine, tv_config)).unwrap();

    // ── LED loop + WiFi command handler ────────────────────────────────────
    let mut led_step: u32 = 0;
    let mut current_pattern = LedPattern::Idle;

    loop {
        // Check for new LED pattern (non-blocking)
        if let Ok(p) = LED_CHANNEL.try_receive() {
            current_pattern = p;
            led_step = 0;
        }

        // Check for WiFi commands (non-blocking)
        if let Ok(cmd) = WIFI_CMD_CH.try_receive() {
            handle_wifi_cmd(cmd, &mut control, &mut flash, stack, tv_config).await;
        }

        // Drive LED
        let on = led_on(current_pattern, led_step);
        control.gpio_set(0, on).await;
        led_step = led_step.wrapping_add(1);

        Timer::after(Duration::from_millis(100)).await;
    }
}

// ── LED pattern logic ───────────────────────────────────────────────────────
fn led_on(pattern: LedPattern, step: u32) -> bool {
    match pattern {
        LedPattern::WifiConnecting => {
            // 200ms on / 200ms off → toggle every 2 steps
            (step / 2) % 2 == 0
        }
        LedPattern::Idle => {
            // 100ms on, then off for 2s (20 steps)
            step % 21 == 0
        }
        LedPattern::Armed => {
            // Double-flash every 2.9s (29 steps)
            let phase = step % 29;
            phase == 0 || phase == 2
        }
        LedPattern::Ducking => true,
        LedPattern::Error => {
            // 3 rapid blinks then off for 1s
            let phase = step % 16;
            phase == 0 || phase == 2 || phase == 4
        }
    }
}

// ── WiFi command handler ────────────────────────────────────────────────────
async fn handle_wifi_cmd(
    cmd: WifiCmd,
    control: &mut cyw43::Control<'_>,
    flash: &mut Flash<'_, FLASH, Blocking, FLASH_SIZE>,
    _stack: &'static Stack<cyw43_pio::NetDriver<'static>>,
    tv_config: &'static Mutex<ThreadModeRawMutex, TvConfig>,
) {
    match cmd {
        WifiCmd::Scan => {
            info!("[net] WiFi scan requested");
            let mut results: heapless::Vec<NetworkInfo, 16> = heapless::Vec::new();
            let mut opts = cyw43::ScanOptions::default();
            let mut scan = control.scan(&mut opts).await;
            while let Some(bss) = scan.next().await {
                if results.len() >= 16 { break; }
                let ssid_str = core::str::from_utf8(&bss.ssid[..bss.ssid_len as usize]).unwrap_or("");
                if ssid_str.is_empty() { continue; }
                // Deduplicate
                if results.iter().any(|r| r.ssid.as_str() == ssid_str) { continue; }
                let mut info = NetworkInfo {
                    ssid: heapless::String::new(),
                    rssi: bss.rssi,
                };
                let _ = info.ssid.push_str(ssid_str);
                let _ = results.push(info);
            }
            drop(scan);
            info!("[net] Scan found {} networks", results.len());
            let _ = WIFI_EVT_CH.try_send(WifiEvent::ScanResults(results));
        }
        WifiCmd::Reconfigure { ssid, pass } => {
            info!("[net] Reconfiguring WiFi → {}", ssid.as_str());
            flash_save_creds(flash, ssid.as_str(), pass.as_str());
            // Soft reboot
            Timer::after(Duration::from_millis(500)).await;
            cortex_m::peripheral::SCB::sys_reset();
        }
        WifiCmd::SaveTvConfig(tv_cfg) => {
            info!("[net] Saving TV config to flash");
            flash_save_tv_config(flash, &tv_cfg);
            // Also update the shared TvConfig
            let mut tc = tv_config.lock().await;
            *tc = tv_cfg;
        }
    }
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
