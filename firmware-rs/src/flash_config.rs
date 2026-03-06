//! flash_config.rs — Config block persistence (256 bytes at 0x1FF000)
//!
//! Layout:
//!   Bytes 0–3:     magic (0xBADC0FFE LE)
//!   Bytes 4–67:    WiFi SSID (null-terminated, max 63)
//!   Bytes 68–131:  WiFi pass (null-terminated, max 63)
//!   Byte  132:     tv_enabled (0=none, 1=configured)
//!   Bytes 133–148: tv_ip (null-terminated, max 15)
//!   Byte  149:     tv_brand (0=Lg, 1=Samsung, 2=Sony, 3=Roku)
//!   Bytes 150–165: samsung_token (null-terminated, max 15)
//!   Bytes 166–173: sony_psk (null-terminated, max 8)
//!   Byte  174:     calibration_valid (0=no, 1=yes)
//!   Bytes 175–178: floor_db (f32 LE)
//!   Bytes 179–182: tripwire_db (f32 LE)
//!   Bytes 183–188: tv_mac (6 bytes, for Wake-on-LAN)
//!   Bytes 189–251: reserved
//!   Bytes 252–255: CRC32 over bytes 0–251

use defmt::*;
use embassy_rp::flash::{Blocking, Flash};
use embassy_rp::peripherals::FLASH;

use crate::tv::{TvBrand, TvConfig};

pub const FLASH_SIZE: usize = 4 * 1024 * 1024; // 4 MB
pub const CONFIG_OFFSET: u32 = 0x1F_F000;       // last 4 KB sector
const CONFIG_MAGIC: u32 = 0xBADC_0FFE;

// ── Shared helpers ──────────────────────────────────────────────────────────

fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

fn read_null_terminated(buf: &[u8]) -> &str {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    core::str::from_utf8(&buf[..end]).unwrap_or("")
}

/// Read the 256-byte config block from flash. Returns None on read error.
fn read_config_block(flash: &mut Flash<'_, FLASH, Blocking, FLASH_SIZE>) -> Option<[u8; 256]> {
    let mut buf = [0u8; 256];
    flash.blocking_read(CONFIG_OFFSET, &mut buf).ok()?;
    Some(buf)
}

/// Validate magic + CRC. If invalid, returns a fresh zeroed block with magic set.
fn validate_or_fresh(buf: &mut [u8; 256]) {
    let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let stored_crc = u32::from_le_bytes([buf[252], buf[253], buf[254], buf[255]]);
    if magic != CONFIG_MAGIC || crc32(&buf[..252]) != stored_crc {
        *buf = [0u8; 256];
        buf[0..4].copy_from_slice(&CONFIG_MAGIC.to_le_bytes());
    }
}

/// Validate magic + CRC for read-only checks.
fn is_valid(buf: &[u8; 256]) -> bool {
    let magic = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if magic != CONFIG_MAGIC { return false; }
    let stored_crc = u32::from_le_bytes([buf[252], buf[253], buf[254], buf[255]]);
    crc32(&buf[..252]) == stored_crc
}

/// Compute CRC, erase sector, write block. Returns false if erase or write failed.
fn write_config_block(flash: &mut Flash<'_, FLASH, Blocking, FLASH_SIZE>, buf: &mut [u8; 256], label: &str) -> bool {
    let crc = crc32(&buf[..252]);
    buf[252..256].copy_from_slice(&crc.to_le_bytes());
    if flash.blocking_erase(CONFIG_OFFSET, CONFIG_OFFSET + 4096).is_err() {
        warn!("[flash] erase failed ({})", label);
        return false;
    }
    if flash.blocking_write(CONFIG_OFFSET, buf).is_err() {
        warn!("[flash] write failed ({})", label);
        return false;
    }
    true
}

// ── WiFi credentials ────────────────────────────────────────────────────────

pub fn flash_load_creds(flash: &mut Flash<'_, FLASH, Blocking, FLASH_SIZE>) -> Option<(heapless::String<64>, heapless::String<64>)> {
    let buf = read_config_block(flash)?;
    if !is_valid(&buf) { return None; }
    let ssid_str = read_null_terminated(&buf[4..68]);
    let pass_str = read_null_terminated(&buf[68..132]);
    if ssid_str.is_empty() { return None; }
    let mut ssid = heapless::String::new();
    let _ = ssid.push_str(ssid_str);
    let mut pass = heapless::String::new();
    let _ = pass.push_str(pass_str);
    Some((ssid, pass))
}

pub fn flash_save_creds(flash: &mut Flash<'_, FLASH, Blocking, FLASH_SIZE>, ssid: &str, pass: &str) -> bool {
    let mut buf = [0u8; 256];
    let _ = flash.blocking_read(CONFIG_OFFSET, &mut buf);
    validate_or_fresh(&mut buf);
    // Write SSID (null-terminated)
    buf[4..68].fill(0);
    let ssid_bytes = ssid.as_bytes();
    buf[4..4 + ssid_bytes.len().min(63)].copy_from_slice(&ssid_bytes[..ssid_bytes.len().min(63)]);
    // Write pass (null-terminated)
    buf[68..132].fill(0);
    let pass_bytes = pass.as_bytes();
    buf[68..68 + pass_bytes.len().min(63)].copy_from_slice(&pass_bytes[..pass_bytes.len().min(63)]);
    write_config_block(flash, &mut buf, "save_creds")
}

/// Clear only WiFi credentials (bytes 4–131), preserving TV config + calibration.
/// Falls back to full sector erase if the existing block is invalid.
pub fn flash_clear_creds(flash: &mut Flash<'_, FLASH, Blocking, FLASH_SIZE>) {
    let mut buf = [0u8; 256];
    let _ = flash.blocking_read(CONFIG_OFFSET, &mut buf);
    if !is_valid(&buf) {
        // Block is invalid — full erase is fine (nothing to preserve)
        let _ = flash.blocking_erase(CONFIG_OFFSET, CONFIG_OFFSET + 4096);
        return;
    }
    buf[4..132].fill(0);
    write_config_block(flash, &mut buf, "clear_creds");
}

// ── TV config ───────────────────────────────────────────────────────────────

pub fn flash_load_tv_config(flash: &mut Flash<'_, FLASH, Blocking, FLASH_SIZE>) -> Option<TvConfig> {
    let buf = read_config_block(flash)?;
    if !is_valid(&buf) { return None; }
    if buf[132] == 0 { return None; }
    let ip_str = read_null_terminated(&buf[133..149]);
    if ip_str.is_empty() { return None; }
    let brand = TvBrand::from_u8(buf[149]);
    let token_str = read_null_terminated(&buf[150..166]);
    let psk_str = read_null_terminated(&buf[166..174]);
    let mut cfg = TvConfig::default();
    cfg.ip.clear();
    let _ = cfg.ip.push_str(ip_str);
    cfg.brand = brand;
    cfg.samsung_token.clear();
    let _ = cfg.samsung_token.push_str(token_str);
    cfg.sony_psk.clear();
    let _ = cfg.sony_psk.push_str(psk_str);
    Some(cfg)
}

pub fn flash_save_tv_config(flash: &mut Flash<'_, FLASH, Blocking, FLASH_SIZE>, tv: &TvConfig) {
    let mut buf = [0u8; 256];
    let _ = flash.blocking_read(CONFIG_OFFSET, &mut buf);
    validate_or_fresh(&mut buf);
    buf[132] = if tv.is_configured() { 1 } else { 0 };
    buf[133..149].fill(0);
    let ip_bytes = tv.ip.as_bytes();
    buf[133..133 + ip_bytes.len().min(15)].copy_from_slice(&ip_bytes[..ip_bytes.len().min(15)]);
    buf[149] = tv.brand.to_u8();
    buf[150..166].fill(0);
    let tok_bytes = tv.samsung_token.as_bytes();
    buf[150..150 + tok_bytes.len().min(15)].copy_from_slice(&tok_bytes[..tok_bytes.len().min(15)]);
    buf[166..174].fill(0);
    let psk_bytes = tv.sony_psk.as_bytes();
    buf[166..166 + psk_bytes.len().min(8)].copy_from_slice(&psk_bytes[..psk_bytes.len().min(8)]);
    write_config_block(flash, &mut buf, "save_tv_config");
}

// ── Calibration ─────────────────────────────────────────────────────────────

pub fn flash_load_calibration(flash: &mut Flash<'_, FLASH, Blocking, FLASH_SIZE>) -> Option<(f32, f32)> {
    let buf = read_config_block(flash)?;
    if !is_valid(&buf) { return None; }
    if buf[174] == 0 { return None; }
    let floor = f32::from_le_bytes([buf[175], buf[176], buf[177], buf[178]]);
    let tripwire = f32::from_le_bytes([buf[179], buf[180], buf[181], buf[182]]);
    if floor < -96.0 || floor > 0.0 || tripwire < -96.0 || tripwire > 0.0 {
        return None;
    }
    Some((floor, tripwire))
}

pub fn flash_save_calibration(flash: &mut Flash<'_, FLASH, Blocking, FLASH_SIZE>, floor: f32, tripwire: f32) {
    let mut buf = [0u8; 256];
    let _ = flash.blocking_read(CONFIG_OFFSET, &mut buf);
    validate_or_fresh(&mut buf);
    buf[174] = 1;
    buf[175..179].copy_from_slice(&floor.to_le_bytes());
    buf[179..183].copy_from_slice(&tripwire.to_le_bytes());
    write_config_block(flash, &mut buf, "save_calibration");
}
