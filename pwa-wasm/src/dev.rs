//! dev.rs — Dev/Debug tab for live firmware diagnostics
//!
//! Only visible when firmware is built with `--features dev-mode`.
//! Shows: live state dashboard, firmware log stream, raw WS inspector,
//! calibration debug, connection stats.

use leptos::prelude::*;
use crate::ws::{DevLogEntry, RawWsEntry, WsState};

// ── Category filter bit positions ───────────────────────────────────────────

const CAT_AUDIO:   u8 = 0;
const CAT_DUCKING: u8 = 1;
const CAT_TV:      u8 = 2;
const CAT_WIFI:    u8 = 3;
const CAT_WS:      u8 = 4;
const CAT_FLASH:   u8 = 5;
const CAT_HTTP:    u8 = 6;
const CAT_OTA:     u8 = 7;

fn cat_bit(cat: &str) -> u8 {
    match cat {
        "audio"   => CAT_AUDIO,
        "ducking" => CAT_DUCKING,
        "tv"      => CAT_TV,
        "wifi"    => CAT_WIFI,
        "ws"      => CAT_WS,
        "flash"   => CAT_FLASH,
        "http"    => CAT_HTTP,
        "ota"     => CAT_OTA,
        _         => 7,
    }
}

fn cat_color(cat: &str) -> &'static str {
    match cat {
        "audio"   => "#22c55e",
        "ducking" => "#f59e0b",
        "tv"      => "#6366f1",
        "wifi"    => "#3b82f6",
        "ws"      => "#8b5cf6",
        "flash"   => "#ec4899",
        "http"    => "#14b8a6",
        "ota"     => "#06b6d4",
        _         => "#94a3b8",
    }
}

fn lvl_color(lvl: &str) -> &'static str {
    match lvl {
        "warn"  => "#fde68a",
        "error" => "#fca5a5",
        _       => "#cbd5e1",
    }
}

// ── Dev screen ──────────────────────────────────────────────────────────────

#[component]
pub fn DevScreen(
    db:               ReadSignal<f32>,
    armed:            ReadSignal<bool>,
    tripwire:         ReadSignal<f32>,
    ducking:          ReadSignal<bool>,
    ws_state:         ReadSignal<WsState>,
    fw_ver:           ReadSignal<String>,
    pwa_ver:          ReadSignal<String>,
    tv_ip:            ReadSignal<String>,
    tv_brand:         ReadSignal<String>,
    tv_status:        ReadSignal<u8>,
    msg_count:        ReadSignal<u32>,
    reconnect_count:  ReadSignal<u32>,
    last_msg_time:    ReadSignal<String>,
    dev_logs:         ReadSignal<Vec<DevLogEntry>>,
    raw_ws_log:       ReadSignal<Vec<RawWsEntry>>,
    on_toggle_logging: impl Fn() + 'static,
    on_clear_logs:     impl Fn() + 'static,
) -> impl IntoView {
    // Category filter bitmask (all on by default)
    let (filter, set_filter) = signal(0xFFu8);
    // Hide telemetry in raw WS inspector
    let (hide_telem, set_hide_telem) = signal(true);
    // Logging paused state (local UI state, not synced with firmware)
    let (paused, set_paused) = signal(false);

    let toggle_logging = move || {
        set_paused.update(|p| *p = !*p);
        on_toggle_logging();
    };

    view! {
        <div style="padding:16px;display:flex;flex-direction:column;gap:12px">
            <h1 style="font-size:22px;font-weight:700;color:#f1f5f9;margin:0">
                "Dev Console"
            </h1>

            // ── Section A: Live State Dashboard ─────────────────────────────
            <div style="background:#1e293b;border-radius:16px;padding:14px;display:grid;\
                         grid-template-columns:1fr 1fr;gap:6px 12px;font-size:12px;font-family:monospace">
                <StateRow label="db"       value=move || format!("{:.1}", db.get()) />
                <StateRow label="armed"    value=move || bool_str(armed.get()) />
                <StateRow label="tripwire" value=move || format!("{:.1}", tripwire.get()) />
                <StateRow label="ducking"  value=move || bool_str(ducking.get()) />
                <StateRow label="ws"       value=move || ws_str(ws_state.get()) />
                <StateRow label="fw"       value=move || fw_ver.get() />
                <StateRow label="tv"       value=move || {
                    let ip = tv_ip.get();
                    if ip.is_empty() { "none".to_string() }
                    else { format!("{}@{}", tv_brand.get(), ip) }
                } />
                <StateRow label="tv_status" value=move || tv_status_str(tv_status.get()) />
                <StateRow label="pwa"      value=move || pwa_ver.get() />
            </div>

            // ── Section B: Log Stream ───────────────────────────────────────
            <div style="background:#1e293b;border-radius:16px;padding:14px">
                <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:8px">
                    <span style="font-size:14px;font-weight:600;color:#f1f5f9">"Firmware Logs"</span>
                    <div style="display:flex;gap:6px">
                        <button
                            on:click=move |_| toggle_logging()
                            style=move || format!(
                                "border:none;border-radius:8px;padding:5px 10px;font-size:11px;\
                                 font-weight:600;cursor:pointer;color:#f1f5f9;background:{}",
                                if paused.get() { "#ef4444" } else { "#334155" }
                            )
                        >
                            {move || if paused.get() { "Paused" } else { "Pause" }}
                        </button>
                        <button
                            on:click=move |_| on_clear_logs()
                            style="border:none;border-radius:8px;padding:5px 10px;font-size:11px;\
                                   font-weight:600;cursor:pointer;color:#94a3b8;background:#334155"
                        >
                            "Clear"
                        </button>
                    </div>
                </div>

                // Filter pills
                <div style="display:flex;flex-wrap:wrap;gap:4px;margin-bottom:8px">
                    {["audio","ducking","tv","wifi","ws","flash","http","ota"].into_iter().map(|cat| {
                        let cat_s: &'static str = cat;
                        view! {
                            <button
                                on:click=move |_| {
                                    set_filter.update(|f| *f ^= 1 << cat_bit(cat_s));
                                }
                                style=move || {
                                    let active = filter.get() & (1 << cat_bit(cat_s)) != 0;
                                    format!(
                                        "border-radius:999px;padding:3px 8px;font-size:10px;\
                                         border:1px solid {};cursor:pointer;font-weight:600;\
                                         background:{};color:{}",
                                        cat_color(cat_s),
                                        if active { cat_color(cat_s) } else { "transparent" },
                                        if active { "#0f172a" } else { cat_color(cat_s) },
                                    )
                                }
                            >
                                {cat_s}
                            </button>
                        }
                    }).collect::<Vec<_>>()}
                </div>

                // Log entries
                <div style="max-height:280px;overflow-y:auto;font-family:monospace;font-size:11px">
                    {move || {
                        let f = filter.get();
                        dev_logs.get().into_iter()
                            .filter(|e| f & (1 << cat_bit(&e.cat)) != 0)
                            .map(|e| {
                                let cc = cat_color(&e.cat);
                                let lc = lvl_color(&e.lvl);
                                view! {
                                    <div style="padding:2px 0;border-bottom:1px solid #0f172a;display:flex;gap:6px;align-items:baseline">
                                        <span style="color:#475569;flex-shrink:0;font-size:10px">{e.time.clone()}</span>
                                        <span style=format!("color:{};flex-shrink:0;width:52px;font-size:10px", cc)>{e.cat.clone()}</span>
                                        <span style=format!("color:{};word-break:break-all", lc)>{e.msg.clone()}</span>
                                    </div>
                                }
                            })
                            .collect::<Vec<_>>()
                    }}
                </div>
            </div>

            // ── Section C: Raw WS Inspector ─────────────────────────────────
            <div style="background:#1e293b;border-radius:16px;padding:14px">
                <div style="display:flex;align-items:center;justify-content:space-between;margin-bottom:8px">
                    <span style="font-size:14px;font-weight:600;color:#f1f5f9">"Raw WebSocket"</span>
                    <label style="display:flex;align-items:center;gap:6px;font-size:11px;color:#94a3b8;cursor:pointer">
                        <input
                            type="checkbox"
                            prop:checked=move || hide_telem.get()
                            on:change=move |_| set_hide_telem.update(|v| *v = !*v)
                        />
                        "Hide telemetry"
                    </label>
                </div>

                <div style="max-height:180px;overflow-y:auto;font-family:monospace;font-size:10px">
                    {move || {
                        let ht = hide_telem.get();
                        raw_ws_log.get().into_iter().rev()
                            .filter(|e| !ht || !e.data.contains(r#""db":"#) || e.data.contains(r#""evt":"#))
                            .map(|e| {
                                let (dir_label, dir_color) = if e.direction == "rx" {
                                    ("RX", "#22c55e")
                                } else {
                                    ("TX", "#3b82f6")
                                };
                                let truncated = if e.data.len() > 120 {
                                    let mut end = 120;
                                    while end > 0 && !e.data.is_char_boundary(end) { end -= 1; }
                                    format!("{}...", &e.data[..end])
                                } else {
                                    e.data.clone()
                                };
                                view! {
                                    <div style="padding:2px 0;border-bottom:1px solid #0f172a;display:flex;gap:6px;align-items:baseline">
                                        <span style=format!("color:{};font-weight:700;flex-shrink:0", dir_color)>{dir_label}</span>
                                        <span style="color:#475569;flex-shrink:0;font-size:9px">{e.time.clone()}</span>
                                        <span style="color:#94a3b8;word-break:break-all">{truncated}</span>
                                    </div>
                                }
                            })
                            .collect::<Vec<_>>()
                    }}
                </div>
            </div>

            // ── Section D: Calibration Debug ────────────────────────────────
            <div style="background:#1e293b;border-radius:16px;padding:14px">
                <div style="font-size:14px;font-weight:600;color:#f1f5f9;margin-bottom:8px">"Calibration Debug"</div>
                <div style="font-family:monospace;font-size:12px;display:grid;grid-template-columns:auto 1fr 1fr;gap:4px 12px">
                    <span style="color:#94a3b8">"key"</span>
                    <span style="color:#94a3b8">"localStorage"</span>
                    <span style="color:#94a3b8">"live signal"</span>

                    <span style="color:#cbd5e1">"floor"</span>
                    <span style="color:#f1f5f9">{move || crate::local_get("cal_floor", "—")}</span>
                    <span style="color:#475569">"—"</span>

                    <span style="color:#cbd5e1">"tripwire"</span>
                    <span style="color:#f1f5f9">{move || crate::local_get("cal_tripwire", "—")}</span>
                    <span style="color:#f1f5f9">{move || format!("{:.1}", tripwire.get())}</span>

                    <span style="color:#cbd5e1">"tv_ip"</span>
                    <span style="color:#f1f5f9">{move || crate::local_get("tv_ip", "—")}</span>
                    <span style="color:#f1f5f9">{move || { let v = tv_ip.get(); if v.is_empty() { "—".to_string() } else { v } }}</span>

                    <span style="color:#cbd5e1">"tv_brand"</span>
                    <span style="color:#f1f5f9">{move || crate::local_get("tv_brand", "—")}</span>
                    <span style="color:#f1f5f9">{move || tv_brand.get()}</span>

                    <span style="color:#cbd5e1">"setup_done"</span>
                    <span style="color:#f1f5f9">{move || crate::local_get("setup_done", "—")}</span>
                    <span style="color:#475569">"—"</span>
                </div>
            </div>

            // ── Section E: Connection Stats ─────────────────────────────────
            <div style="background:#1e293b;border-radius:16px;padding:14px;margin-bottom:20px">
                <div style="font-size:14px;font-weight:600;color:#f1f5f9;margin-bottom:8px">"Connection Stats"</div>
                <div style="font-family:monospace;font-size:12px;display:flex;flex-direction:column;gap:4px">
                    <InfoRow label="Messages" value=move || format!("{}", msg_count.get()) />
                    <InfoRow label="Reconnects" value=move || format!("{}", reconnect_count.get()) />
                    <InfoRow label="Last msg" value=move || {
                        let t = last_msg_time.get();
                        if t.is_empty() { "—".to_string() } else { t }
                    } />
                    <InfoRow label="WS URL" value=move || {
                        let host = web_sys::window()
                            .and_then(|w| w.location().hostname().ok())
                            .unwrap_or_else(|| "?".to_string());
                        format!("ws://{}:81/ws", host)
                    } />
                </div>
            </div>
        </div>
    }
}

// ── Helper components ───────────────────────────────────────────────────────

#[component]
fn StateRow(
    label: &'static str,
    value: impl Fn() -> String + Send + 'static,
) -> impl IntoView {
    view! {
        <div style="display:flex;justify-content:space-between">
            <span style="color:#94a3b8">{label}</span>
            <span style="color:#f1f5f9">{move || value()}</span>
        </div>
    }
}

#[component]
fn InfoRow(
    label: &'static str,
    value: impl Fn() -> String + Send + 'static,
) -> impl IntoView {
    view! {
        <div style="display:flex;justify-content:space-between;font-size:12px">
            <span style="color:#94a3b8">{label}</span>
            <span style="color:#f1f5f9">{move || value()}</span>
        </div>
    }
}

fn bool_str(v: bool) -> String {
    if v { "true".to_string() } else { "false".to_string() }
}

fn tv_status_str(v: u8) -> String {
    match v {
        0 => "off".to_string(),
        1 => "connecting".to_string(),
        2 => "connected".to_string(),
        3 => "error".to_string(),
        _ => format!("unknown({})", v),
    }
}

fn ws_str(s: WsState) -> String {
    match s {
        WsState::Connecting   => "Connecting".to_string(),
        WsState::Connected    => "Connected".to_string(),
        WsState::Disconnected => "Disconnected".to_string(),
    }
}
