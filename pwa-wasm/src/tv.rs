//! tv.rs — TV setup screen (Leptos)
//!
//! Brand selection → IP entry → optional Sony PSK → Connect.
//! Persists config to localStorage (handled by parent via on_connect/on_disconnect).
//! Shows a "connected" card when a TV is configured, with a Change TV button.

use leptos::*;
use wasm_bindgen::JsCast;

const BRANDS: &[(&str, &str)] = &[
    ("lg",      "LG WebOS"),
    ("samsung", "Samsung"),
    ("sony",    "Sony Bravia"),
    ("roku",    "Roku"),
];

// ── TV screen ─────────────────────────────────────────────────────────────────

#[component]
pub fn TvScreen(
    // Read signals (initialized by parent from localStorage)
    tv_ip:        ReadSignal<String>,
    tv_brand:     ReadSignal<String>,
    tv_connected: ReadSignal<bool>,
    // Callbacks: parent sends WS command + saves to localStorage
    on_connect:    impl Fn(String, String, String) + 'static,  // (ip, brand, psk)
    on_disconnect: impl Fn() + 'static,
    // Write signals so this component can update parent state
    set_tv_ip:        WriteSignal<String>,
    set_tv_brand:     WriteSignal<String>,
    set_tv_connected: WriteSignal<bool>,
) -> impl IntoView {
    let on_connect    = store_value(on_connect);
    let on_disconnect = store_value(on_disconnect);

    // Local signals for the form (not committed until Connect is tapped)
    let (brand_sel, set_brand_sel) = create_signal(tv_brand.get_untracked());
    let (result_msg, set_result)   = create_signal(String::new());
    let (result_ok,  set_result_ok) = create_signal(false);

    // ── Connected card ────────────────────────────────────────────────────────
    let connected_card = move || {
        let brand_label = BRANDS.iter()
            .find(|(k, _)| *k == tv_brand.get().as_str())
            .map(|(_, v)| *v)
            .unwrap_or("TV");

        view! {
            <div style="background:#1e293b;border-radius:16px;padding:16px">
                <div style="display:flex;justify-content:space-between;align-items:flex-start">
                    <div>
                        <div style="font-size:11px;color:#94a3b8;margin-bottom:4px">"CONNECTED TV"</div>
                        <div style="font-weight:700;font-size:16px">{brand_label}</div>
                        <div style="font-size:13px;color:#94a3b8;margin-top:4px">{move || tv_ip.get()}</div>
                    </div>
                    <div style="background:#16a34a;border-radius:999px;padding:4px 10px;\
                                font-size:11px;font-weight:600">"Connected"</div>
                </div>
                <button
                    on:click=move |_| {
                        set_tv_ip(String::new());
                        set_tv_connected(false);
                        set_result(String::new());
                        on_disconnect.get_value()();
                    }
                    style="margin-top:14px;width:100%;padding:10px;border-radius:12px;\
                           border:1px solid #475569;background:transparent;color:#f1f5f9;\
                           font-size:14px;font-weight:600;cursor:pointer"
                >
                    "Change TV"
                </button>
            </div>
        }
    };

    // ── Setup card ────────────────────────────────────────────────────────────
    let setup_card = move || {
        // Brand buttons
        let brand_btns = BRANDS.iter().map(|(key, label)| {
            let key   = *key;
            let label = *label;
            view! {
                <button
                    on:click=move |_| set_brand_sel(key.to_string())
                    style=move || format!(
                        "flex:1;padding:10px 4px;border-radius:10px;border:2px solid {};\
                         background:{};color:{};font-size:12px;font-weight:600;\
                         cursor:pointer;transition:all 0.15s",
                        if brand_sel.get() == key { "#6366f1" } else { "#334155" },
                        if brand_sel.get() == key { "#312e81" } else { "transparent" },
                        if brand_sel.get() == key { "#c7d2fe" } else { "#94a3b8" },
                    )
                >
                    {label}
                </button>
            }
        }).collect_view();

        view! {
            <div style="background:#1e293b;border-radius:16px;padding:16px;display:flex;\
                        flex-direction:column;gap:14px">
                <div>
                    <div style="font-weight:700;font-size:16px;margin-bottom:4px">"TV Setup"</div>
                    <div style="font-size:12px;color:#94a3b8">
                        "Select your TV brand and enter its local IP address."
                    </div>
                </div>

                // Brand buttons
                <div style="display:flex;gap:8px">{brand_btns}</div>

                // IP input
                <div style="display:flex;flex-direction:column;gap:6px">
                    <label style="font-size:12px;color:#94a3b8">"TV IP Address"</label>
                    <input
                        id="tv-ip-input"
                        type="text"
                        inputmode="decimal"
                        placeholder="192.168.1.x"
                        style="background:#0f172a;border:1px solid #334155;border-radius:10px;\
                               padding:10px 12px;color:#f1f5f9;font-size:15px;width:100%"
                    />
                    <span style="font-size:11px;color:#475569">
                        "Find in: Router admin page → DHCP clients"
                    </span>
                </div>

                // Sony PSK field (shown only when Sony is selected)
                {move || (brand_sel.get() == "sony").then(|| view! {
                    <div style="display:flex;flex-direction:column;gap:6px">
                        <label style="font-size:12px;color:#94a3b8">"Sony Pre-Shared Key"</label>
                        <input
                            id="tv-psk-input"
                            type="text"
                            inputmode="numeric"
                            placeholder="e.g. 1234"
                            maxlength="8"
                            style="background:#0f172a;border:1px solid #334155;border-radius:10px;\
                                   padding:10px 12px;color:#f1f5f9;font-size:15px;width:100%"
                        />
                        <span style="font-size:11px;color:#475569">
                            "On TV: Settings → Network → Home Network Setup → IP Control → Pre-Shared Key"
                        </span>
                    </div>
                })}

                // Pairing note
                {move || {
                    let note = match brand_sel.get().as_str() {
                        "samsung" => Some("Samsung: TV will show a pairing popup on first connect. Tap OK."),
                        "lg"      => Some("LG: TV will show a pairing popup on first connect. Tap OK."),
                        "sony"    => Some("Sony: Set a PIN in TV Settings → Network → IP Control first."),
                        "roku"    => Some("Roku: No pairing required."),
                        _         => None,
                    };
                    note.map(|msg| view! {
                        <div style="background:#1e3a5f;border-radius:10px;padding:10px;\
                                    font-size:12px;color:#93c5fd">
                            {msg}
                        </div>
                    })
                }}

                // Result message
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

                // Connect button
                <button
                    on:click=move |_| {
                        let ip = get_input_value("tv-ip-input");
                        let brand = brand_sel.get_untracked();
                        let psk = if brand == "sony" {
                            get_input_value("tv-psk-input")
                        } else {
                            String::new()
                        };

                        if ip.is_empty() {
                            set_result("Enter the TV's IP address".to_string());
                            set_result_ok(false);
                            return;
                        }
                        if !is_valid_ip(&ip) {
                            set_result("Use format 192.168.1.x".to_string());
                            set_result_ok(false);
                            return;
                        }
                        if brand == "sony" && psk.is_empty() {
                            set_result("Sony requires a Pre-Shared Key PIN".to_string());
                            set_result_ok(false);
                            return;
                        }

                        let msg = if brand == "samsung" {
                            format!("Connecting to {}… (approve on TV screen)", ip)
                        } else {
                            format!("Connecting to {}…", ip)
                        };
                        set_result(msg);
                        set_result_ok(true);

                        set_tv_ip(ip.clone());
                        set_tv_brand(brand.clone());
                        set_tv_connected(true);
                        on_connect.get_value()(ip, brand, psk);
                    }
                    style="width:100%;padding:14px;border-radius:12px;border:none;\
                           background:#6366f1;color:white;font-size:16px;\
                           font-weight:700;cursor:pointer"
                >
                    "Connect TV"
                </button>
            </div>
        }
    };

    view! {
        <div style="padding:16px;display:flex;flex-direction:column;gap:16px">
            <div style="text-align:center;margin-top:8px">
                <div style="font-size:22px;font-weight:700">"TV Control"</div>
                <div style="color:#94a3b8;font-size:13px;margin-top:4px">
                    "Connect to your TV to enable automatic volume ducking."
                </div>
            </div>

            {move || if tv_connected.get() {
                connected_card().into_view()
            } else {
                setup_card().into_view()
            }}
        </div>
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn get_input_value(id: &str) -> String {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|el| el.value().trim().to_string())
        .unwrap_or_default()
}

fn is_valid_ip(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| p.parse::<u8>().is_ok())
}
