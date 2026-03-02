//! http.rs — HTTP/1.1 server on port 80
//!
//! Serves the Guardian PWA. Falls back to pwa_assets (baked into firmware).
//! POST /api/ota triggers OTA check.

use defmt::*;
use embassy_net::{Stack, TcpSocket};
use embassy_time::{Duration, Timer};
use cyw43_pio::NetDriver;

use crate::pwa_assets as assets;

const TCP_PORT: u16   = 80;
const RX_BUF:   usize = 512;
const TX_BUF:   usize = 2048;

#[embassy_executor::task]
pub async fn http_task(stack: &'static Stack<NetDriver<'static>>) {
    let mut rx_buf = [0u8; RX_BUF];
    let mut tx_buf = [0u8; TX_BUF];

    loop {
        let mut socket = TcpSocket::new(stack, &mut rx_buf, &mut tx_buf);
        socket.set_timeout(Some(Duration::from_secs(10)));

        info!("[http] Waiting for connection on port {}", TCP_PORT);
        if let Err(e) = socket.accept(TCP_PORT).await {
            warn!("[http] Accept error: {:?}", e);
            Timer::after(Duration::from_millis(50)).await;
            continue;
        }

        handle_request(&mut socket).await;
    }
}

async fn handle_request(socket: &mut TcpSocket<'_>) {
    let mut buf = [0u8; 256];
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

    match (method, path) {
        ("GET",  "/" | "/index.html") => {
            serve_asset(socket, assets::INDEX_HTML, "text/html; charset=utf-8").await;
        }
        ("GET",  "/guardian_pwa.js") => {
            serve_asset(socket, assets::WASM_JS, "application/javascript").await;
        }
        ("GET",  "/guardian_pwa_bg.wasm") => {
            serve_asset(socket, assets::WASM_BG, "application/wasm").await;
        }
        ("GET",  "/sw.js") => {
            serve_asset(socket, assets::SW_JS, "application/javascript").await;
        }
        ("GET",  "/manifest.json") => {
            serve_asset(socket, assets::MANIFEST, "application/manifest+json").await;
        }
        ("GET",  "/icon-192.png") => {
            serve_asset(socket, assets::ICON_192, "image/png").await;
        }
        ("GET",  "/icon-512.png") => {
            serve_asset(socket, assets::ICON_512, "image/png").await;
        }
        ("GET",  "/version.json") => {
            // Build version.json dynamically
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

// ── Asset serving ───────────────────────────────────────────────────────────

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
}

// ── OTA endpoint ────────────────────────────────────────────────────────────

async fn handle_ota(socket: &mut TcpSocket<'_>) {
    let result = crate::ota::check_for_update().await;
    let json = crate::ota::status_json(
        false,
        result.available,
        result.current.as_str(),
        result.latest.as_str(),
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
                    return Some(len);
                }
            }
        }
    }
}
