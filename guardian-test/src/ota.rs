//! ota.rs — OTA version comparison and JSON parsing
//! Extracted from firmware ota.rs

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

pub fn parse_tag_name(json: &str) -> Option<&str> {
    let key = "\"tag_name\":";
    let pos = json.find(key)?;
    let after = json[pos + key.len()..].trim_start_matches(|c: char| c == ' ');
    let inner = after.strip_prefix('"')?;
    let end = inner.find('"')?;
    Some(&inner[..end])
}

/// Build OTA status JSON. `fw_version` is passed as parameter (firmware uses crate::FW_VERSION).
pub fn status_json(
    checking: bool,
    available: bool,
    current: &str,
    latest: &str,
    done: bool,
    fw_version: &str,
) -> heapless::String<256> {
    let mut s: heapless::String<256> = heapless::String::new();
    if done {
        let _ = core::fmt::Write::write_fmt(
            &mut s,
            format_args!(
                r#"{{"evt":"ota_done","pwa":"{}","fw":"{}"}}"#,
                latest, fw_version,
            ),
        );
    } else {
        let _ = core::fmt::Write::write_fmt(
            &mut s,
            format_args!(
                r#"{{"evt":"ota_status","checking":{},"available":{},"current":"{}","latest":"{}","fw":"{}"}}"#,
                checking, available, current, latest, fw_version,
            ),
        );
    }
    s
}
