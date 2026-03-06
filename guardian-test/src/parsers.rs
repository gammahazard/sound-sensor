//! parsers.rs — JSON/string parsers extracted from firmware ws.rs and tv.rs

pub fn parse_f32_field(s: &str, key: &str) -> Option<f32> {
    let mut search_from = 0;
    let pos = loop {
        let p = s[search_from..].find(key).map(|i| i + search_from)?;
        if p == 0 || matches!(s.as_bytes()[p - 1], b'{' | b',' | b' ' | b'\n' | b'\t') {
            break p;
        }
        search_from = p + 1;
    };
    let rest = s[pos + key.len()..].trim_start_matches(|c: char| c == ' ');
    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '-' && c != '.')
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

pub fn parse_str_field<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let mut search_from = 0;
    let pos = loop {
        let p = s[search_from..].find(key).map(|i| i + search_from)?;
        if p == 0 || matches!(s.as_bytes()[p - 1], b'{' | b',' | b' ' | b'\n' | b'\t') {
            break p;
        }
        search_from = p + 1;
    };
    let after = &s[pos + key.len()..];
    let inner = after
        .trim_start_matches(|c: char| c == ' ')
        .strip_prefix('"')?;
    // Find closing quote, skipping escaped quotes (\")
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
    if end >= bytes.len() {
        return None;
    }
    Some(&inner[..end])
}

pub fn parse_json_str<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let pos = s.find(key)?;
    let after = &s[pos + key.len()..];
    let inner = after
        .trim_start_matches(|c: char| c == ' ')
        .strip_prefix('"')?;
    let end = inner.find('"')?;
    Some(&inner[..end])
}

pub fn parse_volume_from_json(json: &[u8]) -> Option<u8> {
    let s = core::str::from_utf8(json).ok()?;
    let pos = s.find("\"volume\":")?;
    let rest = s[pos + 9..].trim_start_matches(|c: char| c == ' ' || c == '\t');
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    rest[..end].parse().ok()
}

pub fn extract_ssdp_field<'a>(resp: &'a str, key: &str) -> Option<&'a str> {
    let line = resp.lines().find(|l| {
        if l.len() < key.len() {
            return false;
        }
        l.as_bytes()[..key.len()]
            .iter()
            .zip(key.as_bytes())
            .all(|(a, b)| a.to_ascii_lowercase() == b.to_ascii_lowercase())
    })?;
    let pos = line.find(':')?;
    Some(line[pos + 1..].trim())
}

/// Unescape JSON string escape sequences: `\"` → `"` and `\\` → `\`.
pub fn json_unescape(s: &str) -> heapless::String<64> {
    let mut out: heapless::String<64> = heapless::String::new();
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

/// Escape `"` and `\` for safe embedding inside a JSON string.
pub fn push_json_escaped<const N: usize>(out: &mut heapless::String<N>, s: &str) {
    for &b in s.as_bytes() {
        match b {
            b'"'  => { let _ = out.push_str("\\\""); }
            b'\\' => { let _ = out.push_str("\\\\"); }
            _ => { let _ = out.push(b as char); }
        }
    }
}


/// Parse an IPv4 address string into 4 octets.
pub fn parse_ip(s: &str) -> Option<[u8; 4]> {
    let mut p = s.splitn(4, '.');
    let a = p.next()?.parse::<u8>().ok()?;
    let b = p.next()?.parse::<u8>().ok()?;
    let c = p.next()?.parse::<u8>().ok()?;
    let d = p.next()?.parse::<u8>().ok()?;
    Some([a, b, c, d])
}
