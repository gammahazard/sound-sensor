//! dev_log.rs — Structured debug logging forwarded to WebSocket
//!
//! Compiled only when `dev-mode` Cargo feature is active.
//! Production builds have zero overhead — the dev_log! macro expands to nothing.

use embassy_sync::{blocking_mutex::raw::ThreadModeRawMutex, channel::Channel};
use portable_atomic::AtomicBool;

// ── Log level ──────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Info  => "info",
            Self::Warn  => "warn",
            Self::Error => "error",
        }
    }
}

// ── Log category ───────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq)]
pub enum LogCat {
    Audio,
    Ducking,
    Tv,
    Wifi,
    Ws,
    Flash,
    Http,
    Ota,
}

impl LogCat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Audio   => "audio",
            Self::Ducking => "ducking",
            Self::Tv      => "tv",
            Self::Wifi    => "wifi",
            Self::Ws      => "ws",
            Self::Flash   => "flash",
            Self::Http    => "http",
            Self::Ota     => "ota",
        }
    }
}

// ── Log entry ──────────────────────────────────────────────────────────────────
pub struct LogEntry {
    pub level: LogLevel,
    pub cat:   LogCat,
    pub msg:   heapless::String<128>,
}

// ── Channel: any task → ws_task (non-blocking, 4-deep) ──────────────────────
pub static DEV_LOG_CH: Channel<ThreadModeRawMutex, LogEntry, 4> = Channel::new();

// ── Runtime toggle (mute/unmute from PWA without recompiling) ───────────────
pub static DEV_LOG_ACTIVE: AtomicBool = AtomicBool::new(true);

// Note: the dev_log! macro is defined in main.rs so it's available
// in all modules regardless of whether dev-mode feature is on.
