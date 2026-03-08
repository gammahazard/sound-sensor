//! meter.rs — Real-time dB bar meter screen (Leptos)

use std::cell::RefCell;
use std::rc::Rc;
use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use crate::EventEntry;

const DB_MIN: f32 = -50.0;
const DB_MAX: f32 = -10.0;

fn db_to_pct(db: f32) -> f32 {
    if !db.is_finite() { return 0.0; }
    ((db - DB_MIN) / (DB_MAX - DB_MIN) * 100.0).clamp(0.0, 100.0)
}

fn pct_to_db(pct: f32) -> f32 {
    pct / 100.0 * (DB_MAX - DB_MIN) + DB_MIN
}

fn bar_color(db: f32) -> &'static str {
    if db > -18.0       { "#ef4444" }
    else if db > -30.0  { "#eab308" }
    else                { "#22c55e" }
}

// ── Meter screen ──────────────────────────────────────────────────────────────

#[component]
pub fn MeterScreen(
    db:                  ReadSignal<f32>,
    armed:               ReadSignal<bool>,
    tripwire:            ReadSignal<f32>,
    ducking:             ReadSignal<bool>,
    crying:              ReadSignal<bool>,
    events:              ReadSignal<Vec<EventEntry>>,
    on_arm_toggle:       impl Fn() + 'static,
    on_tripwire_change:  impl Fn(f32) + 'static,
) -> impl IntoView {
    let on_arm_toggle = StoredValue::new_local(on_arm_toggle);
    let on_tripwire_change = StoredValue::new_local(on_tripwire_change);

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

    // Draggable tripwire state
    let bar_ref = NodeRef::<leptos::html::Div>::new();
    let (drag_db, set_drag_db) = signal(None::<f32>);

    // Compute tripwire display: use drag_db while dragging, otherwise tripwire signal
    let tripwire_display = move || drag_db.get().unwrap_or_else(|| tripwire.get());

    // Shared Rc storage for window-level closures (so we can remove them on mouseup/touchend)
    type ClosurePair<E> = Rc<RefCell<Option<(Closure<dyn FnMut(E)>, Closure<dyn FnMut(E)>)>>>;
    let mouse_closures: ClosurePair<web_sys::MouseEvent> = Rc::new(RefCell::new(None));
    let touch_closures: ClosurePair<web_sys::TouchEvent> = Rc::new(RefCell::new(None));

    // Helper: compute dB from a client-X coordinate relative to the bar
    let calc_db_from_x = move |client_x: f64| -> f32 {
        let Some(el) = bar_ref.get() else { return tripwire.get_untracked(); };
        let el: &web_sys::HtmlElement = &el;
        let rect = el.get_bounding_client_rect();
        let pct = ((client_x - rect.left()) / rect.width() * 100.0) as f32;
        pct_to_db(pct.clamp(0.0, 100.0))
    };

    // Mouse drag start
    let mc = mouse_closures.clone();
    let on_mousedown = move |e: web_sys::MouseEvent| {
        e.prevent_default();
        let db_val = calc_db_from_x(e.client_x() as f64);
        set_drag_db.set(Some(db_val));

        let mc_inner = mc.clone();
        let on_move = Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |e: web_sys::MouseEvent| {
            let db_val = calc_db_from_x(e.client_x() as f64);
            set_drag_db.set(Some(db_val));
        });

        let mc_cleanup = mc_inner.clone();
        let on_up = Closure::<dyn FnMut(web_sys::MouseEvent)>::new(move |e: web_sys::MouseEvent| {
            let final_db = calc_db_from_x(e.client_x() as f64);
            set_drag_db.set(None);
            on_tripwire_change.with_value(|f| f(final_db));
            // Cleanup
            if let Some(window) = web_sys::window() {
                if let Some((ref m, ref u)) = *mc_cleanup.borrow() {
                    let _ = window.remove_event_listener_with_callback("mousemove", m.as_ref().unchecked_ref());
                    let _ = window.remove_event_listener_with_callback("mouseup", u.as_ref().unchecked_ref());
                }
            }
            *mc_cleanup.borrow_mut() = None;
        });

        if let Some(window) = web_sys::window() {
            let _ = window.add_event_listener_with_callback("mousemove", on_move.as_ref().unchecked_ref());
            let _ = window.add_event_listener_with_callback("mouseup", on_up.as_ref().unchecked_ref());
        }

        *mc_inner.borrow_mut() = Some((on_move, on_up));
    };

    // Touch drag start
    let tc = touch_closures.clone();
    let on_touchstart = move |e: web_sys::TouchEvent| {
        e.prevent_default();
        if let Some(touch) = e.touches().get(0) {
            let db_val = calc_db_from_x(touch.client_x() as f64);
            set_drag_db.set(Some(db_val));
        }

        let tc_inner = tc.clone();
        let on_move = Closure::<dyn FnMut(web_sys::TouchEvent)>::new(move |e: web_sys::TouchEvent| {
            e.prevent_default();
            if let Some(touch) = e.touches().get(0) {
                let db_val = calc_db_from_x(touch.client_x() as f64);
                set_drag_db.set(Some(db_val));
            }
        });

        let tc_cleanup = tc_inner.clone();
        let on_end = Closure::<dyn FnMut(web_sys::TouchEvent)>::new(move |e: web_sys::TouchEvent| {
            if let Some(touch) = e.changed_touches().get(0) {
                let final_db = calc_db_from_x(touch.client_x() as f64);
                set_drag_db.set(None);
                on_tripwire_change.with_value(|f| f(final_db));
            } else {
                set_drag_db.set(None);
            }
            // Cleanup
            if let Some(window) = web_sys::window() {
                if let Some((ref m, ref u)) = *tc_cleanup.borrow() {
                    let _ = window.remove_event_listener_with_callback("touchmove", m.as_ref().unchecked_ref());
                    let _ = window.remove_event_listener_with_callback("touchend", u.as_ref().unchecked_ref());
                }
            }
            *tc_cleanup.borrow_mut() = None;
        });

        if let Some(window) = web_sys::window() {
            let _ = window.add_event_listener_with_callback("touchmove", on_move.as_ref().unchecked_ref());
            let _ = window.add_event_listener_with_callback("touchend", on_end.as_ref().unchecked_ref());
        }

        *tc_inner.borrow_mut() = Some((on_move, on_end));
    };

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

            // ── Crying banner ──────────────────────────────────────────────
            {move || crying.get().then(|| view! {
                <div style="background:#991b1b;border-radius:16px;padding:14px;\
                            text-align:center;animation:pulse 1.5s ease-in-out infinite">
                    <div style="font-weight:700;font-size:16px;color:white">"Baby Crying Detected"</div>
                    <div style="font-size:12px;color:#fecaca;margin-top:4px">
                        "Rhythmic cry pattern confirmed."
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
            <div
                node_ref=bar_ref
                style="position:relative;height:40px;background:#1e293b;\
                       border-radius:999px;overflow:visible;margin:0 8px"
            >
                // Filled bar (inside rounded container)
                <div style="position:absolute;inset:0;border-radius:999px;overflow:hidden">
                    <div style=move || format!(
                        "height:100%;\
                         background:linear-gradient(to right,#22c55e 0%,#eab308 65%,#ef4444 85%);\
                         width:{}%;transition:width 80ms linear",
                        db_to_pct(db.get())
                    ) />
                </div>
                // Tripwire marker — draggable (24px invisible hit area, 2px visible line)
                <div
                    on:mousedown=on_mousedown
                    on:touchstart=on_touchstart
                    style=move || format!(
                        "position:absolute;top:-4px;bottom:-4px;width:24px;\
                         cursor:ew-resize;z-index:10;\
                         left:calc({}% - 12px);\
                         touch-action:none",
                        db_to_pct(tripwire_display())
                    )
                >
                    // Visible 2px line
                    <div style="position:absolute;left:11px;top:4px;bottom:4px;\
                                width:2px;background:white;opacity:0.85;border-radius:1px" />
                    // Small handle dot for visual affordance
                    <div style="position:absolute;left:8px;top:50%;transform:translateY(-50%);\
                                width:8px;height:8px;border-radius:50%;\
                                background:white;opacity:0.9" />
                </div>
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
                        {move || format!("{:.1}", tripwire_display())}
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
    fn pct_to_db_min() {
        assert_eq!(pct_to_db(0.0), -50.0);
    }

    #[test]
    fn pct_to_db_max() {
        assert_eq!(pct_to_db(100.0), -10.0);
    }

    #[test]
    fn pct_to_db_mid() {
        assert_eq!(pct_to_db(50.0), -30.0);
    }

    #[test]
    fn pct_db_roundtrip() {
        for db in [-50.0, -40.0, -30.0, -20.0, -10.0] {
            let pct = db_to_pct(db);
            let back = pct_to_db(pct);
            assert!((back - db).abs() < 0.01, "roundtrip failed for {}", db);
        }
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
        assert_eq!(bar_color(-30.0), "#22c55e");
    }

    #[test]
    fn bar_color_boundary_red() {
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
