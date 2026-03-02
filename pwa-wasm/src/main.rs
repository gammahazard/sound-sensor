//! main.rs — Guardian PWA root (Leptos 0.7 + WASM)
//!
//! Tab layout:
//!   Meter     — live dB bar, arm/disarm, recent events
//!   Calibrate — two-step calibration + manual slider
//!   TV        — brand selection, IP entry, Sony PSK, connect/disconnect
//!   WiFi      — switch WiFi networks
//!   Info      — firmware/pwa versions, WS state, full event log

use leptos::*;
use wasm_bindgen::prelude::*;

mod calibration;
mod info;
mod meter;
mod tv;
mod wifi;
mod ws;

// ── Shared types ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct EventEntry {
    pub msg:  String,
    pub time: String,
}

// ── localStorage helpers ──────────────────────────────────────────────────────

fn local_get(key: &str, default: &str) -> String {
    web_sys::window()
        .and_then(|w| w.local_storage().ok())
        .flatten()
        .and_then(|s| s.get_item(&format!("guardian_{}", key)).ok())
        .flatten()
        .unwrap_or_else(|| default.to_string())
}

fn local_set(key: &str, val: &str) {
    if let Some(s) = web_sys::window()
        .and_then(|w| w.local_storage().ok())
        .flatten()
    {
        let _ = s.set_item(&format!("guardian_{}", key), val);
    }
}

fn now_hhmm() -> String {
    let d = js_sys::Date::new_0();
    format!("{:02}:{:02}", d.get_hours(), d.get_minutes())
}

// ── App root ──────────────────────────────────────────────────────────────────

#[component]
fn App() -> impl IntoView {
    // ── Reactive signals ──────────────────────────────────────────────────────
    let (db, set_db)               = create_signal(-60.0f32);
    let (armed, set_armed)         = create_signal(false);
    let (tripwire, set_tripwire)   = create_signal(-20.0f32);
    let (ws_state, set_ws_state)   = create_signal(ws::WsState::Connecting);
    let (active_tab, set_tab)      = create_signal("meter");
    let (fw_ver, set_fw_ver)       = create_signal(String::new());
    let (pwa_ver, set_pwa_ver)     = create_signal(String::new());
    let (msg_count, set_msg_count) = create_signal(0u32);
    let (events, set_events)       = create_signal(Vec::<EventEntry>::new());

    // TV settings — initialised from localStorage, persisted on connect/disconnect
    let (tv_ip,        set_tv_ip)        = create_signal(local_get("tv_ip",    ""));
    let (tv_brand,     set_tv_brand)     = create_signal(local_get("tv_brand", "lg"));
    let (tv_connected, set_tv_connected) = create_signal(!local_get("tv_ip", "").is_empty());

    // ── add_event: shared event log writer ────────────────────────────────────
    // StoredValue is Copy, so we can capture it in any number of closures.
    let add_event_sv = store_value({
        move |msg: String| {
            let entry = EventEntry { msg, time: now_hhmm() };
            set_events.update(|evts| {
                evts.insert(0, entry);
                if evts.len() > 30 { evts.truncate(30); }
            });
        }
    });

    // ── WebSocket ─────────────────────────────────────────────────────────────
    let send = ws::use_websocket(
        set_db, set_armed, set_tripwire, set_ws_state,
        set_fw_ver, set_pwa_ver, set_msg_count,
    );
    // StoredValue so we can call it from multiple tab closures without moving
    let send_sv = store_value(send);

    // ── View ──────────────────────────────────────────────────────────────────
    view! {
        <div id="app" style="height:100%;display:flex;flex-direction:column;overflow:hidden">

            // ── Connection banner ─────────────────────────────────────────────
            <ws::ConnectionBanner state=ws_state />

            // ── Active screen (scrollable) ────────────────────────────────────
            <div style="flex:1;overflow-y:auto">
                {move || match active_tab.get() {

                    "meter" => view! {
                        <meter::MeterScreen
                            db=db
                            armed=armed
                            tripwire=tripwire
                            events=events
                            on_arm_toggle=move || {
                                let next = !armed.get_untracked();
                                set_armed(next);
                                let cmd = if next { r#"{"cmd":"arm"}"# } else { r#"{"cmd":"disarm"}"# };
                                send_sv.get_value()(cmd.to_string());
                            }
                        />
                    }.into_view(),

                    "cal" => view! {
                        <calibration::CalibrationScreen
                            current_db=db
                            tripwire=tripwire
                            on_silence=move |db_val: f32| {
                                add_event_sv.with_value(|f| {
                                    f(format!("Quiet level: {:.1} dBFS", db_val))
                                });
                                send_sv.get_value()(
                                    format!(r#"{{"cmd":"calibrate_silence","db":{:.2}}}"#, db_val)
                                );
                            }
                            on_max=move |db_val: f32| {
                                let tw = db_val - 3.0;
                                set_tripwire(tw);
                                add_event_sv.with_value(|f| {
                                    f(format!("Calibrated — tripwire {:.1} dBFS", tw))
                                });
                                send_sv.get_value()(
                                    format!(r#"{{"cmd":"calibrate_max","db":{:.2}}}"#, db_val)
                                );
                            }
                            on_threshold=move |v: f32| {
                                set_tripwire(v);
                                add_event_sv.with_value(|f| {
                                    f(format!("Tripwire set: {:.0} dBFS", v))
                                });
                                send_sv.get_value()(
                                    format!(r#"{{"threshold":{:.1}}}"#, v)
                                );
                            }
                        />
                    }.into_view(),

                    "tv" => view! {
                        <tv::TvScreen
                            tv_ip=tv_ip
                            tv_brand=tv_brand
                            tv_connected=tv_connected
                            set_tv_ip=set_tv_ip
                            set_tv_brand=set_tv_brand
                            set_tv_connected=set_tv_connected
                            on_connect=move |ip: String, brand: String, psk: String| {
                                local_set("tv_ip",    &ip);
                                local_set("tv_brand", &brand);
                                local_set("tv_psk",   &psk);
                                add_event_sv.with_value(|f| {
                                    f(format!("TV: {} @ {}", brand.to_uppercase(), ip))
                                });
                                let mut cmd = format!(
                                    r#"{{"cmd":"set_tv","ip":"{}","brand":"{}""#, ip, brand
                                );
                                if !psk.is_empty() {
                                    cmd.push_str(&format!(r#","psk":"{}""#, psk));
                                }
                                cmd.push('}');
                                send_sv.get_value()(cmd);
                            }
                            on_disconnect=move || {
                                local_set("tv_ip",  "");
                                local_set("tv_psk", "");
                                add_event_sv.with_value(|f| f("TV disconnected".to_string()));
                            }
                        />
                    }.into_view(),

                    "wifi" => view! {
                        <wifi::WifiScreen
                            on_reconfigure=move |ssid: String, pass: String| {
                                add_event_sv.with_value(|f| {
                                    f(format!("WiFi change → \"{}\"", ssid))
                                });
                                send_sv.get_value()(
                                    format!(r#"{{"cmd":"set_wifi","ssid":"{}","pass":"{}"}}"#, ssid, pass)
                                );
                            }
                        />
                    }.into_view(),

                    _ => view! {  // "info" tab
                        <info::InfoScreen
                            ws_state=ws_state
                            fw_ver=fw_ver
                            pwa_ver=pwa_ver
                            msg_count=msg_count
                            events=events
                        />
                    }.into_view(),
                }}
            </div>

            // ── Bottom navigation bar ─────────────────────────────────────────
            <BottomNav active=active_tab on_switch=set_tab />

        </div>
    }
}

// ── Bottom nav ────────────────────────────────────────────────────────────────

#[component]
fn BottomNav(
    active:    ReadSignal<&'static str>,
    on_switch: WriteSignal<&'static str>,
) -> impl IntoView {
    let tab = move |name: &'static str, label: &'static str, icon: &'static str| {
        view! {
            <button
                on:click=move |_| on_switch(name)
                style=move || format!(
                    "flex:1;display:flex;flex-direction:column;align-items:center;\
                     padding:10px 0 calc(10px + env(safe-area-inset-bottom, 0px));\
                     gap:3px;font-size:10px;background:none;border:none;\
                     border-top:2px solid {};color:{};cursor:pointer",
                    if active.get() == name { "#6366f1" } else { "transparent" },
                    if active.get() == name { "#6366f1" } else { "#94a3b8" },
                )
            >
                <span style="font-size:20px">{icon}</span>
                {label}
            </button>
        }
    };

    view! {
        <nav style="background:#1e293b;border-top:1px solid #334155;display:flex;flex-shrink:0">
            {tab("meter",  "Meter",     "\u{1F3B5}")}
            {tab("cal",    "Calibrate", "\u{1F39A}")}
            {tab("tv",     "TV",        "\u{1F4FA}")}
            {tab("wifi",   "WiFi",      "\u{1F4F6}")}
            {tab("info",   "Info",      "\u{2699}")}
        </nav>
    }
}

// ── Entry ─────────────────────────────────────────────────────────────────────

#[wasm_bindgen(start)]
pub fn main() {
    console_error_panic_hook::set_once();
    console_log::init_with_level(log::Level::Debug).ok();

    // Register service worker
    if let Some(window) = web_sys::window() {
        let _ = window.navigator().service_worker().register("sw.js");
    }

    mount_to_body(|| view! { <App /> });
}
