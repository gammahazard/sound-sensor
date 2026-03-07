//! ws.rs — WebSocket client (Leptos / Wasm)
//!
//! Connects to ws://<host>:81/ws and drives reactive signals.
//! Auto-reconnects on close/error.

use leptos::prelude::*;
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
    crying:   Option<bool>,
    #[serde(default)]
    tv_status: Option<u8>,
    #[serde(default)]
    fw:       Option<String>,
    #[serde(default)]
    pwa:      Option<String>,
    #[serde(default)]
    evt:      Option<String>,
    #[serde(default)]
    networks: Option<Vec<NetworkInfo>>,
    #[serde(default)]
    tvs:      Option<Vec<DiscoveredTv>>,
    #[serde(default)]
    available: Option<bool>,
    #[serde(default)]
    latest:   Option<String>,
    #[serde(default)]
    current:  Option<String>,
    #[serde(default)]
    checking: Option<bool>,
    #[serde(default)]
    ssid:     Option<String>,
    #[serde(default)]
    error:    Option<bool>,
    // Dev mode fields
    #[serde(default)]
    dev:      Option<bool>,
    #[serde(default)]
    cat:      Option<String>,
    #[serde(default)]
    lvl:      Option<String>,
    #[serde(default)]
    msg:      Option<String>,
}

/// A structured log entry from the firmware's dev-mode logging system.
#[derive(Clone)]
pub struct DevLogEntry {
    pub cat:  String,
    pub lvl:  String,
    pub msg:  String,
    pub time: String,
}

/// A raw WebSocket message captured for the WS inspector.
#[derive(Clone)]
pub struct RawWsEntry {
    pub direction: &'static str, // "tx" or "rx"
    pub data:      String,
    pub time:      String,
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

#[derive(Clone, PartialEq)]
pub enum OtaStatus {
    Idle,
    Checking,
    Available { latest: String, current: String },
    Downloading,
    UpToDate { current: String },
    Done { pwa: String },
    Error,
}

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
                WsState::Connecting =>
                    "background:#92400e;color:#fef3c7;text-align:center;\
                     padding:8px;font-size:13px;font-weight:500".to_string(),
                WsState::Disconnected =>
                    "background:#7f1d1d;color:#fef3c7;text-align:center;\
                     padding:8px;font-size:13px;font-weight:500".to_string(),
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

pub fn rssi_bars(rssi: i16) -> &'static str {
    if rssi > -50 { "████" }
    else if rssi > -60 { "███░" }
    else if rssi > -70 { "██░░" }
    else { "█░░░" }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rssi_bars_excellent() {
        assert_eq!(rssi_bars(-40), "████");
    }

    #[test]
    fn rssi_bars_good() {
        assert_eq!(rssi_bars(-55), "███░");
    }

    #[test]
    fn rssi_bars_fair() {
        assert_eq!(rssi_bars(-65), "██░░");
    }

    #[test]
    fn rssi_bars_weak() {
        assert_eq!(rssi_bars(-80), "█░░░");
    }

    #[test]
    fn rssi_bars_boundary_50() {
        // Exactly -50 should be "███░" (not > -50)
        assert_eq!(rssi_bars(-50), "███░");
    }

    #[test]
    fn rssi_bars_boundary_60() {
        assert_eq!(rssi_bars(-60), "██░░");
    }

    #[test]
    fn rssi_bars_boundary_70() {
        assert_eq!(rssi_bars(-70), "█░░░");
    }
}

// ── WebSocket hook ──────────────────────────────────────────────────────────

pub struct WsSignals {
    pub set_db:              WriteSignal<f32>,
    pub set_armed:           WriteSignal<bool>,
    pub set_tripwire:        WriteSignal<f32>,
    pub set_ws_state:        WriteSignal<WsState>,
    pub set_fw_ver:          WriteSignal<String>,
    pub set_pwa_ver:         WriteSignal<String>,
    pub set_msg_count:       WriteSignal<u32>,
    pub set_ducking:         WriteSignal<bool>,
    pub set_crying:          WriteSignal<bool>,
    pub set_tv_status:       WriteSignal<u8>,
    pub set_wifi_networks:   WriteSignal<Vec<NetworkInfo>>,
    pub set_discovered_tvs:  WriteSignal<Vec<DiscoveredTv>>,
    pub set_ota_status:      WriteSignal<OtaStatus>,
    pub set_dev_mode:        WriteSignal<bool>,
    pub set_dev_logs:        WriteSignal<Vec<DevLogEntry>>,
    pub set_raw_ws_log:      WriteSignal<Vec<RawWsEntry>>,
    pub set_reconnect_count: WriteSignal<u32>,
    pub set_last_msg_time:   WriteSignal<String>,
}

pub fn use_websocket(signals: WsSignals) -> impl Fn(String) + Clone + 'static {
    let WsSignals {
        set_db, set_armed, set_tripwire, set_ws_state,
        set_fw_ver, set_pwa_ver, set_msg_count,
        set_ducking, set_crying, set_tv_status, set_wifi_networks, set_discovered_tvs, set_ota_status,
        set_dev_mode, set_dev_logs, set_raw_ws_log, set_reconnect_count, set_last_msg_time,
    } = signals;
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<String>();
    let tx_send = tx.clone();

    let mut first_connect = true;
    let mut backoff_ms = RECONNECT_MS;

    spawn_local(async move {
        loop {
            set_ws_state.set(WsState::Connecting);

            let host = web_sys::window()
                .and_then(|w| w.location().hostname().ok())
                .unwrap_or_else(|| "guardian.local".to_string());

            let url = format!("ws://{}:{}/ws", host, WS_PORT);

            match WebSocket::open(&url) {
                Err(_) => {
                    set_ws_state.set(WsState::Disconnected);
                    TimeoutFuture::new(backoff_ms).await;
                    backoff_ms = (backoff_ms * 2).min(30_000);
                    continue;
                }
                Ok(ws) => {
                    set_ws_state.set(WsState::Connected);
                    backoff_ms = RECONNECT_MS; // Reset backoff on successful connect
                    if !first_connect {
                        set_reconnect_count.update(|n| *n += 1);
                    }
                    first_connect = false;

                    let (mut write, read) = ws.split();

                    // Fuse both streams so futures::select! works
                    let mut read = read.fuse();
                    let mut rx_fused = (&mut rx).fuse();

                    loop {
                        futures::select! {
                            msg = read.next() => {
                                match msg {
                                    Some(Ok(Message::Text(text))) => {
                                        // Capture raw RX for dev inspector
                                        set_raw_ws_log.update(|log| {
                                            log.push(RawWsEntry {
                                                direction: "rx",
                                                data: text.clone(),
                                                time: crate::now_hhmmss(),
                                            });
                                            if log.len() > 50 { log.remove(0); }
                                        });
                                        set_last_msg_time.set(crate::now_hhmmss());

                                        if let Ok(m) = serde_json::from_str::<ServerMsg>(&text) {
                                            // Handle dev mode flag
                                            if let Some(v) = m.dev {
                                                set_dev_mode.set(v);
                                            }

                                            set_msg_count.update(|n| *n += 1);
                                            if let Some(evt) = &m.evt {
                                                handle_event(evt, &m,
                                                    set_wifi_networks, set_discovered_tvs,
                                                    set_ota_status, set_dev_logs, set_crying);
                                                continue;
                                            }
                                            if let Some(v) = m.db       { set_db.set(v); }
                                            if let Some(v) = m.armed    { set_armed.set(v); }
                                            if let Some(v) = m.tripwire { set_tripwire.set(v); }
                                            if let Some(v) = m.ducking    { set_ducking.set(v); }
                                            if let Some(v) = m.crying    { set_crying.set(v); }
                                            if let Some(v) = m.tv_status { set_tv_status.set(v); }
                                            if let Some(v) = m.fw        { set_fw_ver.set(v); }
                                            if let Some(v) = m.pwa       { set_pwa_ver.set(v); }
                                        }
                                    }
                                    Some(Ok(Message::Bytes(_))) => {}
                                    _ => break,
                                }
                            }
                            outgoing = rx_fused.next() => {
                                if let Some(msg) = outgoing {
                                    // Capture raw TX for dev inspector
                                    set_raw_ws_log.update(|log| {
                                        log.push(RawWsEntry {
                                            direction: "tx",
                                            data: msg.clone(),
                                            time: crate::now_hhmmss(),
                                        });
                                        if log.len() > 50 { log.remove(0); }
                                    });
                                    use futures::SinkExt;
                                    let _ = write.send(Message::Text(msg)).await;
                                }
                            }
                        }
                    }

                    set_ws_state.set(WsState::Disconnected);
                    TimeoutFuture::new(backoff_ms).await;
                    backoff_ms = (backoff_ms * 2).min(30_000);
                }
            }
        }
    });

    move |msg: String| {
        let _ = tx_send.unbounded_send(msg);
    }
}

fn handle_event(
    evt: &str,
    m: &ServerMsg,
    set_wifi_networks:  WriteSignal<Vec<NetworkInfo>>,
    set_discovered_tvs: WriteSignal<Vec<DiscoveredTv>>,
    set_ota_status:     WriteSignal<OtaStatus>,
    set_dev_logs:       WriteSignal<Vec<DevLogEntry>>,
    set_crying:         WriteSignal<bool>,
) {
    match evt {
        "baby_cry" => {
            set_crying.set(true);
        }
        "wifi_scan" => {
            if let Some(nets) = &m.networks {
                set_wifi_networks.set(nets.clone());
            }
        }
        "discovered" => {
            if let Some(tvs) = &m.tvs {
                set_discovered_tvs.set(tvs.clone());
            }
        }
        "ota_status" => {
            if m.error.unwrap_or(false) {
                set_ota_status.set(OtaStatus::Error);
            } else {
                let avail = m.available.unwrap_or(false);
                let latest = m.latest.clone().unwrap_or_default();
                let current = m.current.clone().unwrap_or_default();
                if avail {
                    set_ota_status.set(OtaStatus::Available { latest, current });
                } else {
                    set_ota_status.set(OtaStatus::UpToDate { current });
                }
            }
        }
        "ota_done" => {
            let pwa = m.pwa.clone().unwrap_or_default();
            set_ota_status.set(OtaStatus::Done { pwa });
        }
        "wifi_reconfiguring" => {
            // Firmware is about to reconfigure WiFi — connection will drop.
            // Log the SSID so the user knows what network is being switched to.
            if let Some(ssid) = &m.ssid {
                log::info!("WiFi reconfiguring → {}", ssid);
            }
        }
        "log" => {
            let entry = DevLogEntry {
                cat:  m.cat.clone().unwrap_or_default(),
                lvl:  m.lvl.clone().unwrap_or_else(|| "info".to_string()),
                msg:  m.msg.clone().unwrap_or_default(),
                time: crate::now_hhmmss(),
            };
            set_dev_logs.update(|logs| {
                logs.insert(0, entry);
                if logs.len() > 200 { logs.truncate(200); }
            });
        }
        _ => {}
    }
}
