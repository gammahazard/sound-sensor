//! meter.rs — Real-time dB bar meter screen (Leptos)

use leptos::prelude::*;
use wasm_bindgen::JsCast;
use crate::EventEntry;

const DB_MIN: f32 = -50.0;
const DB_MAX: f32 = -10.0;

fn db_to_pct(db: f32) -> f32 {
    if !db.is_finite() { return 0.0; }
    ((db - DB_MIN) / (DB_MAX - DB_MIN) * 100.0).clamp(0.0, 100.0)
}

fn bar_color(db: f32) -> &'static str {
    if db > -18.0       { "#ef4444" }
    else if db > -30.0  { "#eab308" }
    else                { "#22c55e" }
}

// ── Meter screen ──────────────────────────────────────────────────────────────

#[component]
pub fn MeterScreen(
    db:            ReadSignal<f32>,
    armed:         ReadSignal<bool>,
    tripwire:      ReadSignal<f32>,
    ducking:       ReadSignal<bool>,
    events:        ReadSignal<Vec<EventEntry>>,
    on_arm_toggle: impl Fn() + 'static,
) -> impl IntoView {
    let on_arm_toggle = StoredValue::new_local(on_arm_toggle);

    // Peak hold (2 seconds)
    let (peak, set_peak)            = signal(DB_MIN);
    let peak_timer: StoredValue<Option<i32>> = StoredValue::new(None);

    Effect::new(move || {
        let current = db.get();
        if current > peak.get_untracked() {
            set_peak.set(current);
            if let Some(id) = peak_timer.get_value() {
                web_sys::window().unwrap().clear_timeout_with_handle(id);
            }
            let set_peak_clone = set_peak.clone();
            let cb = wasm_bindgen::closure::Closure::once_into_js(move || {
                set_peak_clone.set(DB_MIN);
            });
            let id = web_sys::window()
                .unwrap()
                .set_timeout_with_callback_and_timeout_and_arguments_0(
                    cb.as_ref().unchecked_ref(), 2000,
                )
                .unwrap_or(0);
            *peak_timer.write_value() = Some(id);
        }
    });

    view! {
        <div style="padding:16px;display:flex;flex-direction:column;gap:16px">

            // ── Ducking banner ──────────────────────────────────────────────
            {move || ducking.get().then(|| view! {
                <div style="background:#92400e;border-radius:16px;padding:14px;\
                            display:flex;flex-direction:column;gap:4px">
                    <div style="font-weight:700;color:#fef3c7">"Volume Ducked"</div>
                    <div style="font-size:12px;color:#fde68a">
                        "Guardian detected sustained loud noise and reduced TV volume."
                    </div>
                </div>
            })}

            // ── dB readout ──────────────────────────────────────────────────
            <div style="text-align:center;margin-top:16px">
                <div style=move || format!(
                    "font-size:72px;font-weight:900;font-variant-numeric:tabular-nums;\
                     color:{}", bar_color(db.get())
                )>
                    {move || {
                        let d = db.get();
                        if d.is_finite() { format!("{:.1}", d) } else { "—".to_string() }
                    }}
                </div>
                <div style="color:#94a3b8;font-size:13px;margin-top:4px">"dBFS"</div>
            </div>

            // ── Bar meter ───────────────────────────────────────────────────
            <div style="position:relative;height:40px;background:#1e293b;\
                        border-radius:999px;overflow:hidden;margin:0 8px">
                <div style=move || format!(
                    "height:100%;border-radius:999px;\
                     background:linear-gradient(to right,#22c55e 0%,#eab308 65%,#ef4444 85%);\
                     width:{}%;transition:width 80ms linear",
                    db_to_pct(db.get())
                ) />
                // Tripwire marker
                <div style=move || format!(
                    "position:absolute;top:0;bottom:0;width:2px;\
                     background:white;opacity:0.7;left:{}%",
                    db_to_pct(tripwire.get())
                ) />
            </div>

            // Scale labels
            <div style="display:flex;justify-content:space-between;font-size:11px;\
                        color:#475569;margin:-8px 8px 0">
                <span>"-50 dB"</span>
                <span>"-30 dB"</span>
                <span>"-10 dB"</span>
            </div>

            // ── Status row ──────────────────────────────────────────────────
            <div style="display:flex;justify-content:space-between;\
                        background:#1e293b;border-radius:16px;padding:16px">
                <StatusCell label="Status">
                    <span style=move || format!(
                        "font-weight:600;color:{}",
                        if ducking.get() { "#f59e0b" }
                        else if armed.get() { "#22c55e" }
                        else { "#94a3b8" }
                    )>
                        {move || {
                            if ducking.get() { "Ducking" }
                            else if armed.get() { "Armed" }
                            else { "Disarmed" }
                        }}
                    </span>
                </StatusCell>
                <StatusCell label="Tripwire">
                    <span style="font-weight:600;font-variant-numeric:tabular-nums">
                        {move || format!("{:.1}", tripwire.get())}
                    </span>
                </StatusCell>
                <StatusCell label="Peak">
                    <span style="font-weight:600;color:#eab308;\
                                 font-variant-numeric:tabular-nums">
                        {move || {
                            let p = peak.get();
                            if p > DB_MIN { format!("{:.1}", p) } else { "—".to_string() }
                        }}
                    </span>
                </StatusCell>
            </div>

            // ── Arm / Disarm button ─────────────────────────────────────────
            <button
                on:click=move |_| on_arm_toggle.with_value(|f| f())
                style=move || format!(
                    "width:100%;padding:16px;border-radius:16px;border:none;\
                     font-size:18px;font-weight:700;cursor:pointer;\
                     background:{};color:white",
                    if armed.get() { "#dc2626" } else { "#6366f1" }
                )
            >
                {move || if armed.get() { "Disarm Guardian" } else { "Arm Guardian" }}
            </button>

            // ── Recent events ───────────────────────────────────────────────
            {move || {
                let evts = events.get();
                if evts.is_empty() { return ().into_any(); }
                view! {
                    <div style="background:#1e293b;border-radius:16px;padding:12px;\
                                display:flex;flex-direction:column;gap:4px">
                        <div style="font-size:11px;color:#475569;margin-bottom:6px">"RECENT EVENTS"</div>
                        {evts.iter().take(5).map(|e| {
                            let msg  = e.msg.clone();
                            let time = e.time.clone();
                            view! {
                                <div style="display:flex;justify-content:space-between;\
                                            font-size:12px;padding:4px 0">
                                    <span style="color:#cbd5e1">{msg}</span>
                                    <span style="color:#475569;flex-shrink:0;margin-left:8px">{time}</span>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            }}

        </div>
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_to_pct_min() {
        assert_eq!(db_to_pct(-50.0), 0.0);
    }

    #[test]
    fn db_to_pct_max() {
        assert_eq!(db_to_pct(-10.0), 100.0);
    }

    #[test]
    fn db_to_pct_mid() {
        assert_eq!(db_to_pct(-30.0), 50.0);
    }

    #[test]
    fn db_to_pct_clamp_below() {
        assert_eq!(db_to_pct(-100.0), 0.0);
    }

    #[test]
    fn db_to_pct_clamp_above() {
        assert_eq!(db_to_pct(0.0), 100.0);
    }

    #[test]
    fn bar_color_green() {
        assert_eq!(bar_color(-35.0), "#22c55e");
    }

    #[test]
    fn bar_color_yellow() {
        assert_eq!(bar_color(-25.0), "#eab308");
    }

    #[test]
    fn bar_color_red() {
        assert_eq!(bar_color(-15.0), "#ef4444");
    }

    #[test]
    fn bar_color_boundary_yellow() {
        // Exactly -30 should be green (not > -30)
        assert_eq!(bar_color(-30.0), "#22c55e");
    }

    #[test]
    fn bar_color_boundary_red() {
        // Exactly -18 should be yellow (not > -18)
        assert_eq!(bar_color(-18.0), "#eab308");
    }
}

// ── Helper: status cell ─────────────────────────────────────────────────────

#[component]
fn StatusCell(label: &'static str, children: Children) -> impl IntoView {
    view! {
        <div style="display:flex;flex-direction:column;gap:4px">
            <div style="font-size:11px;color:#94a3b8">{label}</div>
            {children()}
        </div>
    }
}
