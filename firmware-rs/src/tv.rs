//! tv.rs — Modular TV volume control
//!
//! Supported brands:
//!   LG WebOS   — ssap:// WebSocket on port 3000
//!   Samsung    — Smart Remote WS on port 8001
//!   Sony       — Bravia REST JSON-RPC on port 80
//!   Roku       — ECP HTTP on port 8060

use core::fmt::Write;
use defmt::*;
use embedded_io_async::Write as _;
use embassy_net::{Stack, IpAddress, IpEndpoint};
use embassy_net::tcp::TcpSocket;
use embassy_net::udp::UdpSocket;
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel, mutex::Mutex};
use embassy_futures::select::{select, Either};
use embassy_time::{Duration, Timer, with_timeout};

use crate::ducking::{DuckCommand, DuckingEngine};
use crate::{WifiCmd, WIFI_CMD_CH, TV_STATUS};

const SAMSUNG_TLS_PORT: u16 = 8002;

const RESTORE_STEP_MS: u64 = 400;
const SAMSUNG_APP_B64: &str = "R3VhcmRpYW5TZW5zb3I=";

// ── TV brand + config ───────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, defmt::Format)]
pub enum TvBrand {
    Lg,
    Samsung,
    Sony,
    Roku,
}

impl TvBrand {
    pub fn supports_absolute_volume(self) -> bool {
        matches!(self, TvBrand::Lg | TvBrand::Sony)
    }

    pub fn default_port(self) -> u16 {
        match self {
            TvBrand::Lg      => 3000,
            TvBrand::Samsung => 8001,
            TvBrand::Sony    => 80,
            TvBrand::Roku    => 8060,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "lg" | "webos" | "lge" => Some(TvBrand::Lg),
            "samsung"              => Some(TvBrand::Samsung),
            "sony" | "bravia"      => Some(TvBrand::Sony),
            "roku"                 => Some(TvBrand::Roku),
            _                      => None,
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            TvBrand::Lg      => 0,
            TvBrand::Samsung => 1,
            TvBrand::Sony    => 2,
            TvBrand::Roku    => 3,
        }
    }

    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => TvBrand::Samsung,
            2 => TvBrand::Sony,
            3 => TvBrand::Roku,
            _ => TvBrand::Lg,
        }
    }

    /// LG and Samsung use persistent WebSocket connections.
    /// Sony and Roku use plain HTTP (reconnect per command).
    pub fn uses_websocket(self) -> bool {
        matches!(self, TvBrand::Lg | TvBrand::Samsung)
    }
}

#[derive(Clone, defmt::Format)]
pub struct TvConfig {
    pub ip:            heapless::String<16>,
    pub brand:         TvBrand,
    pub sony_psk:      heapless::String<8>,
    pub samsung_token: heapless::String<16>,
}

impl TvConfig {
    pub fn default() -> Self {
        let mut ip = heapless::String::new();
        let _ = ip.push_str(match option_env!("GUARDIAN_TV_IP") {
            Some(v) => v,
            None => "",
        });
        Self {
            ip,
            brand:         TvBrand::Lg,
            sony_psk:      heapless::String::new(),
            samsung_token: heapless::String::new(),
        }
    }

    pub fn is_configured(&self) -> bool { !self.ip.is_empty() }
}

// ── Config-change wake signal (ws_task → tv_task) ───────────────────────────
/// ws.rs signals this when a `set_tv` command updates the TV config,
/// so tv_task can abort its retry sleep and read the new config immediately.
pub static TV_WAKE_CH: Channel<ThreadModeRawMutex, (), 1> = Channel::new();

/// Wait for a duration, but wake early if TV config changes.
/// Returns `true` if woken by signal (config changed).
async fn wait_or_wake(dur: Duration) -> bool {
    match select(Timer::after(dur), TV_WAKE_CH.receive()).await {
        Either::First(_) => false,
        Either::Second(_) => true,
    }
}

// ── Duck command channel (ducking_task → tv_task) ────────────────────────────
static DUCK_CHANNEL: Channel<ThreadModeRawMutex, DuckCommand, 8> = Channel::new();

pub async fn send_duck_command(cmd: DuckCommand) {
    if DUCK_CHANNEL.try_send(cmd).is_err() {
        dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Warn,
            "DUCK_CHANNEL full");
    }
}

// ── SSDP discovery ──────────────────────────────────────────────────────────

/// Discovered TV from SSDP scan
#[derive(Clone, defmt::Format)]
pub struct DiscoveredTv {
    pub ip:    heapless::String<16>,
    pub name:  heapless::String<48>,
    pub brand: heapless::String<16>,
}

/// Send multiple SSDP M-SEARCH probes and collect TV responses (~5 seconds).
///
/// Different TV brands require different SSDP search targets:
///   - LG/Samsung: respond to `ssdp:all`
///   - Sony Bravia: requires `urn:schemas-sony-com:service:ScalarWebAPI:1`
///   - Roku: uses `roku:ecp`
///
/// Each probe is sent twice (UDP is unreliable, especially on WiFi).
pub async fn discover_tvs(stack: Stack<'static>) -> heapless::Vec<DiscoveredTv, 8> {
    let mut results: heapless::Vec<DiscoveredTv, 8> = heapless::Vec::new();

    // Join SSDP multicast group so embassy-net accepts responses
    let _ = stack.join_multicast_group(embassy_net::Ipv4Address::new(239, 255, 255, 250));

    let mut rx_buf = [0u8; 1024];
    let mut tx_buf = [0u8; 256];
    let mut rx_meta = [embassy_net::udp::PacketMetadata::EMPTY; 4];
    let mut tx_meta = [embassy_net::udp::PacketMetadata::EMPTY; 4];

    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx_buf, &mut tx_meta, &mut tx_buf);
    if socket.bind(0).is_err() {
        warn!("[tv] SSDP: failed to bind UDP");
        let _ = stack.leave_multicast_group(embassy_net::Ipv4Address::new(239, 255, 255, 250));
        return results;
    }

    let dest = IpEndpoint::new(IpAddress::v4(239, 255, 255, 250), 1900);

    // Brand-specific search targets
    const SEARCH_TARGETS: &[&str] = &[
        "ssdp:all",                                              // LG, Samsung, general
        "urn:schemas-sony-com:service:ScalarWebAPI:1",           // Sony Bravia
        "roku:ecp",                                              // Roku
        "urn:dial-multiscreen-org:service:dial:1",               // DIAL (many smart TVs)
    ];

    // Send each M-SEARCH twice for reliability (UDP packet loss on WiFi)
    for st in SEARCH_TARGETS {
        for _ in 0..2 {
            let mut pkt: heapless::String<256> = heapless::String::new();
            let _ = core::write!(
                pkt,
                "M-SEARCH * HTTP/1.1\r\n\
                 HOST: 239.255.255.250:1900\r\n\
                 MAN: \"ssdp:discover\"\r\n\
                 MX: 3\r\n\
                 ST: {}\r\n\r\n",
                st
            );
            let _ = socket.send_to(pkt.as_bytes(), dest).await;
            Timer::after(Duration::from_millis(100)).await;
        }
    }

    dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Info,
        "ssdp: sent {} probes ({}×2)", SEARCH_TARGETS.len(), SEARCH_TARGETS.len());

    // Collect responses for 5 seconds
    let deadline = embassy_time::Instant::now() + Duration::from_secs(5);
    let mut resp_buf = [0u8; 512];
    let mut rx_count: u32 = 0;

    loop {
        let remaining = deadline.saturating_duration_since(embassy_time::Instant::now());
        if remaining.as_millis() == 0 { break; }

        match with_timeout(remaining, socket.recv_from(&mut resp_buf)).await {
            Ok(Ok((n, from))) => {
                rx_count += 1;
                let ip_str = {
                    let addr = from.endpoint.addr;
                    let mut s: heapless::String<16> = heapless::String::new();
                    let _ = core::write!(s, "{}", addr);
                    s
                };

                let resp = core::str::from_utf8(&resp_buf[..n]).unwrap_or("");

                // Extract ST and SERVER headers for logging and brand detection
                let st_hdr = extract_ssdp_field(resp, "ST:");
                let server_hdr = extract_ssdp_field(resp, "SERVER:");

                // Log received packet with key headers (fits in 128-byte dev_log)
                dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Info,
                    "ssdp rx#{} {} ST={}", rx_count, ip_str.as_str(),
                    st_hdr.unwrap_or("?"));
                dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Info,
                    "ssdp rx#{} SVR={}", rx_count,
                    server_hdr.unwrap_or("(none)"));

                // Determine brand — first check ST header (most reliable),
                // then fall back to keyword scan of the full response body.
                let has = |kw: &[u8]| {
                    resp_buf[..n].windows(kw.len()).any(|w| {
                        w.iter().zip(kw).all(|(a, b)| a.to_ascii_lowercase() == *b)
                    })
                };

                let brand = if has(b"schemas-sony-com") || has(b"sony") || has(b"bravia") || has(b"scalarwebapi") {
                    "sony"
                } else if has(b"webos") || has(b"lge") || (has(b"lg") && has(b"tv")) {
                    "lg"
                } else if has(b"samsung") || has(b"tizen") {
                    "samsung"
                } else if has(b"roku") {
                    "roku"
                } else if has(b"dial-multiscreen") {
                    // DIAL response without brand keywords — could be any smart TV.
                    // Include it as "unknown" so the user can at least see the IP.
                    "unknown"
                } else {
                    dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Warn,
                        "ssdp: skip {}", ip_str.as_str());
                    continue;
                };

                // Deduplicate by IP
                if results.iter().any(|r| r.ip == ip_str) { continue; }

                // Extract friendly name
                let name = server_hdr.unwrap_or(brand);

                let mut tv = DiscoveredTv {
                    ip: ip_str,
                    name: heapless::String::new(),
                    brand: heapless::String::new(),
                };
                let _ = tv.name.push_str(&name[..name.len().min(47)]);
                let _ = tv.brand.push_str(brand);

                dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Info,
                    "ssdp: MATCH {} @ {}", brand, tv.ip.as_str());

                let _ = results.push(tv);
            }
            Ok(Err(_)) => {
                dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Warn,
                    "ssdp: recv error, continuing");
                continue;
            }
            Err(_) => break, // Timeout — deadline reached
        }
    }

    dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Info,
        "ssdp: multicast done, {} rx, {} found", rx_count, results.len());

    // Leave SSDP multicast group (done with multicast phase)
    let _ = stack.leave_multicast_group(embassy_net::Ipv4Address::new(239, 255, 255, 250));

    // ── Phase 2: Unicast sweep on nearby subnets ──────────────────────────
    // Multicast doesn't cross VLANs, but unicast routing often does.
    // Send M-SEARCH directly to each IP on common home subnets.
    let own_octets = stack.config_v4().map(|c| c.address.address().octets());
    if let Some(own) = own_octets {
        // Build list of /24 prefixes to sweep (skip our own subnet — already covered by multicast)
        let mut subnets: heapless::Vec<[u8; 3], 4> = heapless::Vec::new();
        const COMMON_PREFIXES: &[[u8; 3]] = &[
            [192, 168, 1],
            [192, 168, 0],
            [10, 0, 0],
            [10, 0, 1],
        ];
        for prefix in COMMON_PREFIXES {
            if prefix[0] == own[0] && prefix[1] == own[1] && prefix[2] == own[2] {
                continue; // Skip our own subnet
            }
            let _ = subnets.push(*prefix);
        }

        if !subnets.is_empty() {
            dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Info,
                "ssdp: unicast sweep on {} subnets", subnets.len());

            // Reuse the same socket — send unicast M-SEARCH to each host
            let search = "M-SEARCH * HTTP/1.1\r\n\
                          HOST: 239.255.255.250:1900\r\n\
                          MAN: \"ssdp:discover\"\r\n\
                          MX: 1\r\nST: ssdp:all\r\n\r\n";

            for prefix in &subnets {
                for host in 1u8..=254 {
                    let dest = IpEndpoint::new(
                        IpAddress::v4(prefix[0], prefix[1], prefix[2], host),
                        1900,
                    );
                    let _ = socket.send_to(search.as_bytes(), dest).await;
                    // Small yield every 16 hosts to not starve other tasks
                    if host % 16 == 0 {
                        Timer::after(Duration::from_millis(1)).await;
                    }
                }
            }

            dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Info,
                "ssdp: unicast probes sent, collecting...");

            // Collect unicast responses for 4 more seconds
            let deadline2 = embassy_time::Instant::now() + Duration::from_secs(4);
            loop {
                let remaining = deadline2.saturating_duration_since(embassy_time::Instant::now());
                if remaining.as_millis() == 0 { break; }

                match with_timeout(remaining, socket.recv_from(&mut resp_buf)).await {
                    Ok(Ok((n, from))) => {
                        rx_count += 1;
                        let ip_str = {
                            let addr = from.endpoint.addr;
                            let mut s: heapless::String<16> = heapless::String::new();
                            let _ = core::write!(s, "{}", addr);
                            s
                        };

                        // Skip if already found in multicast phase
                        if results.iter().any(|r| r.ip == ip_str) { continue; }

                        let resp = core::str::from_utf8(&resp_buf[..n]).unwrap_or("");
                        let server_hdr = extract_ssdp_field(resp, "SERVER:");

                        let has = |kw: &[u8]| {
                            resp_buf[..n].windows(kw.len()).any(|w| {
                                w.iter().zip(kw).all(|(a, b)| a.to_ascii_lowercase() == *b)
                            })
                        };

                        let brand = if has(b"schemas-sony-com") || has(b"sony") || has(b"bravia") || has(b"scalarwebapi") {
                            "sony"
                        } else if has(b"webos") || has(b"lge") || (has(b"lg") && has(b"tv")) {
                            "lg"
                        } else if has(b"samsung") || has(b"tizen") {
                            "samsung"
                        } else if has(b"roku") {
                            "roku"
                        } else if has(b"dial-multiscreen") {
                            "unknown"
                        } else {
                            continue;
                        };

                        let name = server_hdr.unwrap_or(brand);
                        let mut tv = DiscoveredTv {
                            ip: ip_str,
                            name: heapless::String::new(),
                            brand: heapless::String::new(),
                        };
                        let _ = tv.name.push_str(&name[..name.len().min(47)]);
                        let _ = tv.brand.push_str(brand);

                        dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Info,
                            "ssdp(uni): MATCH {} @ {}", brand, tv.ip.as_str());

                        let _ = results.push(tv);
                    }
                    Ok(Err(_)) => continue,
                    Err(_) => break,
                }
            }
        }
    }

    dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Info,
        "ssdp: total {} TVs found", results.len());

    info!("[tv] SSDP discovered {} TVs", results.len());
    results
}

fn extract_ssdp_field<'a>(resp: &'a str, key: &str) -> Option<&'a str> {
    // Case-insensitive line match without alloc (no to_ascii_lowercase)
    let line = resp.lines().find(|l| {
        if l.len() < key.len() { return false; }
        l.as_bytes()[..key.len()].iter().zip(key.as_bytes()).all(|(a, b)| a.to_ascii_lowercase() == b.to_ascii_lowercase())
    })?;
    let pos = line.find(':')?;
    Some(line[pos + 1..].trim())
}

// ── TV task ─────────────────────────────────────────────────────────────────
#[embassy_executor::task]
pub async fn tv_task(
    stack:     Stack<'static>,
    engine:    &'static Mutex<ThreadModeRawMutex, DuckingEngine>,
    tv_config: &'static Mutex<ThreadModeRawMutex, TvConfig>,
    tls_seed:  u64,
) {
    let _ = tls_seed; // Used for Samsung port 8002 TLS (activated when needed)
    Timer::after(Duration::from_secs(3)).await;

    let mut rx_buf    = [0u8; 1024];
    let mut tx_buf    = [0u8; 1024];
    let mut out_frame = [0u8; 512];

    let mut active_ip: heapless::String<16> = heapless::String::new();

    loop {
        let config = {
            let c = tv_config.lock().await;
            c.clone()
        };

        if !config.is_configured() {
            TV_STATUS.store(0, portable_atomic::Ordering::Relaxed);
            info!("[tv] No TV configured. Waiting…");
            wait_or_wake(Duration::from_secs(10)).await;
            continue;
        }

        if config.ip != active_ip {
            info!("[tv] TV config changed → {}", config.ip.as_str());
        }

        let tv_port = config.brand.default_port();
        let tv_addr = match parse_ip(config.ip.as_str()) {
            Some(a) => IpEndpoint::new(a, tv_port),
            None => {
                TV_STATUS.store(3, portable_atomic::Ordering::Relaxed);
                warn!("[tv] Invalid IP: {}", config.ip.as_str());
                wait_or_wake(Duration::from_secs(10)).await;
                continue;
            }
        };

        TV_STATUS.store(1, portable_atomic::Ordering::Relaxed);
        info!("[tv] Connecting to {} ({:?}) port {}", config.ip.as_str(), config.brand, tv_port);
        dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Info,
            "connecting {}:{} brand={:?}", config.ip.as_str(), tv_port, config.brand);

        let mut socket = TcpSocket::new(stack, &mut rx_buf, &mut tx_buf);
        socket.set_timeout(Some(Duration::from_secs(5)));

        if let Err(_e) = socket.connect(tv_addr).await {
            // Only set error if config hasn't changed while we were connecting
            let current_ip = { tv_config.lock().await.ip.clone() };
            if current_ip == config.ip {
                TV_STATUS.store(3, portable_atomic::Ordering::Relaxed);
            }
            dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Error,
                "tcp_fail {}:{}", config.ip.as_str(), tv_port);
            warn!("[tv] TCP connect failed");
            wait_or_wake(Duration::from_secs(5)).await;
            continue;
        }

        // ── Brand-specific handshake ────────────────────────────────────────
        let connected = match config.brand {
            TvBrand::Lg => lg_connect(&mut socket, config.ip.as_str(), tv_port, &mut out_frame).await,

            TvBrand::Samsung => {
                let token = {
                    let c = tv_config.lock().await;
                    c.samsung_token.clone()
                };
                let samsung_result = samsung_connect(&mut socket, config.ip.as_str(), tv_port, &token, &mut out_frame).await;

                // If plain WS on 8001 failed, log that 8002 TLS may be needed (Samsung 2021+)
                if samsung_result.is_none() && tv_port == 8001 {
                    info!("[tv/samsung] Port 8001 failed — Samsung 2021+ TVs may require port {} (TLS)", SAMSUNG_TLS_PORT);
                    // Full TLS WebSocket on port 8002 requires wrapping the socket in
                    // embedded_tls::TlsConnection. The embedded-tls crate and tls_seed are
                    // available. A future update will add TvSocket enum to abstract
                    // plain vs TLS transport for the command loop.
                }

                match samsung_result {
                    Some(new_token) => {
                        if !new_token.is_empty() {
                            let cfg_to_save = {
                                let mut c = tv_config.lock().await;
                                c.samsung_token = new_token;
                                c.clone()
                            };
                            info!("[tv/samsung] Token received (len={}), saving to flash", cfg_to_save.samsung_token.len());
                            if WIFI_CMD_CH.try_send(WifiCmd::SaveTvConfig(cfg_to_save)).is_err() {
                                warn!("[tv] Failed to send SaveTvConfig to wifi_task");
                            }
                        }
                        true
                    }
                    None => {
                        let mut c = tv_config.lock().await;
                        if !c.samsung_token.is_empty() {
                            info!("[tv/samsung] Clearing expired token");
                            c.samsung_token.clear();
                            let cfg_to_save = c.clone();
                            drop(c);
                            let _ = WIFI_CMD_CH.try_send(WifiCmd::SaveTvConfig(cfg_to_save));
                        }
                        false
                    }
                }
            }

            TvBrand::Sony => {
                // Validate PSK + connectivity by probing volume API
                match sony_get_volume(&mut socket, &mut out_frame, &config).await {
                    Some(vol) => {
                        info!("[tv/sony] Validated OK (volume={})", vol);
                        true
                    }
                    None => {
                        warn!("[tv/sony] Validation failed — check PSK and TV power");
                        false
                    }
                }
            }
            TvBrand::Roku => true,
        };

        if !connected {
            let current_ip = { tv_config.lock().await.ip.clone() };
            if current_ip == config.ip {
                TV_STATUS.store(3, portable_atomic::Ordering::Relaxed);
            }
            dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Warn,
                "handshake fail {:?}", config.brand);
            warn!("[tv] Handshake failed");
            wait_or_wake(Duration::from_secs(5)).await;
            continue;
        }

        TV_STATUS.store(2, portable_atomic::Ordering::Relaxed);
        active_ip.clear();
        let _ = active_ip.push_str(config.ip.as_str());
        dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Info,
            "handshake ok {:?}", config.brand);
        info!("[tv] Ready ({:?})", config.brand);

        // ── Command loop ────────────────────────────────────────────────────
        // LG/Samsung: persistent WebSocket — keepalive every 25s.
        // Sony/Roku: plain HTTP — fresh TCP connection per command (servers
        //   close idle keep-alive connections after ~15-30s, and limit total
        //   requests per connection).
        if config.brand.uses_websocket() {
            // ── WebSocket path (LG, Samsung) — persistent connection ─────
            'cmd: loop {
                let current_ip = {
                    let c = tv_config.lock().await;
                    c.ip.clone()
                };
                if current_ip != active_ip {
                    info!("[tv] Config changed — reconnecting");
                    break 'cmd;
                }

                let cmd = match with_timeout(Duration::from_secs(25), DUCK_CHANNEL.receive()).await {
                    Ok(cmd) => cmd,
                    Err(_) => {
                        if tv_keepalive(config.brand, &mut socket, &mut out_frame, &config).await {
                            dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Info,
                                "keepalive ok");
                            continue;
                        } else {
                            dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Warn,
                                "keepalive fail, reconnecting");
                            info!("[tv] Keepalive failed — reconnecting");
                            break 'cmd;
                        }
                    }
                };

                let ok = exec_duck_cmd(
                    cmd, config.brand, &mut socket, &mut out_frame, &config, engine,
                ).await;
                if !ok { break 'cmd; }
            }
            TV_STATUS.store(1, portable_atomic::Ordering::Relaxed);
            wait_or_wake(Duration::from_secs(2)).await;
        } else {
            // ── HTTP path (Sony, Roku) — fresh socket per command ─────────
            // Drop the handshake socket so we can create fresh ones per cmd.
            drop(socket);

            'cmd: loop {
                let current_ip = {
                    let c = tv_config.lock().await;
                    c.ip.clone()
                };
                if current_ip != active_ip {
                    info!("[tv] Config changed");
                    break 'cmd;
                }

                // Wait for duck command with timeout so we periodically check config
                let cmd = match with_timeout(Duration::from_secs(30), DUCK_CHANNEL.receive()).await {
                    Ok(cmd) => cmd,
                    Err(_) => continue, // Timeout — loop back to check config
                };
                if let DuckCommand::None = cmd { continue; }

                // Fresh TCP socket for this command
                let mut cmd_socket = TcpSocket::new(stack, &mut rx_buf, &mut tx_buf);
                cmd_socket.set_timeout(Some(Duration::from_secs(5)));

                if let Err(_) = cmd_socket.connect(tv_addr).await {
                    dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Error,
                        "http_connect_fail");
                    warn!("[tv] HTTP connect failed");
                    TV_STATUS.store(3, portable_atomic::Ordering::Relaxed);
                    Timer::after(Duration::from_secs(5)).await;
                    // Retry: don't break 'cmd, just try next command with a new socket
                    TV_STATUS.store(2, portable_atomic::Ordering::Relaxed);
                    continue;
                }

                let ok = exec_duck_cmd(
                    cmd, config.brand, &mut cmd_socket, &mut out_frame, &config, engine,
                ).await;

                if !ok {
                    dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Warn,
                        "http cmd fail, backoff 2s");
                    warn!("[tv] HTTP command failed — backoff 2s");
                    // Back off before next attempt to avoid hammering a rate-limited TV
                    Timer::after(Duration::from_secs(2)).await;
                }
                // cmd_socket dropped here — connection cleanly closed
            }
            // Config changed — loop back to re-read config
            TV_STATUS.store(1, portable_atomic::Ordering::Relaxed);
            Timer::after(Duration::from_millis(200)).await;
        }
    }
}

// ── Ramp helpers ────────────────────────────────────────────────────────────

async fn tv_ramp_up_absolute(
    brand: TvBrand, socket: &mut TcpSocket<'_>, out: &mut [u8; 512],
    cfg: &TvConfig, current: u8, target: u8,
) -> bool {
    let steps = target.saturating_sub(current);
    if steps == 0 { return true; }
    for i in 1..=steps {
        let vol = current + i;
        if !tv_set_volume(brand, socket, out, cfg, vol).await { return false; }
        info!("[tv] Ramp -> {}", vol);
        if i < steps { Timer::after(Duration::from_millis(RESTORE_STEP_MS)).await; }
    }
    true
}

async fn tv_ramp_up_relative(
    brand: TvBrand, socket: &mut TcpSocket<'_>, out: &mut [u8; 512],
    cfg: &TvConfig, steps: u8,
) -> bool {
    for i in 0..steps {
        if !tv_volume_up(brand, socket, out, cfg).await { return false; }
        info!("[tv] Ramp step {}/{}", i + 1, steps);
        if i + 1 < steps { Timer::after(Duration::from_millis(RESTORE_STEP_MS)).await; }
    }
    true
}

// ── Brand dispatch ──────────────────────────────────────────────────────────

async fn tv_get_volume(brand: TvBrand, s: &mut TcpSocket<'_>, out: &mut [u8; 512], cfg: &TvConfig) -> Option<u8> {
    match brand {
        TvBrand::Lg      => lg_get_volume(s, out).await,
        TvBrand::Sony    => sony_get_volume(s, out, cfg).await,
        TvBrand::Samsung | TvBrand::Roku => None,
    }
}

async fn tv_volume_down(brand: TvBrand, s: &mut TcpSocket<'_>, out: &mut [u8; 512], cfg: &TvConfig) -> bool {
    match brand {
        TvBrand::Lg      => lg_volume_down(s, out).await,
        TvBrand::Samsung => samsung_key(s, out, "KEY_VOLDOWN").await,
        TvBrand::Sony    => sony_volume_step(s, out, cfg, false).await,
        TvBrand::Roku    => roku_key(s, out, cfg, "VolumeDown").await,
    }
}

async fn tv_volume_up(brand: TvBrand, s: &mut TcpSocket<'_>, out: &mut [u8; 512], cfg: &TvConfig) -> bool {
    match brand {
        TvBrand::Lg      => lg_volume_up(s, out).await,
        TvBrand::Samsung => samsung_key(s, out, "KEY_VOLUP").await,
        TvBrand::Sony    => sony_volume_step(s, out, cfg, true).await,
        TvBrand::Roku    => roku_key(s, out, cfg, "VolumeUp").await,
    }
}

async fn tv_set_volume(brand: TvBrand, s: &mut TcpSocket<'_>, out: &mut [u8; 512], cfg: &TvConfig, vol: u8) -> bool {
    match brand {
        TvBrand::Lg   => lg_set_volume(s, out, vol).await,
        TvBrand::Sony => sony_set_volume(s, out, cfg, vol).await,
        TvBrand::Samsung | TvBrand::Roku => false,
    }
}

// ── Keepalive probe ──────────────────────────────────────────────────────────
/// Send a lightweight probe to keep the TCP connection alive.
/// Returns true if the connection is still healthy.
async fn tv_keepalive(brand: TvBrand, s: &mut TcpSocket<'_>, out: &mut [u8; 512], cfg: &TvConfig) -> bool {
    match brand {
        // LG/Sony: query current volume (small response, no side-effects)
        TvBrand::Lg   => lg_get_volume(s, out).await.is_some(),
        TvBrand::Sony => sony_get_volume(s, out, cfg).await.is_some(),
        // Samsung: send a WebSocket ping frame (opcode 0x9)
        TvBrand::Samsung => {
            out[0] = 0x89; // FIN + ping opcode
            out[1] = 0x80; // MASK bit, 0-length payload
            out[2..6].copy_from_slice(&[0x37, 0x5A, 0x1E, 0x9C]); // mask key
            s.write_all(&out[..6]).await.is_ok()
        }
        // Roku: lightweight device-info query
        TvBrand::Roku => {
            let mut req: heapless::String<128> = heapless::String::new();
            let _ = core::write!(req,
                "GET /query/device-info HTTP/1.1\r\nHost: {}:8060\r\nConnection: keep-alive\r\n\r\n",
                cfg.ip.as_str()
            );
            if s.write_all(req.as_bytes()).await.is_err() { return false; }
            read_http_response(s, out).await.is_some()
        }
    }
}

// ── Duck command executor (shared by WS and HTTP paths) ─────────────────────
async fn exec_duck_cmd(
    cmd:    DuckCommand,
    brand:  TvBrand,
    socket: &mut TcpSocket<'_>,
    out:    &mut [u8; 512],
    cfg:    &TvConfig,
    engine: &Mutex<ThreadModeRawMutex, DuckingEngine>,
) -> bool {
    match cmd {
        DuckCommand::None => true,

        DuckCommand::VolumeUp => {
            dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Info,
                "duck_cmd: VolumeUp");
            let ok = tv_volume_up(brand, socket, out, cfg).await;
            if !ok {
                dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Error,
                    "vol_up_fail");
                warn!("[tv] VolumeUp failed");
            } else {
                info!("[tv] Volume up");
            }
            ok
        }

        DuckCommand::VolumeDown => {
            dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Info,
                "duck_cmd: VolumeDown");
            let needs_query = {
                let eng = engine.lock().await;
                eng.original_volume.is_none() && brand.supports_absolute_volume()
            };
            if needs_query {
                if let Some(vol) = tv_get_volume(brand, socket, out, cfg).await {
                    let mut eng = engine.lock().await;
                    eng.set_original_volume(vol);
                    dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Info,
                        "got_volume={}", vol);
                    info!("[tv] Captured original volume: {}", vol);
                } else {
                    dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Warn,
                        "volume_query_fail");
                }
            }
            let ok = tv_volume_down(brand, socket, out, cfg).await;
            if !ok {
                dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Error,
                    "vol_down_fail");
                warn!("[tv] VolumeDown failed");
            } else {
                info!("[tv] Volume down");
            }
            ok
        }

        DuckCommand::Restore { original_volume: orig, steps } => {
            dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Info,
                "duck_cmd: Restore orig={:?} steps={}", orig, steps);
            let ok = if brand.supports_absolute_volume() {
                if let Some(orig_vol) = orig {
                    let current = orig_vol.saturating_sub(steps);
                    tv_ramp_up_absolute(brand, socket, out, cfg, current, orig_vol).await
                } else {
                    tv_ramp_up_relative(brand, socket, out, cfg, steps).await
                }
            } else {
                tv_ramp_up_relative(brand, socket, out, cfg, steps).await
            };

            if !ok {
                dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Error,
                    "restore_fail, clearing duck state");
                warn!("[tv] Restore failed — clearing duck state");
                let mut eng = engine.lock().await;
                eng.clear_duck_state();
            } else {
                dev_log!(crate::dev_log::LogCat::Tv, crate::dev_log::LogLevel::Info,
                    "restored ok");
                info!("[tv] Volume restored");
                let mut eng = engine.lock().await;
                if eng.state() == crate::ducking::DuckingState::Restoring {
                    eng.clear_duck_state();
                }
            }
            ok
        }
    }
}

// ── LG WebOS ────────────────────────────────────────────────────────────────

const LG_PAIR_MSG: &str = r#"{
  "type":"register","id":"reg_1",
  "payload":{
    "forcePairing":false,"pairingType":"PROMPT",
    "manifest":{
      "manifestVersion":1,"appVersion":"1.0",
      "signed":{
        "created":"20250101","appId":"com.guardian.soundsensor",
        "vendorId":"com.guardian",
        "localizedAppNames":{"":"Guardian Sound Sensor"},
        "localizedVendorNames":{"":"Guardian"},
        "permissions":["CONTROL_AUDIO","READ_CURRENT_CHANNEL"],
        "serial":"2025010100001"
      }
    }
  }
}"#;

async fn lg_connect(socket: &mut TcpSocket<'_>, host: &str, port: u16, out: &mut [u8; 512]) -> bool {
    if !client_ws_handshake(socket, host, port, "/").await { return false; }
    let n = ws_frame_masked(LG_PAIR_MSG.as_bytes(), out);
    if socket.write_all(&out[..n]).await.is_err() { return false; }
    info!("[tv/lg] Pairing sent — accept on TV if prompted");
    true
}

async fn lg_get_volume(socket: &mut TcpSocket<'_>, out: &mut [u8; 512]) -> Option<u8> {
    let req = r#"{"type":"request","id":"vol_q","uri":"ssap://audio/getVolume"}"#;
    let n = ws_frame_masked(req.as_bytes(), out);
    socket.write_all(&out[..n]).await.ok()?;
    let mut rx = [0u8; 256];
    let len = with_timeout(Duration::from_secs(2), read_ws_frame(socket, &mut rx)).await.ok()??;
    parse_volume_from_json(&rx[..len])
}

async fn lg_volume_down(socket: &mut TcpSocket<'_>, out: &mut [u8; 512]) -> bool {
    let msg = r#"{"type":"request","id":"vol_d","uri":"ssap://audio/volumeDown"}"#;
    let n = ws_frame_masked(msg.as_bytes(), out);
    socket.write_all(&out[..n]).await.is_ok()
}

async fn lg_volume_up(socket: &mut TcpSocket<'_>, out: &mut [u8; 512]) -> bool {
    let msg = r#"{"type":"request","id":"vol_u","uri":"ssap://audio/volumeUp"}"#;
    let n = ws_frame_masked(msg.as_bytes(), out);
    socket.write_all(&out[..n]).await.is_ok()
}

async fn lg_set_volume(socket: &mut TcpSocket<'_>, out: &mut [u8; 512], vol: u8) -> bool {
    let mut msg: heapless::String<128> = heapless::String::new();
    let _ = core::write!(
        msg,
        r#"{{"type":"request","id":"vol_s","uri":"ssap://audio/setVolume","payload":{{"volume":{}}}}}"#,
        vol
    );
    let n = ws_frame_masked(msg.as_bytes(), out);
    socket.write_all(&out[..n]).await.is_ok()
}

// ── Samsung Tizen ───────────────────────────────────────────────────────────

async fn samsung_connect(
    socket: &mut TcpSocket<'_>,
    host: &str,
    port: u16,
    existing_token: &heapless::String<16>,
    out: &mut [u8; 512],
) -> Option<heapless::String<16>> {
    let mut path: heapless::String<128> = heapless::String::new();
    if existing_token.is_empty() {
        let _ = core::write!(path, "/api/v2/channels/samsung.remote.control?name={}", SAMSUNG_APP_B64);
    } else {
        let _ = core::write!(
            path,
            "/api/v2/channels/samsung.remote.control?name={}&token={}",
            SAMSUNG_APP_B64, existing_token.as_str()
        );
    }

    if !client_ws_handshake(socket, host, port, path.as_str()).await { return None; }

    info!("[tv/samsung] Waiting for TV pairing event…");
    let mut ws_buf = [0u8; 512];
    let frame_len = match with_timeout(
        Duration::from_secs(30),
        read_ws_frame(socket, &mut ws_buf),
    ).await {
        Ok(Some(n)) => n,
        _ => {
            warn!("[tv/samsung] Pairing timeout or read error");
            return None;
        }
    };

    let frame = core::str::from_utf8(&ws_buf[..frame_len]).unwrap_or("");

    if frame.contains("ms.channel.unauthorized") {
        warn!("[tv/samsung] TV rejected connection");
        return None;
    }
    if !frame.contains("ms.channel.connect") {
        warn!("[tv/samsung] Unexpected event");
        return None;
    }

    let mut token: heapless::String<16> = heapless::String::new();
    if let Some(tok) = parse_json_str(frame, "\"token\":") {
        // Truncate to 16 chars max (heapless::String<16> is all-or-nothing)
        let _ = token.push_str(&tok[..tok.len().min(16)]);
        info!("[tv/samsung] Paired (token len={})", tok.len());
    } else {
        info!("[tv/samsung] Connected (no token — older TV)");
    }

    let _ = out;
    Some(token)
}

async fn samsung_key(socket: &mut TcpSocket<'_>, out: &mut [u8; 512], key: &str) -> bool {
    let mut msg: heapless::String<192> = heapless::String::new();
    let _ = core::write!(
        msg,
        r#"{{"method":"ms.remote.control","params":{{"Cmd":"Click","DataOfCmd":"{}","Option":"false","TypeOfRemote":"SendRemoteKey"}}}}"#,
        key
    );
    let n = ws_frame_masked(msg.as_bytes(), out);
    socket.write_all(&out[..n]).await.is_ok()
}

// ── Sony Bravia ─────────────────────────────────────────────────────────────

async fn sony_get_volume(socket: &mut TcpSocket<'_>, out: &mut [u8; 512], cfg: &TvConfig) -> Option<u8> {
    const BODY: &str = r#"{"method":"getVolumeInformation","id":33,"params":[],"version":"1.0"}"#;
    let resp_bytes = sony_http_post(socket, out, &cfg.ip, &cfg.sony_psk, "/sony/audio", BODY).await?;
    let resp = core::str::from_utf8(resp_bytes).ok()?;
    let body_start = resp.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
    let body = &resp[body_start..];
    let speaker_pos = body.find(r#""speaker""#)?;
    let vol_key = r#""volume":"#;
    let vol_pos = speaker_pos + body[speaker_pos..].find(vol_key)?;
    let rest = &body[vol_pos + vol_key.len()..];
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

async fn sony_set_volume(socket: &mut TcpSocket<'_>, out: &mut [u8; 512], cfg: &TvConfig, vol: u8) -> bool {
    let mut body: heapless::String<128> = heapless::String::new();
    let _ = core::write!(
        body,
        r#"{{"method":"setAudioVolume","id":98,"params":[{{"target":"speaker","volume":"{}"}}],"version":"1.2"}}"#,
        vol
    );
    sony_http_post(socket, out, &cfg.ip, &cfg.sony_psk, "/sony/audio", body.as_str()).await.is_some()
}

/// Step Sony volume by exactly 1 using relative volume strings ("+1" / "-1").
async fn sony_volume_step(socket: &mut TcpSocket<'_>, out: &mut [u8; 512], cfg: &TvConfig, up: bool) -> bool {
    let vol_str = if up { "+1" } else { "-1" };
    let mut body: heapless::String<128> = heapless::String::new();
    let _ = core::write!(
        body,
        r#"{{"method":"setAudioVolume","id":98,"params":[{{"target":"speaker","volume":"{}"}}],"version":"1.0"}}"#,
        vol_str
    );
    sony_http_post(socket, out, &cfg.ip, &cfg.sony_psk, "/sony/audio", body.as_str()).await.is_some()
}

async fn sony_http_post<'b>(
    socket: &mut TcpSocket<'_>,
    out: &'b mut [u8; 512],
    ip: &str,
    psk: &str,
    path: &str,
    body: &str,
) -> Option<&'b [u8]> {
    let mut headers: heapless::String<256> = heapless::String::new();
    let _ = core::write!(
        headers,
        "POST {} HTTP/1.1\r\n\
         Host: {}\r\n\
         Content-Type: application/json\r\n\
         X-Auth-PSK: {}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        path, ip, psk, body.len()
    );
    socket.write_all(headers.as_bytes()).await.ok()?;
    socket.write_all(body.as_bytes()).await.ok()?;

    let n = read_http_response(socket, out).await?;
    if out[..n.min(12)].starts_with(b"HTTP/1.1 200") { Some(&out[..n]) } else { None }
}

// ── Roku ECP ────────────────────────────────────────────────────────────────

async fn roku_key(socket: &mut TcpSocket<'_>, out: &mut [u8; 512], cfg: &TvConfig, key: &str) -> bool {
    let mut req: heapless::String<128> = heapless::String::new();
    let _ = core::write!(
        req,
        "POST /keypress/{} HTTP/1.1\r\nHost: {}:8060\r\nContent-Length: 0\r\n\r\n",
        key, cfg.ip.as_str()
    );
    if socket.write_all(req.as_bytes()).await.is_err() { return false; }
    let resp_result = read_http_response(socket, out).await;
    match resp_result {
        Some(n) => {
            let status_ok = n >= 12 && (out[..12].starts_with(b"HTTP/1.1 200") || out[..12].starts_with(b"HTTP/1.1 204"));
            if !status_ok {
                warn!("[tv/roku] Non-2xx response");
                return false;
            }
            true
        }
        None => {
            warn!("[tv/roku] No response (connection lost)");
            false
        }
    }
}

// ── Low-level helpers ───────────────────────────────────────────────────────

async fn client_ws_handshake(socket: &mut TcpSocket<'_>, host: &str, port: u16, path: &str) -> bool {
    let mut req: heapless::String<384> = heapless::String::new();
    let _ = core::write!(
        req,
        "GET {} HTTP/1.1\r\nHost: {}:{}\r\nUpgrade: websocket\r\n\
         Connection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n",
        path, host, port
    );
    if socket.write_all(req.as_bytes()).await.is_err() { return false; }

    let mut buf = [0u8; 256];
    let mut len = 0;
    loop {
        match socket.read(&mut buf[len..]).await {
            Ok(0) | Err(_) => return false,
            Ok(n) => {
                len += n;
                if buf[..len].windows(4).any(|w| w == b"\r\n\r\n") { break; }
                if len >= buf.len() { return false; }
            }
        }
    }
    buf[..len].starts_with(b"HTTP/1.1 101")
}

async fn read_http_response(socket: &mut TcpSocket<'_>, buf: &mut [u8; 512]) -> Option<usize> {
    let mut len = 0usize;
    loop {
        match socket.read(&mut buf[len..]).await {
            Ok(0) | Err(_) => { if len == 0 { return None; } break; }
            Ok(n) => {
                len += n;
                if buf[..len].windows(4).any(|w| w == b"\r\n\r\n") { break; }
                if len >= buf.len() { break; }
            }
        }
    }

    let header_text = core::str::from_utf8(&buf[..len.min(400)]).unwrap_or("");
    if let Some(cl) = header_text.lines()
        .find(|l| {
            l.len() >= 15 && l.as_bytes()[..15].iter().zip(b"content-length:").all(|(a, b)| a.to_ascii_lowercase() == *b)
        })
        .and_then(|l| l.split(':').nth(1))
        .and_then(|v| v.trim().parse::<usize>().ok())
    {
        let header_end = buf[..len].windows(4).position(|w| w == b"\r\n\r\n")
            .map(|i| i + 4).unwrap_or(len);
        let body_received = len.saturating_sub(header_end);
        let remaining = cl.saturating_sub(body_received);
        if remaining > 0 && len + remaining <= buf.len() {
            if read_exact(socket, &mut buf[len..len + remaining]).await.is_some() {
                len += remaining;
            }
        }
    }

    Some(len)
}

async fn read_ws_frame(socket: &mut TcpSocket<'_>, buf: &mut [u8]) -> Option<usize> {
    let mut hdr = [0u8; 2];
    read_exact(socket, &mut hdr).await?;
    let raw_len = (hdr[1] & 0x7F) as usize;
    let payload_len = match raw_len {
        126 => {
            let mut ext = [0u8; 2];
            read_exact(socket, &mut ext).await?;
            u16::from_be_bytes(ext) as usize
        }
        127 => return None,
        n => n,
    };
    if payload_len > buf.len() { return None; }
    read_exact(socket, &mut buf[..payload_len]).await?;
    Some(payload_len)
}

async fn read_exact(socket: &mut TcpSocket<'_>, buf: &mut [u8]) -> Option<()> {
    let mut pos = 0;
    while pos < buf.len() {
        match socket.read(&mut buf[pos..]).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => pos += n,
        }
    }
    Some(())
}

/// Build a masked WebSocket text frame (RFC 6455 requires clients to mask).
/// Uses a fixed masking key — sufficient for non-security purposes.
fn ws_frame_masked(payload: &[u8], out: &mut [u8]) -> usize {
    const MASK_KEY: [u8; 4] = [0x37, 0x5A, 0x1E, 0x9C];
    let len = payload.len();
    let hlen = if len < 126 { 2 } else { 4 };
    out[0] = 0x81; // FIN + text opcode
    if len < 126 {
        out[1] = 0x80 | len as u8; // MASK bit set
    } else {
        out[1] = 0x80 | 126;
        out[2] = (len >> 8) as u8;
        out[3] = (len & 0xFF) as u8;
    }
    // Write masking key
    out[hlen..hlen + 4].copy_from_slice(&MASK_KEY);
    // Write masked payload
    for (i, &b) in payload.iter().enumerate() {
        out[hlen + 4 + i] = b ^ MASK_KEY[i % 4];
    }
    hlen + 4 + len
}

fn parse_volume_from_json(json: &[u8]) -> Option<u8> {
    let s = core::str::from_utf8(json).ok()?;
    let pos = s.find("\"volume\":")?;
    let rest = s[pos + 9..].trim_start_matches(|c: char| c == ' ' || c == '\t');
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    if end == 0 { return None; }
    rest[..end].parse().ok()
}

fn parse_json_str<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let pos   = s.find(key)?;
    let after = &s[pos + key.len()..];
    let inner = after.trim_start_matches(|c: char| c == ' ').strip_prefix('"')?;
    let end   = inner.find('"')?;
    Some(&inner[..end])
}

fn parse_ip(s: &str) -> Option<IpAddress> {
    let mut p = s.splitn(4, '.');
    let a = p.next()?.parse::<u8>().ok()?;
    let b = p.next()?.parse::<u8>().ok()?;
    let c = p.next()?.parse::<u8>().ok()?;
    let d = p.next()?.parse::<u8>().ok()?;
    Some(IpAddress::v4(a, b, c, d))
}
