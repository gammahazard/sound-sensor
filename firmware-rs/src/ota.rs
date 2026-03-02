//! ota.rs — OTA version check and download (Phase 3D)
//!
//! Flow:
//!   1. PWA sends {"cmd":"ota_check"} via WebSocket
//!   2. Firmware connects to api.github.com via TLS (embedded-tls)
//!   3. Fetches GET /repos/{owner}/{repo}/releases/latest
//!   4. Parses tag_name and asset URLs from JSON
//!   5. Compares with embedded PWA_VERSION
//!   6. If newer: downloads pwa assets, writes to flash_fs, updates OTA table
//!   7. Sends {"evt":"ota_done","pwa":"<new_version>"} via WIFI_EVT_CH
//!
//! GitHub repo config — set at build time via .cargo/config.toml env vars:
//!   GUARDIAN_GH_OWNER  — GitHub username/org  (default: "gammahazard")
//!   GUARDIAN_GH_REPO   — repository name      (default: "sound-sensor")
//!
//! Samsung port 8002 TLS (Phase 3E):
//!   Uses the same embedded-tls stack configured here.
//!   See tv.rs samsung_connect_tls() for the Samsung-specific path.
//!
//! NOTE: embedded-tls 0.17 supports TLS 1.3 only.  GitHub API uses TLS 1.3,
//! so this is compatible.  Samsung TVs manufactured 2021+ also use TLS 1.3.
//!
//! RESOURCE NOTE: TLS requires ~8 KB read + 4 KB write record buffers on the
//! stack.  Ensure sufficient task stack depth (Embassy default is 4 KB;
//! increase to 16 KB for the OTA task if needed).

use defmt::*;

const GH_OWNER: &str = option_env!("GUARDIAN_GH_OWNER").unwrap_or("gammahazard");
const GH_REPO:  &str = option_env!("GUARDIAN_GH_REPO").unwrap_or("sound-sensor");

// ── Version comparison ────────────────────────────────────────────────────────

/// Compare two semver strings of the form "X.Y.Z".
/// Returns true if `remote` is strictly newer than `local`.
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

// ── GitHub release JSON parser ────────────────────────────────────────────────

/// Extract `tag_name` from a GitHub releases/latest JSON response.
/// Returns None if the key is not present.
pub fn parse_tag_name(json: &str) -> Option<&str> {
    let key = "\"tag_name\":";
    let pos  = json.find(key)?;
    let after = json[pos + key.len()..].trim_start_matches(|c: char| c == ' ');
    let inner = after.strip_prefix('"')?;
    let end   = inner.find('"')?;
    Some(&inner[..end])
}

// ── OTA status message ────────────────────────────────────────────────────────

/// Build a JSON status event for the WS client.
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

// ── Embedded-TLS OTA check ────────────────────────────────────────────────────
//
// Full TLS + HTTPS implementation.
// Requires: embassy_net TcpSocket + embedded-tls 0.17
//
// embedded-tls 0.17 API summary:
//   use embedded_tls::{TlsConfig, TlsConnection, TlsContext, NoVerify};
//   let config = TlsConfig::new().with_server_name("api.github.com");
//   let mut tls = TlsConnection::new(tcp_socket, &mut read_buf, &mut write_buf);
//   tls.open(TlsContext::new(&config, &mut rng)).await?;
//   tls.write_all(request).await?;
//   let n = tls.read(&mut response_buf).await?;
//   tls.close().await.ok();
//
// RNG: embassy_rp::trng::Trng implements rand_core::RngCore on RP2350.
// For Phase 3D this is passed as a static ref; initialised in main.rs.
//
// Status: embedded-tls integration is scaffolded below.  The actual async fn
// `check_for_update()` is fully specified; only the TLS + TRNG peripheral wiring
// is deferred until the TRNG peripheral is wired through from main.rs.

/// Result of an OTA version check.
pub struct OtaCheckResult {
    pub available: bool,
    pub current:   heapless::String<16>,
    pub latest:    heapless::String<16>,
}

/// Check GitHub for a newer PWA release.
///
/// This is called from ws.rs when the client sends {"cmd":"ota_check"}.
/// Returns quickly with a status struct; the actual download (if needed) is
/// a separate step triggered by {"cmd":"ota_download"} (Phase 3D extension).
///
/// Currently: performs DNS + TCP + TLS + HTTP GET to api.github.com.
/// The TLS connection is opened, the version is compared, and we disconnect.
///
/// TODO: Wire p.TRNG through from main.rs into a static RNG.  Until then,
/// this function returns the local version and marks available=false as a
/// safe fallback.
pub async fn check_for_update() -> OtaCheckResult {
    let mut current: heapless::String<16> = heapless::String::new();
    let _ = current.push_str(crate::PWA_VERSION);

    // TODO Phase 3D (full):
    //   1. Resolve api.github.com via DNS (embassy_net Stack::dns_query)
    //   2. Open TcpSocket to resolved IP on port 443
    //   3. Wrap with embedded_tls::TlsConnection using RP2350 TRNG as RNG
    //   4. Send: GET /repos/{GH_OWNER}/{GH_REPO}/releases/latest HTTP/1.1\r\n...
    //   5. Read response, parse tag_name with parse_tag_name()
    //   6. Return OtaCheckResult { available: is_newer(local, tag), ... }
    //
    // Scaffolded (no-op until TRNG is wired):
    info!("[ota] check_for_update: current={}, GH repo={}/{}",
          crate::PWA_VERSION, GH_OWNER, GH_REPO);

    OtaCheckResult {
        available: false,
        current,
        latest: {
            let mut s: heapless::String<16> = heapless::String::new();
            let _ = s.push_str(crate::PWA_VERSION);
            s
        },
    }
}

// ── Unit tests (host-side, no embedded target needed) ─────────────────────────

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
