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

        handle_request(&mut socket).await;
    }
}

async fn handle_request(socket: &mut TcpSocket<'_>) {
    // 512 bytes — iOS Safari sends ~350 bytes of headers (User-Agent alone is 100+)
    let mut buf = [0u8; 512];
    let len = match read_until_double_crlf(socket, &mut buf).await {
        Some(n) => n,
        None => {
            let _ = send_error(socket, 400, "Bad Request").await;
            return;
        }
    };

    let request = core::str::from_utf8(&buf[..len]).unwrap_or("");
    let first_line = request.lines().next().unwrap_or("");

    let mut parts = first_line.splitn(3, ' ');
    let method = parts.next().unwrap_or("");
    let path   = parts.next().unwrap_or("/");

    info!("[http] {} {}", method, path);

    // In AP mode: captive portal — serve setup page for ALL GET requests.
    // iOS probes captive.apple.com/hotspot-detect.html expecting "Success" text.
    // Android probes connectivitycheck.gstatic.com/generate_204 expecting 204.
    // Our DNS resolves everything to 192.168.4.1, so these probes hit us.
    // By returning our setup page (not "Success" / not 204), the OS detects
    // "captive portal" and auto-opens the CNA sheet with our page inside it.
    if crate::AP_MODE.load(Ordering::Relaxed) {
        if method == "GET" {
            serve_asset(socket, crate::setup_html::SETUP_PAGE, "text/html; charset=utf-8").await;
        } else {
            send_error(socket, 404, "Not Found").await;
        }
        return;
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
                serve_flash_gz(socket, ota.index_html, "text/html; charset=utf-8").await;
            } else {
                serve_asset(socket, assets::INDEX_HTML, "text/html; charset=utf-8").await;
            }
        }
        ("GET", "/guardian-pwa.js") => {
            if ota.guardian_js.1 > 0 {
                serve_flash_gz(socket, ota.guardian_js, "application/javascript").await;
            } else {
                serve_asset_gz(socket, assets::WASM_JS_GZ, "application/javascript").await;
            }
        }
        ("GET", "/guardian-pwa_bg.wasm") => {
            if ota.guardian_wasm.1 > 0 {
                serve_flash_gz(socket, ota.guardian_wasm, "application/wasm").await;
            } else {
                serve_asset_gz(socket, assets::WASM_BG_GZ, "application/wasm").await;
            }
        }
        ("GET", "/sw.js") => {
            if ota.sw_js.1 > 0 {
                serve_flash_gz(socket, ota.sw_js, "application/javascript").await;
            } else {
                serve_asset(socket, assets::SW_JS, "application/javascript").await;
            }
        }
        ("GET", "/manifest.json") => {
            if ota.manifest_json.1 > 0 {
                serve_flash_gz(socket, ota.manifest_json, "application/manifest+json").await;
            } else {
                serve_asset(socket, assets::MANIFEST, "application/manifest+json").await;
            }
        }
        ("GET", "/icon-192.png") => {
            serve_asset(socket, assets::ICON_192, "image/png").await;
        }
        ("GET", "/icon-512.png") => {
            serve_asset(socket, assets::ICON_512, "image/png").await;
        }
        ("GET", "/version.json") => {
            let mut body: heapless::String<128> = heapless::String::new();
            let _ = core::write!(
                body,
                r#"{{"fw":"{}","pwa":"{}"}}"#,
                crate::FW_VERSION,
                crate::PWA_VERSION,
            );
            serve_dynamic(socket, body.as_bytes(), "application/json").await;
        }
        ("POST", "/api/ota") => {
            handle_ota(socket).await;
        }
        _ => {
            send_error(socket, 404, "Not Found").await;
        }
    }
}

// ── Asset serving (baked-in) ─────────────────────────────────────────────────

async fn serve_asset(socket: &mut TcpSocket<'_>, body: &[u8], content_type: &str) {
    let mut headers: heapless::String<256> = heapless::String::new();
    let _ = core::write!(
        headers,
        "HTTP/1.1 200 OK\r\n\
         Content-Type: {}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-cache\r\n\
         Connection: close\r\n\
         \r\n",
        content_type,
        body.len(),
    );

    if socket.write_all(headers.as_bytes()).await.is_err() { return; }

    let mut pos = 0;
    while pos < body.len() {
        let end = (pos + 1024).min(body.len());
        if socket.write_all(&body[pos..end]).await.is_err() { return; }
        pos = end;
    }

    let _ = socket.flush().await;
}

// ── Asset serving (baked-in, pre-gzipped) ───────────────────────────────────

async fn serve_asset_gz(socket: &mut TcpSocket<'_>, body: &[u8], content_type: &str) {
    let mut headers: heapless::String<256> = heapless::String::new();
    let _ = core::write!(
        headers,
        "HTTP/1.1 200 OK\r\n\
         Content-Type: {}\r\n\
         Content-Length: {}\r\n\
         Content-Encoding: gzip\r\n\
         Cache-Control: no-cache\r\n\
         Connection: close\r\n\
         \r\n",
        content_type,
        body.len(),
    );

    if socket.write_all(headers.as_bytes()).await.is_err() { return; }

    let mut pos = 0;
    while pos < body.len() {
        let end = (pos + 1024).min(body.len());
        if socket.write_all(&body[pos..end]).await.is_err() { return; }
        pos = end;
    }

    let _ = socket.flush().await;
}

// ── Asset serving (flash FS, gzip-compressed) ────────────────────────────────

async fn serve_flash_gz(
    socket: &mut TcpSocket<'_>,
    (offset, size): (u32, u32),
    content_type: &str,
) {
    let mut headers: heapless::String<256> = heapless::String::new();
    let _ = core::write!(
        headers,
        "HTTP/1.1 200 OK\r\n\
         Content-Type: {}\r\n\
         Content-Length: {}\r\n\
         Content-Encoding: gzip\r\n\
         Cache-Control: no-cache\r\n\
         Connection: close\r\n\
         \r\n",
        content_type,
        size,
    );

    if socket.write_all(headers.as_bytes()).await.is_err() { return; }

    // Stream from flash in 1 KB chunks via XIP memory-mapped read
    let mut chunk = [0u8; 1024];
    let mut pos = 0u32;
    while pos < size {
        let n = ((size - pos) as usize).min(1024);
        if !flash_read_chunk(offset + pos, &mut chunk[..n]) { return; }
        if socket.write_all(&chunk[..n]).await.is_err() { return; }
        pos += n as u32;
    }

    let _ = socket.flush().await;
}

async fn serve_dynamic(socket: &mut TcpSocket<'_>, body: &[u8], content_type: &str) {
    let mut headers: heapless::String<256> = heapless::String::new();
    let _ = core::write!(
        headers,
        "HTTP/1.1 200 OK\r\n\
         Content-Type: {}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-cache\r\n\
         Connection: close\r\n\
         \r\n",
        content_type,
        body.len(),
    );

    if socket.write_all(headers.as_bytes()).await.is_err() { return; }
    let _ = socket.write_all(body).await;
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

    let mut resp: heapless::String<512> = heapless::String::new();
    let _ = core::write!(
        resp,
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        json.len(),
    );

    if socket.write_all(resp.as_bytes()).await.is_ok() {
        let _ = socket.write_all(json.as_bytes()).await;
    }
    let _ = socket.flush().await;
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
