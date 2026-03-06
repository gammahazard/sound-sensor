//! wifi.rs — WiFi settings screen (Leptos)
//!
//! Shows current connection, scan button, network list, and credentials form.

use leptos::prelude::*;
use wasm_bindgen::JsCast;
use crate::ws::{WsState, NetworkInfo, rssi_bars};

// ── WiFi screen ─────────────────────────────────────────────────────────────

#[component]
pub fn WifiScreen(
    ws_state:       ReadSignal<WsState>,
    wifi_networks:  ReadSignal<Vec<NetworkInfo>>,
    on_scan:        impl Fn() + 'static,
    on_reconfigure: impl Fn(String, String) + 'static,
) -> impl IntoView {
    let on_scan        = StoredValue::new_local(on_scan);
    let on_reconfigure = StoredValue::new_local(on_reconfigure);

    let current_host = web_sys::window()
        .and_then(|w| w.location().hostname().ok())
        .unwrap_or_else(|| "guardian.local".to_string());

    let (result_msg, set_result)    = signal(String::new());
    let (result_ok,  set_result_ok) = signal(false);
    let (scanning, set_scanning)    = signal(false);
    let (confirming, set_confirming) = signal(false);

    // Clear scanning when results arrive (avoids setting signal in reactive closure)
    Effect::new(move || {
        let nets = wifi_networks.get();
        if !nets.is_empty() {
            set_scanning.set(false);
        }
    });

    view! {
        <div style="padding:16px;display:flex;flex-direction:column;gap:16px">

            <div style="text-align:center;margin-top:8px">
                <div style="font-size:22px;font-weight:700">"WiFi Settings"</div>
                <div style="color:#94a3b8;font-size:13px;margin-top:4px">
                    "Manage the Guardian device's WiFi connection."
                </div>
            </div>

            // ── Current connection ──────────────────────────────────────────
            <div style="background:#1e293b;border-radius:16px;padding:16px;display:flex;\
                        flex-direction:column;gap:10px">
                <div style="font-weight:700;margin-bottom:2px">"Current Connection"</div>
                <div style="display:flex;justify-content:space-between;align-items:center">
                    <span style="font-size:13px;color:#94a3b8">"Device host"</span>
                    <span style="font-size:13px;font-weight:600;color:#6366f1">
                        {current_host.clone()}
                    </span>
                </div>
                <div style="display:flex;justify-content:space-between;align-items:center">
                    <span style="font-size:13px;color:#94a3b8">"Status"</span>
                    <span style=move || format!(
                        "font-size:13px;font-weight:600;color:{}",
                        match ws_state.get() {
                            WsState::Connected    => "#22c55e",
                            WsState::Connecting   => "#eab308",
                            WsState::Disconnected => "#ef4444",
                        }
                    )>
                        {move || match ws_state.get() {
                            WsState::Connected    => "Connected",
                            WsState::Connecting   => "Connecting…",
                            WsState::Disconnected => "Disconnected",
                        }}
                    </span>
                </div>
            </div>

            // ── Scan button + network list ──────────────────────────────────
            <div style="background:#1e293b;border-radius:16px;padding:16px;display:flex;\
                        flex-direction:column;gap:12px">
                <div style="font-weight:700">"Available Networks"</div>
                <button
                    on:click=move |_| {
                        set_scanning.set(true);
                        on_scan.with_value(|f| f());
                        let set_s = set_scanning.clone();
                        wasm_bindgen_futures::spawn_local(async move {
                            gloo_timers::future::TimeoutFuture::new(8_000).await;
                            set_s.set(false);
                        });
                    }
                    disabled=move || scanning.get()
                    style=move || format!(
                        "width:100%;padding:10px;border-radius:12px;border:1px solid #475569;\
                         background:transparent;color:{};font-size:13px;\
                         font-weight:600;cursor:pointer",
                        if scanning.get() { "#475569" } else { "#93c5fd" }
                    )
                >
                    {move || if scanning.get() { "Scanning…" } else { "Scan WiFi Networks" }}
                </button>

                {move || {
                    let nets = wifi_networks.get();
                    if nets.is_empty() {
                        return ().into_any();
                    }
                    view! {
                        <div style="display:flex;flex-direction:column;gap:6px">
                            {nets.iter().map(|net| {
                                let ssid = net.ssid.clone();
                                let rssi = net.rssi;
                                let ssid2 = ssid.clone();
                                view! {
                                    <button
                                        on:click=move |_| {
                                            set_input_value("wifi-ssid", &ssid2);
                                        }
                                        style="text-align:left;width:100%;padding:8px 12px;\
                                               border-radius:8px;border:1px solid #334155;\
                                               background:#0f172a;color:#f1f5f9;\
                                               cursor:pointer;display:flex;\
                                               justify-content:space-between;align-items:center"
                                    >
                                        <span style="font-size:13px">{ssid}</span>
                                        <span style="font-size:11px;color:#475569;font-family:monospace">
                                            {rssi_bars(rssi)}
                                        </span>
                                    </button>
                                }
                            }).collect_view()}
                        </div>
                    }.into_any()
                }}
            </div>

            // ── Change network ──────────────────────────────────────────────
            <div style="background:#1e293b;border-radius:16px;padding:16px;display:flex;\
                        flex-direction:column;gap:14px">
                <div>
                    <div style="font-weight:700;margin-bottom:4px">"Change WiFi Network"</div>
                    <div style="font-size:12px;color:#94a3b8">
                        "Enter the new network credentials. Guardian will reconnect automatically."
                    </div>
                </div>

                <div style="display:flex;flex-direction:column;gap:6px">
                    <label style="font-size:12px;color:#94a3b8">"Network Name (SSID)"</label>
                    <input
                        id="wifi-ssid"
                        type="text"
                        placeholder="MyHomeNetwork"
                        autocomplete="off"
                        style="background:#0f172a;border:1px solid #334155;border-radius:10px;\
                               padding:10px 12px;color:#f1f5f9;font-size:16px;width:100%"
                    />
                </div>

                <div style="display:flex;flex-direction:column;gap:6px">
                    <label style="font-size:12px;color:#94a3b8">"Password"</label>
                    <input
                        id="wifi-pass"
                        type="password"
                        placeholder="WiFi password"
                        autocomplete="current-password"
                        style="background:#0f172a;border:1px solid #334155;border-radius:10px;\
                               padding:10px 12px;color:#f1f5f9;font-size:16px;width:100%"
                    />
                </div>

                {move || (!result_msg.get().is_empty()).then(|| view! {
                    <div style=move || format!(
                        "border-radius:10px;padding:10px;font-size:13px;font-weight:500;\
                         background:{};color:{}",
                        if result_ok.get() { "#14532d" } else { "#450a0a" },
                        if result_ok.get() { "#86efac" } else { "#fca5a5" },
                    )>
                        {move || result_msg.get()}
                    </div>
                })}

                // Inline confirmation warning (replaces browser confirm())
                {move || confirming.get().then(|| view! {
                    <div style="background:#451a03;border-radius:10px;padding:12px;\
                                display:flex;flex-direction:column;gap:8px">
                        <div style="font-size:13px;color:#fbbf24;font-weight:500">
                            "This will disconnect Guardian from the current network. Continue?"
                        </div>
                        <div style="display:flex;gap:8px">
                            <button
                                on:click=move |_| {
                                    set_confirming.set(false);
                                    let ssid = get_input_value("wifi-ssid");
                                    let pass = get_input_value("wifi-pass");
                                    set_result.set(
                                        format!("Reconnecting to \"{}\"… Guardian may be unreachable briefly.", ssid)
                                    );
                                    set_result_ok.set(true);
                                    on_reconfigure.with_value(|f| f(ssid, pass));
                                }
                                style="flex:1;padding:10px;border-radius:10px;border:none;\
                                       background:#f59e0b;color:#0f172a;font-size:14px;\
                                       font-weight:700;cursor:pointer"
                            >
                                "Confirm"
                            </button>
                            <button
                                on:click=move |_| set_confirming.set(false)
                                style="flex:1;padding:10px;border-radius:10px;\
                                       border:1px solid #475569;background:transparent;\
                                       color:#f1f5f9;font-size:14px;font-weight:600;\
                                       cursor:pointer"
                            >
                                "Cancel"
                            </button>
                        </div>
                    </div>
                })}

                <button
                    on:click=move |_| {
                        let ssid = get_input_value("wifi-ssid");
                        if ssid.is_empty() {
                            set_result.set("Enter the network name (SSID)".to_string());
                            set_result_ok.set(false);
                            return;
                        }
                        set_confirming.set(true);
                    }
                    style="width:100%;padding:14px;border-radius:12px;border:none;\
                           background:#6366f1;color:white;font-size:16px;\
                           font-weight:700;cursor:pointer"
                >
                    "Reconnect"
                </button>
            </div>

            // ── Note ────────────────────────────────────────────────────────
            <div style="background:#1e293b;border-radius:16px;padding:12px;\
                        font-size:12px;color:#94a3b8">
                <strong style="color:#f1f5f9">"Note: "</strong>
                "After reconnecting, Guardian will drop from this network. "
                "Open the app again once Guardian joins the new network."
            </div>

        </div>
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn get_input_value(id: &str) -> String {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|el| el.value().trim().to_string())
        .unwrap_or_default()
}

fn set_input_value(id: &str, val: &str) {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
    {
        el.set_value(val);
    }
}
