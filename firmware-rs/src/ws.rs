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

use defmt::*;
use embassy_futures::select::{select, Either};
use embassy_net::Stack;
use embassy_net::TcpSocket;
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, mutex::Mutex};
use embassy_time::{Duration, Timer};
use cyw43_pio::NetDriver;

use crate::{
    DB_CHANNEL, LED_CHANNEL, WIFI_CMD_CH, WIFI_EVT_CH,
    LedPattern, WifiCmd, WifiEvent,
    ducking::{DuckCommand, DuckingEngine, DuckingState},
    tv::{TvBrand, TvConfig},
};

const TCP_PORT: u16   = 81;
const TX_BUF:   usize = 1024;
const RX_BUF:   usize = 512;

#[embassy_executor::task]
pub async fn websocket_task(
    stack:     &'static Stack<NetDriver<'static>>,
    engine:    &'static Mutex<ThreadModeRawMutex, DuckingEngine>,
    tv_config: &'static Mutex<ThreadModeRawMutex, TvConfig>,
) {
    let mut rx_buf = [0u8; RX_BUF];
    let mut tx_buf = [0u8; TX_BUF];

    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buf, &mut tx_buf);
        socket.set_timeout(Some(Duration::from_secs(60)));

        info!("[ws] Waiting for connection on port {}", TCP_PORT);
        if let Err(e) = socket.accept(TCP_PORT).await {
            warn!("[ws] Accept error: {:?}", e);
            Timer::after(Duration::from_millis(100)).await;
            continue;
        }
        info!("[ws] Client connected");

        if !ws_handshake(&mut socket).await {
            warn!("[ws] Handshake failed");
            continue;
        }

        handle_client(socket, stack, engine, tv_config).await;
        info!("[ws] Client disconnected");
    }
}

// ── Handshake ─────────────────────────────────────────────────────────────

async fn ws_handshake(socket: &mut TcpSocket<'_>) -> bool {
    let mut buf = [0u8; 512];
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
        .find(|l| l.to_ascii_lowercase().starts_with("sec-websocket-key"))
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
    stack: &'static Stack<NetDriver<'static>>,
    engine:    &'static Mutex<ThreadModeRawMutex, DuckingEngine>,
    tv_config: &'static Mutex<ThreadModeRawMutex, TvConfig>,
) {
    let mut out_frame = [0u8; 1100];
    let mut last_db = -60.0f32;

    loop {
        let mut rx_buf = [0u8; 384];

        match select(DB_CHANNEL.receive(), socket.read(&mut rx_buf)).await {

            Either::First(db) => {
                last_db = db;

                // Tick ducking engine
                let (duck_cmd, armed, tripwire, ducking) = {
                    let mut eng = engine.lock().await;
                    let cmd = eng.tick(db);
                    let ducking = eng.state() == DuckingState::Ducking;
                    (cmd, eng.armed, eng.tripwire_db, ducking)
                };

                // Update LED pattern based on state
                if ducking {
                    let _ = LED_CHANNEL.try_send(LedPattern::Ducking);
                } else if armed {
                    let _ = LED_CHANNEL.try_send(LedPattern::Armed);
                }

                if duck_cmd != DuckCommand::None {
                    crate::tv::send_duck_command(duck_cmd).await;
                }

                // Check for WiFi scan results to forward
                if let Ok(evt) = WIFI_EVT_CH.try_receive() {
                    match evt {
                        WifiEvent::ScanResults(networks) => {
                            let json = format_wifi_scan(&networks);
                            let n = ws_text_frame(json.as_bytes(), &mut out_frame);
                            if socket.write_all(&out_frame[..n]).await.is_err() { break; }
                        }
                    }
                }

                // Broadcast telemetry
                let mut json: heapless::String<192> = heapless::String::new();
                let _ = core::write!(
                    json,
                    r#"{{"db":{:.2},"armed":{},"tripwire":{:.2},"ducking":{},"fw":"{}","pwa":"{}"}}"#,
                    db, armed, tripwire, ducking,
                    crate::FW_VERSION,
                    crate::PWA_VERSION,
                );
                let n = ws_text_frame(json.as_bytes(), &mut out_frame);
                if socket.write_all(&out_frame[..n]).await.is_err() {
                    break;
                }
            }

            Either::Second(Ok(n)) if n > 0 => {
                process_frame(&rx_buf[..n], stack, engine, tv_config, last_db, &mut socket, &mut out_frame).await;
            }

            Either::Second(_) => break,
        }
    }
}

/// Unmask an incoming WS frame and dispatch the JSON payload.
async fn process_frame(
    raw:       &[u8],
    stack:     &'static Stack<NetDriver<'static>>,
    engine:    &'static Mutex<ThreadModeRawMutex, DuckingEngine>,
    tv_config: &'static Mutex<ThreadModeRawMutex, TvConfig>,
    last_db:   f32,
    socket:    &mut TcpSocket<'_>,
    out_frame: &mut [u8; 1100],
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

    apply_command(&payload[..plen], stack, engine, tv_config, last_db, socket, out_frame).await;
}

async fn apply_command(
    payload:   &[u8],
    stack:     &'static Stack<NetDriver<'static>>,
    engine:    &'static Mutex<ThreadModeRawMutex, DuckingEngine>,
    tv_config: &'static Mutex<ThreadModeRawMutex, TvConfig>,
    last_db:   f32,
    socket:    &mut TcpSocket<'_>,
    out_frame: &mut [u8; 1100],
) {
    let Ok(s) = core::str::from_utf8(payload) else { return };

    if s.contains(r#""cmd":"arm""#) {
        let mut eng = engine.lock().await;
        eng.arm();
        let _ = LED_CHANNEL.try_send(LedPattern::Armed);
        info!("[ws] Armed");

    } else if s.contains(r#""cmd":"disarm""#) {
        let mut eng = engine.lock().await;
        eng.disarm();
        let _ = LED_CHANNEL.try_send(LedPattern::Idle);
        info!("[ws] Disarmed");

    } else if s.contains(r#""cmd":"calibrate_silence""#) {
        let db = parse_f32_field(s, r#""db":"#).unwrap_or(last_db);
        let mut eng = engine.lock().await;
        eng.set_floor(db);
        info!("[ws] Floor set to {} dBFS", db);

    } else if s.contains(r#""cmd":"calibrate_max""#) {
        let db = parse_f32_field(s, r#""db":"#).unwrap_or(last_db);
        let tripwire = db - 3.0;
        let mut eng = engine.lock().await;
        eng.set_tripwire(tripwire);
        info!("[ws] Tripwire set to {} dBFS (TV at {})", tripwire, db);

    } else if s.contains(r#""threshold":"#) {
        if let Some(v) = parse_f32_field(s, r#""threshold":"#) {
            let mut eng = engine.lock().await;
            eng.set_tripwire(v);
            info!("[ws] Manual tripwire: {}", v);
        }

    } else if s.contains(r#""cmd":"scan_wifi""#) {
        info!("[ws] WiFi scan requested");
        let _ = WIFI_CMD_CH.try_send(WifiCmd::Scan);

    } else if s.contains(r#""cmd":"set_wifi""#) {
        let ssid = parse_str_field(s, r#""ssid":"#).unwrap_or("");
        let pass = parse_str_field(s, r#""pass":"#).unwrap_or("");
        info!("[ws] WiFi reconfigure → {}", ssid);

        // Send wifi_reconfiguring event back to client
        let mut evt: heapless::String<128> = heapless::String::new();
        let _ = core::write!(evt, r#"{{"evt":"wifi_reconfiguring","ssid":"{}"}}"#, ssid);
        let n = ws_text_frame(evt.as_bytes(), out_frame);
        let _ = socket.write_all(&out_frame[..n]).await;

        let mut ssid_h: heapless::String<64> = heapless::String::new();
        let _ = ssid_h.push_str(ssid);
        let mut pass_h: heapless::String<64> = heapless::String::new();
        let _ = pass_h.push_str(pass);
        let _ = WIFI_CMD_CH.try_send(WifiCmd::Reconfigure { ssid: ssid_h, pass: pass_h });

    } else if s.contains(r#""cmd":"set_tv""#) {
        let ip    = parse_str_field(s, r#""ip":"#);
        let brand = parse_str_field(s, r#""brand":"#).and_then(TvBrand::parse);
        let psk   = parse_str_field(s, r#""psk":"#);

        // Disconnect case: ip is empty
        if let Some(ip_str) = ip {
            if ip_str.is_empty() {
                // Clear TV config
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
                info!("[ws] TV disconnected");
                return;
            }

            if let Some(brand) = brand {
                let cfg_to_save = {
                    let mut cfg = tv_config.lock().await;
                    cfg.samsung_token.clear();
                    cfg.ip.clear();
                    let _ = cfg.ip.push_str(ip_str);
                    cfg.brand = brand;
                    if let Some(p) = psk {
                        cfg.sony_psk.clear();
                        let _ = cfg.sony_psk.push_str(&p[..p.len().min(8)]);
                    }
                    cfg.clone()
                };
                if WIFI_CMD_CH.try_send(WifiCmd::SaveTvConfig(cfg_to_save)).is_err() {
                    warn!("[ws] Failed to send SaveTvConfig");
                }
                info!("[ws] TV → {} ({:?})", ip_str, brand);
            } else {
                warn!("[ws] set_tv: unknown brand");
            }
        }

    } else if s.contains(r#""cmd":"discover_tvs""#) {
        info!("[ws] TV discovery requested");
        let tvs = crate::tv::discover_tvs(stack).await;
        let json = format_discovered_tvs(&tvs);
        let n = ws_text_frame(json.as_bytes(), out_frame);
        let _ = socket.write_all(&out_frame[..n]).await;

    } else if s.contains(r#""cmd":"ota_check""#) {
        info!("[ws] OTA check requested");
        let result = crate::ota::check_for_update().await;
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
}

// ── Event formatters ────────────────────────────────────────────────────────

fn format_wifi_scan(networks: &[crate::NetworkInfo]) -> heapless::String<1024> {
    let mut s: heapless::String<1024> = heapless::String::new();
    let _ = s.push_str(r#"{"evt":"wifi_scan","networks":["#);
    for (i, net) in networks.iter().enumerate() {
        if i > 0 { let _ = s.push(','); }
        let _ = core::write!(s, r#"{{"ssid":"{}","rssi":{}}}"#, net.ssid.as_str(), net.rssi);
    }
    let _ = s.push_str("]}");
    s
}

fn format_discovered_tvs(tvs: &[crate::tv::DiscoveredTv]) -> heapless::String<1024> {
    let mut s: heapless::String<1024> = heapless::String::new();
    let _ = s.push_str(r#"{"evt":"discovered","tvs":["#);
    for (i, tv) in tvs.iter().enumerate() {
        if i > 0 { let _ = s.push(','); }
        let _ = core::write!(s, r#"{{"ip":"{}","name":"{}","brand":"{}"}}"#,
            tv.ip.as_str(), tv.name.as_str(), tv.brand.as_str());
    }
    let _ = s.push_str("]}");
    s
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn parse_f32_field(s: &str, key: &str) -> Option<f32> {
    let pos  = s.find(key)?;
    let rest = s[pos + key.len()..].trim_start_matches(|c: char| c == ' ');
    let end  = rest.find(|c: char| !c.is_ascii_digit() && c != '-' && c != '.')
                   .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn parse_str_field<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let pos   = s.find(key)?;
    let after = &s[pos + key.len()..];
    let inner = after.trim_start_matches(|c: char| c == ' ').strip_prefix('"')?;
    let end   = inner.find('"')?;
    Some(&inner[..end])
}

// ── WebSocket frame encoder ─────────────────────────────────────────────────

fn ws_text_frame(payload: &[u8], out: &mut [u8]) -> usize {
    let len  = payload.len();
    let hlen = if len < 126 { 2 } else { 4 };
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
