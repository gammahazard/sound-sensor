use guardian_test::ducking::*;

/// Helper: run N ticks at 100ms intervals with given dB, starting at `start_ms`.
/// Returns the last DuckCommand and the final time in ms.
fn run_ticks(eng: &mut DuckingEngine, db: f32, count: u32, start_ms: u64) -> (DuckCommand, u64) {
    let mut t = start_ms;
    let mut last_cmd = DuckCommand::None;
    for _ in 0..count {
        last_cmd = eng.tick_at(db, t);
        t += 100;
    }
    (last_cmd, t)
}

fn is_restore(cmd: &DuckCommand) -> bool {
    matches!(cmd, DuckCommand::Restore { .. })
}

#[test]
fn quiet_when_not_armed() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    assert_eq!(eng.tick_at(0.0, 0), DuckCommand::None);
    assert_eq!(eng.tick_at(-5.0, 100), DuckCommand::None);
    assert_eq!(eng.state(), DuckingState::Quiet);
}

#[test]
fn sustained_accumulation() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let (_, _) = run_ticks(&mut eng, -10.0, 30, 0);
    assert_eq!(eng.sustained_ms(), 3000);
}

#[test]
fn sustained_decay() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let (_, t) = run_ticks(&mut eng, -10.0, 10, 0);
    assert_eq!(eng.sustained_ms(), 1000);
    let (_, _) = run_ticks(&mut eng, -30.0, 10, t);
    assert_eq!(eng.sustained_ms(), 500);
}

#[test]
fn duck_trigger_at_3s() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let (cmd, t) = run_ticks(&mut eng, -10.0, 29, 0);
    assert_eq!(cmd, DuckCommand::None);
    assert_eq!(eng.sustained_ms(), 2900);
    let cmd = eng.tick_at(-10.0, t);
    assert_eq!(cmd, DuckCommand::VolumeDown);
    assert_eq!(eng.state(), DuckingState::Ducking);
}

#[test]
fn duck_rate_crisis() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let (_, mut t) = run_ticks(&mut eng, -2.0, 30, 0);
    for _ in 0..4 {
        let cmd = eng.tick_at(-2.0, t);
        assert_eq!(cmd, DuckCommand::None);
        t += 100;
    }
    let cmd = eng.tick_at(-2.0, t);
    assert_eq!(cmd, DuckCommand::VolumeDown);
}

#[test]
fn duck_rate_standard() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let (_, mut t) = run_ticks(&mut eng, -10.0, 30, 0);
    for _ in 0..9 {
        let cmd = eng.tick_at(-10.0, t);
        assert_eq!(cmd, DuckCommand::None);
        t += 100;
    }
    let cmd = eng.tick_at(-10.0, t);
    assert_eq!(cmd, DuckCommand::VolumeDown);
}

#[test]
fn duck_rate_gentle() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let (_, mut t) = run_ticks(&mut eng, -18.0, 30, 0);
    for _ in 0..19 {
        let cmd = eng.tick_at(-18.0, t);
        assert_eq!(cmd, DuckCommand::None);
        t += 100;
    }
    let cmd = eng.tick_at(-18.0, t);
    assert_eq!(cmd, DuckCommand::VolumeDown);
}

#[test]
fn restore_path_a_near_silence() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let (_, t) = run_ticks(&mut eng, -10.0, 31, 0);
    assert_eq!(eng.state(), DuckingState::Ducking);
    let cmd = eng.tick_at(-65.0, t);
    assert!(is_restore(&cmd));
    assert_eq!(eng.state(), DuckingState::Restoring);
}

#[test]
fn restore_path_a_carries_params() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    eng.set_original_volume(30);
    let (_, t) = run_ticks(&mut eng, -10.0, 31, 0);
    let steps_before = eng.duck_steps_taken;
    assert!(steps_before > 0);
    let cmd = eng.tick_at(-65.0, t);
    match cmd {
        DuckCommand::Restore { original_volume, steps } => {
            assert_eq!(original_volume, Some(30));
            assert_eq!(steps, steps_before);
        }
        _ => panic!("Expected Restore, got {:?}", cmd),
    }
}

#[test]
fn restore_path_b_hold_timer() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let (_, t) = run_ticks(&mut eng, -10.0, 30, 0);
    assert_eq!(eng.state(), DuckingState::Ducking);
    let (_, _t2) = run_ticks(&mut eng, -40.0, 60, t);
    assert_eq!(eng.sustained_ms(), 0);
    assert_eq!(eng.state(), DuckingState::Ducking);
    let cmd = eng.tick_at(-40.0, 33_000);
    assert!(is_restore(&cmd));
    assert_eq!(eng.state(), DuckingState::Restoring);
}

#[test]
fn no_restore_during_hold() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let (_, t) = run_ticks(&mut eng, -10.0, 30, 0);
    assert_eq!(eng.state(), DuckingState::Ducking);
    let (_, _) = run_ticks(&mut eng, -40.0, 60, t);
    assert_eq!(eng.sustained_ms(), 0);
    assert_eq!(eng.state(), DuckingState::Ducking);
    let cmd = eng.tick_at(-40.0, 2900 + 15_000);
    assert_eq!(cmd, DuckCommand::None);
    assert_eq!(eng.state(), DuckingState::Ducking);
}

#[test]
fn disarm_clears_everything() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    eng.set_original_volume(25);
    let (_, _) = run_ticks(&mut eng, -10.0, 31, 0);
    assert_eq!(eng.state(), DuckingState::Ducking);
    let steps_before = eng.duck_steps_taken;
    assert!(steps_before > 0);

    let cmd = eng.disarm();
    // Should return Restore with the captured params
    match cmd {
        DuckCommand::Restore { original_volume, steps } => {
            assert_eq!(original_volume, Some(25));
            assert_eq!(steps, steps_before);
        }
        _ => panic!("Expected Restore from disarm, got {:?}", cmd),
    }
    // Engine state should be fully cleared
    assert_eq!(eng.sustained_ms(), 0);
    assert_eq!(eng.state(), DuckingState::Quiet);
    assert_eq!(eng.duck_steps_taken, 0);
    assert!(eng.original_volume.is_none());
    assert!(!eng.armed);
}

#[test]
fn disarm_not_ducking_returns_none() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let (_, _) = run_ticks(&mut eng, -10.0, 5, 0);
    assert_eq!(eng.state(), DuckingState::Watching);
    let cmd = eng.disarm();
    assert_eq!(cmd, DuckCommand::None);
}

#[test]
fn set_floor_clamps_dead_mic() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.set_floor(-85.0);
    assert_eq!(eng.floor_db, -60.0);
}

#[test]
fn set_floor_bumps_tripwire() {
    let mut eng = DuckingEngine::new(-28.0, -60.0);
    eng.set_floor(-30.0);
    assert!(eng.tripwire_db >= -24.0);
}

#[test]
fn set_tripwire_enforces_gap() {
    let mut eng = DuckingEngine::new(-20.0, -40.0);
    eng.set_tripwire(-37.0);
    assert_eq!(eng.tripwire_db, -34.0);
}

#[test]
fn intermittent_noise_no_reset() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let (_, t) = run_ticks(&mut eng, -10.0, 20, 0);
    assert_eq!(eng.sustained_ms(), 2000);
    let (_, t) = run_ticks(&mut eng, -25.0, 1, t);
    assert_eq!(eng.sustained_ms(), 1950);
    let (_, _) = run_ticks(&mut eng, -10.0, 11, t);
    assert!(eng.sustained_ms() >= 3000);
}

#[test]
fn oscillation_prevention() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let (_, t) = run_ticks(&mut eng, -10.0, 31, 0);
    assert_eq!(eng.state(), DuckingState::Ducking);
    let cmd = eng.tick_at(-65.0, t);
    assert!(is_restore(&cmd));
    eng.clear_duck_state();
    eng.arm();
    let (cmd, _) = run_ticks(&mut eng, -10.0, 29, t + 100);
    assert_eq!(cmd, DuckCommand::None);
}

#[test]
fn duck_steps_increment() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let (_, mut t) = run_ticks(&mut eng, -10.0, 30, 0);
    assert_eq!(eng.duck_steps_taken, 1);
    t += 1000;
    let cmd = eng.tick_at(-10.0, t);
    assert_eq!(cmd, DuckCommand::VolumeDown);
    assert_eq!(eng.duck_steps_taken, 2);
}

#[test]
fn clear_duck_state_resets_restoring() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    // Drive into Ducking
    let (_, t) = run_ticks(&mut eng, -10.0, 31, 0);
    assert_eq!(eng.state(), DuckingState::Ducking);
    // Trigger restore → Restoring
    let cmd = eng.tick_at(-65.0, t);
    assert!(is_restore(&cmd));
    assert_eq!(eng.state(), DuckingState::Restoring);
    // clear_duck_state should reset to Quiet (not leave Restoring)
    eng.clear_duck_state();
    assert_eq!(eng.state(), DuckingState::Quiet);
    assert_eq!(eng.duck_steps_taken, 0);
    assert!(eng.original_volume.is_none());
}

#[test]
fn restore_path_b_carries_params() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    eng.set_original_volume(42);
    // Drive into ducking
    let (_, t) = run_ticks(&mut eng, -10.0, 30, 0);
    assert_eq!(eng.state(), DuckingState::Ducking);
    let steps = eng.duck_steps_taken;
    assert!(steps > 0);
    // Decay sustained_ms to 0 without hitting Path A
    let (_, _) = run_ticks(&mut eng, -40.0, 60, t);
    // Wait for hold timer to elapse (30s)
    let cmd = eng.tick_at(-40.0, 33_000);
    match cmd {
        DuckCommand::Restore { original_volume, steps: s } => {
            assert_eq!(original_volume, Some(42));
            assert_eq!(s, steps);
        }
        _ => panic!("Expected Restore via Path B, got {:?}", cmd),
    }
}

#[test]
fn original_volume_captured_once() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.set_original_volume(50);
    assert_eq!(eng.original_volume, Some(50));
    eng.set_original_volume(30);
    assert_eq!(eng.original_volume, Some(50));
}

#[test]
fn watching_to_quiet_transition() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let (_, t) = run_ticks(&mut eng, -10.0, 5, 0);
    assert_eq!(eng.state(), DuckingState::Watching);
    assert_eq!(eng.sustained_ms(), 500);
    let (_, _) = run_ticks(&mut eng, -30.0, 10, t);
    assert_eq!(eng.sustained_ms(), 0);
    assert_eq!(eng.state(), DuckingState::Quiet);
}

#[test]
fn ducking_stays_ducking_during_hold() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let (_, t) = run_ticks(&mut eng, -10.0, 30, 0);
    assert_eq!(eng.state(), DuckingState::Ducking);
    let (_, _) = run_ticks(&mut eng, -40.0, 60, t);
    assert_eq!(eng.sustained_ms(), 0);
    assert_eq!(eng.state(), DuckingState::Ducking);
}

#[test]
fn nan_db_returns_none() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let cmd = eng.tick_at(f32::NAN, 0);
    assert_eq!(cmd, DuckCommand::None);
    // NaN should not affect sustained_ms
    assert_eq!(eng.sustained_ms(), 0);
}

#[test]
fn nan_db_preserves_ducking_state() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let (_, t) = run_ticks(&mut eng, -10.0, 30, 0);
    assert_eq!(eng.state(), DuckingState::Ducking);
    // NaN during ducking should not change state
    let cmd = eng.tick_at(f32::NAN, t);
    assert_eq!(cmd, DuckCommand::None);
    assert_eq!(eng.state(), DuckingState::Ducking);
}

#[test]
fn restoring_blocks_ducking_reentry() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    // Drive into Ducking
    let (_, t) = run_ticks(&mut eng, -10.0, 31, 0);
    assert_eq!(eng.state(), DuckingState::Ducking);
    // Trigger restore → Restoring
    let cmd = eng.tick_at(-65.0, t);
    assert!(is_restore(&cmd));
    assert_eq!(eng.state(), DuckingState::Restoring);
    // Simulate loud noise during restore ramp — should NOT transition to Ducking
    let (cmd, _) = run_ticks(&mut eng, -10.0, 40, t + 100);
    assert_eq!(cmd, DuckCommand::None);
    assert_eq!(eng.state(), DuckingState::Restoring);
    // Sustained_ms accumulates but state stays Restoring
    assert!(eng.sustained_ms() >= 3000);
}

#[test]
fn restoring_does_not_become_watching() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let (_, t) = run_ticks(&mut eng, -10.0, 31, 0);
    let cmd = eng.tick_at(-65.0, t);
    assert!(is_restore(&cmd));
    assert_eq!(eng.state(), DuckingState::Restoring);
    // Below-tripwire noise should not change state to Watching
    let (_, _) = run_ticks(&mut eng, -25.0, 5, t + 100);
    assert_eq!(eng.state(), DuckingState::Restoring);
}

#[test]
fn duck_steps_capped_at_max() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    // Drive past 3s threshold
    let (_, mut t) = run_ticks(&mut eng, -10.0, 30, 0);
    assert_eq!(eng.duck_steps_taken, 1);
    // Send many more VolumeDown commands (1 per second for 40 seconds)
    for _ in 0..39 {
        t += 1000;
        eng.tick_at(-10.0, t);
    }
    // Steps should cap at 30, not go to 40
    assert_eq!(eng.duck_steps_taken, 30);
    // Additional ticks should return None — stop ducking at cap
    t += 1000;
    let cmd = eng.tick_at(-10.0, t);
    assert_eq!(cmd, DuckCommand::None);
    assert_eq!(eng.duck_steps_taken, 30);
}

#[test]
fn restore_after_cap_uses_capped_steps() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    eng.set_original_volume(50);
    let (_, mut t) = run_ticks(&mut eng, -10.0, 30, 0);
    // Duck 40 times (past the 30 cap)
    for _ in 0..39 {
        t += 1000;
        eng.tick_at(-10.0, t);
    }
    assert_eq!(eng.duck_steps_taken, 30);
    // Trigger restore
    let cmd = eng.tick_at(-65.0, t + 100);
    match cmd {
        DuckCommand::Restore { original_volume, steps } => {
            assert_eq!(original_volume, Some(50));
            assert_eq!(steps, 30); // Capped, not 40
        }
        _ => panic!("Expected Restore, got {:?}", cmd),
    }
}

#[test]
fn clear_duck_state_guarded_by_restoring() {
    // Simulates the tv_task guard: only clear if still Restoring
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let (_, t) = run_ticks(&mut eng, -10.0, 31, 0);
    assert_eq!(eng.state(), DuckingState::Ducking);
    // Trigger restore
    let cmd = eng.tick_at(-65.0, t);
    assert!(is_restore(&cmd));
    assert_eq!(eng.state(), DuckingState::Restoring);
    // Simulate disarm + rearm during ramp
    eng.disarm();
    assert_eq!(eng.state(), DuckingState::Quiet);
    eng.arm();
    // If tv_task checks state before clearing, it won't clear (state is Quiet, not Restoring)
    if eng.state() == DuckingState::Restoring {
        eng.clear_duck_state();
    }
    // Engine should still be armed and ready for new session
    assert!(eng.armed);
    assert_eq!(eng.state(), DuckingState::Quiet);
}

// ── Infinity tests ─────────────────────────────────────────────────────────

#[test]
fn positive_infinity_returns_none() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let cmd = eng.tick_at(f32::INFINITY, 0);
    assert_eq!(cmd, DuckCommand::None);
    assert_eq!(eng.sustained_ms(), 0);
}

#[test]
fn negative_infinity_returns_none() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let cmd = eng.tick_at(f32::NEG_INFINITY, 0);
    assert_eq!(cmd, DuckCommand::None);
    assert_eq!(eng.sustained_ms(), 0);
}

#[test]
fn infinity_preserves_ducking_state() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let (_, t) = run_ticks(&mut eng, -10.0, 30, 0);
    assert_eq!(eng.state(), DuckingState::Ducking);
    // +inf during ducking should not change state or trigger restore
    let cmd = eng.tick_at(f32::INFINITY, t);
    assert_eq!(cmd, DuckCommand::None);
    assert_eq!(eng.state(), DuckingState::Ducking);
}

#[test]
fn neg_infinity_preserves_ducking_state() {
    let mut eng = DuckingEngine::new(-20.0, -60.0);
    eng.arm();
    let (_, t) = run_ticks(&mut eng, -10.0, 30, 0);
    assert_eq!(eng.state(), DuckingState::Ducking);
    let cmd = eng.tick_at(f32::NEG_INFINITY, t);
    assert_eq!(cmd, DuckCommand::None);
    assert_eq!(eng.state(), DuckingState::Ducking);
}
