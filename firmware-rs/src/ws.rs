//! ws.rs — WebSocket server task (port 81)
//!
//! Server → Client (every 100ms):
//!   {"db":-32.5,"armed":false,"tripwire":-20.0,"ducking":false,"fw":"0.3.0","pwa":"0.1.0"}
//!
//! Events:
//!   {"evt":"wifi_scan","networks":[...]}
//!   {"evt":"discovered","tvs":[...]}
//!   {"evt":"ota_status","checking":bool,"available":bool,...}
//!   {"evt":"wifi_reconfiguring","ssid":"..."}
//!
//! Client → Server commands:
//!   arm, disarm, calibrate_silence, calibrate_max, threshold,
//!   scan_wifi, set_wifi, set_tv, discover_tvs, ota_check

use core::fmt::Write;
use defmt::*;
use embedded_io_async::Write as _;
use embassy_futures::select::{select, Either};
use embassy_net::Stack;
use embassy_net::tcp::TcpSocket;
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, mutex::Mutex};
use embassy_time::{Duration, Timer};

use crate::{
    TELEM_SIGNAL, TELEMETRY, LED_CHANNEL, WIFI_CMD_CH, WIFI_EVT_CH,
    LedPattern, WifiCmd, WifiEvent,
    ducking::{DuckCommand, DuckingEngine},
    tv::{TvBrand, TvConfig},
};

const TCP_PORT: u16   = 81;
const TX_BUF:   usize = 1024;
const RX_BUF:   usize = 768;

#[embassy_executor::task]
pub async fn websocket_task(
    stack:     Stack<'static>,
    engine:    &'static Mutex<ThreadModeRawMutex, DuckingEngine>,
    tv_config: &'static Mutex<ThreadModeRawMutex, TvConfig>,
    tls_seed:  u64,
    ota_tcp:   &'static embassy_net::tcp::client::TcpClientState<1, 1024, 1024>,
) {
    let mut rx_buf = [0u8; RX_BUF];
    let mut tx_buf = [0u8; TX_BUF];

    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buf, &mut tx_buf);
        socket.set_timeout(Some(Duration::from_secs(60)));

        info!("[ws] Waiting for connection on port {}", TCP_PORT);
        if let Err(_e) = socket.accept(TCP_PORT).await {
            warn!("[ws] Accept error");
            Timer::after(Duration::from_millis(100)).await;
            continue;
        }
        info!("[ws] Client connected");

        if !ws_handshake(&mut socket).await {
            warn!("[ws] Handshake failed");
            continue;
        }

        handle_client(socket, stack, engine, tv_config, tls_seed, ota_tcp).await;
        info!("[ws] Client disconnected");
    }
}

// ── Handshake ─────────────────────────────────────────────────────────────

async fn ws_handshake(socket: &mut TcpSocket<'_>) -> bool {
    let mut buf = [0u8; 768];
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

    let request = core::str::from_utf8(&buf[..len]).unwrap_or("");
    let key = request
        .lines()
        .find(|l| {
            l.len() >= 17 && l.as_bytes()[..17].iter().zip(b"sec-websocket-key").all(|(a, b)| a.to_ascii_lowercase() == *b)
        })
        .and_then(|l| l.split(':').nth(1))
        .map(|k| k.trim());

    let Some(key) = key else { return false; };
    let accept = ws_accept_header(key);

    let mut resp: heapless::String<256> = heapless::String::new();
    let _ = core::write!(
        resp,
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
         Connection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        accept.as_str()
    );
    socket.write_all(resp.as_bytes()).await.is_ok()
}

// ── Per-client handler ──────────────────────────────────────────────────────

async fn handle_client(
    mut socket: TcpSocket<'_>,
    stack: Stack<'static>,
    engine:    &'static Mutex<ThreadModeRawMutex, DuckingEngine>,
    tv_config: &'static Mutex<ThreadModeRawMutex, TvConfig>,
    tls_seed:  u64,
    ota_tcp:   &'static embassy_net::tcp::client::TcpClientState<1, 1024, 1024>,
) {
    let mut out_frame = [0u8; 1100];

    loop {
        let mut rx_buf = [0u8; 384];

        // Nested select: wait for (telemetry OR 500ms timer) vs socket data.
        // The 500ms timer ensures WiFi events (scan results) get forwarded even
        // when no audio telemetry is flowing (e.g. mic not connected, AP mode setup).
        let telem_or_tick = select(
            TELEM_SIGNAL.receive(),
            Timer::after(Duration::from_millis(500)),
        );

        match select(telem_or_tick, socket.read(&mut rx_buf)).await {

            Either::First(_) => {
                // Read latest telemetry snapshot (written by ducking_task)
                let (db, armed, tripwire, ducking) = {
                    let t = TELEMETRY.lock().await;
                    (t.db, t.armed, t.tripwire, t.ducking)
                };

                // Check for WiFi events to forward
                if let Ok(evt) = WIFI_EVT_CH.try_receive() {
                    match evt {
                        WifiEvent::ScanResults(networks) => {
                            let json = format_wifi_scan(&networks);
                            let n = ws_text_frame(json.as_bytes(), &mut out_frame);
                            if socket.write_all(&out_frame[..n]).await.is_err() { break; }
                        }
                        WifiEvent::OtaComplete { success, version } => {
                            let mut json: heapless::String<256> = heapless::String::new();
                            if success {
                                let _ = core::write!(
                                    json,
                                    r#"{{"evt":"ota_done","pwa":"{}","fw":"{}"}}"#,
                                    version.as_str(), crate::FW_VERSION,
                                );
                            } else {
                                let _ = core::write!(
                                    json,
                                    r#"{{"evt":"ota_status","checking":false,"available":false,"current":"{}","latest":"{}","fw":"{}","error":true}}"#,
                                    crate::PWA_VERSION, crate::PWA_VERSION, crate::FW_VERSION,
                                );
                            }
                            let n = ws_text_frame(json.as_bytes(), &mut out_frame);
                            let _ = socket.write_all(&out_frame[..n]).await;
                        }
                    }
                }

                // Broadcast telemetry
                let tv_status = crate::TV_STATUS.load(portable_atomic::Ordering::Relaxed);
                let mut json: heapless::String<224> = heapless::String::new();
                let _ = core::write!(
                    json,
                    r#"{{"db":{:.2},"armed":{},"tripwire":{:.2},"ducking":{},"tv_status":{},"fw":"{}","pwa":"{}""#,
                    db, armed, tripwire, ducking, tv_status,
                    crate::FW_VERSION,
                    crate::PWA_VERSION,
                );
                #[cfg(feature = "dev-mode")]
                { let _ = json.push_str(r#","dev":true"#); }
                let _ = json.push('}');
                let n = ws_text_frame(json.as_bytes(), &mut out_frame);
                if socket.write_all(&out_frame[..n]).await.is_err() {
                    break;
                }

                // Drain dev-mode log entries and forward to client
                #[cfg(feature = "dev-mode")]
                {
                    use crate::dev_log::{DEV_LOG_CH, DEV_LOG_ACTIVE};
                    if DEV_LOG_ACTIVE.load(portable_atomic::Ordering::Relaxed) {
                        while let Ok(entry) = DEV_LOG_CH.try_receive() {
                            let mut log_json: heapless::String<256> = heapless::String::new();
                            let _ = core::write!(
                                log_json,
                                r#"{{"evt":"log","cat":"{}","lvl":"{}","msg":""#,
                                entry.cat.as_str(),
                                entry.level.as_str(),
                            );
                            push_json_escaped(&mut log_json, entry.msg.as_str());
                            let _ = log_json.push_str("\"}");
                            let n = ws_text_frame(log_json.as_bytes(), &mut out_frame);
                            if socket.write_all(&out_frame[..n]).await.is_err() { break; }
                        }
                    }
                }
            }

            Either::Second(Ok(n)) if n > 0 => {
                let last_db = { TELEMETRY.lock().await.db };
                process_frame(&rx_buf[..n], stack, engine, tv_config, last_db, &mut socket, &mut out_frame, tls_seed, ota_tcp).await;
            }

            Either::Second(_) => break,
        }
    }
}

/// Unmask an incoming WS frame and dispatch the JSON payload.
async fn process_frame(
    raw:       &[u8],
    stack:     Stack<'static>,
    engine:    &'static Mutex<ThreadModeRawMutex, DuckingEngine>,
    tv_config: &'static Mutex<ThreadModeRawMutex, TvConfig>,
    last_db:   f32,
    socket:    &mut TcpSocket<'_>,
    out_frame: &mut [u8; 1100],
    tls_seed:  u64,
    ota_tcp:   &'static embassy_net::tcp::client::TcpClientState<1, 1024, 1024>,
) {
    if raw.len() < 2 { return; }
    let masked  = (raw[1] & 0x80) != 0;
    let raw_len = (raw[1] & 0x7F) as usize;

    // Handle extended length (RFC 6455)
    let (payload_len, hdr_extra) = if raw_len == 126 {
        if raw.len() < 4 { return; }
        let ext = u16::from_be_bytes([raw[2], raw[3]]) as usize;
        (ext, 2)
    } else if raw_len == 127 {
        return; // We don't support 8-byte extended length
    } else {
        (raw_len, 0)
    };

    let mask_offset = 2 + hdr_extra;
    let hlen = mask_offset + if masked { 4 } else { 0 };
    if raw.len() < hlen + payload_len { return; }

    let mut payload = [0u8; 384];
    let plen = payload_len.min(payload.len());
    if masked {
        let mask = &raw[mask_offset..mask_offset + 4];
        for (i, b) in raw[hlen..hlen + plen].iter().enumerate() {
            payload[i] = b ^ mask[i % 4];
        }
    } else {
        payload[..plen].copy_from_slice(&raw[hlen..hlen + plen]);
    }

    apply_command(&payload[..plen], stack, engine, tv_config, last_db, socket, out_frame, tls_seed, ota_tcp).await;
}

async fn apply_command(
    payload:   &[u8],
    stack:     Stack<'static>,
    engine:    &'static Mutex<ThreadModeRawMutex, DuckingEngine>,
    tv_config: &'static Mutex<ThreadModeRawMutex, TvConfig>,
    last_db:   f32,
    socket:    &mut TcpSocket<'_>,
    out_frame: &mut [u8; 1100],
    tls_seed:  u64,
    ota_tcp:   &'static embassy_net::tcp::client::TcpClientState<1, 1024, 1024>,
) {
    let Ok(s) = core::str::from_utf8(payload) else { return };
    let cmd = parse_str_field(s, r#""cmd":"#);

    if cmd == Some("arm") {
        let mut eng = engine.lock().await;
        eng.arm();
        let _ = LED_CHANNEL.try_send(LedPattern::Armed);
        info!("[ws] Armed");

    } else if cmd == Some("disarm") {
        let restore_cmd = {
            let mut eng = engine.lock().await;
            eng.disarm()
        };
        if let DuckCommand::Restore { .. } = restore_cmd {
            crate::tv::send_duck_command(restore_cmd).await;
            info!("[ws] Disarmed (restoring TV volume)");
        } else {
            info!("[ws] Disarmed");
        }
        let _ = LED_CHANNEL.try_send(LedPattern::Idle);

    } else if cmd == Some("calibrate_silence") {
        let db = parse_f32_field(s, r#""db":"#).unwrap_or(last_db);
        let (floor, tripwire) = {
            let mut eng = engine.lock().await;
            eng.set_floor(db);
            (eng.floor_db, eng.tripwire_db)
        };
        let _ = WIFI_CMD_CH.try_send(WifiCmd::SaveCalibration { floor, tripwire });
        info!("[ws] Floor set to {} dBFS", db);

    } else if cmd == Some("calibrate_max") {
        let db = parse_f32_field(s, r#""db":"#).unwrap_or(last_db);
        let tripwire = db - 3.0;
        let (floor, tripwire) = {
            let mut eng = engine.lock().await;
            eng.set_tripwire(tripwire);
            (eng.floor_db, eng.tripwire_db)
        };
        let _ = WIFI_CMD_CH.try_send(WifiCmd::SaveCalibration { floor, tripwire });
        info!("[ws] Tripwire set to {} dBFS (TV at {})", tripwire, db);

    } else if cmd == Some("threshold") || s.contains(r#""threshold":"#) {
        if let Some(v) = parse_f32_field(s, r#""threshold":"#) {
            let (floor, tripwire) = {
                let mut eng = engine.lock().await;
                eng.set_tripwire(v);
                (eng.floor_db, eng.tripwire_db)
            };
            let _ = WIFI_CMD_CH.try_send(WifiCmd::SaveCalibration { floor, tripwire });
            info!("[ws] Manual tripwire: {}", v);
        }

    } else if cmd == Some("scan_wifi") {
        info!("[ws] WiFi scan requested");
        let _ = WIFI_CMD_CH.try_send(WifiCmd::Scan);

    } else if cmd == Some("set_wifi") {
        let raw_ssid = parse_str_field(s, r#""ssid":"#).unwrap_or("");
        let raw_pass = parse_str_field(s, r#""pass":"#).unwrap_or("");
        // Unescape JSON sequences: \" → " and \\ → \
        let ssid_h: heapless::String<64> = json_unescape(raw_ssid);
        let pass_h: heapless::String<64> = json_unescape(raw_pass);
        info!("[ws] WiFi reconfigure → {}", ssid_h.as_str());

        // Send wifi_reconfiguring event back to client (JSON-escape the SSID)
        let mut evt: heapless::String<256> = heapless::String::new();
        let _ = evt.push_str(r#"{"evt":"wifi_reconfiguring","ssid":""#);
        push_json_escaped(&mut evt, ssid_h.as_str());
        let _ = evt.push_str(r#""}"#);
        let n = ws_text_frame(evt.as_bytes(), out_frame);
        let _ = socket.write_all(&out_frame[..n]).await;

        WIFI_CMD_CH.send(WifiCmd::Reconfigure { ssid: ssid_h, pass: pass_h }).await;

    } else if cmd == Some("set_tv") {
        let ip    = parse_str_field(s, r#""ip":"#);
        let brand = parse_str_field(s, r#""brand":"#).and_then(TvBrand::parse);
        let psk   = parse_str_field(s, r#""psk":"#);

        // Disconnect case: ip is empty
        if let Some(ip_str) = ip {
            if ip_str.is_empty() {
                // Clear TV config
                crate::TV_STATUS.store(0, portable_atomic::Ordering::Relaxed);
                let cfg_to_save = {
                    let mut cfg = tv_config.lock().await;
                    cfg.ip.clear();
                    cfg.samsung_token.clear();
                    cfg.sony_psk.clear();
                    cfg.clone()
                };
                if WIFI_CMD_CH.try_send(WifiCmd::SaveTvConfig(cfg_to_save)).is_err() {
                    warn!("[ws] Failed to send SaveTvConfig (clear)");
                }
                let _ = crate::tv::TV_WAKE_CH.try_send(());
                info!("[ws] TV disconnected");
                return;
            }

            if let Some(brand) = brand {
                crate::TV_STATUS.store(1, portable_atomic::Ordering::Relaxed);
                let cfg_to_save = {
                    let mut cfg = tv_config.lock().await;
                    cfg.samsung_token.clear();
                    cfg.ip.clear();
                    let _ = cfg.ip.push_str(ip_str);
                    cfg.brand = brand;
                    if let Some(p) = psk {
                        let unescaped: heapless::String<16> = json_unescape(p);
                        cfg.sony_psk.clear();
                        let _ = cfg.sony_psk.push_str(&unescaped.as_str()[..unescaped.len().min(8)]);
                    }
                    cfg.clone()
                };
                if WIFI_CMD_CH.try_send(WifiCmd::SaveTvConfig(cfg_to_save)).is_err() {
                    warn!("[ws] Failed to send SaveTvConfig");
                }
                let _ = crate::tv::TV_WAKE_CH.try_send(());
                info!("[ws] TV → {} ({:?})", ip_str, brand);
            } else {
                warn!("[ws] set_tv: unknown brand");
            }
        }

    } else if cmd == Some("vol_up") {
        info!("[ws] Volume up (test)");
        crate::tv::send_duck_command(DuckCommand::VolumeUp).await;

    } else if cmd == Some("vol_down") {
        info!("[ws] Volume down (test)");
        crate::tv::send_duck_command(DuckCommand::VolumeDown).await;

    } else if cmd == Some("discover_tvs") {
        info!("[ws] TV discovery requested");
        let tvs = crate::tv::discover_tvs(stack).await;
        let json = format_discovered_tvs(&tvs);
        let n = ws_text_frame(json.as_bytes(), out_frame);
        let _ = socket.write_all(&out_frame[..n]).await;

    } else if cmd == Some("ota_check") {
        if crate::AP_MODE.load(portable_atomic::Ordering::Relaxed) {
            warn!("[ws] OTA check ignored in AP mode");
        } else {
            info!("[ws] OTA check requested");
            let result = crate::ota::check_for_update(
                stack,
                ota_tcp,
                tls_seed,
            ).await;
            let json = crate::ota::status_json(
                false,
                result.available,
                result.current.as_str(),
                result.latest.as_str(),
                false,
            );
            let n = ws_text_frame(json.as_bytes(), out_frame);
            let _ = socket.write_all(&out_frame[..n]).await;
        }

    } else if cmd == Some("ota_download") {
        if crate::AP_MODE.load(portable_atomic::Ordering::Relaxed) {
            warn!("[ws] OTA download ignored in AP mode");
        } else {
            info!("[ws] OTA download requested");
            let _ = WIFI_CMD_CH.try_send(WifiCmd::OtaDownload { tls_seed });
        }

    }

    // Dev-mode toggle (only compiled with dev-mode feature)
    #[cfg(feature = "dev-mode")]
    if cmd == Some("dev_toggle") {
        use crate::dev_log::DEV_LOG_ACTIVE;
        let was = DEV_LOG_ACTIVE.load(portable_atomic::Ordering::Relaxed);
        DEV_LOG_ACTIVE.store(!was, portable_atomic::Ordering::Relaxed);
        info!("[ws] Dev logging toggled: {}", !was);
    }
}

// ── Event formatters ────────────────────────────────────────────────────────

/// Append a JSON-escaped string to the output (escapes `"` and `\`).
fn push_json_escaped<const N: usize>(out: &mut heapless::String<N>, s: &str) {
    for &b in s.as_bytes() {
        match b {
            b'"'  => { let _ = out.push_str("\\\""); }
            b'\\' => { let _ = out.push_str("\\\\"); }
            _ => { let _ = out.push(b as char); }
        }
    }
}

fn format_wifi_scan(networks: &[crate::NetworkInfo]) -> heapless::String<1024> {
    let mut s: heapless::String<1024> = heapless::String::new();
    let _ = s.push_str(r#"{"evt":"wifi_scan","networks":["#);
    for (i, net) in networks.iter().enumerate() {
        if i > 0 { let _ = s.push(','); }
        let _ = s.push_str(r#"{"ssid":""#);
        push_json_escaped(&mut s, net.ssid.as_str());
        let _ = core::write!(s, r#"","rssi":{}}}"#, net.rssi);
    }
    let _ = s.push_str("]}");
    s
}

fn format_discovered_tvs(tvs: &[crate::tv::DiscoveredTv]) -> heapless::String<1024> {
    let mut s: heapless::String<1024> = heapless::String::new();
    let _ = s.push_str(r#"{"evt":"discovered","tvs":["#);
    for (i, tv) in tvs.iter().enumerate() {
        if i > 0 { let _ = s.push(','); }
        let _ = s.push_str(r#"{"ip":""#);
        push_json_escaped(&mut s, tv.ip.as_str());
        let _ = s.push_str(r#"","name":""#);
        push_json_escaped(&mut s, tv.name.as_str());
        let _ = s.push_str(r#"","brand":""#);
        push_json_escaped(&mut s, tv.brand.as_str());
        let _ = s.push_str(r#""}"#);
    }
    let _ = s.push_str("]}");
    s
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn parse_f32_field(s: &str, key: &str) -> Option<f32> {
    let mut search_from = 0;
    let pos = loop {
        let p = s[search_from..].find(key).map(|i| i + search_from)?;
        if p == 0 || matches!(s.as_bytes()[p - 1], b'{' | b',' | b' ' | b'\n' | b'\t') {
            break p;
        }
        search_from = p + 1;
    };
    let rest = s[pos + key.len()..].trim_start_matches(|c: char| c == ' ');
    let end  = rest.find(|c: char| !c.is_ascii_digit() && c != '-' && c != '.')
                   .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn parse_str_field<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    // Find key at a JSON structural position (after { , or whitespace, not inside a value)
    let mut search_from = 0;
    let pos = loop {
        let p = s[search_from..].find(key).map(|i| i + search_from)?;
        if p == 0 || matches!(s.as_bytes()[p - 1], b'{' | b',' | b' ' | b'\n' | b'\t') {
            break p;
        }
        search_from = p + 1;
    };
    let after = &s[pos + key.len()..];
    let inner = after.trim_start_matches(|c: char| c == ' ').strip_prefix('"')?;
    // Find the closing quote, skipping escaped quotes (\")
    // Count consecutive backslashes: even count means the quote is real
    let mut end = 0;
    let bytes = inner.as_bytes();
    while end < bytes.len() {
        if bytes[end] == b'"' {
            let mut bs = 0;
            while end > bs && bytes[end - 1 - bs] == b'\\' { bs += 1; }
            if bs % 2 == 0 { break; } // even backslashes → real closing quote
        }
        end += 1;
    }
    if end >= bytes.len() { return None; }
    Some(&inner[..end])
}

/// Unescape JSON string escape sequences: `\"` → `"` and `\\` → `\`.
/// Copies into a heapless::String, returning the unescaped result.
fn json_unescape<const N: usize>(s: &str) -> heapless::String<N> {
    let mut out: heapless::String<N> = heapless::String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'"'  => { let _ = out.push('"'); i += 2; }
                b'\\' => { let _ = out.push('\\'); i += 2; }
                _     => { let _ = out.push('\\'); i += 1; }
            }
        } else {
            let _ = out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

// ── WebSocket frame encoder ─────────────────────────────────────────────────

fn ws_text_frame(payload: &[u8], out: &mut [u8]) -> usize {
    let len  = payload.len();
    let hlen = if len < 126 { 2 } else { 4 };
    if hlen + len > out.len() {
        // Truncate payload to fit in output buffer
        dev_log!(crate::dev_log::LogCat::Ws, crate::dev_log::LogLevel::Warn,
            "ws_text_frame truncating {}→{}", len, out.len().saturating_sub(4));
        // Recalculate: try 2-byte header first, then 4-byte if needed
        let max2 = out.len().saturating_sub(2);
        let (trunc_hlen, trunc_len) = if max2 < 126 {
            (2, max2)
        } else {
            (4, out.len().saturating_sub(4))
        };
        out[0] = 0x81;
        if trunc_len < 126 {
            out[1] = trunc_len as u8;
        } else {
            out[1] = 126;
            out[2] = (trunc_len >> 8) as u8;
            out[3] = (trunc_len & 0xFF) as u8;
        }
        out[trunc_hlen..trunc_hlen + trunc_len].copy_from_slice(&payload[..trunc_len]);
        return trunc_hlen + trunc_len;
    }
    out[0] = 0x81;
    if len < 126 {
        out[1] = len as u8;
    } else {
        out[1] = 126;
        out[2] = (len >> 8) as u8;
        out[3] = (len & 0xFF) as u8;
    }
    out[hlen..hlen + len].copy_from_slice(payload);
    hlen + len
}

// ── SHA-1 (inline, no_std) ──────────────────────────────────────────────────

fn ws_accept_header(key: &str) -> heapless::String<32> {
    const GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut sha = Sha1::new();
    sha.update(key.as_bytes());
    sha.update(GUID);
    let hash = sha.finalise();
    let mut out: heapless::String<32> = heapless::String::new();
    base64_encode(&hash, &mut out);
    out
}

struct Sha1 { state: [u32; 5], count: u64, buf: [u8; 64], buf_len: usize }

impl Sha1 {
    fn new() -> Self {
        Self { state: [0x67452301,0xEFCDAB89,0x98BADCFE,0x10325476,0xC3D2E1F0],
               count: 0, buf: [0u8;64], buf_len: 0 }
    }
    fn update(&mut self, data: &[u8]) {
        for &b in data {
            self.buf[self.buf_len] = b;
            self.buf_len += 1;
            self.count   += 8;
            if self.buf_len == 64 { self.compress(); self.buf_len = 0; }
        }
    }
    fn finalise(mut self) -> [u8; 20] {
        self.buf[self.buf_len] = 0x80; self.buf_len += 1;
        if self.buf_len > 56 {
            while self.buf_len < 64 { self.buf[self.buf_len] = 0; self.buf_len += 1; }
            self.compress(); self.buf_len = 0;
        }
        while self.buf_len < 56 { self.buf[self.buf_len] = 0; self.buf_len += 1; }
        let b = self.count;
        self.buf[56..64].copy_from_slice(&b.to_be_bytes());
        self.compress();
        let mut o = [0u8; 20];
        for (i, &w) in self.state.iter().enumerate() {
            o[i*4..i*4+4].copy_from_slice(&w.to_be_bytes());
        }
        o
    }
    fn compress(&mut self) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(self.buf[i*4..i*4+4].try_into().unwrap());
        }
        for i in 16..80 {
            w[i] = (w[i-3]^w[i-8]^w[i-14]^w[i-16]).rotate_left(1);
        }
        let [mut a,mut b,mut c,mut d,mut e] = self.state;
        for i in 0..80 {
            let (f, k) = match i {
                 0..=19 => ((b&c)|((!b)&d),            0x5A827999u32),
                20..=39 => (b^c^d,                      0x6ED9EBA1u32),
                40..=59 => ((b&c)|(b&d)|(c&d),          0x8F1BBCDCu32),
                _        => (b^c^d,                      0xCA62C1D6u32),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e)
                     .wrapping_add(k).wrapping_add(w[i]);
            e=d; d=c; c=b.rotate_left(30); b=a; a=t;
        }
        self.state[0]=self.state[0].wrapping_add(a);
        self.state[1]=self.state[1].wrapping_add(b);
        self.state[2]=self.state[2].wrapping_add(c);
        self.state[3]=self.state[3].wrapping_add(d);
        self.state[4]=self.state[4].wrapping_add(e);
    }
}

const B64: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(input: &[u8], out: &mut heapless::String<32>) {
    let mut i = 0;
    while i + 2 < input.len() {
        let [b0, b1, b2] = [input[i] as usize, input[i+1] as usize, input[i+2] as usize];
        let _ = out.push(B64[ b0>>2                  ] as char);
        let _ = out.push(B64[((b0&3)<<4)|(b1>>4)     ] as char);
        let _ = out.push(B64[((b1&0xF)<<2)|(b2>>6)   ] as char);
        let _ = out.push(B64[ b2&0x3F                ] as char);
        i += 3;
    }
    match input.len() - i {
        1 => {
            let b0 = input[i] as usize;
            let _ = out.push(B64[b0>>2] as char);
            let _ = out.push(B64[(b0&3)<<4] as char);
            let _ = out.push('='); let _ = out.push('=');
        }
        2 => {
            let [b0, b1] = [input[i] as usize, input[i+1] as usize];
            let _ = out.push(B64[b0>>2] as char);
            let _ = out.push(B64[((b0&3)<<4)|(b1>>4)] as char);
            let _ = out.push(B64[(b1&0xF)<<2] as char);
            let _ = out.push('=');
        }
        _ => {}
    }
}
