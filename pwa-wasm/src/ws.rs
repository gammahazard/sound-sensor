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

// ── Server → Client message ───────────────────────────────────────────────────

#[derive(Deserialize)]
struct ServerMsg {
    #[serde(default)]
    db:       Option<f32>,
    #[serde(default)]
    armed:    Option<bool>,
    #[serde(default)]
    tripwire: Option<f32>,
    #[serde(default)]
    fw:       Option<String>,
    #[serde(default)]
    pwa:      Option<String>,
}

// ── Connection state ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
pub enum WsState {
    Connecting,
    Connected,
    Disconnected,
}

// ── Connection banner ─────────────────────────────────────────────────────────

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

// ── WebSocket hook ────────────────────────────────────────────────────────────

/// Spawns the WebSocket reconnect loop and drives all reactive signals.
/// Returns a `send_fn` closure the caller uses to send raw JSON strings.
pub fn use_websocket(
    set_db:        WriteSignal<f32>,
    set_armed:     WriteSignal<bool>,
    set_tripwire:  WriteSignal<f32>,
    set_ws_state:  WriteSignal<WsState>,
    set_fw_ver:    WriteSignal<String>,
    set_pwa_ver:   WriteSignal<String>,
    set_msg_count: WriteSignal<u32>,
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
                                            set_msg_count.update(|n| *n += 1);
                                            if let Some(v) = m.db       { set_db(v); }
                                            if let Some(v) = m.armed    { set_armed(v); }
                                            if let Some(v) = m.tripwire { set_tripwire(v); }
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
                                    use gloo_net::websocket::futures::WebSocketSink;
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
