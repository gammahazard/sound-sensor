//! ota.rs — OTA version check scaffold
//!
//! Provides version comparison, GitHub JSON parsing, and status event building.
//! Full TLS download deferred until TRNG is wired (Phase 4).

use defmt::*;

const GH_OWNER: &str = option_env!("GUARDIAN_GH_OWNER").unwrap_or("gammahazard");
const GH_REPO:  &str = option_env!("GUARDIAN_GH_REPO").unwrap_or("sound-sensor");

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

// ── OTA check scaffold ──────────────────────────────────────────────────────

pub struct OtaCheckResult {
    pub available: bool,
    pub current:   heapless::String<16>,
    pub latest:    heapless::String<16>,
}

pub async fn check_for_update() -> OtaCheckResult {
    let mut current: heapless::String<16> = heapless::String::new();
    let _ = current.push_str(crate::PWA_VERSION);

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
