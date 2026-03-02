//! tv.rs — Modular TV volume control
//!
//! Supported brands (runtime-selectable via PWA "TV" tab):
//!   LG WebOS   — ssap:// WebSocket on port 3000           [IMPLEMENTED]
//!   Samsung    — Smart Remote WS on port 8001              [IMPLEMENTED]
//!   Sony       — Bravia REST JSON-RPC on port 80           [IMPLEMENTED]
//!   Roku       — ECP HTTP on port 8060                     [IMPLEMENTED]
//!
//! Brand protocol summary (from research):
//!   LG:      ssap:// WebSocket, pairing popup, absolute volume via getVolume/setVolume
//!   Samsung: WebSocket key-press only (port 8001 plain, 8002 TLS not supported yet).
//!            No absolute volume via WS. Pairing token stored in TvConfig for the session.
//!            Relative restore only (N × KEY_VOLUP). Token persists in-memory; Phase 3 adds flash.
//!   Sony:    HTTP JSON-RPC at /sony/audio port 80. PSK auth via X-Auth-PSK header.
//!            User sets a PIN in TV Settings → Network → IP Control → Pre-Shared Key.
//!            Absolute volume: getVolumeInformation + setAudioVolume v1.2 (volume is a string).
//!   Roku:    ECP HTTP on port 8060. No auth. No absolute volume. POST /keypress/VolumeDown.
//!            Only works on Roku TVs (not Roku sticks). Relative restore only.
//!
//! Restore behaviour:
//!   LG/Sony  — absolute: ramp setVolume from ducked level back to original, RESTORE_STEP_MS apart
//!   Samsung/Roku — relative: N × VolumeUp (duck_steps_taken), RESTORE_STEP_MS apart

use defmt::*;
use embassy_net::{Stack, TcpSocket};
use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel, mutex::Mutex};
use embassy_time::{Duration, Timer, with_timeout};
use cyw43_pio::NetDriver;

use crate::ducking::{DuckCommand, DuckingEngine};

// ── Restore ramp rate ─────────────────────────────────────────────────────────
/// Milliseconds between each volume step when ramping back up.
const RESTORE_STEP_MS: u64 = 400;

/// BASE64("GuardianSensor") — identifies our app to the Samsung TV on pairing.
const SAMSUNG_APP_B64: &str = "R3VhcmRpYW5TZW5zb3I=";

// ── TV brand + config ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, defmt::Format)]
pub enum TvBrand {
    Lg,
    Samsung,
    Sony,
    Roku,
}

impl TvBrand {
    /// LG and Sony expose absolute-volume APIs; Samsung and Roku use key-press steps only.
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
}

#[derive(Clone)]
pub struct TvConfig {
    /// Dotted-decimal IP, e.g. "192.168.1.100". Empty = not yet configured.
    pub ip:            heapless::String<16>,
    pub brand:         TvBrand,
    /// Sony Bravia Pre-Shared Key — user sets this in TV Settings → IP Control.
    /// Sent as X-Auth-PSK header on every request. Typically 4 digits ("1234").
    pub sony_psk:      heapless::String<8>,
    /// Samsung pairing token — received on first WS connect, stored for the session.
    /// Eliminates the "allow connection?" popup on reconnects within the same session.
    /// Phase 3: persist this to flash so it survives reboots.
    pub samsung_token: heapless::String<16>,
}

impl TvConfig {
    pub fn default() -> Self {
        let mut ip = heapless::String::new();
        let _ = ip.push_str(env!("GUARDIAN_TV_IP", ""));
        Self {
            ip,
            brand:         TvBrand::Lg,
            sony_psk:      heapless::String::new(),
            samsung_token: heapless::String::new(),
        }
    }

    pub fn is_configured(&self) -> bool { !self.ip.is_empty() }
}

// ── Duck command channel (ws_task → tv_task) ──────────────────────────────────
static DUCK_CHANNEL: Channel<ThreadModeRawMutex, DuckCommand, 4> = Channel::new();

pub async fn send_duck_command(cmd: DuckCommand) {
    let _ = DUCK_CHANNEL.try_send(cmd);
}

// ── TV task ───────────────────────────────────────────────────────────────────
#[embassy_executor::task]
pub async fn tv_task(
    stack:     &'static Stack<NetDriver<'static>>,
    engine:    &'static Mutex<ThreadModeRawMutex, DuckingEngine>,
    tv_config: &'static Mutex<ThreadModeRawMutex, TvConfig>,
) {
    Timer::after(Duration::from_secs(3)).await;

    let mut rx_buf    = [0u8; 1024];
    let mut tx_buf    = [0u8; 1024];
    let mut out_frame = [0u8; 512];

    let mut active_ip: heapless::String<16> = heapless::String::new();

    loop {
        // ── Read current config ───────────────────────────────────────────────
        let config = {
            let c = tv_config.lock().await;
            c.clone()
        };

        if !config.is_configured() {
            info!("[tv] No TV configured. Waiting…");
            Timer::after(Duration::from_secs(10)).await;
            continue;
        }

        if config.ip != active_ip {
            info!("[tv] TV config changed → {}", config.ip.as_str());
        }

        let tv_port = config.brand.default_port();
        let tv_addr = match parse_ip(config.ip.as_str()) {
            Some(a) => embassy_net::IpEndpoint::new(a, tv_port),
            None => {
                warn!("[tv] Invalid IP: {}", config.ip.as_str());
                Timer::after(Duration::from_secs(30)).await;
                continue;
            }
        };

        info!("[tv] Connecting to {} ({:?}) port {}", config.ip.as_str(), config.brand, tv_port);

        let mut socket = TcpSocket::new(stack, &mut rx_buf, &mut tx_buf);
        socket.set_timeout(Some(Duration::from_secs(15)));

        if let Err(e) = socket.connect(tv_addr).await {
            warn!("[tv] TCP connect failed: {:?} — retry in 15s", e);
            Timer::after(Duration::from_secs(15)).await;
            continue;
        }

        // ── Brand-specific handshake / pairing ────────────────────────────────
        let connected = match config.brand {
            TvBrand::Lg => lg_connect(&mut socket, config.ip.as_str(), tv_port, &mut out_frame).await,

            TvBrand::Samsung => {
                let token = {
                    let c = tv_config.lock().await;
                    c.samsung_token.clone()
                };
                match samsung_connect(&mut socket, config.ip.as_str(), tv_port, &token, &mut out_frame).await {
                    Some(new_token) => {
                        // Store or update the token for this session
                        if !new_token.is_empty() {
                            let mut c = tv_config.lock().await;
                            c.samsung_token = new_token;
                        }
                        true
                    }
                    None => false,
                }
            }

            TvBrand::Sony => {
                // Sony uses stateless HTTP — no persistent connection handshake needed.
                // We just verify the TCP connection is alive by sending a getVolumeInformation.
                // (sony_connect is a no-op; the actual ping is done in the command loop.)
                true
            }

            TvBrand::Roku => true, // ECP needs no handshake
        };

        if !connected {
            warn!("[tv] Handshake failed — retry in 10s");
            Timer::after(Duration::from_secs(10)).await;
            continue;
        }

        active_ip.clear();
        let _ = active_ip.push_str(config.ip.as_str());
        info!("[tv] Ready ({:?})", config.brand);

        // ── Command loop ─────────────────────────────────────────────────────
        'cmd: loop {
            // Reconnect if the user changed the TV config
            let current_ip = {
                let c = tv_config.lock().await;
                c.ip.clone()
            };
            if current_ip != active_ip {
                info!("[tv] Config changed — reconnecting");
                break 'cmd;
            }

            let cmd = DUCK_CHANNEL.receive().await;
            let brand = config.brand;

            match cmd {
                DuckCommand::None => continue,

                DuckCommand::VolumeDown => {
                    // Capture original volume before the first step (absolute-volume brands only)
                    let needs_query = {
                        let eng = engine.lock().await;
                        eng.original_volume.is_none() && brand.supports_absolute_volume()
                    };
                    if needs_query {
                        if let Some(vol) = tv_get_volume(brand, &mut socket, &mut out_frame, &config).await {
                            let mut eng = engine.lock().await;
                            eng.set_original_volume(vol);
                            info!("[tv] Captured original volume: {}", vol);
                        }
                    }

                    let ok = tv_volume_down(brand, &mut socket, &mut out_frame, &config).await;
                    if !ok { warn!("[tv] VolumeDown failed — reconnecting"); break 'cmd; }
                    info!("[tv] Volume ↓");
                }

                DuckCommand::Restore => {
                    let (orig, steps) = {
                        let eng = engine.lock().await;
                        (eng.original_volume, eng.duck_steps_taken)
                    };

                    let ok = if brand.supports_absolute_volume() {
                        if let Some(orig_vol) = orig {
                            let current = orig_vol.saturating_sub(steps);
                            tv_ramp_up_absolute(brand, &mut socket, &mut out_frame, &config, current, orig_vol).await
                        } else {
                            tv_volume_up(brand, &mut socket, &mut out_frame, &config).await
                        }
                    } else {
                        tv_ramp_up_relative(brand, &mut socket, &mut out_frame, &config, steps).await
                    };

                    if !ok { warn!("[tv] Restore failed — reconnecting"); break 'cmd; }
                    info!("[tv] Volume restored");

                    let mut eng = engine.lock().await;
                    eng.clear_duck_state();
                }
            }
        }

        Timer::after(Duration::from_secs(5)).await;
    }
}

// ── Ramp helpers ──────────────────────────────────────────────────────────────

async fn tv_ramp_up_absolute(
    brand: TvBrand, socket: &mut TcpSocket<'_>, out: &mut [u8; 512],
    cfg: &TvConfig, current: u8, target: u8,
) -> bool {
    let steps = target.saturating_sub(current);
    if steps == 0 { return true; }
    for i in 1..=steps {
        let vol = current + i;
        if !tv_set_volume(brand, socket, out, cfg, vol).await { return false; }
        info!("[tv] Ramp → {}", vol);
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

// ── Brand dispatch ─────────────────────────────────────────────────────────────

async fn tv_get_volume(brand: TvBrand, s: &mut TcpSocket<'_>, out: &mut [u8; 512], cfg: &TvConfig) -> Option<u8> {
    match brand {
        TvBrand::Lg      => lg_get_volume(s, out).await,
        TvBrand::Sony    => sony_get_volume(s, out, cfg).await,
        TvBrand::Samsung | TvBrand::Roku => None, // no absolute volume API
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

// ── LG WebOS ──────────────────────────────────────────────────────────────────
// ssap:// WebSocket on port 3000. Pairing popup on TV (once). Absolute volume.

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
    let n = ws_frame_unmasked(LG_PAIR_MSG.as_bytes(), out);
    if socket.write_all(&out[..n]).await.is_err() { return false; }
    info!("[tv/lg] Pairing sent — accept on TV if prompted");
    true
}

async fn lg_get_volume(socket: &mut TcpSocket<'_>, out: &mut [u8; 512]) -> Option<u8> {
    let req = r#"{"type":"request","id":"vol_q","uri":"ssap://audio/getVolume"}"#;
    let n = ws_frame_unmasked(req.as_bytes(), out);
    socket.write_all(&out[..n]).await.ok()?;
    let mut rx = [0u8; 256];
    let len = with_timeout(Duration::from_secs(2), read_ws_frame(socket, &mut rx)).await.ok()??;
    parse_volume_from_json(&rx[..len])
}

async fn lg_volume_down(socket: &mut TcpSocket<'_>, out: &mut [u8; 512]) -> bool {
    let msg = r#"{"type":"request","id":"vol_d","uri":"ssap://audio/volumeDown"}"#;
    let n = ws_frame_unmasked(msg.as_bytes(), out);
    socket.write_all(&out[..n]).await.is_ok()
}

async fn lg_volume_up(socket: &mut TcpSocket<'_>, out: &mut [u8; 512]) -> bool {
    let msg = r#"{"type":"request","id":"vol_u","uri":"ssap://audio/volumeUp"}"#;
    let n = ws_frame_unmasked(msg.as_bytes(), out);
    socket.write_all(&out[..n]).await.is_ok()
}

async fn lg_set_volume(socket: &mut TcpSocket<'_>, out: &mut [u8; 512], vol: u8) -> bool {
    let mut msg: heapless::String<128> = heapless::String::new();
    let _ = core::write!(
        msg,
        r#"{{"type":"request","id":"vol_s","uri":"ssap://audio/setVolume","payload":{{"volume":{}}}}}"#,
        vol
    );
    let n = ws_frame_unmasked(msg.as_bytes(), out);
    socket.write_all(&out[..n]).await.is_ok()
}

// ── Samsung Tizen (port 8001, plain WS) ────────────────────────────────────────
// Key-press remote control only. No absolute volume via WS API.
// First connect: TV shows approval popup. Returns pairing token (stored in TvConfig).
// Subsequent connects within same session: include token in URL, no popup.
// Port 8002 (WSS/TLS) not supported yet — requires embedded-tls (Phase 3).

async fn samsung_connect(
    socket: &mut TcpSocket<'_>,
    host: &str,
    port: u16,
    existing_token: &heapless::String<16>,
    out: &mut [u8; 512],
) -> Option<heapless::String<16>> {
    // Build the WS upgrade path — include token if we have one from a previous connect
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

    // Wait up to 30 seconds for the user to approve on the TV
    // (or immediately if token already accepted)
    info!("[tv/samsung] Waiting for TV pairing event (approve on screen if prompted)…");
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
        warn!("[tv/samsung] Unexpected event: {}", frame);
        return None;
    }

    // Extract token — present as "token":"<digits>" in the response JSON
    let mut token: heapless::String<16> = heapless::String::new();
    if let Some(tok) = parse_json_str(frame, "\"token\":") {
        let _ = token.push_str(tok);
        info!("[tv/samsung] Paired. Token: {}", tok);
    } else {
        info!("[tv/samsung] Connected (no token — older TV)");
    }

    // Signal that the out buffer is available (we used ws_buf for the response)
    let _ = out; // suppress unused warning
    Some(token)
}

async fn samsung_key(socket: &mut TcpSocket<'_>, out: &mut [u8; 512], key: &str) -> bool {
    // Samsung Smart Remote WS key-press payload
    // Cmd: "Click" sends a press+release. Option must be the string "false".
    let mut msg: heapless::String<192> = heapless::String::new();
    let _ = core::write!(
        msg,
        r#"{{"method":"ms.remote.control","params":{{"Cmd":"Click","DataOfCmd":"{}","Option":"false","TypeOfRemote":"SendRemoteKey"}}}}"#,
        key
    );
    let n = ws_frame_unmasked(msg.as_bytes(), out);
    socket.write_all(&out[..n]).await.is_ok()
}

// ── Sony Bravia REST JSON-RPC ─────────────────────────────────────────────────
// Plain HTTP on port 80. Auth: X-Auth-PSK header with user's pre-configured key.
// User sets PSK in TV: Settings → Network → Home Network Setup → IP Control
//   → Authentication → Normal and Pre-Shared Key → set a key (e.g. "1234").
// Absolute volume: getVolumeInformation + setAudioVolume v1.2 (volume as string!).

async fn sony_get_volume(socket: &mut TcpSocket<'_>, out: &mut [u8; 512], cfg: &TvConfig) -> Option<u8> {
    const BODY: &str = r#"{"method":"getVolumeInformation","id":33,"params":[],"version":"1.0"}"#;
    sony_http_post(socket, out, &cfg.ip, &cfg.sony_psk, BODY).await?;

    // Parse response: find "speaker" target, then its "volume" value
    let resp = core::str::from_utf8(out).ok()?;
    let body_start = resp.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
    let body = &resp[body_start..];

    // Body: {"result":[[{"target":"speaker","volume":25,...}]],"id":33}
    let speaker_pos = body.find(r#""speaker""#)?;
    let vol_key = r#""volume":"#;
    let vol_pos = speaker_pos + body[speaker_pos..].find(vol_key)?;
    let rest = &body[vol_pos + vol_key.len()..];
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

async fn sony_set_volume(socket: &mut TcpSocket<'_>, out: &mut [u8; 512], cfg: &TvConfig, vol: u8) -> bool {
    // v1.2 API: volume field must be a JSON string, not a number
    let mut body: heapless::String<128> = heapless::String::new();
    let _ = core::write!(
        body,
        r#"{{"method":"setAudioVolume","id":98,"params":[{{"target":"speaker","volume":"{}"}}],"version":"1.2"}}"#,
        vol
    );
    sony_http_post(socket, out, &cfg.ip, &cfg.sony_psk, body.as_str()).await.is_some()
}

async fn sony_volume_step(socket: &mut TcpSocket<'_>, out: &mut [u8; 512], cfg: &TvConfig, up: bool) -> bool {
    // Sony doesn't have a relative step command — get current and set current±1
    // This is only called in the Samsung/Roku-style relative-ramp path, which
    // shouldn't happen for Sony (supports_absolute_volume = true). But handle it gracefully.
    if let Some(vol) = sony_get_volume(socket, out, cfg).await {
        let new_vol = if up { vol.saturating_add(1).min(100) } else { vol.saturating_sub(1) };
        sony_set_volume(socket, out, cfg, new_vol).await
    } else {
        false
    }
}

/// Send a JSON-RPC request to /sony/audio and read the HTTP response into `out`.
/// Returns Some(&out) on HTTP 200, None on error.
async fn sony_http_post<'b>(
    socket: &mut TcpSocket<'_>,
    out: &'b mut [u8; 512],
    ip: &str,
    psk: &str,
    body: &str,
) -> Option<&'b [u8]> {
    // Send headers separately from body to avoid one large stack allocation
    let mut headers: heapless::String<256> = heapless::String::new();
    let _ = core::write!(
        headers,
        "POST /sony/audio HTTP/1.1\r\n\
         Host: {}\r\n\
         Content-Type: application/json\r\n\
         X-Auth-PSK: {}\r\n\
         Content-Length: {}\r\n\
         Connection: keep-alive\r\n\
         \r\n",
        ip, psk, body.len()
    );
    socket.write_all(headers.as_bytes()).await.ok()?;
    socket.write_all(body.as_bytes()).await.ok()?;

    let n = read_http_response(socket, out).await?;
    // Confirm HTTP 200
    if out[..n.min(12)].starts_with(b"HTTP/1.1 200") { Some(&out[..n]) } else { None }
}

// ── Roku ECP (External Control Protocol) ────────────────────────────────────────
// Plain HTTP POST on port 8060. No auth. No body. No absolute volume.
// Only works on Roku TV (the television sets), not Roku streaming sticks.

async fn roku_key(socket: &mut TcpSocket<'_>, out: &mut [u8; 512], cfg: &TvConfig, key: &str) -> bool {
    let mut req: heapless::String<128> = heapless::String::new();
    let _ = core::write!(
        req,
        "POST /keypress/{} HTTP/1.1\r\nHost: {}:8060\r\nContent-Length: 0\r\n\r\n",
        key, cfg.ip.as_str()
    );
    if socket.write_all(req.as_bytes()).await.is_err() { return false; }
    // Drain the response (HTTP 200, empty body) — ignore errors
    let _ = read_http_response(socket, out).await;
    true
}

// ── Low-level helpers ─────────────────────────────────────────────────────────

/// HTTP Upgrade handshake for WebSocket client connections.
/// `path` is the full request path including query string, e.g. "/api/v2/channels/...?name=..."
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

/// Read an HTTP response (headers + body) into `buf`. Returns bytes read.
async fn read_http_response(socket: &mut TcpSocket<'_>, buf: &mut [u8; 512]) -> Option<usize> {
    let mut len = 0usize;

    // Read until we have complete headers (\r\n\r\n)
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

    // Parse Content-Length and read remaining body bytes if needed
    let header_text = core::str::from_utf8(&buf[..len.min(400)]).unwrap_or("");
    if let Some(cl) = header_text.lines()
        .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
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

/// Read a WebSocket frame into `buf`. Returns payload length.
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

/// Encode a WebSocket text frame (server-side, unmasked).
fn ws_frame_unmasked(payload: &[u8], out: &mut [u8]) -> usize {
    let len = payload.len();
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

fn parse_volume_from_json(json: &[u8]) -> Option<u8> {
    let s = core::str::from_utf8(json).ok()?;
    let pos = s.find("\"volume\":")?;
    let rest = s[pos + 9..].trim_start_matches(|c: char| c == ' ' || c == '\t');
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    if end == 0 { return None; }
    rest[..end].parse().ok()
}

/// Extract a quoted JSON string value: find `key` then the next `"..."`.
fn parse_json_str<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let pos   = s.find(key)?;
    let after = &s[pos + key.len()..];
    let inner = after.trim_start_matches(|c: char| c == ' ').strip_prefix('"')?;
    let end   = inner.find('"')?;
    Some(&inner[..end])
}

fn parse_ip(s: &str) -> Option<embassy_net::IpAddress> {
    let mut p = s.splitn(4, '.');
    let a = p.next()?.parse::<u8>().ok()?;
    let b = p.next()?.parse::<u8>().ok()?;
    let c = p.next()?.parse::<u8>().ok()?;
    let d = p.next()?.parse::<u8>().ok()?;
    Some(embassy_net::IpAddress::v4(a, b, c, d))
}
