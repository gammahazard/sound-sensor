//! tv.rs — TV setup screen (Leptos)
//!
//! Brand selection → IP entry → optional Sony PSK → Connect.
//! SSDP discover button shows discovered TVs with tap-to-autofill.

use leptos::prelude::*;
use wasm_bindgen::JsCast;
use crate::ws::DiscoveredTv;

const BRANDS: &[(&str, &str)] = &[
    ("lg",      "LG WebOS"),
    ("samsung", "Samsung"),
    ("sony",    "Sony Bravia"),
    ("roku",    "Roku"),
];

// ── TV screen ───────────────────────────────────────────────────────────────

#[component]
pub fn TvScreen(
    tv_ip:          ReadSignal<String>,
    tv_brand:       ReadSignal<String>,
    tv_connected:   ReadSignal<bool>,
    on_connect:     impl Fn(String, String, String) + 'static,
    on_disconnect:  impl Fn() + 'static,
    set_tv_ip:      WriteSignal<String>,
    set_tv_brand:   WriteSignal<String>,
    set_tv_connected: WriteSignal<bool>,
    discovered_tvs: ReadSignal<Vec<DiscoveredTv>>,
    on_discover:    impl Fn() + 'static,
) -> impl IntoView {
    let on_connect    = StoredValue::new_local(on_connect);
    let on_disconnect = StoredValue::new_local(on_disconnect);
    let on_discover   = StoredValue::new_local(on_discover);

    let (brand_sel, set_brand_sel)     = signal(tv_brand.get_untracked());
    let (result_msg, set_result)       = signal(String::new());
    let (result_ok,  set_result_ok)    = signal(false);
    let (discovering, set_discovering) = signal(false);

    // Clear discovering spinner when results arrive
    Effect::new(move || {
        if !discovered_tvs.get().is_empty() {
            set_discovering.set(false);
        }
    });

    // ── Connected card ──────────────────────────────────────────────────────
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
                        crate::haptic();
                        set_tv_ip.set(String::new());
                        set_tv_connected.set(false);
                        set_result.set(String::new());
                        on_disconnect.with_value(|f| f());
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

    // ── Setup card ──────────────────────────────────────────────────────────
    let setup_card = move || {
        let brand_btns = BRANDS.iter().map(|(key, label)| {
            let key   = *key;
            let label = *label;
            view! {
                <button
                    on:click=move |_| set_brand_sel.set(key.to_string())
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

                // SSDP discover button
                <button
                    on:click=move |_| {
                        set_discovering.set(true);
                        on_discover.with_value(|f| f());
                        // Auto-reset after 4s
                        let set_d = set_discovering.clone();
                        wasm_bindgen_futures::spawn_local(async move {
                            gloo_timers::future::TimeoutFuture::new(4_000).await;
                            set_d.set(false);
                        });
                    }
                    disabled=move || discovering.get()
                    style=move || format!(
                        "width:100%;padding:10px;border-radius:12px;border:1px solid #475569;\
                         background:transparent;color:{};font-size:13px;\
                         font-weight:600;cursor:pointer",
                        if discovering.get() { "#475569" } else { "#93c5fd" }
                    )
                >
                    {move || if discovering.get() { "Scanning network…" } else { "Discover TVs on Network" }}
                </button>

                // Discovered TV list
                {move || {
                    let tvs = discovered_tvs.get();
                    if tvs.is_empty() { return ().into_any(); }
                    view! {
                        <div style="display:flex;flex-direction:column;gap:6px">
                            <div style="font-size:11px;color:#475569">"FOUND TVs"</div>
                            {tvs.iter().map(|tv| {
                                let ip = tv.ip.clone();
                                let brand = tv.brand.clone();
                                let name = tv.name.clone();
                                let ip2 = ip.clone();
                                let brand2 = brand.clone();
                                view! {
                                    <button
                                        on:click=move |_| {
                                            set_input_value("tv-ip-input", &ip2);
                                            set_brand_sel.set(brand2.clone());
                                        }
                                        style="text-align:left;width:100%;padding:10px 12px;\
                                               border-radius:10px;border:1px solid #334155;\
                                               background:#0f172a;color:#f1f5f9;\
                                               cursor:pointer;display:flex;\
                                               justify-content:space-between;align-items:center"
                                    >
                                        <div>
                                            <div style="font-size:13px;font-weight:600">{name}</div>
                                            <div style="font-size:11px;color:#94a3b8">{ip.clone()}</div>
                                        </div>
                                        <div style="font-size:11px;color:#6366f1;font-weight:600">
                                            {brand.to_uppercase()}
                                        </div>
                                    </button>
                                }
                            }).collect_view()}
                        </div>
                    }.into_any()
                }}

                // IP input
                <div style="display:flex;flex-direction:column;gap:6px">
                    <label style="font-size:12px;color:#94a3b8">"TV IP Address"</label>
                    <input
                        id="tv-ip-input"
                        type="text"
                        inputmode="decimal"
                        placeholder="192.168.1.x"
                        style="background:#0f172a;border:1px solid #334155;border-radius:10px;\
                               padding:10px 12px;color:#f1f5f9;font-size:16px;width:100%"
                    />
                    <span style="font-size:11px;color:#475569">
                        "Find in: Router admin page or use Discover above"
                    </span>
                </div>

                // Sony PSK field
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
                                   padding:10px 12px;color:#f1f5f9;font-size:16px;width:100%"
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
                        crate::haptic();
                        let ip = get_input_value("tv-ip-input");
                        let brand = brand_sel.get_untracked();
                        let psk = if brand == "sony" {
                            get_input_value("tv-psk-input")
                        } else {
                            String::new()
                        };

                        if ip.is_empty() {
                            set_result.set("Enter the TV's IP address".to_string());
                            set_result_ok.set(false);
                            return;
                        }
                        if !is_valid_ip(&ip) {
                            set_result.set("Use format 192.168.1.x".to_string());
                            set_result_ok.set(false);
                            return;
                        }
                        if brand == "sony" && psk.is_empty() {
                            set_result.set("Sony requires a Pre-Shared Key PIN".to_string());
                            set_result_ok.set(false);
                            return;
                        }

                        let msg = if brand == "samsung" {
                            format!("Connecting to {}… (approve on TV screen)", ip)
                        } else {
                            format!("Connecting to {}…", ip)
                        };
                        set_result.set(msg);
                        set_result_ok.set(true);

                        set_tv_ip.set(ip.clone());
                        set_tv_brand.set(brand.clone());
                        set_tv_connected.set(true);
                        on_connect.with_value(|f| f(ip, brand, psk));
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
                connected_card().into_any()
            } else {
                setup_card().into_any()
            }}
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

fn is_valid_ip(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| p.parse::<u8>().is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_valid_ip_good() {
        assert!(is_valid_ip("192.168.1.100"));
    }

    #[test]
    fn is_valid_ip_zeros() {
        assert!(is_valid_ip("0.0.0.0"));
    }

    #[test]
    fn is_valid_ip_max() {
        assert!(is_valid_ip("255.255.255.255"));
    }

    #[test]
    fn is_valid_ip_too_few() {
        assert!(!is_valid_ip("192.168.1"));
    }

    #[test]
    fn is_valid_ip_too_many() {
        assert!(!is_valid_ip("192.168.1.1.1"));
    }

    #[test]
    fn is_valid_ip_overflow() {
        assert!(!is_valid_ip("192.168.1.256"));
    }

    #[test]
    fn is_valid_ip_letters() {
        assert!(!is_valid_ip("abc.def.ghi.jkl"));
    }

    #[test]
    fn is_valid_ip_empty() {
        assert!(!is_valid_ip(""));
    }

    #[test]
    fn is_valid_ip_negative() {
        assert!(!is_valid_ip("192.168.1.-1"));
    }
}
