//! ota.rs — OTA version check + download pipeline
//!
//! Checks GitHub Releases API over HTTPS for newer PWA versions.
//! Uses reqwless + embedded-tls for TLS on the RP2350's hardware TRNG.

use core::fmt::Write;
use defmt::*;
use embedded_io_async::Read as _;
use embassy_net::Stack;
use embassy_net::dns::DnsSocket;
use embassy_net::tcp::client::{TcpClient, TcpClientState};
use reqwless::client::{HttpClient, TlsConfig, TlsVerify};
use reqwless::request::{Method, RequestBuilder};
use static_cell::StaticCell;

const GH_OWNER: &str = match option_env!("GUARDIAN_GH_OWNER") {
    Some(v) => v,
    None => "gammahazard",
};
const GH_REPO: &str = match option_env!("GUARDIAN_GH_REPO") {
    Some(v) => v,
    None => "sound-sensor",
};

// Static state for the reqwless TCP client (1 concurrent connection, 1 KB buffers)
static OTA_TCP_STATE: StaticCell<TcpClientState<1, 1024, 1024>> = StaticCell::new();

/// One-time init — call from wifi_task after stack is up. Returns a static reference
/// for passing to OTA check/download calls.
pub fn init_tcp_state() -> &'static TcpClientState<1, 1024, 1024> {
    OTA_TCP_STATE.init(TcpClientState::new())
}

// ── Version comparison ────────────────────────────────────────────────────────

pub fn is_newer(local: &str, remote: &str) -> bool {
    fn parse(s: &str) -> (u32, u32, u32) {
        let mut parts = s.trim_start_matches('v').splitn(3, '.');
        let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let minor = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let patch = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        (major, minor, patch)
    }
    parse(remote) > parse(local)
}

// ── GitHub release JSON parser ──────────────────────────────────────────────

pub fn parse_tag_name(json: &str) -> Option<&str> {
    let key = "\"tag_name\":";
    let pos  = json.find(key)?;
    let after = json[pos + key.len()..].trim_start_matches(|c: char| c == ' ');
    let inner = after.strip_prefix('"')?;
    let end   = inner.find('"')?;
    Some(&inner[..end])
}

// ── OTA status message ──────────────────────────────────────────────────────

pub fn status_json(
    checking: bool,
    available: bool,
    current: &str,
    latest: &str,
    done: bool,
) -> heapless::String<256> {
    let mut s: heapless::String<256> = heapless::String::new();
    if done {
        let _ = core::write!(
            s,
            r#"{{"evt":"ota_done","pwa":"{}","fw":"{}"}}"#,
            latest, crate::FW_VERSION,
        );
    } else {
        let _ = core::write!(
            s,
            r#"{{"evt":"ota_status","checking":{},"available":{},"current":"{}","latest":"{}","fw":"{}"}}"#,
            checking, available, current, latest, crate::FW_VERSION,
        );
    }
    s
}

// ── OTA check result ────────────────────────────────────────────────────────

pub struct OtaCheckResult {
    pub available: bool,
    pub current:   heapless::String<16>,
    pub latest:    heapless::String<16>,
}

// ── Live OTA version check via HTTPS ────────────────────────────────────────

pub async fn check_for_update(
    stack: Stack<'static>,
    tcp_state: &'static TcpClientState<1, 1024, 1024>,
    tls_seed: u64,
) -> OtaCheckResult {
    let mut current: heapless::String<16> = heapless::String::new();
    let _ = current.push_str(crate::PWA_VERSION);

    info!("[ota] Checking {}/{} for updates (current PWA={})",
          GH_OWNER, GH_REPO, crate::PWA_VERSION);

    // Build the GitHub API URL
    let mut url: heapless::String<128> = heapless::String::new();
    let _ = core::write!(
        url,
        "https://api.github.com/repos/{}/{}/releases/latest",
        GH_OWNER, GH_REPO
    );

    // TLS buffers (stack-allocated, only live during this function)
    let mut tls_read_buf  = [0u8; 16384];
    let mut tls_write_buf = [0u8; 16384];
    let tls_config = TlsConfig::new(
        tls_seed,
        &mut tls_read_buf,
        &mut tls_write_buf,
        TlsVerify::None, // Public read-only API — skip cert verification for no_std
    );

    let tcp = TcpClient::new(stack, tcp_state);
    let dns = DnsSocket::new(stack);
    let mut client = HttpClient::new_with_tls(&tcp, &dns, tls_config);

    // Response buffer (GitHub releases JSON is typically 2-4 KB)
    let mut rx_buf = [0u8; 4096];

    let result = async {
        let req = client.request(Method::GET, url.as_str()).await.map_err(|_| ())?;
        let mut req_with_headers = req
            .headers(&[
                ("User-Agent", "Guardian/0.3"),
                ("Accept", "application/json"),
            ]);
        let resp = req_with_headers
            .send(&mut rx_buf)
            .await
            .map_err(|_| ())?;

        let status = resp.status.0;
        info!("[ota] GitHub API response: HTTP {}", status);

        if status != 200 {
            warn!("[ota] Non-200 response from GitHub API");
            return Err(());
        }

        // Read the response body
        let body = resp.body().read_to_end().await.map_err(|_| ())?;
        let json = core::str::from_utf8(body).map_err(|_| ())?;

        info!("[ota] Response body: {} bytes", json.len());

        if let Some(tag) = parse_tag_name(json) {
            info!("[ota] Latest release: {}", tag);
            let clean_tag = tag.trim_start_matches('v'); // Strip "v" prefix for display
            let mut latest: heapless::String<16> = heapless::String::new();
            let _ = latest.push_str(&clean_tag[..clean_tag.len().min(16)]);
            let avail = is_newer(crate::PWA_VERSION, tag);
            info!("[ota] Update available: {}", avail);
            return Ok((avail, latest));
        }

        warn!("[ota] Could not parse tag_name from response");
        Err(())
    }
    .await;

    match result {
        Ok((available, latest)) => OtaCheckResult {
            available,
            current,
            latest,
        },
        Err(()) => {
            warn!("[ota] Check failed — returning no update");
            OtaCheckResult {
                available: false,
                current: current.clone(),
                latest: current,
            }
        }
    }
}

// ── Asset URL parser ────────────────────────────────────────────────────────

/// Extract download URLs from a GitHub release JSON.
/// Returns (name, browser_download_url, size) for each .gz asset.
pub fn parse_asset_urls<'a>(json: &'a str) -> heapless::Vec<(&'a str, &'a str, u32), 8> {
    let mut result: heapless::Vec<(&str, &str, u32), 8> = heapless::Vec::new();

    // Find the "assets":[ array
    let assets_key = "\"assets\":[";
    let Some(start) = json.find(assets_key) else { return result; };
    let assets_json = &json[start + assets_key.len()..];

    // Find each top-level asset object, handling nested objects (e.g. "uploader":{...})
    let mut pos = 0;
    while pos < assets_json.len() {
        let Some(obj_start) = assets_json[pos..].find('{') else { break; };
        let abs_start = pos + obj_start;
        // Count braces to find the matching closing brace
        let mut depth = 0i32;
        let mut obj_end = abs_start;
        for (i, &b) in assets_json[abs_start..].as_bytes().iter().enumerate() {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 { obj_end = abs_start + i; break; }
                }
                _ => {}
            }
        }
        if depth != 0 { break; } // malformed
        let obj = &assets_json[abs_start..obj_end + 1];
        pos = obj_end + 1;

        // Only process .gz files
        let name = match parse_str_in_obj(obj, "\"name\":") {
            Some(n) if n.ends_with(".gz") => n,
            _ => continue,
        };
        let url = match parse_str_in_obj(obj, "\"browser_download_url\":") {
            Some(u) => u,
            None => continue,
        };
        let size = parse_u32_in_obj(obj, "\"size\":").unwrap_or(0);

        let _ = result.push((name, url, size));
    }

    result
}

fn parse_str_in_obj<'a>(obj: &'a str, key: &str) -> Option<&'a str> {
    let pos = obj.find(key)?;
    let after = &obj[pos + key.len()..];
    let inner = after.trim_start().strip_prefix('"')?;
    let end = inner.find('"')?;
    Some(&inner[..end])
}

fn parse_u32_in_obj(obj: &str, key: &str) -> Option<u32> {
    let pos = obj.find(key)?;
    let after = &obj[pos + key.len()..].trim_start();
    let end = after.find(|c: char| !c.is_ascii_digit()).unwrap_or(after.len());
    if end == 0 { return None; }
    after[..end].parse().ok()
}

// ── OTA download pipeline ───────────────────────────────────────────────────

use crate::flash_fs::{FlashFs, OtaFileTable, OTA_FILE_OFFSETS};

/// Page-aligned write buffer for flash (RP2350 requires 256-byte page writes).
struct PageWriter {
    buf: [u8; 256],
    buf_len: usize,
    offset: u32,
}

impl PageWriter {
    fn new(offset: u32) -> Self {
        Self { buf: [0u8; 256], buf_len: 0, offset }
    }

    fn feed(&mut self, data: &[u8], fs: &mut FlashFs) -> bool {
        let mut pos = 0;
        while pos < data.len() {
            let space = 256 - self.buf_len;
            let n = space.min(data.len() - pos);
            self.buf[self.buf_len..self.buf_len + n].copy_from_slice(&data[pos..pos + n]);
            self.buf_len += n;
            pos += n;

            if self.buf_len == 256 {
                if !fs.write_chunk(self.offset, &self.buf) {
                    return false;
                }
                self.offset += 256;
                self.buf_len = 0;
            }
        }
        true
    }

    fn flush(&mut self, fs: &mut FlashFs) -> bool {
        if self.buf_len == 0 { return true; }
        // Pad remaining buffer with 0xFF (erased flash state)
        self.buf[self.buf_len..].fill(0xFF);
        if !fs.write_chunk(self.offset, &self.buf) {
            return false;
        }
        self.buf_len = 0;
        true
    }
}

/// Download gzipped PWA assets from the latest GitHub Release and write to flash FS.
/// Returns the new PWA version string on success.
pub async fn download_update(
    stack: Stack<'static>,
    tcp_state: &'static TcpClientState<1, 1024, 1024>,
    tls_seed: u64,
    fs: &mut FlashFs,
) -> Option<heapless::String<16>> {
    info!("[ota] Starting OTA download…");

    // Step 1: Fetch release JSON to get asset URLs
    let mut url: heapless::String<128> = heapless::String::new();
    let _ = core::write!(
        url,
        "https://api.github.com/repos/{}/{}/releases/latest",
        GH_OWNER, GH_REPO
    );

    let mut tls_read_buf  = [0u8; 16384];
    let mut tls_write_buf = [0u8; 16384];
    let tls_config = TlsConfig::new(
        tls_seed,
        &mut tls_read_buf,
        &mut tls_write_buf,
        TlsVerify::None,
    );

    let tcp = TcpClient::new(stack, tcp_state);
    let dns = DnsSocket::new(stack);
    let mut client = HttpClient::new_with_tls(&tcp, &dns, tls_config);

    let mut rx_buf = [0u8; 4096];
    let release_json = {
        let req = client.request(Method::GET, url.as_str()).await.ok()?;
        let mut req_h = req.headers(&[
            ("User-Agent", "Guardian/0.3"),
            ("Accept", "application/json"),
        ]);
        let resp = req_h.send(&mut rx_buf).await.ok()?;
        if resp.status.0 != 200 {
            warn!("[ota] GitHub API returned HTTP {}", resp.status.0);
            return None;
        }
        let body = resp.body().read_to_end().await.ok()?;
        core::str::from_utf8(body).ok()?
    };

    // Step 2: Parse version and asset list
    let tag = parse_tag_name(release_json)?;
    if !is_newer(crate::PWA_VERSION, tag) {
        info!("[ota] Already up to date ({})", tag);
        return None;
    }

    let assets = parse_asset_urls(release_json);
    if assets.is_empty() {
        warn!("[ota] No .gz assets found in release");
        return None;
    }
    info!("[ota] Found {} assets to download", assets.len());

    // Step 3: Clear stale OTA offsets BEFORE erasing (prevents serving garbage on failure)
    {
        let mut table = OTA_FILE_OFFSETS.lock().await;
        *table = OtaFileTable::new();
    }

    // Step 4: Erase flash FS partition
    if !fs.reset_partition() {
        warn!("[ota] Flash erase failed");
        return None;
    }

    // Step 5: Download each asset and write to flash
    let mut asset_idx = 0u64;
    for &(name, download_url, size) in assets.iter() {
        info!("[ota] Downloading {} ({} bytes)", name, size);

        // Allocate space in flash FS (256-byte page aligned)
        let offset = match fs.alloc_file(name, size) {
            Some(o) => o,
            None => {
                warn!("[ota] Flash alloc failed for {}", name);
                return None;
            }
        };

        // New TLS connection per asset (GitHub redirects to different CDN hosts)
        let mut asset_tls_read  = [0u8; 16384];
        let mut asset_tls_write = [0u8; 16384];
        asset_idx += 1;
        let asset_tls = TlsConfig::new(
            tls_seed.wrapping_add(asset_idx),
            &mut asset_tls_read,
            &mut asset_tls_write,
            TlsVerify::None,
        );
        let asset_tcp = TcpClient::new(stack, tcp_state);
        let asset_dns = DnsSocket::new(stack);
        let mut asset_client = HttpClient::new_with_tls(&asset_tcp, &asset_dns, asset_tls);

        let download_ok = async {
            // First request — get the status and detect redirects
            let mut resp_buf = [0u8; 4096];
            let mut redirect_url_buf: heapless::String<256> = heapless::String::new();
            {
                let req = asset_client.request(Method::GET, download_url).await.map_err(|_| ())?;
                let mut req_h = req.headers(&[("User-Agent", "Guardian/0.3")]);
                let resp = req_h.send(&mut resp_buf).await.map_err(|_| ())?;

                let status = resp.status.0;
                if status == 301 || status == 302 {
                    // Parse Location header from response buffer
                    let hdr = core::str::from_utf8(&resp_buf[..resp_buf.len().min(2048)]).unwrap_or("");
                    if let Some(loc) = hdr.lines()
                        .find(|l| l.len() >= 9 && l.as_bytes()[..9].iter()
                            .zip(b"location:").all(|(a, b)| a.to_ascii_lowercase() == *b))
                        .and_then(|l| l.split_once(':').map(|(_, v)| v.trim()))
                    {
                        let _ = redirect_url_buf.push_str(&loc[..loc.len().min(255)]);
                    } else {
                        warn!("[ota] 302 but no Location header for {}", name);
                        return Err(());
                    }
                } else if status == 200 {
                    // Direct 200 — stream body to flash
                    let mut writer = PageWriter::new(offset);
                    let mut body_reader = resp.body().reader();
                    let mut chunk = [0u8; 1024];
                    let mut total = 0u32;
                    loop {
                        match body_reader.read(&mut chunk).await {
                            Ok(0) => break,
                            Ok(n) => {
                                if !writer.feed(&chunk[..n], fs) { return Err(()); }
                                total += n as u32;
                            }
                            Err(_) => { return Err(()); }
                        }
                    }
                    if !writer.flush(fs) { return Err(()); }
                    info!("[ota] Written {} ({} bytes)", name, total);
                    return Ok(());
                } else {
                    warn!("[ota] Asset download returned HTTP {}", status);
                    return Err(());
                }
            }
            // asset_client borrow released here

            if !redirect_url_buf.is_empty() {
                info!("[ota] Following redirect for {}", name);
                let mut redir_tls_r = [0u8; 16384];
                let mut redir_tls_w = [0u8; 16384];
                let redir_tls = TlsConfig::new(
                    tls_seed.wrapping_add(asset_idx + 100),
                    &mut redir_tls_r, &mut redir_tls_w,
                    TlsVerify::None,
                );
                let redir_tcp = TcpClient::new(stack, tcp_state);
                let redir_dns = DnsSocket::new(stack);
                let mut redir_client = HttpClient::new_with_tls(&redir_tcp, &redir_dns, redir_tls);

                let req2 = redir_client.request(Method::GET, redirect_url_buf.as_str()).await.map_err(|_| ())?;
                let mut req2_h = req2.headers(&[("User-Agent", "Guardian/0.3")]);
                let mut resp_buf2 = [0u8; 4096];
                let resp2 = req2_h.send(&mut resp_buf2).await.map_err(|_| ())?;

                if resp2.status.0 != 200 {
                    warn!("[ota] CDN returned HTTP {} for {}", resp2.status.0, name);
                    return Err(());
                }

                let mut writer = PageWriter::new(offset);
                let mut body_reader = resp2.body().reader();
                let mut chunk = [0u8; 1024];
                let mut total = 0u32;
                loop {
                    match body_reader.read(&mut chunk).await {
                        Ok(0) => break,
                        Ok(n) => {
                            if !writer.feed(&chunk[..n], fs) { return Err(()); }
                            total += n as u32;
                        }
                        Err(_) => { return Err(()); }
                    }
                }
                if !writer.flush(fs) { return Err(()); }
                info!("[ota] Written {} ({} bytes via redirect)", name, total);
            }
            Ok(())
        }.await;

        if download_ok.is_err() {
            warn!("[ota] Download failed for {}", name);
            return None;
        }
    }

    // Step 5: Commit the flash FS directory
    if !fs.commit_dir() {
        warn!("[ota] Directory commit failed");
        return None;
    }

    // Step 6: Update OTA file offsets table
    {
        let mut table = OTA_FILE_OFFSETS.lock().await;
        *table = OtaFileTable {
            index_html:    fs.find_in_dir("index.html.gz"),
            guardian_js:   fs.find_in_dir("guardian-pwa.js.gz"),
            guardian_wasm: fs.find_in_dir("guardian-pwa_bg.wasm.gz"),
            sw_js:         fs.find_in_dir("sw.js.gz"),
            manifest_json: fs.find_in_dir("manifest.json.gz"),
            version_json:  (0, 0),
        };
    }

    let clean = tag.trim_start_matches('v');
    let mut version: heapless::String<16> = heapless::String::new();
    let _ = version.push_str(&clean[..clean.len().min(16)]);
    info!("[ota] OTA download complete! New version: {}", version.as_str());
    Some(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer() {
        assert!( is_newer("0.1.0", "0.2.0"));
        assert!( is_newer("0.1.0", "v0.2.0"));
        assert!(!is_newer("0.2.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!( is_newer("0.1.9", "0.2.0"));
        assert!( is_newer("1.0.0", "2.0.0"));
    }

    #[test]
    fn test_parse_tag_name() {
        let json = r#"{"tag_name":"v0.2.0","name":"Release 0.2.0"}"#;
        assert_eq!(parse_tag_name(json), Some("v0.2.0"));
    }
}
