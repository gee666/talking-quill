//! Capture lossless keyboard/mouse originals into the bounded journal.

use super::*;

pub(super) fn defer_callback_edge(
    context: &CallbackContext,
    event_type: u32,
    event: ffi::CGEventRef,
    source: InputSource,
    source_pid: i64,
    original_marker: i64,
    uncertain_owned: bool,
) -> CurrentEdgeDisposition {
    if event.is_null() {
        fail_recovery_edge_journal(context);
        return CurrentEdgeDisposition::Owned;
    }
    let flags = unsafe { ffi::CGEventGetFlags(event) };
    let timestamp = unsafe { ffi::CGEventGetTimestamp(event) };
    let mut journal = match context.recovery_edges.try_lock() {
        Ok(journal) => journal,
        Err(_) => {
            fail_recovery_edge_journal(context);
            return CurrentEdgeDisposition::Owned;
        }
    };
    let mut edge = injection::DeferredEvent {
        event_type,
        flags,
        original_timestamp: timestamp,
        source,
        source_pid,
        original_marker,
        uncertain_owned,
        ..injection::DeferredEvent::EMPTY
    };

    if is_mouse_event_type(event_type) {
        let Some(button) = deferred_mouse_button(event_type, event).filter(|button| *button < 32)
        else {
            journal.enter_overflow();
            journal.settle_overflow();
            drop(journal);
            fail_recovery_edge_journal(context);
            return CurrentEdgeDisposition::Owned;
        };
        let foreground_was_down = journal.foreground_mouse_was_down(source, source_pid, button);
        let phase = journal.observe_mouse_phase(
            source,
            source_pid,
            button,
            mouse_event_is_down(event_type),
            foreground_was_down,
        );
        let (foreground_balance, hidden_generation) = match phase {
            Some(phase) => phase,
            None if source == InputSource::External => (false, false),
            None => {
                journal.enter_overflow();
                journal.settle_overflow();
                drop(journal);
                fail_recovery_edge_journal(context);
                return CurrentEdgeDisposition::Owned;
            }
        };
        edge.is_down = mouse_event_is_down(event_type);
        edge.foreground_balance = foreground_balance;
        edge.hidden_generation = hidden_generation;
        edge.location = unsafe { ffi::CGEventGetLocation(event) };
        edge.mouse_number =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_NUMBER) };
        edge.mouse_click_state =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_CLICK_STATE) };
        edge.mouse_pressure =
            unsafe { ffi::CGEventGetDoubleValueField(event, ffi::K_CG_MOUSE_EVENT_PRESSURE) };
        edge.mouse_button = i64::from(button);
        edge.mouse_delta_x =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_DELTA_X) };
        edge.mouse_delta_y =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_DELTA_Y) };
        edge.mouse_instant_mouser = unsafe {
            ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_INSTANT_MOUSER)
        };
        edge.mouse_subtype =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_MOUSE_EVENT_SUBTYPE) };
    } else {
        let key_code =
            unsafe { ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYCODE) };
        let Ok(key_code) = u16::try_from(key_code)
            .ok()
            .filter(|key| *key < 128)
            .ok_or(())
        else {
            journal.enter_overflow();
            journal.settle_overflow();
            drop(journal);
            fail_recovery_edge_journal(context);
            return CurrentEdgeDisposition::Owned;
        };
        let repeat = event_type == ffi::K_CG_EVENT_KEY_DOWN
            && unsafe {
                ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT) != 0
            };
        edge.keyboard_type = unsafe {
            ffi::CGEventGetIntegerValueField(event, ffi::K_CG_KEYBOARD_EVENT_KEYBOARD_TYPE)
        };

        let tracked_was_down = if source.is_physical() {
            context.keyboard.try_lock().ok().map(|keyboard| {
                if event_type == ffi::K_CG_EVENT_FLAGS_CHANGED {
                    modifier_side_for_key_code(key_code)
                        .is_some_and(|side| keyboard.modifiers.sides().contains(side))
                } else {
                    keyboard.physical.is_held(key_code)
                }
            })
        } else {
            Some(journal.key_was_down(source, source_pid, key_code))
        };
        if source.is_physical()
            && event_type == ffi::K_CG_EVENT_FLAGS_CHANGED
            && let Some(was_down) = tracked_was_down
        {
            journal.seed_modifier_side(source, source_pid, key_code, was_down);
        }
        let mut foreground_was_down = tracked_was_down.unwrap_or(false);
        let mut source_model_available = true;
        let is_down = if event_type == ffi::K_CG_EVENT_KEY_UP {
            false
        } else if event_type == ffi::K_CG_EVENT_FLAGS_CHANGED {
            let physical_hid =
                (source == InputSource::Physical).then(|| native_key_is_down(key_code));
            match journal.modifier_side_transition(source, source_pid, key_code, physical_hid) {
                Some((transition, side_was_down)) => {
                    foreground_was_down = side_was_down;
                    transition
                }
                None if source == InputSource::External => {
                    // External flagsChanged is replayed from its exact scalar
                    // event shape; phase modeling is not required for ordering.
                    source_model_available = false;
                    false
                }
                None => {
                    journal.enter_overflow();
                    journal.settle_overflow();
                    drop(journal);
                    fail_recovery_edge_journal(context);
                    return CurrentEdgeDisposition::Owned;
                }
            }
        } else {
            true
        };
        if is_down && !repeat {
            // A nonrepeat down is a new deferred generation. The recovery
            // classifier may already have updated the native tracker for this
            // exact edge; that post-edge state is not a foreground baseline.
            foreground_was_down = false;
        }
        let modeled_phase = source_model_available.then(|| {
            journal.observe_key_phase(
                source,
                source_pid,
                key_code,
                is_down,
                repeat,
                foreground_was_down,
            )
        });
        let (generation, foreground_balance, hidden_generation) = match modeled_phase.flatten() {
            Some(phase) => phase,
            None if source == InputSource::External => (0, false, false),
            None => {
                journal.enter_overflow();
                journal.settle_overflow();
                drop(journal);
                fail_recovery_edge_journal(context);
                return CurrentEdgeDisposition::Owned;
            }
        };
        edge.key_code = key_code;
        edge.repeat = repeat;
        edge.is_down = is_down;
        edge.generation = generation;
        edge.foreground_balance = foreground_balance;
        edge.hidden_generation = hidden_generation;
        if source.is_physical()
            && let Ok(mut keyboard) = context.keyboard.try_lock()
        {
            if event_type == ffi::K_CG_EVENT_FLAGS_CHANGED {
                let _ = keyboard.modifiers.observe_flags_changed(key_code, is_down);
            } else {
                let phase = if is_down {
                    KeyPhase::Down
                } else {
                    KeyPhase::Up
                };
                let _ = keyboard.physical.observe(key_code, phase);
            }
        }
        #[cfg(feature = "transactional-shortcuts-dev")]
        if context.test_physical_seam_enabled && injection::is_test_physical_marker(original_marker)
        {
            crate::platform::macos::observe_macos_test_physical_key(key_code, is_down);
        }
    }

    let appended = journal.append(edge);
    let overflow = journal.overflow;
    journal.settle_overflow();
    drop(journal);
    if !appended || overflow {
        // Capacity loss is terminal for admission, but only concrete balancing
        // suffixes and already-submitted exact observations retain lifetime.
        fail_recovery_edge_journal(context);
    } else {
        arm_maintenance_timer(context);
    }
    CurrentEdgeDisposition::Owned
}
