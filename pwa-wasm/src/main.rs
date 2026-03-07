//! main.rs — Guardian PWA root (Leptos 0.7 + WASM)
//!
//! Tab layout:
//!   Meter     — live dB bar, arm/disarm, recent events
//!   Calibrate — two-step calibration + manual slider
//!   TV        — brand selection, IP entry, Sony PSK, connect/disconnect
//!   WiFi      — switch WiFi networks
//!   Info      — firmware/pwa versions, WS state, full event log

use leptos::prelude::*;
use leptos::mount::mount_to_body;
use wasm_bindgen::prelude::*;

mod calibration;
mod dev;
mod info;
mod meter;
mod setup;
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

pub(crate) fn local_get(key: &str, default: &str) -> String {
    web_sys::window()
        .and_then(|w| w.local_storage().ok())
        .flatten()
        .and_then(|s| s.get_item(&format!("guardian_{}", key)).ok())
        .flatten()
        .unwrap_or_else(|| default.to_string())
}

pub(crate) fn local_set(key: &str, val: &str) {
    if let Some(s) = web_sys::window()
        .and_then(|w| w.local_storage().ok())
        .flatten()
    {
        let _ = s.set_item(&format!("guardian_{}", key), val);
    }
}

fn now_hhmmss() -> String {
    let d = js_sys::Date::new_0();
    format!("{:02}:{:02}:{:02}", d.get_hours(), d.get_minutes(), d.get_seconds())
}

/// Escape a string for safe JSON embedding.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"'  => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                // Control chars → \u00XX
                let _ = core::fmt::Write::write_fmt(
                    &mut out,
                    format_args!("\\u{:04x}", c as u32),
                );
            }
            c => out.push(c),
        }
    }
    out
}

/// Trigger a short haptic pulse (Android Chrome). No-ops silently on iOS.
pub fn haptic() {
    use wasm_bindgen::JsCast;
    if let Some(window) = web_sys::window() {
        let nav = window.navigator();
        if let Ok(vibrate_fn) = js_sys::Reflect::get(&nav, &"vibrate".into()) {
            if let Ok(f) = vibrate_fn.dyn_into::<js_sys::Function>() {
                let _ = f.call1(&nav, &50.into());
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_escape_clean() {
        assert_eq!(json_escape("hello"), "hello");
    }

    #[test]
    fn json_escape_quotes() {
        assert_eq!(json_escape(r#"say "hi""#), r#"say \"hi\""#);
    }

    #[test]
    fn json_escape_backslash() {
        assert_eq!(json_escape(r"path\to"), r"path\\to");
    }

    #[test]
    fn json_escape_both() {
        assert_eq!(json_escape(r#"a\"b"#), r#"a\\\"b"#);
    }

    #[test]
    fn json_escape_empty() {
        assert_eq!(json_escape(""), "");
    }

    #[test]
    fn json_escape_newline() {
        assert_eq!(json_escape("a\nb"), "a\\nb");
    }

    #[test]
    fn json_escape_tab_and_cr() {
        assert_eq!(json_escape("a\tb\rc"), "a\\tb\\rc");
    }

    #[test]
    fn json_escape_control_char() {
        // \x01 → \u0001
        assert_eq!(json_escape("x\x01y"), "x\\u0001y");
    }
}

// ── App root ──────────────────────────────────────────────────────────────────

#[component]
fn App() -> impl IntoView {
    // ── Reactive signals ──────────────────────────────────────────────────────
    let (db, set_db)               = signal(-60.0f32);
    let (armed, set_armed)         = signal(false);
    let (tripwire, set_tripwire)   = signal(-20.0f32);
    let (ducking, set_ducking)     = signal(false);
    let (crying, set_crying)       = signal(false);
    let (ws_state, set_ws_state)   = signal(ws::WsState::Connecting);
    let (active_tab, set_tab)      = signal("meter");
    let (fw_ver, set_fw_ver)       = signal(String::new());
    let (pwa_ver, set_pwa_ver)     = signal(String::new());
    let (msg_count, set_msg_count) = signal(0u32);
    let (events, set_events)       = signal(Vec::<EventEntry>::new());

    // TV settings
    let (tv_ip,        set_tv_ip)        = signal(local_get("tv_ip",    ""));
    let (tv_brand,     set_tv_brand)     = signal(local_get("tv_brand", "lg"));
    let (tv_status, set_tv_status) = signal(
        if local_get("tv_ip", "").is_empty() { 0u8 } else { 1u8 }
    );

    // WiFi networks from scan
    let (wifi_networks, set_wifi_networks) = signal(Vec::<ws::NetworkInfo>::new());

    // Discovered TVs from SSDP
    let (discovered_tvs, set_discovered_tvs) = signal(Vec::<ws::DiscoveredTv>::new());

    // OTA status
    let (ota_status, set_ota_status) = signal(ws::OtaStatus::Idle);

    // Dev mode signals
    let (dev_mode, set_dev_mode)             = signal(false);
    let (dev_logs, set_dev_logs)             = signal(Vec::<ws::DevLogEntry>::new());
    let (raw_ws_log, set_raw_ws_log)         = signal(Vec::<ws::RawWsEntry>::new());
    let (reconnect_count, set_reconnect_count) = signal(0u32);
    let (last_msg_time, set_last_msg_time)   = signal(String::new());

    // ── First-time setup wizard ──────────────────────────────────────────
    let setup_already_done = local_get("setup_done", "") == "true";
    let (show_wizard, set_show_wizard) = signal(!setup_already_done);
    let (wizard_step, set_wizard_step) = signal(0u8); // 0=Welcome,1=Cal,2=Tv,3=Done

    // ── add_event ───────────────────────────────────────────────────────────
    let add_event_sv = StoredValue::new_local({
        move |msg: String| {
            let entry = EventEntry { msg, time: now_hhmmss() };
            set_events.update(|evts| {
                evts.insert(0, entry);
                if evts.len() > 30 { evts.truncate(30); }
            });
        }
    });

    // ── Ducking state change → auto-generate events ─────────────────────────
    let prev_ducking = StoredValue::new(false);
    Effect::new(move || {
        let d = ducking.get();
        let prev = prev_ducking.get_value();
        if d && !prev {
            add_event_sv.with_value(|f| f("Volume ducked".to_string()));
        } else if !d && prev {
            add_event_sv.with_value(|f| f("Volume restored".to_string()));
        }
        *prev_ducking.write_value() = d;
    });

    // ── Crying state change → auto-generate events ────────────────────────
    let prev_crying = StoredValue::new(false);
    Effect::new(move || {
        let c = crying.get();
        let prev = prev_crying.get_value();
        if c && !prev {
            add_event_sv.with_value(|f| f("Baby crying detected".to_string()));
        } else if !c && prev {
            add_event_sv.with_value(|f| f("Crying stopped".to_string()));
        }
        *prev_crying.write_value() = c;
    });

    // ── WebSocket ───────────────────────────────────────────────────────────
    let send = ws::use_websocket(ws::WsSignals {
        set_db, set_armed, set_tripwire, set_ws_state,
        set_fw_ver, set_pwa_ver, set_msg_count,
        set_ducking, set_crying, set_tv_status, set_wifi_networks, set_discovered_tvs, set_ota_status,
        set_dev_mode, set_dev_logs, set_raw_ws_log, set_reconnect_count, set_last_msg_time,
    });
    let send_sv = StoredValue::new_local(send);

    // ── View ────────────────────────────────────────────────────────────────
    view! {
        <div id="app" style="height:100%;display:flex;flex-direction:column;overflow:hidden">

            // ── First-time setup wizard ──────────────────────────────────
            {move || show_wizard.get().then(|| {
                let step = wizard_step.get_untracked();
                view! {
                    <setup::SetupWizard
                        initial_step=step
                        on_dismiss=move || set_show_wizard.set(false)
                        on_tab=move |tab: &'static str, next_step: u8| {
                            set_wizard_step.set(next_step);
                            set_show_wizard.set(false);
                            set_tab.set(tab);
                        }
                    />
                }
            })}

            // ── Continue Setup banner (shown when wizard is hidden but not complete) ──
            {move || {
                let not_done = local_get("setup_done", "") != "true";
                (!show_wizard.get() && not_done).then(|| view! {
                    <button
                        on:click=move |_| set_show_wizard.set(true)
                        style="width:100%;padding:10px;background:#312e81;color:#c7d2fe;\
                               border:none;font-size:13px;font-weight:600;cursor:pointer;\
                               text-align:center"
                    >
                        "Continue Setup"
                    </button>
                })
            }}

            // ── Connection banner ─────────────────────────────────────────
            <ws::ConnectionBanner state=ws_state />

            // ── Active screen (scrollable) ────────────────────────────────
            <div style="flex:1;overflow-y:auto">
                {move || match active_tab.get() {

                    "meter" => view! {
                        <meter::MeterScreen
                            db=db
                            armed=armed
                            tripwire=tripwire
                            ducking=ducking
                            crying=crying
                            events=events
                            on_arm_toggle=move || {
                                haptic();
                                // Debounce: ignore toggles within 1s of last
                                let now = js_sys::Date::now();
                                static LAST_TOGGLE: std::sync::atomic::AtomicU64 =
                                    std::sync::atomic::AtomicU64::new(0);
                                let last = LAST_TOGGLE.load(std::sync::atomic::Ordering::Relaxed);
                                if now - (last as f64) < 1000.0 { return; }
                                LAST_TOGGLE.store(now as u64, std::sync::atomic::Ordering::Relaxed);
                                let next = !armed.get_untracked();
                                set_armed.set(next);
                                let cmd = if next { r#"{"cmd":"arm"}"# } else { r#"{"cmd":"disarm"}"# };
                                send_sv.with_value(|f| f(cmd.to_string()));
                            }
                        />
                    }.into_any(),

                    "cal" => view! {
                        <calibration::CalibrationScreen
                            current_db=db
                            tripwire=tripwire
                            on_silence=move |db_val: f32| {
                                add_event_sv.with_value(|f| {
                                    f(format!("Quiet level: {:.1} dBFS", db_val))
                                });
                                send_sv.with_value(|f| f(
                                    format!(r#"{{"cmd":"calibrate_silence","db":{:.2}}}"#, db_val)
                                ));
                            }
                            on_max=move |db_val: f32| {
                                let tw = db_val - 3.0;
                                set_tripwire.set(tw);
                                add_event_sv.with_value(|f| {
                                    f(format!("Calibrated — tripwire {:.1} dBFS", tw))
                                });
                                send_sv.with_value(|f| f(
                                    format!(r#"{{"cmd":"calibrate_max","db":{:.2}}}"#, db_val)
                                ));
                            }
                            on_threshold=move |v: f32| {
                                set_tripwire.set(v);
                                add_event_sv.with_value(|f| {
                                    f(format!("Tripwire set: {:.0} dBFS", v))
                                });
                                send_sv.with_value(|f| f(
                                    format!(r#"{{"cmd":"threshold","threshold":{:.1}}}"#, v)
                                ));
                            }
                        />
                    }.into_any(),

                    "tv" => view! {
                        <tv::TvScreen
                            tv_ip=tv_ip
                            tv_brand=tv_brand
                            tv_status=tv_status
                            set_tv_ip=set_tv_ip
                            set_tv_brand=set_tv_brand
                            set_tv_status=set_tv_status
                            discovered_tvs=discovered_tvs
                            on_connect=move |ip: String, brand: String, psk: String| {
                                local_set("tv_ip",    &ip);
                                local_set("tv_brand", &brand);
                                local_set("tv_psk",   &psk);
                                set_tv_status.set(1); // Optimistic "Connecting..."
                                add_event_sv.with_value(|f| {
                                    f(format!("TV: {} @ {}", brand.to_uppercase(), ip))
                                });
                                let mut cmd = format!(
                                    r#"{{"cmd":"set_tv","ip":"{}","brand":"{}""#, json_escape(&ip), json_escape(&brand)
                                );
                                if !psk.is_empty() {
                                    cmd.push_str(&format!(r#","psk":"{}""#, json_escape(&psk)));
                                }
                                cmd.push('}');
                                send_sv.with_value(|f| f(cmd));
                            }
                            on_disconnect=move || {
                                local_set("tv_ip",    "");
                                local_set("tv_psk",   "");
                                local_set("tv_brand", "lg");
                                set_tv_brand.set("lg".to_string());
                                set_tv_status.set(0);
                                set_discovered_tvs.set(Vec::new());
                                add_event_sv.with_value(|f| f("TV disconnected".to_string()));
                                // Send clear command to firmware
                                send_sv.with_value(|f| f(r#"{"cmd":"set_tv","ip":"","brand":"lg"}"#.to_string()));
                            }
                            on_discover=move || {
                                send_sv.with_value(|f| f(r#"{"cmd":"discover_tvs"}"#.to_string()));
                            }
                            on_vol_test=move |dir: &str| {
                                let cmd = if dir == "up" { r#"{"cmd":"vol_up"}"# } else { r#"{"cmd":"vol_down"}"# };
                                send_sv.with_value(|f| f(cmd.to_string()));
                            }
                        />
                    }.into_any(),

                    "wifi" => view! {
                        <wifi::WifiScreen
                            ws_state=ws_state
                            wifi_networks=wifi_networks
                            on_scan=move || {
                                send_sv.with_value(|f| f(r#"{"cmd":"scan_wifi"}"#.to_string()));
                            }
                            on_reconfigure=move |ssid: String, pass: String| {
                                add_event_sv.with_value(|f| {
                                    f(format!("WiFi change → \"{}\"", ssid))
                                });
                                send_sv.with_value(|f| f(
                                    format!(r#"{{"cmd":"set_wifi","ssid":"{}","pass":"{}"}}"#, json_escape(&ssid), json_escape(&pass))
                                ));
                            }
                        />
                    }.into_any(),

                    "dev" => view! {
                        <dev::DevScreen
                            db=db
                            armed=armed
                            tripwire=tripwire
                            ducking=ducking
                            ws_state=ws_state
                            fw_ver=fw_ver
                            pwa_ver=pwa_ver
                            tv_ip=tv_ip
                            tv_brand=tv_brand
                            tv_status=tv_status
                            msg_count=msg_count
                            reconnect_count=reconnect_count
                            last_msg_time=last_msg_time
                            dev_logs=dev_logs
                            raw_ws_log=raw_ws_log
                            on_toggle_logging=move || {
                                send_sv.with_value(|f| f(r#"{"cmd":"dev_toggle"}"#.to_string()));
                            }
                            on_clear_logs=move || {
                                set_dev_logs.set(Vec::new());
                            }
                        />
                    }.into_any(),

                    _ => view! {  // "info" tab
                        <info::InfoScreen
                            ws_state=ws_state
                            fw_ver=fw_ver
                            pwa_ver=pwa_ver
                            msg_count=msg_count
                            events=events
                            ota_status=ota_status
                            on_ota_check=move || {
                                set_ota_status.set(ws::OtaStatus::Checking);
                                send_sv.with_value(|f| f(r#"{"cmd":"ota_check"}"#.to_string()));
                                // 15s timeout to reset Checking state
                                let set_ota = set_ota_status.clone();
                                wasm_bindgen_futures::spawn_local(async move {
                                    gloo_timers::future::TimeoutFuture::new(15_000).await;
                                    set_ota.update(|s| {
                                        if *s == ws::OtaStatus::Checking {
                                            *s = ws::OtaStatus::Idle;
                                        }
                                    });
                                });
                            }
                            on_ota_download=move || {
                                set_ota_status.set(ws::OtaStatus::Downloading);
                                send_sv.with_value(|f| f(r#"{"cmd":"ota_download"}"#.to_string()));
                                add_event_sv.with_value(|f| f("OTA download started".to_string()));
                                // 120s timeout — show error if still downloading
                                let set_ota = set_ota_status.clone();
                                let add_evt = add_event_sv.clone();
                                wasm_bindgen_futures::spawn_local(async move {
                                    gloo_timers::future::TimeoutFuture::new(120_000).await;
                                    set_ota.update(|s| {
                                        if *s == ws::OtaStatus::Downloading {
                                            *s = ws::OtaStatus::Error;
                                            add_evt.with_value(|f| f("OTA download timed out".to_string()));
                                        }
                                    });
                                });
                            }
                        />
                    }.into_any(),
                }}
            </div>

            // ── Bottom navigation bar ─────────────────────────────────────
            <BottomNav active=active_tab on_switch=set_tab dev_mode=dev_mode />

        </div>
    }
}

// ── Bottom nav ──────────────────────────────────────────────────────────────

#[component]
fn BottomNav(
    active:    ReadSignal<&'static str>,
    on_switch: WriteSignal<&'static str>,
    dev_mode:  ReadSignal<bool>,
) -> impl IntoView {
    let tab = move |name: &'static str, label: &'static str, icon: &'static str| {
        view! {
            <button
                on:click=move |_| on_switch.set(name)
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
            {move || dev_mode.get().then(|| tab("dev", "Dev", "\u{1F41E}"))}
        </nav>
    }
}

// ── Entry ───────────────────────────────────────────────────────────────────

#[wasm_bindgen(start)]
pub fn main() {
    console_error_panic_hook::set_once();
    console_log::init_with_level(log::Level::Debug).ok();

    if let Some(window) = web_sys::window() {
        // Service workers require a secure context (HTTPS or localhost).
        // On plain HTTP (e.g. guardian.local), navigator.serviceWorker is undefined.
        let nav = window.navigator();
        let has_sw = js_sys::Reflect::get(nav.as_ref(), &wasm_bindgen::JsValue::from_str("serviceWorker"))
            .map(|v| !v.is_undefined())
            .unwrap_or(false);
        if has_sw {
            let _ = nav.service_worker().register("sw.js");
        }
    }

    mount_to_body(|| view! { <App /> });

    // Remove loading spinner now that WASM has mounted
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("loading"))
    {
        el.remove();
    }
}
