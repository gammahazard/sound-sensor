//! flash_layout.rs — Config block serialization/deserialization
//!
//! Pure functions operating on [u8; 256] buffers — no flash hardware needed.
//! Layout matches firmware net.rs exactly:
//!   Bytes 0–3:     magic (0xBADC0FFE le)
//!   Bytes 4–67:    WiFi SSID (null-terminated, max 63 chars)
//!   Bytes 68–131:  WiFi pass (null-terminated, max 63 chars)
//!   Bytes 132:     tv_enabled (0=none, 1=configured)
//!   Bytes 133–148: tv_ip (null-terminated, max 15 chars)
//!   Bytes 149:     tv_brand (0=Lg, 1=Samsung, 2=Sony, 3=Roku)
//!   Bytes 150–165: samsung_token (null-terminated, max 15 chars)
//!   Bytes 166–173: sony_psk (null-terminated, max 8 chars — but field is 8 bytes)
//!   Byte  174:     calibration_valid (0=no, 1=yes)
//!   Bytes 175–178: floor_db (f32 LE)
//!   Bytes 179–182: tripwire_db (f32 LE)
//!   Bytes 183–188: tv_mac (6 bytes, for Wake-on-LAN)
//!   Bytes 189–251: reserved
//!   Bytes 252–255: CRC32 over bytes 0–251

use crate::crypto::crc32;
use crate::tv_brand::TvBrand;

const CONFIG_MAGIC: u32 = 0xBADC_0FFE;

#[derive(Debug, Clone, PartialEq)]
pub struct WifiCreds {
    pub ssid: heapless::String<64>,
    pub pass: heapless::String<64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TvConfig {
    pub ip: heapless::String<16>,
    pub brand: TvBrand,
    pub samsung_token: heapless::String<16>,
    pub sony_psk: heapless::String<8>,
}

fn read_null_terminated(buf: &[u8]) -> &str {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    core::str::from_utf8(&buf[..end]).unwrap_or("")
}

fn write_field(buf: &mut [u8], s: &str, max_len: usize) {
    buf[..max_len].fill(0);
    let n = s.len().min(max_len);
    buf[..n].copy_from_slice(&s.as_bytes()[..n]);
}

fn finalize_crc(buf: &mut [u8; 256]) {
    let c = crc32(&buf[..252]);
    buf[252..256].copy_from_slice(&c.to_le_bytes());
}

fn validate_block(buf: &[u8; 256]) -> bool {
    let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if magic != CONFIG_MAGIC {
        return false;
    }
    let stored_crc = u32::from_le_bytes([buf[252], buf[253], buf[254], buf[255]]);
    crc32(&buf[..252]) == stored_crc
}

/// Save WiFi credentials into a config block (read-modify-write preserves TV fields).
pub fn save_wifi_creds(buf: &mut [u8; 256], ssid: &str, pass: &str) {
    // Validate existing block; if invalid, start fresh
    if !validate_block(buf) {
        *buf = [0u8; 256];
    }
    // Magic
    buf[0..4].copy_from_slice(&CONFIG_MAGIC.to_le_bytes());
    // SSID (max 63 chars null-terminated in 64 bytes)
    write_field(&mut buf[4..68], ssid, 63);
    buf[67] = 0; // ensure null terminator
    // Pass (max 63 chars null-terminated in 64 bytes)
    write_field(&mut buf[68..132], pass, 63);
    buf[131] = 0;
    finalize_crc(buf);
}

/// Load WiFi credentials from a config block.
pub fn load_wifi_creds(buf: &[u8; 256]) -> Option<WifiCreds> {
    if !validate_block(buf) {
        return None;
    }
    let ssid_str = read_null_terminated(&buf[4..68]);
    let pass_str = read_null_terminated(&buf[68..132]);
    if ssid_str.is_empty() {
        return None;
    }
    let mut ssid = heapless::String::new();
    let _ = ssid.push_str(ssid_str);
    let mut pass = heapless::String::new();
    let _ = pass.push_str(pass_str);
    Some(WifiCreds { ssid, pass })
}

/// Save TV config into a config block (read-modify-write preserves WiFi fields).
pub fn save_tv_config(buf: &mut [u8; 256], tv: &TvConfig) {
    if !validate_block(buf) {
        *buf = [0u8; 256];
        buf[0..4].copy_from_slice(&CONFIG_MAGIC.to_le_bytes());
    }
    buf[132] = if !tv.ip.is_empty() { 1 } else { 0 };
    write_field(&mut buf[133..149], tv.ip.as_str(), 15);
    buf[148] = 0;
    buf[149] = tv.brand.to_u8();
    write_field(&mut buf[150..166], tv.samsung_token.as_str(), 16);
    write_field(&mut buf[166..174], tv.sony_psk.as_str(), 8);
    finalize_crc(buf);
}

/// Load TV config from a config block.
pub fn load_tv_config(buf: &[u8; 256]) -> Option<TvConfig> {
    if !validate_block(buf) {
        return None;
    }
    if buf[132] == 0 {
        return None;
    }
    let ip_str = read_null_terminated(&buf[133..149]);
    if ip_str.is_empty() {
        return None;
    }
    let brand = TvBrand::from_u8(buf[149]);
    let token_str = read_null_terminated(&buf[150..166]);
    let psk_str = read_null_terminated(&buf[166..174]);

    let mut ip = heapless::String::new();
    let _ = ip.push_str(ip_str);
    let mut samsung_token = heapless::String::new();
    let _ = samsung_token.push_str(token_str);
    let mut sony_psk = heapless::String::new();
    let _ = sony_psk.push_str(psk_str);

    Some(TvConfig {
        ip,
        brand,
        samsung_token,
        sony_psk,
    })
}

/// Clear only WiFi credentials (bytes 4–131), preserving TV config + calibration.
/// Falls back to zeroing the entire block if it's invalid.
pub fn clear_wifi_creds(buf: &mut [u8; 256]) {
    if !validate_block(buf) {
        *buf = [0u8; 256];
        return;
    }
    buf[4..132].fill(0);
    finalize_crc(buf);
}

/// Save calibration data into a config block (read-modify-write).
pub fn save_calibration(buf: &mut [u8; 256], floor: f32, tripwire: f32) {
    if !validate_block(buf) {
        *buf = [0u8; 256];
        buf[0..4].copy_from_slice(&CONFIG_MAGIC.to_le_bytes());
    }
    buf[174] = 1; // calibration_valid
    buf[175..179].copy_from_slice(&floor.to_le_bytes());
    buf[179..183].copy_from_slice(&tripwire.to_le_bytes());
    finalize_crc(buf);
}

/// Load calibration data from a config block.
pub fn load_calibration(buf: &[u8; 256]) -> Option<(f32, f32)> {
    if !validate_block(buf) {
        return None;
    }
    if buf[174] == 0 {
        return None;
    }
    let floor = f32::from_le_bytes([buf[175], buf[176], buf[177], buf[178]]);
    let tripwire = f32::from_le_bytes([buf[179], buf[180], buf[181], buf[182]]);
    if floor < -96.0 || floor > 0.0 || tripwire < -96.0 || tripwire > 0.0 {
        return None;
    }
    Some((floor, tripwire))
}
