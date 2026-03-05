use guardian_test::parsers::*;

// ── parse_f32_field ──────────────────────────────────────────────────────────

#[test]
fn parse_f32_positive() {
    let s = r#"{"db":42.5,"armed":true}"#;
    assert_eq!(parse_f32_field(s, r#""db":"#), Some(42.5));
}

#[test]
fn parse_f32_negative() {
    let s = r#"{"db":-32.5}"#;
    assert_eq!(parse_f32_field(s, r#""db":"#), Some(-32.5));
}

#[test]
fn parse_f32_missing_key() {
    let s = r#"{"armed":true}"#;
    assert_eq!(parse_f32_field(s, r#""db":"#), None);
}

#[test]
fn parse_f32_no_digits() {
    let s = r#"{"db":abc}"#;
    assert_eq!(parse_f32_field(s, r#""db":"#), None);
}

#[test]
fn parse_f32_trailing_chars() {
    let s = r#"{"db":12.3}"#;
    assert_eq!(parse_f32_field(s, r#""db":"#), Some(12.3));
}

#[test]
fn parse_f32_integer() {
    let s = r#"{"threshold":42}"#;
    assert_eq!(parse_f32_field(s, r#""threshold":"#), Some(42.0));
}

// ── parse_str_field ──────────────────────────────────────────────────────────

#[test]
fn parse_str_basic() {
    let s = r#"{"ssid":"MyNet","pass":"secret"}"#;
    assert_eq!(parse_str_field(s, r#""ssid":"#), Some("MyNet"));
}

#[test]
fn parse_str_empty() {
    let s = r#"{"ssid":"","pass":"secret"}"#;
    assert_eq!(parse_str_field(s, r#""ssid":"#), Some(""));
}

#[test]
fn parse_str_missing_close_quote() {
    let s = r#"{"ssid":"abc"#;
    // There's no closing quote → find('"') returns None
    assert_eq!(parse_str_field(s, r#""ssid":"#), None);
}

#[test]
fn parse_str_with_embedded_quote() {
    // Unescaped quote still truncates (no backslash before it)
    let s = r#"{"ssid":"my"net"}"#;
    assert_eq!(parse_str_field(s, r#""ssid":"#), Some("my"));
}

#[test]
fn parse_str_with_escaped_quote() {
    // Escaped quote is skipped, finds the real closing quote
    let s = r#"{"ssid":"my\"net"}"#;
    assert_eq!(parse_str_field(s, r#""ssid":"#), Some(r#"my\"net"#));
}

#[test]
fn parse_str_second_field() {
    let s = r#"{"ssid":"first","pass":"secret"}"#;
    assert_eq!(parse_str_field(s, r#""pass":"#), Some("secret"));
}

#[test]
fn parse_str_double_backslash_before_quote() {
    // Input: "path\\" — the \\ is an escaped backslash, so the " after it is real
    let s = r#"{"val":"path\\"}"#;
    assert_eq!(parse_str_field(s, r#""val":"#), Some(r#"path\\"#));
}

#[test]
fn parse_str_triple_backslash_before_quote() {
    // Input: "a\\\"b" — \\\\ is two backslashes, \" is escaped quote
    let s = r#"{"val":"a\\\"b"}"#;
    assert_eq!(parse_str_field(s, r#""val":"#), Some(r#"a\\\"b"#));
}

#[test]
fn parse_str_samsung_token_boundary() {
    // Samsung tokens are exactly 16 chars
    let s = r#"{"token":"abcdefghijklmnop"}"#;
    let token = parse_str_field(s, r#""token":"#);
    assert_eq!(token, Some("abcdefghijklmnop"));
    assert_eq!(token.unwrap().len(), 16);
}

// ── parse_ip ─────────────────────────────────────────────────────────────────

#[test]
fn parse_ip_valid() {
    assert_eq!(parse_ip("192.168.1.100"), Some([192, 168, 1, 100]));
}

#[test]
fn parse_ip_invalid_octet() {
    assert_eq!(parse_ip("192.168.1.256"), None);
}

#[test]
fn parse_ip_too_few_parts() {
    assert_eq!(parse_ip("192.168.1"), None);
}

#[test]
fn parse_ip_zeros() {
    assert_eq!(parse_ip("0.0.0.0"), Some([0, 0, 0, 0]));
}

#[test]
fn parse_ip_max() {
    assert_eq!(parse_ip("255.255.255.255"), Some([255, 255, 255, 255]));
}

#[test]
fn parse_ip_letters() {
    assert_eq!(parse_ip("abc.def.ghi.jkl"), None);
}

// ── parse_volume_from_json ───────────────────────────────────────────────────

#[test]
fn parse_volume_basic() {
    let json = br#"{"type":"response","payload":{"volume":25}}"#;
    assert_eq!(parse_volume_from_json(json), Some(25));
}

#[test]
fn parse_volume_missing() {
    let json = br#"{"type":"response","payload":{}}"#;
    assert_eq!(parse_volume_from_json(json), None);
}

#[test]
fn parse_volume_zero() {
    let json = br#"{"volume":0}"#;
    assert_eq!(parse_volume_from_json(json), Some(0));
}

#[test]
fn parse_volume_hundred() {
    let json = br#"{"volume":100}"#;
    assert_eq!(parse_volume_from_json(json), Some(100));
}

// ── extract_ssdp_field ───────────────────────────────────────────────────────

#[test]
fn extract_ssdp_case_insensitive() {
    let resp = "HTTP/1.1 200 OK\r\nSERVER: LG WebOS TV\r\nST: ssdp:all\r\n\r\n";
    assert_eq!(extract_ssdp_field(resp, "SERVER:"), Some("LG WebOS TV"));
    assert_eq!(extract_ssdp_field(resp, "server:"), Some("LG WebOS TV"));
    assert_eq!(extract_ssdp_field(resp, "Server:"), Some("LG WebOS TV"));
}

#[test]
fn extract_ssdp_missing_field() {
    let resp = "HTTP/1.1 200 OK\r\nST: ssdp:all\r\n\r\n";
    assert_eq!(extract_ssdp_field(resp, "SERVER:"), None);
}

#[test]
fn extract_ssdp_st_field() {
    let resp = "HTTP/1.1 200 OK\r\nST: urn:roku-com:device\r\n\r\n";
    assert_eq!(
        extract_ssdp_field(resp, "ST:"),
        Some("urn:roku-com:device")
    );
}

// ── json_unescape ────────────────────────────────────────────────────────

#[test]
fn unescape_clean() {
    assert_eq!(json_unescape("hello").as_str(), "hello");
}

#[test]
fn unescape_quote() {
    assert_eq!(json_unescape(r#"my\"pass"#).as_str(), r#"my"pass"#);
}

#[test]
fn unescape_backslash() {
    assert_eq!(json_unescape(r#"path\\to"#).as_str(), r#"path\to"#);
}

#[test]
fn unescape_both() {
    assert_eq!(json_unescape(r#"a\\\"b"#).as_str(), r#"a\"b"#);
}

#[test]
fn unescape_empty() {
    assert_eq!(json_unescape("").as_str(), "");
}

#[test]
fn unescape_roundtrip_with_escape() {
    // Simulate: user input "my"pass" → json_escape → "my\"pass" → json_unescape → "my"pass"
    let original = r#"my"pass"#;
    let escaped = original.replace('\\', "\\\\").replace('"', "\\\"");
    let unescaped = json_unescape(&escaped);
    assert_eq!(unescaped.as_str(), original);
}

// ── parse_json_str ───────────────────────────────────────────────────────────

#[test]
fn parse_json_str_basic() {
    let s = r#"{"event":"ms.channel.connect","data":{"token":"abc123"}}"#;
    assert_eq!(parse_json_str(s, "\"token\":"), Some("abc123"));
}

#[test]
fn parse_json_str_missing() {
    let s = r#"{"event":"ms.channel.connect"}"#;
    assert_eq!(parse_json_str(s, "\"token\":"), None);
}

// ── parse_str_field structural position ─────────────────────────────────────

#[test]
fn parse_str_key_not_inside_value() {
    // Key "ip" appears inside the psk value — should NOT match there
    let s = r#"{"cmd":"set_tv","psk":"x\"ip\":\"evil\"","ip":"192.168.1.1"}"#;
    assert_eq!(parse_str_field(s, r#""ip":"#), Some("192.168.1.1"));
}

#[test]
fn parse_f32_key_not_inside_value() {
    // Key "db" appears inside a string value — should match the real one
    let s = r#"{"note":"\"db\":99","db":-32.5}"#;
    assert_eq!(parse_f32_field(s, r#""db":"#), Some(-32.5));
}
