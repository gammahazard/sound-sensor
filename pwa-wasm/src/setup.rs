//! setup.rs — First-time setup wizard overlay (Leptos)
//!
//! Shown on first PWA open when setup_done is not set in localStorage.
//! Guides user through: Calibrate → Connect TV → Arm.
//! Stores `guardian_setup_done = true` in localStorage after completion.
//!
//! The wizard step is managed by the parent (main.rs) so it persists when
//! the user navigates to a tab and comes back via "Continue Setup".

use leptos::prelude::*;

#[derive(Clone, Copy, PartialEq)]
enum Step {
    Welcome,
    Calibrate,
    Tv,
    Done,
}

fn step_from_u8(v: u8) -> Step {
    match v {
        1 => Step::Calibrate,
        2 => Step::Tv,
        3 => Step::Done,
        _ => Step::Welcome,
    }
}

/// `on_tab(tab_name, next_wizard_step)` — navigates to a tab and saves
/// the step the wizard should resume at when reopened.
#[component]
pub fn SetupWizard(
    initial_step: u8,
    on_dismiss:   impl Fn() + 'static,
    on_tab:       impl Fn(&'static str, u8) + 'static,
) -> impl IntoView {
    let on_dismiss = StoredValue::new_local(on_dismiss);
    let on_tab     = StoredValue::new_local(on_tab);
    let (step, set_step) = signal(step_from_u8(initial_step));
    let (cal_done, set_cal_done) = signal(false);
    let (tv_done, set_tv_done)   = signal(false);

    view! {
        <div style="position:fixed;inset:0;background:#0f172a;z-index:1000;\
                    display:flex;flex-direction:column;overflow-y:auto">
            <div style="max-width:400px;width:100%;margin:0 auto;padding:24px;\
                        display:flex;flex-direction:column;gap:20px;flex:1">

                {move || match step.get() {
                    Step::Welcome => view! {
                        <div style="flex:1;display:flex;flex-direction:column;\
                                    justify-content:center;gap:20px;text-align:center">
                            <div style="font-size:48px">"🛡️"</div>
                            <div style="font-size:26px;font-weight:700">"Welcome to Guardian"</div>
                            <div style="color:#94a3b8;font-size:15px;line-height:1.5">
                                "Guardian protects your child's hearing by automatically "
                                "lowering the TV volume when things get too loud."
                            </div>
                            <div style="color:#94a3b8;font-size:13px;line-height:1.5;margin-top:8px">
                                "Quick setup takes about 2 minutes:"
                                <br/>"1. Calibrate the sound sensor"
                                <br/>"2. Connect your TV"
                                <br/>"3. Arm the system"
                            </div>
                            <button
                                on:click=move |_| set_step.set(Step::Calibrate)
                                style="margin-top:16px;padding:16px;border-radius:14px;border:none;\
                                       background:#6366f1;color:white;font-size:17px;\
                                       font-weight:700;cursor:pointer"
                            >
                                "Get Started"
                            </button>
                        </div>
                    }.into_any(),

                    Step::Calibrate => view! {
                        <div style="display:flex;flex-direction:column;gap:16px">
                            <StepHeader number=1 title="Calibrate the Sensor" />
                            <div style="background:#1e293b;border-radius:16px;padding:16px;\
                                        display:flex;flex-direction:column;gap:12px">
                                <div style="font-size:14px;color:#94a3b8;line-height:1.5">
                                    "Place the sensor where the child usually is (not next to the TV). "
                                    "Then use the Calibrate tab to:"
                                </div>
                                <div style="font-size:14px;color:#f1f5f9;line-height:1.6">
                                    <strong>"Step 1:"</strong>" Record silence (TV off, quiet room)"
                                    <br/><strong>"Step 2:"</strong>" Record TV at your preferred max volume"
                                </div>
                            </div>
                            <button
                                on:click=move |_| {
                                    set_cal_done.set(true);
                                    on_tab.with_value(|f| f("cal", 2)); // next: Tv step
                                }
                                style="padding:14px;border-radius:12px;border:none;\
                                       background:#6366f1;color:white;font-size:15px;\
                                       font-weight:700;cursor:pointer"
                            >
                                "Open Calibration"
                            </button>
                            <button
                                on:click=move |_| set_step.set(Step::Tv)
                                style="padding:12px;border-radius:12px;border:1px solid #475569;\
                                       background:transparent;color:#94a3b8;font-size:14px;\
                                       font-weight:600;cursor:pointer"
                            >
                                "Skip for now"
                            </button>
                            <button
                                on:click=move |_| {
                                    set_cal_done.set(true);
                                    set_step.set(Step::Tv);
                                }
                                style="padding:12px;border-radius:12px;border:1px solid #334155;\
                                       background:#1e293b;color:#f1f5f9;font-size:14px;\
                                       font-weight:600;cursor:pointer"
                            >
                                "Done calibrating — Next"
                            </button>
                        </div>
                    }.into_any(),

                    Step::Tv => view! {
                        <div style="display:flex;flex-direction:column;gap:16px">
                            <StepHeader number=2 title="Connect Your TV" />
                            <div style="background:#1e293b;border-radius:16px;padding:16px;\
                                        display:flex;flex-direction:column;gap:12px">
                                <div style="font-size:14px;color:#94a3b8;line-height:1.5">
                                    "Select your TV brand, discover it on the network, "
                                    "and connect. Guardian will control its volume automatically."
                                </div>
                            </div>
                            <button
                                on:click=move |_| {
                                    set_tv_done.set(true);
                                    on_tab.with_value(|f| f("tv", 3)); // next: Done step
                                }
                                style="padding:14px;border-radius:12px;border:none;\
                                       background:#6366f1;color:white;font-size:15px;\
                                       font-weight:700;cursor:pointer"
                            >
                                "Open TV Setup"
                            </button>
                            <button
                                on:click=move |_| set_step.set(Step::Done)
                                style="padding:12px;border-radius:12px;border:1px solid #475569;\
                                       background:transparent;color:#94a3b8;font-size:14px;\
                                       font-weight:600;cursor:pointer"
                            >
                                "Skip for now"
                            </button>
                            <button
                                on:click=move |_| {
                                    set_tv_done.set(true);
                                    set_step.set(Step::Done);
                                }
                                style="padding:12px;border-radius:12px;border:1px solid #334155;\
                                       background:#1e293b;color:#f1f5f9;font-size:14px;\
                                       font-weight:600;cursor:pointer"
                            >
                                "TV connected — Next"
                            </button>
                        </div>
                    }.into_any(),

                    Step::Done => {
                        let both_skipped = !cal_done.get_untracked() && !tv_done.get_untracked();
                        let any_skipped = !cal_done.get_untracked() || !tv_done.get_untracked();
                        let (title, msg) = if both_skipped {
                            ("Setup Incomplete",
                             "Calibration and TV setup were skipped. Open the Calibrate and TV tabs to finish.")
                        } else if any_skipped {
                            ("Almost There!",
                             if !cal_done.get_untracked() {
                                 "Calibration was skipped. Open the Calibrate tab to complete setup."
                             } else {
                                 "TV setup was skipped. Open the TV tab to connect your TV."
                             })
                        } else {
                            ("You're All Set!",
                             "Tap the button below to arm Guardian. It will automatically duck the TV volume when sustained loud noise is detected.")
                        };
                        view! {
                            <div style="flex:1;display:flex;flex-direction:column;\
                                        justify-content:center;gap:20px;text-align:center">
                                <div style="font-size:48px">{if both_skipped { "\u{26A0}\u{FE0F}" } else { "\u{1F389}" }}</div>
                                <div style="font-size:26px;font-weight:700">{title}</div>
                                <div style="color:#94a3b8;font-size:15px;line-height:1.5">
                                    {msg}
                                </div>
                                <button
                                    on:click=move |_| {
                                        crate::local_set("setup_done", "true");
                                        on_dismiss.with_value(|f| f());
                                    }
                                    style="margin-top:16px;padding:16px;border-radius:14px;border:none;\
                                           background:#16a34a;color:white;font-size:17px;\
                                           font-weight:700;cursor:pointer"
                                >
                                    "Finish Setup"
                                </button>
                            </div>
                        }
                    }.into_any(),
                }}
            </div>
        </div>
    }
}

#[component]
fn StepHeader(number: usize, title: &'static str) -> impl IntoView {
    view! {
        <div style="display:flex;align-items:center;gap:12px;margin-top:16px">
            <div style="width:36px;height:36px;border-radius:50%;background:#6366f1;\
                        display:flex;align-items:center;justify-content:center;\
                        font-weight:700;font-size:16px;flex-shrink:0">
                {number.to_string()}
            </div>
            <div style="font-size:20px;font-weight:700">{title}</div>
        </div>
    }
}
