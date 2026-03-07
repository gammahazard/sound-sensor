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
//!   Byte  174:     calibration_valid (0=no, 1=yes)
//!   Bytes 175–178: floor_db (f32 LE)
//!   Bytes 179–182: tripwire_db (f32 LE)
//!   Bytes 183–188: tv_mac (6 bytes, for Wake-on-LAN power on)
//!   Bytes 189–251: reserved
//!   Bytes 252–255: CRC32 over bytes 0–251

use defmt::*;
use embassy_executor::Spawner;
use embassy_net::{Config as NetConfig, IpAddress, IpEndpoint, Ipv4Address, Ipv4Cidr, StackResources, StaticConfigV4};
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_rp::{
    flash::{Blocking, Flash},
    peripherals::{DMA_CH1, FLASH, PIN_23, PIN_24, PIN_25, PIN_29, PIO1, TRNG},
    pio::Pio,
    trng::Trng,
};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, mutex::Mutex};
use embassy_time::{Duration, Timer};
use static_cell::StaticCell;
use cyw43::JoinOptions;
use cyw43_pio::PioSpi;

use crate::ducking::DuckingEngine;
use crate::tv::TvConfig;
use crate::{LedPattern, LED_CHANNEL, WifiCmd, WifiEvent, WIFI_CMD_CH, WIFI_EVT_CH, NetworkInfo};

use crate::flash_config::*;

// ── Credentials (compile-time defaults, override with env vars) ─────────────
const DEFAULT_SSID: &str = match option_env!("GUARDIAN_SSID") {
    Some(v) => v,
    None => "MyHomeNetwork",
};
const DEFAULT_PASS: &str = match option_env!("GUARDIAN_PASS") {
    Some(v) => v,
    None => "password",
};

// ── Static allocations ─────────────────────────────────────────────────────
static STATE:     StaticCell<cyw43::State>     = StaticCell::new();
static RESOURCES: StaticCell<StackResources<8>>  = StaticCell::new();

const CYW43_FIRMWARE: &[u8] = include_bytes!("../cyw43-firmware/43439A0.bin");
const CYW43_CLM:      &[u8] = include_bytes!("../cyw43-firmware/43439A0_clm.bin");

// ── WiFi task ───────────────────────────────────────────────────────────────
#[embassy_executor::task]
pub async fn wifi_task(
    pio:       PIO1,
    pwr:       PIN_23,
    dio:       PIN_24,
    cs:        PIN_25,
    clk:       PIN_29,
    dma:       DMA_CH1,
    flash_per: FLASH,
    trng_per:  TRNG,
    spawner:   Spawner,
    engine:    &'static Mutex<ThreadModeRawMutex, DuckingEngine>,
    tv_config: &'static Mutex<ThreadModeRawMutex, TvConfig>,
) {
    info!("[net] Starting WiFi…");

    // Generate TLS seed from hardware TRNG (RP2350 built-in, no extra wiring)
    let mut trng = Trng::new(trng_per, crate::Irqs, Default::default());
    let mut seed_bytes = [0u8; 8];
    trng.blocking_fill_bytes(&mut seed_bytes);
    let tls_seed = u64::from_le_bytes(seed_bytes);
    info!("[net] TLS seed generated");
    let _ = LED_CHANNEL.try_send(LedPattern::WifiConnecting);

    let Pio { mut common, sm0, irq0, .. } = embassy_rp::pio::Pio::new(pio, crate::Irqs);

    let pwr = embassy_rp::gpio::Output::new(pwr, embassy_rp::gpio::Level::Low);
    let cs  = embassy_rp::gpio::Output::new(cs,  embassy_rp::gpio::Level::High);
    let spi = PioSpi::new(&mut common, sm0, 2u8.into(), irq0, cs, dio, clk, dma);

    let state = STATE.init(cyw43::State::new());
    let (net_device, mut control, runner) =
        cyw43::new(state, pwr, spi, CYW43_FIRMWARE).await;

    spawner.spawn(cyw43_task(runner)).unwrap();

    control.init(CYW43_CLM).await;
    // PowerSave drops incoming multicast (mDNS queries) — disable since we're USB-powered
    control.set_power_management(cyw43::PowerManagementMode::None).await;

    // ── Load credentials from flash ────────────────────────────────────────
    let flash = Flash::<_, Blocking, FLASH_SIZE>::new_blocking(flash_per);
    let mut fs = crate::flash_fs::FlashFs::new(flash);

    let (flash_ssid, flash_pass) = flash_load_creds(fs.flash_mut())
        .unwrap_or_else(|| {
            info!("[net] No flash creds found");
            dev_log!(crate::dev_log::LogCat::Flash, crate::dev_log::LogLevel::Info,
                "no flash creds (empty or crc_invalid)");
            (heapless::String::new(), heapless::String::new())
        });
    #[cfg(feature = "dev-mode")]
    if !flash_ssid.is_empty() {
        dev_log!(crate::dev_log::LogCat::Flash, crate::dev_log::LogLevel::Info,
            "flash creds loaded ssid_len={}", flash_ssid.len());
    }

    // Load TV config from flash
    if let Some(tv_cfg) = flash_load_tv_config(fs.flash_mut()) {
        info!("[net] TV config loaded from flash: {}", tv_cfg.ip.as_str());
        dev_log!(crate::dev_log::LogCat::Flash, crate::dev_log::LogLevel::Info,
            "tv_config loaded ip={} brand={:?}", tv_cfg.ip.as_str(), tv_cfg.brand);
        let mut tc = tv_config.lock().await;
        *tc = tv_cfg;
    } else {
        dev_log!(crate::dev_log::LogCat::Flash, crate::dev_log::LogLevel::Info,
            "no tv_config in flash");
    }

    // Load calibration from flash
    if let Some((floor, tripwire)) = flash_load_calibration(fs.flash_mut()) {
        info!("[net] Calibration loaded: floor={}, tripwire={}", floor, tripwire);
        dev_log!(crate::dev_log::LogCat::Flash, crate::dev_log::LogLevel::Info,
            "calibration loaded floor={:.1} trip={:.1}", floor, tripwire);
        let mut eng = engine.lock().await;
        eng.set_floor(floor);
        eng.set_tripwire(tripwire);
    } else {
        dev_log!(crate::dev_log::LogCat::Flash, crate::dev_log::LogLevel::Info,
            "no calibration in flash");
    }

    // Check for existing OTA files in flash FS from a previous download
    if let Some((_, sz)) = fs.find("guardian-pwa.js.gz") {
        if sz > 0 {
            info!("[net] Found OTA files in flash FS");
            let mut table = crate::flash_fs::OTA_FILE_OFFSETS.lock().await;
            table.index_html    = fs.find("index.html.gz").unwrap_or((0, 0));
            table.guardian_js   = fs.find("guardian-pwa.js.gz").unwrap_or((0, 0));
            table.guardian_wasm = fs.find("guardian-pwa_bg.wasm.gz").unwrap_or((0, 0));
            table.sw_js         = fs.find("sw.js.gz").unwrap_or((0, 0));
            table.manifest_json = fs.find("manifest.json.gz").unwrap_or((0, 0));
        }
    }

    // ── Determine WiFi mode ────────────────────────────────────────────────
    // AP mode only if: no flash creds AND compile-time creds are still defaults
    let have_flash_creds = !flash_ssid.is_empty();
    let have_compile_creds = DEFAULT_SSID != "MyHomeNetwork" || DEFAULT_PASS != "password";
    let enter_ap_mode = !have_flash_creds && !have_compile_creds;

    dev_log!(crate::dev_log::LogCat::Wifi, crate::dev_log::LogLevel::Info,
        "cred_source={}", if have_flash_creds { "flash" } else if have_compile_creds { "compile" } else { "none(ap)" });

    // Initialize OTA TCP client state (shared by both AP and station mode paths)
    let ota_tcp = crate::ota::init_tcp_state();

    let stack: embassy_net::Stack<'static> = if enter_ap_mode {
        // ── AP MODE: No creds → start setup hotspot ─────────────────────
        info!("[net] No WiFi creds — entering AP mode");
        dev_log!(crate::dev_log::LogCat::Wifi, crate::dev_log::LogLevel::Info,
            "ap_mode: no credentials found");
        crate::AP_MODE.store(true, portable_atomic::Ordering::Relaxed);

        control.start_ap_open("Guardian-Setup", 6).await;
        info!("[net] AP 'Guardian-Setup' started on channel 6");

        let cfg = NetConfig::ipv4_static(StaticConfigV4 {
            address: Ipv4Cidr::new(Ipv4Address::new(192, 168, 4, 1), 24),
            gateway: Some(Ipv4Address::new(192, 168, 4, 1)),
            dns_servers: heapless::Vec::new(),
        });

        let resources = RESOURCES.init(StackResources::new());
        let (stack, net_runner) = embassy_net::new(net_device, cfg, resources, tls_seed);
        spawner.spawn(net_stack_task(net_runner)).unwrap();

        info!("[net] Waiting for AP stack…");
        stack.wait_config_up().await;
        info!("[net] AP mode ready at 192.168.4.1");

        // Register mDNS multicast MAC with CYW43 hardware filter
        let _ = control.add_multicast_address([0x01, 0x00, 0x5E, 0x00, 0x00, 0xFB]).await;

        // Spawn setup tasks (no tv_task, no OTA)
        spawner.spawn(crate::ducking::ducking_task(engine)).unwrap();
        spawner.spawn(crate::http::http_task(stack)).unwrap();
        spawner.spawn(crate::ws::websocket_task(stack, engine, tv_config, tls_seed, ota_tcp)).unwrap();
        spawner.spawn(crate::ap_services::dhcp_server_task(stack)).unwrap();
        spawner.spawn(crate::ap_services::dns_server_task(stack)).unwrap();
        spawner.spawn(mdns_responder_task(stack, "guardiansetup")).unwrap();
        // LED stays in WifiConnecting (fast blink) as visual cue for setup mode

        stack
    } else {
        // ── STATION MODE: Join home WiFi ────────────────────────────────
        let mut joined = false;

        // Try flash creds first (5 attempts)
        if have_flash_creds {
            for attempt in 0..5 {
                info!("[net] Joining flash SSID: {} (attempt {})", flash_ssid.as_str(), attempt + 1);
                dev_log!(crate::dev_log::LogCat::Wifi, crate::dev_log::LogLevel::Info,
                    "join attempt={}/5 source=flash", attempt + 1);
                // Blink LED on during attempt (LED loop hasn't started yet)
                control.gpio_set(0, true).await;
                match control.join(flash_ssid.as_str(), JoinOptions::new(flash_pass.as_bytes())).await {
                    Ok(_) => {
                        dev_log!(crate::dev_log::LogCat::Wifi, crate::dev_log::LogLevel::Info,
                            "joined flash ssid");
                        info!("[net] Joined flash SSID!");
                        joined = true;
                        break;
                    }
                    Err(e) => {
                        dev_log!(crate::dev_log::LogCat::Wifi, crate::dev_log::LogLevel::Warn,
                            "join_fail attempt={}/5 status={}", attempt + 1, e.status);
                        warn!("[net] Join failed: status={}", e.status);
                        control.gpio_set(0, false).await;
                        Timer::after(Duration::from_secs(3)).await;
                    }
                }
            }
            if !joined {
                warn!("[net] Flash creds failed 5× — trying compile-time creds");
            }
        }

        // Try compile-time creds (5 attempts) if flash creds didn't work
        if !joined && have_compile_creds {
            for attempt in 0..5 {
                info!("[net] Joining compile-time SSID: {} (attempt {})", DEFAULT_SSID, attempt + 1);
                dev_log!(crate::dev_log::LogCat::Wifi, crate::dev_log::LogLevel::Info,
                    "join attempt={}/5 source=compile", attempt + 1);
                control.gpio_set(0, true).await;
                match control.join(DEFAULT_SSID, JoinOptions::new(DEFAULT_PASS.as_bytes())).await {
                    Ok(_) => {
                        dev_log!(crate::dev_log::LogCat::Wifi, crate::dev_log::LogLevel::Info,
                            "joined compile ssid");
                        info!("[net] Joined compile-time SSID!");
                        joined = true;
                        break;
                    }
                    Err(e) => {
                        dev_log!(crate::dev_log::LogCat::Wifi, crate::dev_log::LogLevel::Warn,
                            "join_fail attempt={}/5 status={}", attempt + 1, e.status);
                        warn!("[net] Join failed: status={}", e.status);
                        control.gpio_set(0, false).await;
                        Timer::after(Duration::from_secs(3)).await;
                    }
                }
            }
        }

        // If all credentials failed, erase bad flash creds and reboot into AP mode
        if !joined {
            warn!("[net] All WiFi creds failed — erasing flash creds, rebooting to AP mode");
            dev_log!(crate::dev_log::LogCat::Wifi, crate::dev_log::LogLevel::Error,
                "all creds failed, erasing → AP mode");
            // Blink error pattern: 3 rapid blinks
            for _ in 0..3 {
                control.gpio_set(0, true).await;
                Timer::after(Duration::from_millis(100)).await;
                control.gpio_set(0, false).await;
                Timer::after(Duration::from_millis(100)).await;
            }
            // Clear only WiFi creds, preserving TV config + calibration
            flash_clear_creds(fs.flash_mut());
            Timer::after(Duration::from_millis(100)).await;
            cortex_m::peripheral::SCB::sys_reset();
        }

        // ── DHCP ─────────────────────────────────────────────────────────
        // Turn LED off briefly to signal "join succeeded, waiting for IP"
        control.gpio_set(0, false).await;

        let cfg = NetConfig::dhcpv4(Default::default());
        let seed = tls_seed;

        let resources = RESOURCES.init(StackResources::new());
        let (stack, net_runner) = embassy_net::new(net_device, cfg, resources, seed);
        spawner.spawn(net_stack_task(net_runner)).unwrap();

        info!("[net] Waiting for IP…");

        // Poll for DHCP with LED blink and 30s timeout
        let mut dhcp_step = 0u32;
        loop {
            if stack.config_v4().is_some() { break; }
            // Slow blink: 300ms on / 300ms off (distinct from fast AP blink)
            control.gpio_set(0, (dhcp_step / 3) % 2 == 0).await;
            dhcp_step += 1;
            if dhcp_step > 300 {
                // 300 × 100ms = 30s — DHCP failed, erase creds and reboot to AP mode
                warn!("[net] DHCP timeout — erasing creds, rebooting to AP mode");
                for _ in 0..3 {
                    control.gpio_set(0, true).await;
                    Timer::after(Duration::from_millis(100)).await;
                    control.gpio_set(0, false).await;
                    Timer::after(Duration::from_millis(100)).await;
                }
                flash_clear_creds(fs.flash_mut());
                Timer::after(Duration::from_millis(100)).await;
                cortex_m::peripheral::SCB::sys_reset();
            }
            Timer::after(Duration::from_millis(100)).await;
        }

        let ip_cfg = stack.config_v4().unwrap();
        let addr = ip_cfg.address.address();
        let octets = addr.octets();
        info!("[net] IP: {}.{}.{}.{}", octets[0], octets[1], octets[2], octets[3]);
        dev_log!(crate::dev_log::LogCat::Wifi, crate::dev_log::LogLevel::Info,
            "dhcp ip={}.{}.{}.{}", octets[0], octets[1], octets[2], octets[3]);

        let _ = LED_CHANNEL.try_send(LedPattern::Idle);

        // Register multicast MACs with CYW43 hardware filter
        let _ = control.add_multicast_address([0x01, 0x00, 0x5E, 0x00, 0x00, 0xFB]).await; // mDNS 224.0.0.251
        let _ = control.add_multicast_address([0x01, 0x00, 0x5E, 0x7F, 0xFF, 0xFA]).await; // SSDP 239.255.255.250

        // ── Spawn application-layer tasks ────────────────────────────────
        spawner.spawn(crate::ducking::ducking_task(engine)).unwrap();
        spawner.spawn(crate::http::http_task(stack)).unwrap();
        spawner.spawn(crate::ws::websocket_task(stack, engine, tv_config, tls_seed, ota_tcp)).unwrap();
        spawner.spawn(crate::tv::tv_task(stack, engine, tv_config, tls_seed)).unwrap();
        spawner.spawn(mdns_responder_task(stack, "guardian")).unwrap();

        stack
    };

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
            handle_wifi_cmd(cmd, &mut control, &mut fs, stack, tv_config, ota_tcp).await;
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
    fs: &mut crate::flash_fs::FlashFs,
    stack: embassy_net::Stack<'static>,
    tv_config: &'static Mutex<ThreadModeRawMutex, TvConfig>,
    ota_tcp: &'static embassy_net::tcp::client::TcpClientState<1, 1024, 1024>,
) {
    match cmd {
        WifiCmd::Scan => {
            info!("[net] WiFi scan requested");
            let mut results: heapless::Vec<NetworkInfo, 16> = heapless::Vec::new();
            let opts = cyw43::ScanOptions::default();
            let mut scan = control.scan(opts).await;
            while let Some(bss) = scan.next().await {
                if results.len() >= 16 { break; }
                let ssid_len = (bss.ssid_len as usize).min(bss.ssid.len());
                let ssid_str = core::str::from_utf8(&bss.ssid[..ssid_len]).unwrap_or("");
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
            let saved = flash_save_creds(fs.flash_mut(), ssid.as_str(), pass.as_str());
            if saved {
                Timer::after(Duration::from_millis(500)).await;
                cortex_m::peripheral::SCB::sys_reset();
            } else {
                warn!("[net] Cred flash write failed!");
                let _ = LED_CHANNEL.try_send(LedPattern::Error);
            }
        }
        WifiCmd::SaveTvConfig(tv_cfg) => {
            info!("[net] Saving TV config to flash");
            flash_save_tv_config(fs.flash_mut(), &tv_cfg);
            // Also update the shared TvConfig
            let mut tc = tv_config.lock().await;
            *tc = tv_cfg;
        }
        WifiCmd::SaveCalibration { floor, tripwire } => {
            info!("[net] Saving calibration: floor={}, tripwire={}", floor, tripwire);
            dev_log!(crate::dev_log::LogCat::Flash, crate::dev_log::LogLevel::Info,
                "saved calibration floor={:.1} trip={:.1}", floor, tripwire);
            flash_save_calibration(fs.flash_mut(), floor, tripwire);
        }
        WifiCmd::OtaDownload { tls_seed } => {
            info!("[net] OTA download requested");
            let result = crate::ota::download_update(stack, ota_tcp, tls_seed, fs).await;
            match result {
                Some(version) => {
                    info!("[net] OTA download succeeded: {}", version.as_str());
                    let _ = WIFI_EVT_CH.try_send(WifiEvent::OtaComplete {
                        success: true,
                        version,
                    });
                }
                None => {
                    warn!("[net] OTA download failed");
                    let _ = WIFI_EVT_CH.try_send(WifiEvent::OtaComplete {
                        success: false,
                        version: heapless::String::new(),
                    });
                }
            }
        }
    }
}

#[embassy_executor::task]
async fn cyw43_task(
    runner: cyw43::Runner<'static, embassy_rp::gpio::Output<'static>, PioSpi<'static, PIO1, 0, DMA_CH1>>,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_stack_task(
    mut runner: embassy_net::Runner<'static, cyw43::NetDriver<'static>>,
) -> ! {
    runner.run().await
}

// ── mDNS Responder ──────────────────────────────────────────────────────────
//
// Announces hostname.local → our IP via multicast (224.0.0.251:5353).
// Also responds to incoming mDNS A queries for our hostname.

#[embassy_executor::task]
pub async fn mdns_responder_task(stack: embassy_net::Stack<'static>, hostname: &'static str) {
    stack.wait_config_up().await;

    let ip_octets = match stack.config_v4() {
        Some(cfg) => cfg.address.address().octets(),
        None => return,
    };

    // Join mDNS multicast group at IP stack level
    let _ = stack.join_multicast_group(Ipv4Address::new(224, 0, 0, 251));

    let mut rx_meta = [PacketMetadata::EMPTY; 2];
    let mut tx_meta = [PacketMetadata::EMPTY; 2];
    let mut rx_buf = [0u8; 512];
    let mut tx_buf = [0u8; 512];

    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx_buf, &mut tx_meta, &mut tx_buf);
    if socket.bind(5353).is_err() {
        warn!("[mdns] Failed to bind port 5353");
        return;
    }
    info!("[mdns] Responder ready for {}.local", hostname);

    let mdns_dest = IpEndpoint::new(
        IpAddress::Ipv4(Ipv4Address::new(224, 0, 0, 251)),
        5353,
    );

    // Send 3 startup announcements (1s apart per RFC 6762)
    let mut pkt = [0u8; 512];
    let ann_len = build_mdns_announce(hostname, &ip_octets, &mut pkt);
    for _ in 0..3 {
        let _ = socket.send_to(&pkt[..ann_len], mdns_dest).await;
        Timer::after(Duration::from_secs(1)).await;
    }

    // Main loop: respond to queries + re-announce every 60s
    let mut ticks: u32 = 0;
    let mut recv_pkt = [0u8; 512];
    loop {
        match embassy_time::with_timeout(
            Duration::from_secs(1),
            socket.recv_from(&mut recv_pkt),
        ).await {
            Ok(Ok((n, _sender))) => {
                let mut resp = [0u8; 512];
                if let Some(resp_len) = match_mdns_query(&recv_pkt[..n], hostname, &ip_octets, &mut resp) {
                    let _ = socket.send_to(&resp[..resp_len], mdns_dest).await;
                }
            }
            _ => {} // Timeout or recv error
        }

        ticks += 1;
        if ticks >= 60 {
            ticks = 0;
            let ann_len = build_mdns_announce(hostname, &ip_octets, &mut pkt);
            let _ = socket.send_to(&pkt[..ann_len], mdns_dest).await;
        }
    }
}

/// Encode "hostname.local" in DNS label format. Returns bytes written.
fn encode_mdns_name(hostname: &str, buf: &mut [u8]) -> usize {
    let hn = hostname.as_bytes();
    let mut pos = 0;
    buf[pos] = hn.len() as u8; pos += 1;
    buf[pos..pos + hn.len()].copy_from_slice(hn); pos += hn.len();
    buf[pos] = 5; pos += 1;
    buf[pos..pos + 5].copy_from_slice(b"local"); pos += 5;
    buf[pos] = 0; pos += 1;
    pos
}

/// Build an mDNS announcement (unsolicited response) for hostname.local → ip.
fn build_mdns_announce(hostname: &str, ip: &[u8; 4], buf: &mut [u8; 512]) -> usize {
    buf.fill(0);
    // Header: ID=0, flags=0x8400 (QR=1, AA=1), QDCOUNT=0, ANCOUNT=1
    buf[2] = 0x84; buf[3] = 0x00;
    buf[6] = 0; buf[7] = 1;

    let mut pos = 12;
    pos += encode_mdns_name(hostname, &mut buf[pos..]);
    // TYPE A
    buf[pos] = 0; buf[pos + 1] = 1; pos += 2;
    // CLASS IN with cache-flush bit (0x8001)
    buf[pos] = 0x80; buf[pos + 1] = 0x01; pos += 2;
    // TTL: 120s
    buf[pos..pos + 4].copy_from_slice(&120u32.to_be_bytes()); pos += 4;
    // RDLENGTH: 4
    buf[pos] = 0; buf[pos + 1] = 4; pos += 2;
    // RDATA: IP
    buf[pos..pos + 4].copy_from_slice(ip); pos += 4;

    pos
}

/// If query is an mDNS A/ANY query for our hostname.local, build a response.
fn match_mdns_query(query: &[u8], hostname: &str, ip: &[u8; 4], resp: &mut [u8; 512]) -> Option<usize> {
    if query.len() < 12 { return None; }
    // Must be a query (QR=0)
    if query[2] & 0x80 != 0 { return None; }
    let qdcount = u16::from_be_bytes([query[4], query[5]]);
    if qdcount < 1 { return None; }

    // Parse QNAME at offset 12
    let mut pos = 12usize;
    if pos >= query.len() { return None; }

    // First label: must match hostname (case-insensitive)
    let label1_len = query[pos] as usize;
    if label1_len != hostname.len() { return None; }
    pos += 1;
    if pos + label1_len > query.len() { return None; }
    let hn = hostname.as_bytes();
    for i in 0..label1_len {
        if query[pos + i].to_ascii_lowercase() != hn[i].to_ascii_lowercase() {
            return None;
        }
    }
    pos += label1_len;

    // Second label: must be "local"
    if pos >= query.len() { return None; }
    if query[pos] as usize != 5 { return None; }
    pos += 1;
    if pos + 5 > query.len() { return None; }
    for (i, &expected) in b"local".iter().enumerate() {
        if query[pos + i].to_ascii_lowercase() != expected {
            return None;
        }
    }
    pos += 5;

    // Null terminator
    if pos >= query.len() || query[pos] != 0 { return None; }
    pos += 1;

    // QTYPE + QCLASS
    if pos + 4 > query.len() { return None; }
    let qtype = u16::from_be_bytes([query[pos], query[pos + 1]]);
    // Only respond to A (1) or ANY (255)
    if qtype != 1 && qtype != 255 { return None; }

    Some(build_mdns_announce(hostname, ip, resp))
}
