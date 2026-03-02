//! ws.rs — WebSocket client (Leptos / Wasm)
//!
//! Connects to ws://<host>:81/ws and drives reactive signals.
//! Auto-reconnects on close/error.

use leptos::*;
use serde::Deserialize;
use gloo_net::websocket::{futures::WebSocket, Message};
use wasm_bindgen_futures::spawn_local;
use futures::StreamExt;
use gloo_timers::future::TimeoutFuture;

const WS_PORT:      u16 = 81;
const RECONNECT_MS: u32 = 3_000;

// ── Server → Client message ────────────────────────────────────────────────

#[derive(Deserialize)]
struct ServerMsg {
    #[serde(default)]
    db:       Option<f32>,
    #[serde(default)]
    armed:    Option<bool>,
    #[serde(default)]
    tripwire: Option<f32>,
    #[serde(default)]
    ducking:  Option<bool>,
    #[serde(default)]
    fw:       Option<String>,
    #[serde(default)]
    pwa:      Option<String>,
    // Event fields
    #[serde(default)]
    evt:      Option<String>,
    #[serde(default)]
    networks: Option<Vec<NetworkInfo>>,
    #[serde(default)]
    tvs:      Option<Vec<DiscoveredTv>>,
    // OTA fields
    #[serde(default)]
    available: Option<bool>,
    #[serde(default)]
    latest:   Option<String>,
    #[serde(default)]
    current:  Option<String>,
    #[serde(default)]
    checking: Option<bool>,
    // WiFi reconfiguring
    #[serde(default)]
    ssid:     Option<String>,
}

#[derive(Clone, Deserialize, Debug)]
pub struct NetworkInfo {
    pub ssid: String,
    pub rssi: i16,
}

#[derive(Clone, Deserialize, Debug)]
pub struct DiscoveredTv {
    pub ip:    String,
    pub name:  String,
    pub brand: String,
}

// ── OTA status ──────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq)]
pub enum OtaStatus {
    Idle,
    Checking,
    Available { latest: String, current: String },
    UpToDate { current: String },
    Done { pwa: String },
}

// ── Connection state ────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum WsState {
    Connecting,
    Connected,
    Disconnected,
}

// ── Connection banner ───────────────────────────────────────────────────────

#[component]
pub fn ConnectionBanner(state: ReadSignal<WsState>) -> impl IntoView {
    view! {
        <div style=move || {
            match state.get() {
                WsState::Connected => "display:none".to_string(),
                WsState::Connecting => format!(
                    "background:#92400e;color:#fef3c7;text-align:center;\
                     padding:8px;font-size:13px;font-weight:500"
                ),
                WsState::Disconnected => format!(
                    "background:#7f1d1d;color:#fef3c7;text-align:center;\
                     padding:8px;font-size:13px;font-weight:500"
                ),
            }
        }>
            {move || match state.get() {
                WsState::Connecting   => "Connecting to Guardian…",
                WsState::Disconnected => "Disconnected — retrying…",
                WsState::Connected    => "",
            }}
        </div>
    }
}

// ── RSSI helper ─────────────────────────────────────────────────────────────

pub fn rssi_bars(rssi: i16) -> &'static str {
    if rssi > -50 { "████" }
    else if rssi > -60 { "███░" }
    else if rssi > -70 { "██░░" }
    else { "█░░░" }
}

// ── WebSocket hook ──────────────────────────────────────────────────────────

pub fn use_websocket(
    set_db:               WriteSignal<f32>,
    set_armed:            WriteSignal<bool>,
    set_tripwire:         WriteSignal<f32>,
    set_ws_state:         WriteSignal<WsState>,
    set_fw_ver:           WriteSignal<String>,
    set_pwa_ver:          WriteSignal<String>,
    set_msg_count:        WriteSignal<u32>,
    set_ducking:          WriteSignal<bool>,
    set_wifi_networks:    WriteSignal<Vec<NetworkInfo>>,
    set_discovered_tvs:   WriteSignal<Vec<DiscoveredTv>>,
    set_ota_status:       WriteSignal<OtaStatus>,
) -> impl Fn(String) + Clone + 'static {
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<String>();
    let tx_send = tx.clone();

    spawn_local(async move {
        loop {
            set_ws_state(WsState::Connecting);

            let host = web_sys::window()
                .and_then(|w| w.location().hostname().ok())
                .unwrap_or_else(|| "guardian.local".to_string());

            let url = format!("ws://{}:{}/ws", host, WS_PORT);

            match WebSocket::open(&url) {
                Err(_) => {
                    set_ws_state(WsState::Disconnected);
                    TimeoutFuture::new(RECONNECT_MS).await;
                    continue;
                }
                Ok(ws) => {
                    set_ws_state(WsState::Connected);
                    let (mut write, mut read) = ws.split();

                    loop {
                        futures::select! {
                            msg = read.next() => {
                                match msg {
                                    Some(Ok(Message::Text(text))) => {
                                        if let Ok(m) = serde_json::from_str::<ServerMsg>(&text) {
                                            // Handle events
                                            if let Some(evt) = &m.evt {
                                                match evt.as_str() {
                                                    "wifi_scan" => {
                                                        if let Some(nets) = m.networks {
                                                            set_wifi_networks(nets);
                                                        }
                                                    }
                                                    "discovered" => {
                                                        if let Some(tvs) = m.tvs {
                                                            set_discovered_tvs(tvs);
                                                        }
                                                    }
                                                    "ota_status" => {
                                                        let avail = m.available.unwrap_or(false);
                                                        let latest = m.latest.clone().unwrap_or_default();
                                                        let current = m.current.clone().unwrap_or_default();
                                                        if avail {
                                                            set_ota_status(OtaStatus::Available { latest, current });
                                                        } else {
                                                            set_ota_status(OtaStatus::UpToDate { current });
                                                        }
                                                    }
                                                    "ota_done" => {
                                                        let pwa = m.pwa.clone().unwrap_or_default();
                                                        set_ota_status(OtaStatus::Done { pwa });
                                                    }
                                                    "wifi_reconfiguring" => {
                                                        // handled by the UI already
                                                    }
                                                    _ => {}
                                                }
                                                continue;
                                            }

                                            // Regular telemetry
                                            set_msg_count.update(|n| *n += 1);
                                            if let Some(v) = m.db       { set_db(v); }
                                            if let Some(v) = m.armed    { set_armed(v); }
                                            if let Some(v) = m.tripwire { set_tripwire(v); }
                                            if let Some(v) = m.ducking  { set_ducking(v); }
                                            if let Some(v) = m.fw       { set_fw_ver(v); }
                                            if let Some(v) = m.pwa      { set_pwa_ver(v); }
                                        }
                                    }
                                    Some(Ok(Message::Bytes(_))) => {}
                                    _ => break,
                                }
                            }
                            outgoing = rx.next() => {
                                if let Some(msg) = outgoing {
                                    use futures::SinkExt;
                                    let _ = write.send(Message::Text(msg)).await;
                                }
                            }
                        }
                    }

                    set_ws_state(WsState::Disconnected);
                    TimeoutFuture::new(RECONNECT_MS).await;
                }
            }
        }
    });

    move |msg: String| {
        let _ = tx_send.unbounded_send(msg);
    }
}
