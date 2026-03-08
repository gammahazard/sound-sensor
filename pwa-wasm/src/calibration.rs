//! calibration.rs — Calibration screen (Leptos)
//!
//! Single-step calibration: record quiet level → firmware auto-derives tripwire (floor + 6 dB).
//! Manual slider for fine-tuning.

use leptos::prelude::*;

fn local_get_f32(key: &str, default: f32) -> f32 {
    let k = format!("cal_{}", key);
    let v = crate::local_get(&k, &default.to_string()).parse().unwrap_or(default);
    if v.is_finite() { v } else { default }
}

fn local_set_f32(key: &str, val: f32) {
    let k = format!("cal_{}", key);
    crate::local_set(&k, &val.to_string());
}

// ── Calibration screen ──────────────────────────────────────────────────────

#[component]
pub fn CalibrationScreen(
    current_db:   ReadSignal<f32>,
    tripwire:     ReadSignal<f32>,
    on_silence:   impl Fn(f32) + 'static,
    on_threshold: impl Fn(f32) + 'static,
) -> impl IntoView {
    let on_silence   = StoredValue::new_local(on_silence);
    let on_threshold = StoredValue::new_local(on_threshold);

    let (silence_done, set_silence_done) = signal(false);
    let (silence_db,   set_silence_db)   = signal(local_get_f32("floor", -60.0));
    let (slider_val,   set_slider_val)   = signal(local_get_f32("tripwire", -20.0));
    let (user_editing, set_user_editing) = signal(false);

    Effect::new(move || {
        let tw = tripwire.get();
        if !user_editing.get() {
            set_slider_val.set(tw);
        }
    });

    view! {
        <div style="padding:16px;display:flex;flex-direction:column;gap:16px">

            <div style="text-align:center;margin-top:16px">
                <div style="font-size:22px;font-weight:700">"Calibration"</div>
                <div style="color:#94a3b8;font-size:13px;margin-top:4px">
                    "Set the quiet level for your baby's room"
                </div>
            </div>

            // ── Placement reminder ──────────────────────────────────────────
            <div style="background:#1e3a5f;border-radius:16px;padding:14px;\
                        display:flex;flex-direction:column;gap:4px">
                <div style="font-weight:700;color:#93c5fd">"Sensor Placement"</div>
                <div style="font-size:12px;color:#bfdbfe">
                    "Place the Guardian sensor at the baby's door, facing the hallway. "
                    "It should hear what reaches the child's room."
                </div>
            </div>

            // ── Record quiet level ──────────────────────────────────────────
            <div style="background:#1e293b;border-radius:16px;padding:16px">
                <div style="margin-bottom:12px">
                    <div style="font-weight:600">"Record Quiet Level"</div>
                    <div style="font-size:12px;color:#94a3b8">
                        "Quiet room — TV off, no talking. Measures ambient noise floor."
                    </div>
                </div>
                <button
                    on:click=move |_| {
                        crate::haptic();
                        let db = current_db.get_untracked();
                        set_silence_db.set(db);
                        local_set_f32("floor", db);
                        set_silence_done.set(true);
                        on_silence.with_value(|f| f(db));
                    }
                    style="width:100%;padding:12px;border-radius:12px;border:none;\
                           background:#334155;color:#f1f5f9;font-size:15px;\
                           font-weight:600;cursor:pointer"
                >
                    "Record Quiet Level"
                </button>
                <div style=move || format!(
                    "text-align:center;font-size:12px;margin-top:8px;\
                     color:{};min-height:16px",
                    if silence_done.get() { "#22c55e" } else { "#94a3b8" }
                )>
                    {move || if silence_done.get() {
                        format!("Quiet level: {:.1} dBFS — tripwire auto-set to {:.1} dBFS",
                            silence_db.get(), silence_db.get() + 6.0)
                    } else {
                        String::new()
                    }}
                </div>
            </div>

            // ── Manual threshold ────────────────────────────────────────────
            <div style="background:#1e293b;border-radius:16px;padding:16px">
                <div style="font-weight:600;margin-bottom:12px">"Manual Threshold"</div>
                <div style="display:flex;align-items:center;gap:12px">
                    <input
                        type="range"
                        min="-60" max="-3" step="1"
                        prop:value=move || slider_val.get().to_string()
                        on:input=move |e| {
                            set_user_editing.set(true);
                            let val: f32 = event_target_value(&e).parse().unwrap_or(-20.0);
                            set_slider_val.set(val);
                        }
                        style="flex:1;accent-color:#6366f1"
                    />
                    <span style="font-size:13px;width:72px;text-align:right;\
                                 font-variant-numeric:tabular-nums">
                        {move || format!("{:.0} dBFS", slider_val.get())}
                    </span>
                </div>
                <button
                    on:click=move |_| {
                        set_user_editing.set(false);
                        let v = slider_val.get_untracked();
                        local_set_f32("tripwire", v);
                        on_threshold.with_value(|f| f(v));
                    }
                    style="margin-top:12px;width:100%;padding:10px;border-radius:12px;\
                           border:none;background:#6366f1;color:white;\
                           font-size:14px;font-weight:600;cursor:pointer"
                >
                    "Apply Threshold"
                </button>
            </div>

            // ── How it works note ────────────────────────────────────────────
            <div style="background:#1e293b;border-radius:16px;padding:12px;\
                        font-size:12px;color:#94a3b8;display:flex;flex-direction:column;gap:8px">
                <div>
                    <strong style="color:#f1f5f9">"3-Second Rule: "</strong>
                    "Ducking only fires after sustained loud noise for 3 seconds — "
                    "brief sounds (footsteps, doors) are ignored."
                </div>
                <div>
                    <strong style="color:#f1f5f9">"Auto-Restore: "</strong>
                    "Volume returns to normal ~30 seconds after the loud scene ends. "
                    "If the room goes completely quiet, it restores immediately."
                </div>
                <div>
                    <strong style="color:#f1f5f9">"Drag to Adjust: "</strong>
                    "You can also drag the white tripwire line on the Meter tab "
                    "for quick adjustments."
                </div>
            </div>

        </div>
    }
}

fn event_target_value(e: &web_sys::Event) -> String {
    use wasm_bindgen::JsCast;
    e.target()
        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|el| el.value())
        .unwrap_or_default()
}
