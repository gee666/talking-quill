//! Prepare replay execution, target notifications, and initial physical state.
use super::*;

pub(super) fn spawn_replay_worker(
    thread_id: u32,
    replay_receiver: Receiver<ReplayWork>,
    replay_accepted: Arc<AtomicU64>,
    replay_markers: injection::InjectionMarkers,
) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("talking-quill-win-replay".into())
        .spawn(move || {
            while let Ok(work) = replay_receiver.recv() {
                let accepted = match work {
                    ReplayWork::Replay {
                        batch,
                        target,
                        desktop,
                    } => (current_input_desktop() == Some(desktop)
                        && revalidate_candidate_target(target))
                    .then(|| injection::inject_replay(replay_markers, batch)),
                    ReplayWork::NeutralizeMenu {
                        modifiers,
                        target,
                        desktop,
                    } => (current_input_desktop() == Some(desktop)
                        && revalidate_candidate_target(target))
                    .then(|| injection::neutralize_menu(replay_markers, modifiers)),
                };
                replay_accepted.store(
                    accepted.map_or(1, |accepted| {
                        u64::try_from(accepted)
                            .unwrap_or(u64::MAX)
                            .saturating_add(2)
                    }),
                    Ordering::Release,
                );
                // Completion is durable in replay_accepted. The owner timer
                // polls it, so a failed wake cannot strand replay authority.
                let _ = post_owner_message_once(thread_id, WM_OWNER_REPLAY);
            }
        })
}

pub(super) fn install_target_events() -> [HWINEVENTHOOK; 3] {
    // Focus/foreground/caret epochs make A->B->A transitions observable even
    // when the point-in-time HWND tuple returns to its original values.
    // SAFETY: the system-ABI callback uses only a static atomic. Out-of-context
    // callbacks run on this owner thread; its installation guard unhooks every
    // non-null returned handle before the message-loop resources are dropped.
    unsafe {
        [
            SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                null_mut(),
                Some(target_change_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            ),
            SetWinEventHook(
                EVENT_OBJECT_FOCUS,
                EVENT_OBJECT_FOCUS,
                null_mut(),
                Some(target_change_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            ),
            SetWinEventHook(
                EVENT_OBJECT_LOCATIONCHANGE,
                EVENT_OBJECT_LOCATIONCHANGE,
                null_mut(),
                Some(target_change_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT,
            ),
        ]
    }
}

pub(super) fn seed_keyboard_state(
    context: &mut CallbackContext,
    input_desktop: windows_sys::Win32::System::StationsAndDesktops::HDESK,
) {
    // Low-level callbacks are delivered on this message-loop thread. Seed all
    // tracked physical state after installation and before readiness so a key
    // already held cannot begin an activation or session sequence.
    let physical = physical_tracker_from_state(native_physical_key_is_down);
    let modifiers = ModifierTracker::from_state(key_is_down);
    let keyboard = match context.keyboard.get_mut() {
        Ok(keyboard) => keyboard,
        Err(poisoned) => {
            context.terminal.trigger(TerminalReason::ReducerPoisoned);
            poisoned.into_inner()
        }
    };
    keyboard.physical = physical;
    keyboard.modifiers = modifiers;
    // Bind candidate admission to the exact desktop handle assigned to
    // this windowless hook thread. Reopening the input desktop here can
    // race a switch and previously made a valid owner start targetless.
    keyboard.input_desktop = desktop_identity(input_desktop);
    keyboard.logical_v_down = key_is_down(VK_V);
    keyboard.altgr_active = conservative_altgr(&keyboard.modifiers, keyboard.altgr_synthetic_ctrl);
    keyboard.modifiers_fenced = keyboard.modifiers.mask() != ModifierMask::default();
    keyboard.transactional = TransactionEngine::with_physical_snapshot(
        CompiledActivationConfig::default(),
        PhysicalSnapshot::new(
            keyboard.physical.held_letter_bits(),
            keyboard.modifiers.transactional_sides(),
            keyboard.altgr_active,
        ),
    );
}
