//! info.rs — Info / status screen (Leptos)
//!
//! Shows firmware version, PWA version, WebSocket connection state,
//! message count, and the rolling event log.

use leptos::*;
use crate::EventEntry;
use crate::ws::WsState;

// ── Info screen ───────────────────────────────────────────────────────────────

#[component]
pub fn InfoScreen(
    ws_state:  ReadSignal<WsState>,
    fw_ver:    ReadSignal<String>,
    pwa_ver:   ReadSignal<String>,
    msg_count: ReadSignal<u32>,
    events:    ReadSignal<Vec<EventEntry>>,
) -> impl IntoView {
    let host = web_sys::window()
        .and_then(|w| w.location().hostname().ok())
        .unwrap_or_else(|| "guardian.local".to_string());

    view! {
        <div style="padding:16px;display:flex;flex-direction:column;gap:16px">

            <div style="text-align:center;margin-top:8px">
                <div style="font-size:22px;font-weight:700">"Info"</div>
                <div style="color:#94a3b8;font-size:13px;margin-top:4px">
                    "Connection status and system information."
                </div>
            </div>

            // ── Connection card ───────────────────────────────────────────────
            <div style="background:#1e293b;border-radius:16px;padding:16px;\
                        display:flex;flex-direction:column;gap:12px">
                <div style="font-weight:700">"Connection"</div>

                <InfoRow label="Status">
                    <span style=move || format!(
                        "font-weight:600;font-size:13px;padding:3px 10px;\
                         border-radius:999px;background:{};color:{}",
                        match ws_state.get() {
                            WsState::Connected    => "#14532d",
                            WsState::Connecting   => "#451a03",
                            WsState::Disconnected => "#450a0a",
                        },
                        match ws_state.get() {
                            WsState::Connected    => "#86efac",
                            WsState::Connecting   => "#fde68a",
                            WsState::Disconnected => "#fca5a5",
                        },
                    )>
                        {move || match ws_state.get() {
                            WsState::Connected    => "Connected",
                            WsState::Connecting   => "Connecting",
                            WsState::Disconnected => "Disconnected",
                        }}
                    </span>
                </InfoRow>

                <InfoRow label="Host">
                    <span style="font-size:13px;font-weight:600;color:#6366f1">{host}</span>
                </InfoRow>

                <InfoRow label="Messages">
                    <span style="font-size:13px;font-weight:600">
                        {move || msg_count.get().to_string()}
                    </span>
                </InfoRow>
            </div>

            // ── Versions card ─────────────────────────────────────────────────
            <div style="background:#1e293b;border-radius:16px;padding:16px;\
                        display:flex;flex-direction:column;gap:12px">
                <div style="font-weight:700">"Versions"</div>

                <InfoRow label="Firmware">
                    <span style="font-size:13px;font-weight:600">
                        {move || {
                            let v = fw_ver.get();
                            if v.is_empty() { "—".to_string() } else { v }
                        }}
                    </span>
                </InfoRow>

                <InfoRow label="PWA">
                    <span style="font-size:13px;font-weight:600">
                        {move || {
                            let v = pwa_ver.get();
                            if v.is_empty() { "—".to_string() } else { v }
                        }}
                    </span>
                </InfoRow>
            </div>

            // ── Event log ─────────────────────────────────────────────────────
            <div style="background:#1e293b;border-radius:16px;padding:16px;\
                        display:flex;flex-direction:column;gap:10px">
                <div style="font-weight:700">"Event Log"</div>
                {move || {
                    let evts = events.get();
                    if evts.is_empty() {
                        view! {
                            <div style="font-size:12px;color:#475569;text-align:center;padding:8px 0">
                                "No events yet."
                            </div>
                        }.into_view()
                    } else {
                        evts.iter().map(|e| {
                            let msg  = e.msg.clone();
                            let time = e.time.clone();
                            view! {
                                <div style="display:flex;justify-content:space-between;\
                                            align-items:center;font-size:13px;padding:6px 0;\
                                            border-bottom:1px solid #334155">
                                    <span>{msg}</span>
                                    <span style="font-size:11px;color:#475569;flex-shrink:0;\
                                                 margin-left:8px">{time}</span>
                                </div>
                            }
                        }).collect_view().into_view()
                    }
                }}
            </div>

        </div>
    }
}

// ── Helper: labelled row ──────────────────────────────────────────────────────

#[component]
fn InfoRow(label: &'static str, children: Children) -> impl IntoView {
    view! {
        <div style="display:flex;justify-content:space-between;align-items:center">
            <span style="font-size:13px;color:#94a3b8">{label}</span>
            {children()}
        </div>
    }
}
