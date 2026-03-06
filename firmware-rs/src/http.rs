//! http.rs — HTTP/1.1 server on port 80
//!
//! Serves the Guardian PWA. Checks flash FS for OTA-updated assets first,
//! falls back to pwa_assets (baked into firmware) if not found.
//! POST /api/ota returns version info.

use core::fmt::Write;
use defmt::*;
use embedded_io_async::Write as _;
use embassy_net::Stack;
use embassy_net::tcp::TcpSocket;
use embassy_time::{Duration, Timer};

use portable_atomic::Ordering;

use crate::pwa_assets as assets;
use crate::flash_fs::{OTA_FILE_OFFSETS, flash_read_chunk};

const TCP_PORT: u16   = 80;
const RX_BUF:   usize = 512;
const TX_BUF:   usize = 2048;

#[embassy_executor::task]
pub async fn http_task(stack: Stack<'static>) {
    let mut rx_buf = [0u8; RX_BUF];
    let mut tx_buf = [0u8; TX_BUF];

    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buf, &mut tx_buf);
        socket.set_timeout(Some(Duration::from_secs(10)));

        info!("[http] Waiting for connection on port {}", TCP_PORT);
        if let Err(_e) = socket.accept(TCP_PORT).await {
            warn!("[http] Accept error");
            Timer::after(Duration::from_millis(50)).await;
            continue;
        }

        // Keep-alive loop: serve multiple requests per TCP connection
        loop {
            socket.set_timeout(Some(Duration::from_secs(5))); // idle timeout
            let keep = handle_one_request(&mut socket).await;
            if !keep { break; }
        }
    }
}

/// Handle one HTTP request on an existing connection.
/// Returns `true` to keep the connection alive, `false` to close it.
async fn handle_one_request(socket: &mut TcpSocket<'_>) -> bool {
    // 512 bytes — iOS Safari sends ~350 bytes of headers (User-Agent alone is 100+)
    let mut buf = [0u8; 512];
    let len = match read_until_double_crlf(socket, &mut buf).await {
        Some(n) => n,
        None => return false, // client closed or timeout — drop connection
    };

    let request = core::str::from_utf8(&buf[..len]).unwrap_or("");
    let first_line = request.lines().next().unwrap_or("");

    let mut parts = first_line.splitn(3, ' ');
    let method = parts.next().unwrap_or("");
    let path   = parts.next().unwrap_or("/");

    info!("[http] {} {}", method, path);

    // Extend timeout for response transfer
    socket.set_timeout(Some(Duration::from_secs(30)));

    // In AP mode: captive portal — serve setup page for ALL GET requests.
    // iOS probes captive.apple.com/hotspot-detect.html expecting "Success" text.
    // Android probes connectivitycheck.gstatic.com/generate_204 expecting 204.
    // Our DNS resolves everything to 192.168.4.1, so these probes hit us.
    // By returning our setup page (not "Success" / not 204), the OS detects
    // "captive portal" and auto-opens the CNA sheet with our page inside it.
    if crate::AP_MODE.load(Ordering::Relaxed) {
        if method == "GET" {
            serve_response(socket, "text/html; charset=utf-8", ServeBody::Bytes(crate::setup_html::SETUP_PAGE), false).await;
        } else {
            send_error(socket, 404, "Not Found").await;
        }
        return false; // AP mode: close after each request (captive portal compat)
    }

    // Snapshot the OTA file table (non-blocking try_lock to avoid contention)
    let ota = {
        let table = OTA_FILE_OFFSETS.try_lock();
        match table {
            Ok(t) => *t,
            Err(_) => crate::flash_fs::OtaFileTable::new(), // fallback: no OTA
        }
    };

    match (method, path) {
        ("GET", "/" | "/index.html") => {
            if ota.index_html.1 > 0 {
                serve_response(socket, "text/html; charset=utf-8", ServeBody::Flash(ota.index_html.0, ota.index_html.1), true).await;
            } else {
                serve_response(socket, "text/html; charset=utf-8", ServeBody::Bytes(assets::INDEX_HTML), false).await;
            }
        }
        ("GET", "/guardian-pwa.js") => {
            if ota.guardian_js.1 > 0 {
                serve_response(socket, "application/javascript", ServeBody::Flash(ota.guardian_js.0, ota.guardian_js.1), true).await;
            } else {
                serve_response(socket, "application/javascript", ServeBody::Bytes(assets::WASM_JS_GZ), true).await;
            }
        }
        ("GET", "/guardian-pwa_bg.wasm") => {
            if ota.guardian_wasm.1 > 0 {
                serve_response(socket, "application/wasm", ServeBody::Flash(ota.guardian_wasm.0, ota.guardian_wasm.1), true).await;
            } else {
                serve_response(socket, "application/wasm", ServeBody::Bytes(assets::WASM_BG_GZ), true).await;
            }
        }
        ("GET", "/sw.js") => {
            if ota.sw_js.1 > 0 {
                serve_response(socket, "application/javascript", ServeBody::Flash(ota.sw_js.0, ota.sw_js.1), true).await;
            } else {
                serve_response(socket, "application/javascript", ServeBody::Bytes(assets::SW_JS), false).await;
            }
        }
        ("GET", "/manifest.json") => {
            if ota.manifest_json.1 > 0 {
                serve_response(socket, "application/manifest+json", ServeBody::Flash(ota.manifest_json.0, ota.manifest_json.1), true).await;
            } else {
                serve_response(socket, "application/manifest+json", ServeBody::Bytes(assets::MANIFEST), false).await;
            }
        }
        ("GET", "/icon-192.png") => {
            serve_response(socket, "image/png", ServeBody::Bytes(assets::ICON_192), false).await;
        }
        ("GET", "/icon-512.png") => {
            serve_response(socket, "image/png", ServeBody::Bytes(assets::ICON_512), false).await;
        }
        ("GET", "/version.json") => {
            let mut body: heapless::String<128> = heapless::String::new();
            let _ = core::write!(
                body,
                r#"{{"fw":"{}","pwa":"{}"}}"#,
                crate::FW_VERSION,
                crate::PWA_VERSION,
            );
            serve_response(socket, "application/json", ServeBody::Bytes(body.as_bytes()), false).await;
        }
        ("POST", "/api/ota") => {
            handle_ota(socket).await;
        }
        _ => {
            send_error(socket, 404, "Not Found").await;
            return false;
        }
    }

    true // keep connection alive for next request
}

// ── Unified response body types ──────────────────────────────────────────────

enum ServeBody<'a> {
    Bytes(&'a [u8]),
    Flash(u32, u32), // (offset, size)
}

async fn serve_response(socket: &mut TcpSocket<'_>, content_type: &str, body: ServeBody<'_>, gzip: bool) {
    let content_len = match &body {
        ServeBody::Bytes(b) => b.len() as u32,
        ServeBody::Flash(_, size) => *size,
    };

    let mut headers: heapless::String<256> = heapless::String::new();
    let _ = core::write!(
        headers,
        "HTTP/1.1 200 OK\r\n\
         Content-Type: {}\r\n\
         Content-Length: {}\r\n",
        content_type, content_len,
    );
    if gzip {
        let _ = headers.push_str("Content-Encoding: gzip\r\n");
    }
    let _ = headers.push_str("Cache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n");

    if socket.write_all(headers.as_bytes()).await.is_err() { return; }

    match body {
        ServeBody::Bytes(data) => {
            let mut pos = 0;
            while pos < data.len() {
                let end = (pos + 1024).min(data.len());
                if socket.write_all(&data[pos..end]).await.is_err() { return; }
                pos = end;
            }
        }
        ServeBody::Flash(offset, size) => {
            let mut chunk = [0u8; 1024];
            let mut pos = 0u32;
            while pos < size {
                let n = ((size - pos) as usize).min(1024);
                if !flash_read_chunk(offset + pos, &mut chunk[..n]) { return; }
                if socket.write_all(&chunk[..n]).await.is_err() { return; }
                pos += n as u32;
            }
        }
    }

    let _ = socket.flush().await;
}

async fn send_error(socket: &mut TcpSocket<'_>, code: u16, msg: &str) {
    let mut resp: heapless::String<128> = heapless::String::new();
    let _ = core::write!(
        resp,
        "HTTP/1.1 {} {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        code, msg,
    );
    let _ = socket.write_all(resp.as_bytes()).await;
    let _ = socket.flush().await;
}

// ── OTA endpoint ────────────────────────────────────────────────────────────

async fn handle_ota(socket: &mut TcpSocket<'_>) {
    let mut current: heapless::String<16> = heapless::String::new();
    let _ = current.push_str(crate::PWA_VERSION);
    let json = crate::ota::status_json(
        false,
        false,
        current.as_str(),
        current.as_str(),
        false,
    );
    serve_response(socket, "application/json", ServeBody::Bytes(json.as_bytes()), false).await;
}

// ── Helpers ─────────────────────────────────────────────────────────────────

async fn read_until_double_crlf(socket: &mut TcpSocket<'_>, buf: &mut [u8]) -> Option<usize> {
    let mut len = 0;
    loop {
        match socket.read(&mut buf[len..]).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => {
                len += n;
                if buf[..len].windows(4).any(|w| w == b"\r\n\r\n") {
                    return Some(len);
                }
                if len >= buf.len() {
                    // Buffer full without finding \r\n\r\n — reject oversized headers
                    return None;
                }
            }
        }
    }
}
