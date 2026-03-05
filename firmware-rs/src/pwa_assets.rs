//! pwa_assets.rs — Embedded PWA files baked into firmware flash.
//!
//! Build order:
//!   1. cd pwa-wasm && trunk build --release   (produces dist/)
//!   2. cd ../firmware-rs && cargo build --release

/// Fallback PWA version tag embedded at build time.
pub const EMBEDDED_PWA_VERSION: &str = "0.1.0";

// ── WASM bundle (gzip-compressed by build.rs) ───────────────────────────────

pub static WASM_JS_GZ: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/guardian-pwa.js.gz"));

pub static WASM_BG_GZ: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/guardian-pwa_bg.wasm.gz"));

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

// version.json is built dynamically in http.rs using FW_VERSION and PWA_VERSION.
