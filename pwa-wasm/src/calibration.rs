//! calibration.rs — Calibration screen (Leptos)
//!
//! Two-step calibration:
//!   Step 1: Record silence → sets noise floor
//!   Step 2: Record TV at preferred max → sets tripwire 3 dB below

use leptos::prelude::*;

fn local_get_f32(key: &str, default: f32) -> f32 {
    web_sys::window()
        .and_then(|w| w.local_storage().ok())
        .flatten()
        .and_then(|s| s.get_item(&format!("guardian_cal_{}", key)).ok())
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn local_set_f32(key: &str, val: f32) {
    if let Some(storage) = web_sys::window()
        .and_then(|w| w.local_storage().ok())
        .flatten()
    {
        let _ = storage.set_item(&format!("guardian_cal_{}", key), &val.to_string());
    }
}

// ── Calibration screen ──────────────────────────────────────────────────────

#[component]
pub fn CalibrationScreen(
    current_db:   ReadSignal<f32>,
    tripwire:     ReadSignal<f32>,
    on_silence:   impl Fn(f32) + 'static,
    on_max:       impl Fn(f32) + 'static,
    on_threshold: impl Fn(f32) + 'static,
) -> impl IntoView {
    let on_silence   = StoredValue::new_local(on_silence);
    let on_max       = StoredValue::new_local(on_max);
    let on_threshold = StoredValue::new_local(on_threshold);

    let (silence_done, set_silence_done) = signal(false);
    let (silence_db,   set_silence_db)   = signal(local_get_f32("floor",    -60.0));
    let (max_done,     set_max_done)     = signal(false);
    let (max_db,       set_max_db)       = signal(local_get_f32("max",      -20.0));
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
                    "Two-step setup — takes about 20 seconds"
                </div>
            </div>

            // ── Placement reminder ──────────────────────────────────────────
            <div style="background:#1e3a5f;border-radius:16px;padding:14px;\
                        display:flex;flex-direction:column;gap:4px">
                <div style="font-weight:700;color:#93c5fd">"Sensor Placement"</div>
                <div style="font-size:12px;color:#bfdbfe">
                    "Place the Guardian sensor where the baby/child usually is — "
                    "not next to the TV. The sensor should hear what the child hears."
                </div>
            </div>

            // ── Step 1: Silence ─────────────────────────────────────────────
            <CalStep
                number=1
                done=silence_done
                title="Record Silence"
                description="Quiet room — TV off, no talking. Measures ambient noise floor."
            >
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
                        format!("Quiet level: {:.1} dBFS", silence_db.get())
                    } else {
                        String::new()
                    }}
                </div>
            </CalStep>

            // ── Step 2: Max volume ──────────────────────────────────────────
            <CalStep
                number=2
                done=max_done
                title="Set Max Comfortable Volume"
                description="Turn TV to your preferred maximum. This sets the trigger threshold."
            >
                <button
                    on:click=move |_| {
                        crate::haptic();
                        let db = current_db.get_untracked();
                        set_max_db.set(db);
                        local_set_f32("max", db);
                        let tw = db - 3.0;
                        set_slider_val.set(tw);
                        local_set_f32("tripwire", tw);
                        set_max_done.set(true);
                        on_max.with_value(|f| f(db));
                    }
                    style="width:100%;padding:12px;border-radius:12px;border:none;\
                           background:#334155;color:#f1f5f9;font-size:15px;\
                           font-weight:600;cursor:pointer"
                >
                    "Record TV Volume Level"
                </button>
                <div style=move || format!(
                    "text-align:center;font-size:12px;margin-top:8px;\
                     color:{};min-height:16px",
                    if max_done.get() { "#22c55e" } else { "#94a3b8" }
                )>
                    {move || if max_done.get() {
                        format!("Tripwire: {:.1} dBFS  (TV: {:.1})", slider_val.get(), max_db.get())
                    } else {
                        String::new()
                    }}
                </div>
            </CalStep>

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
                    <strong style="color:#f1f5f9">"Minimum Gap: "</strong>
                    "The tripwire is always at least 6 dB above the quiet level "
                    "to prevent false triggers during normal viewing."
                </div>
            </div>

        </div>
    }
}

// ── Step card ───────────────────────────────────────────────────────────────

#[component]
fn CalStep(
    number:      usize,
    done:        ReadSignal<bool>,
    title:       &'static str,
    description: &'static str,
    children:    Children,
) -> impl IntoView {
    view! {
        <div style="background:#1e293b;border-radius:16px;padding:16px">
            <div style="display:flex;align-items:center;gap:12px;margin-bottom:12px">
                <div style=move || format!(
                    "width:32px;height:32px;border-radius:50%;display:flex;\
                     align-items:center;justify-content:center;\
                     font-weight:700;font-size:14px;flex-shrink:0;background:{}",
                    if done.get() { "#16a34a" } else { "#475569" }
                )>
                    {number.to_string()}
                </div>
                <div>
                    <div style="font-weight:600">{title}</div>
                    <div style="font-size:12px;color:#94a3b8">{description}</div>
                </div>
            </div>
            {children()}
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
