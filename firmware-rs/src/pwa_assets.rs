//! pwa_assets.rs — Embedded PWA files baked into firmware flash.
//!
//! These are the "safety net" copies served when LittleFS is empty or
//! unavailable (first boot, corrupt FS, factory reset).
//!
//! Build order:
//!   1. cd pwa-wasm && trunk build --release   (produces dist/)
//!   2. cd ../firmware-rs && cargo build --release
//!
//! Trunk is configured with data-no-hash so output filenames are fixed:
//!   guardian_pwa.js        — JS glue / bootstrap
//!   guardian_pwa_bg.wasm   — compiled Leptos app (main binary)
//!   index.html, sw.js, manifest.json, icon-*.png
//!
//! OTA updates write newer versions to LittleFS; http_task checks
//! LittleFS first and falls back to these constants only when no
//! LittleFS copy exists.

/// Fallback PWA version tag embedded at build time.
pub const EMBEDDED_PWA_VERSION: &str = "0.1.0";

// ── WASM bundle ───────────────────────────────────────────────────────────────

/// JS glue that loads and initialises the WASM module.
pub static WASM_JS: &[u8] =
    include_bytes!("../../pwa-wasm/dist/guardian_pwa.js");

/// Compiled Leptos application (WebAssembly binary).
pub static WASM_BG: &[u8] =
    include_bytes!("../../pwa-wasm/dist/guardian_pwa_bg.wasm");

// ── App shell ─────────────────────────────────────────────────────────────────

pub static INDEX_HTML: &[u8] =
    include_bytes!("../../pwa-wasm/dist/index.html");

pub static SW_JS: &[u8] =
    include_bytes!("../../pwa-wasm/dist/sw.js");

pub static MANIFEST: &[u8] =
    include_bytes!("../../pwa-wasm/dist/manifest.json");

pub static ICON_192: &[u8] =
    include_bytes!("../../pwa-wasm/dist/icon-192.png");

pub static ICON_512: &[u8] =
    include_bytes!("../../pwa-wasm/dist/icon-512.png");

/// version.json served at /version.json when no LittleFS copy exists.
pub static VERSION_JSON: &[u8] =
    br#"{"pwa":"0.1.0","fw":"0.2.0","built":"2026-03-02"}"#;
