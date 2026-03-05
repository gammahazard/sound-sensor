//! guardian-test — Host-side pure-logic extracted from firmware
//!
//! Copies of no_std-compatible pure functions from firmware-rs,
//! adapted for host testing (Instant replaced with injectable u64 timestamps).

pub mod ducking;
pub mod parsers;
pub mod crypto;
pub mod audio;
pub mod ota;
pub mod flash_layout;
pub mod ws_frame;
pub mod tv_brand;
