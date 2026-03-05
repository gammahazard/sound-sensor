use guardian_test::flash_layout::*;
use guardian_test::tv_brand::TvBrand;

fn make_tv_config(ip: &str, brand: TvBrand, token: &str, psk: &str) -> TvConfig {
    let mut c = TvConfig {
        ip: heapless::String::new(),
        brand,
        samsung_token: heapless::String::new(),
        sony_psk: heapless::String::new(),
    };
    let _ = c.ip.push_str(ip);
    let _ = c.samsung_token.push_str(token);
    // Truncate PSK to 8 chars (matches firmware ws.rs call site)
    let _ = c.sony_psk.push_str(&psk[..psk.len().min(8)]);
    c
}

// ── WiFi roundtrip ──────────────────────────────────────────────────────────

#[test]
fn roundtrip_wifi_creds() {
    let mut buf = [0u8; 256];
    save_wifi_creds(&mut buf, "MyNetwork", "password123");
    let creds = load_wifi_creds(&buf).unwrap();
    assert_eq!(creds.ssid.as_str(), "MyNetwork");
    assert_eq!(creds.pass.as_str(), "password123");
}

#[test]
fn roundtrip_tv_config() {
    let mut buf = [0u8; 256];
    // Must have valid magic/CRC first
    save_wifi_creds(&mut buf, "test", "test");
    let tv = make_tv_config("192.168.1.50", TvBrand::Samsung, "tok123", "");
    save_tv_config(&mut buf, &tv);
    let loaded = load_tv_config(&buf).unwrap();
    assert_eq!(loaded.ip.as_str(), "192.168.1.50");
    assert_eq!(loaded.brand, TvBrand::Samsung);
    assert_eq!(loaded.samsung_token.as_str(), "tok123");
}

#[test]
fn interleaved_saves() {
    let mut buf = [0u8; 256];
    save_wifi_creds(&mut buf, "HomeWiFi", "pass456");
    let tv = make_tv_config("10.0.0.5", TvBrand::Sony, "", "1234");
    save_tv_config(&mut buf, &tv);
    // Both should survive
    let creds = load_wifi_creds(&buf).unwrap();
    assert_eq!(creds.ssid.as_str(), "HomeWiFi");
    assert_eq!(creds.pass.as_str(), "pass456");
    let tv_loaded = load_tv_config(&buf).unwrap();
    assert_eq!(tv_loaded.ip.as_str(), "10.0.0.5");
    assert_eq!(tv_loaded.brand, TvBrand::Sony);
    assert_eq!(tv_loaded.sony_psk.as_str(), "1234");
}

#[test]
fn interleaved_saves_reverse_order() {
    let mut buf = [0u8; 256];
    // Save TV first, then WiFi — WiFi should preserve TV fields
    let tv = make_tv_config("10.0.0.5", TvBrand::Lg, "", "");
    save_tv_config(&mut buf, &tv);
    save_wifi_creds(&mut buf, "Network2", "secret");
    let creds = load_wifi_creds(&buf).unwrap();
    assert_eq!(creds.ssid.as_str(), "Network2");
    let tv_loaded = load_tv_config(&buf).unwrap();
    assert_eq!(tv_loaded.ip.as_str(), "10.0.0.5");
}

// ── Bad data ────────────────────────────────────────────────────────────────

#[test]
fn bad_magic_returns_none() {
    let buf = [0xAA; 256];
    assert!(load_wifi_creds(&buf).is_none());
    assert!(load_tv_config(&buf).is_none());
}

#[test]
fn bad_crc_returns_none() {
    let mut buf = [0u8; 256];
    save_wifi_creds(&mut buf, "test", "test");
    // Flip a byte in the data area
    buf[10] ^= 0xFF;
    assert!(load_wifi_creds(&buf).is_none());
}

// ── SSID edge cases ─────────────────────────────────────────────────────────

#[test]
fn max_length_ssid() {
    let ssid = "a".repeat(63);
    let mut buf = [0u8; 256];
    save_wifi_creds(&mut buf, &ssid, "p");
    let creds = load_wifi_creds(&buf).unwrap();
    assert_eq!(creds.ssid.len(), 63);
}

#[test]
fn truncation_ssid_64() {
    let ssid = "b".repeat(64);
    let mut buf = [0u8; 256];
    save_wifi_creds(&mut buf, &ssid, "p");
    let creds = load_wifi_creds(&buf).unwrap();
    assert_eq!(creds.ssid.len(), 63); // Truncated
}

#[test]
fn sony_psk_max_8() {
    let mut buf = [0u8; 256];
    save_wifi_creds(&mut buf, "net", "pass");
    let tv = make_tv_config("1.2.3.4", TvBrand::Sony, "", "12345678");
    save_tv_config(&mut buf, &tv);
    let loaded = load_tv_config(&buf).unwrap();
    assert_eq!(loaded.sony_psk.as_str(), "12345678");
}

#[test]
fn sony_psk_truncated_9() {
    let mut buf = [0u8; 256];
    save_wifi_creds(&mut buf, "net", "pass");
    // PSK field is 8 bytes (166–173), writing 9 chars gets truncated to 8
    let tv = make_tv_config("1.2.3.4", TvBrand::Sony, "", "123456789");
    save_tv_config(&mut buf, &tv);
    let loaded = load_tv_config(&buf).unwrap();
    assert_eq!(loaded.sony_psk.len(), 8);
}

#[test]
fn tv_disabled_on_empty_ip() {
    let mut buf = [0u8; 256];
    save_wifi_creds(&mut buf, "net", "pass");
    let tv = make_tv_config("", TvBrand::Lg, "", "");
    save_tv_config(&mut buf, &tv);
    assert!(load_tv_config(&buf).is_none());
}

#[test]
fn brand_roundtrip_all() {
    for brand in [TvBrand::Lg, TvBrand::Samsung, TvBrand::Sony, TvBrand::Roku] {
        let mut buf = [0u8; 256];
        save_wifi_creds(&mut buf, "net", "pass");
        let tv = make_tv_config("1.2.3.4", brand, "", "");
        save_tv_config(&mut buf, &tv);
        let loaded = load_tv_config(&buf).unwrap();
        assert_eq!(loaded.brand, brand, "Brand roundtrip failed for {:?}", brand);
    }
}

#[test]
fn null_in_ssid() {
    let mut buf = [0u8; 256];
    // Write a SSID with embedded null
    save_wifi_creds(&mut buf, "abc", "pass");
    // Manually insert a null at position 6 (byte index 4+2=6)
    buf[6] = 0; // buf[4]='a', buf[5]='b', buf[6]='\0'
    // Re-compute CRC manually
    let crc = guardian_test::crypto::crc32(&buf[..252]);
    buf[252..256].copy_from_slice(&crc.to_le_bytes());
    let creds = load_wifi_creds(&buf).unwrap();
    assert_eq!(creds.ssid.as_str(), "ab"); // Truncated at null
}

#[test]
fn empty_ssid_returns_none() {
    let mut buf = [0u8; 256];
    save_wifi_creds(&mut buf, "", "pass");
    assert!(load_wifi_creds(&buf).is_none());
}

// ── clear_wifi_creds ────────────────────────────────────────────────────────

#[test]
fn clear_creds_preserves_tv_config() {
    let mut buf = [0u8; 256];
    save_wifi_creds(&mut buf, "HomeWiFi", "secret123");
    let tv = make_tv_config("192.168.1.50", TvBrand::Samsung, "tok999", "");
    save_tv_config(&mut buf, &tv);
    // Verify both exist
    assert!(load_wifi_creds(&buf).is_some());
    assert!(load_tv_config(&buf).is_some());
    // Clear creds
    clear_wifi_creds(&mut buf);
    // WiFi should be gone, TV should survive
    assert!(load_wifi_creds(&buf).is_none());
    let tv_loaded = load_tv_config(&buf).unwrap();
    assert_eq!(tv_loaded.ip.as_str(), "192.168.1.50");
    assert_eq!(tv_loaded.brand, TvBrand::Samsung);
    assert_eq!(tv_loaded.samsung_token.as_str(), "tok999");
}

#[test]
fn clear_creds_preserves_calibration() {
    let mut buf = [0u8; 256];
    save_wifi_creds(&mut buf, "Net", "pass");
    save_calibration(&mut buf, -42.0, -20.0);
    // Clear creds
    clear_wifi_creds(&mut buf);
    // WiFi gone
    assert!(load_wifi_creds(&buf).is_none());
    // Calibration survives
    let (floor, tripwire) = load_calibration(&buf).unwrap();
    assert!((floor - (-42.0)).abs() < 0.001);
    assert!((tripwire - (-20.0)).abs() < 0.001);
}

#[test]
fn clear_creds_on_invalid_block() {
    // An invalid block should be fully zeroed (no panic)
    let mut buf = [0xAA; 256];
    clear_wifi_creds(&mut buf);
    // Should be all zeros now
    assert!(buf.iter().all(|&b| b == 0));
}

// ── Calibration ─────────────────────────────────────────────────────────────

#[test]
fn calibration_roundtrip() {
    let mut buf = [0u8; 256];
    save_wifi_creds(&mut buf, "Net", "pass");
    save_calibration(&mut buf, -45.5, -18.3);
    let (floor, tripwire) = load_calibration(&buf).unwrap();
    assert!((floor - (-45.5)).abs() < 0.001);
    assert!((tripwire - (-18.3)).abs() < 0.001);
    // WiFi should survive
    let creds = load_wifi_creds(&buf).unwrap();
    assert_eq!(creds.ssid.as_str(), "Net");
}
