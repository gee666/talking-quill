//! Paste barrier contracts.

use super::*;

#[test]
fn paste_modifier_epoch_and_barrier_reject_edges_on_either_side() {
    assert!(paste_modifier_barrier_valid(Some(7), 7, true, true, true));
    assert!(!paste_modifier_barrier_valid(Some(7), 8, true, true, true));
    assert!(!paste_modifier_barrier_valid(Some(7), 7, false, true, true));
    assert!(!paste_modifier_barrier_valid(Some(7), 7, true, false, true));
    assert!(!paste_modifier_barrier_valid(Some(7), 7, true, true, false));
    assert!(!paste_modifier_barrier_valid(None, 7, true, true, true));
}

#[test]
fn production_barrier_callback_rejects_focus_and_modifier_edges_after_validation() {
    fn run(invalidate_focus: bool, mutate_modifier: bool) -> (PasteResult, u64) {
        let barrier = injection::OperationToken::for_test(if invalidate_focus { 901 } else { 902 });
        let event = tagged_keyboard_event(ffi::K_CG_EVENT_KEY_UP, 127, 0, barrier.marker());
        let (mut context, _outbound, _commands) = test_context();
        install_event_source_identity(&mut context, event);
        let handle = crate::platform::macos::target::target_handle_for_test(7);
        let cache = TargetCache::with_open_validation_queue_for_test();
        cache.install_current_handle_for_test(handle, 11, 13);
        let insertion_request = cache.prepare_insertion(
            handle,
            11,
            13,
            1,
            crate::platform::ClipboardTextHash::from_bytes([7; 32]),
            Instant::now() + Duration::from_secs(1),
        );
        context.target_cache = Some(cache);
        let state = Arc::new(AtomicU8::new(PasteCommandState::Waiting as u8));
        let result = Arc::new(crate::platform::macos::PasteResultSlot::new());
        let (acknowledgement, _observed) = bounded(1);
        *context.pending_paste.lock().unwrap() = Some(PendingPaste {
            state,
            result: Arc::clone(&result),
            acknowledgement,
            evidence: handle,
            expected_clipboard_sha256: crate::platform::ClipboardTextHash::from_bytes([7; 32]),
            validation_request: None,
            validated_target_epoch: Some(11),
            validated_target_boundary_epoch: Some(13),
            validated_selected_range_epoch: Some(1),
            insertion_request,
            neutral_modifier_epoch: Some(1),
            neutral_barrier_state: 1,
            neutral_barrier_token: Some(barrier),
            deadline: Instant::now() + Duration::from_secs(1),
            injection_cutoff: Instant::now() + Duration::from_secs(1),
            modifier_wait: ModifierNeutralWait::new(Arc::clone(&context.observability)),
        });
        if mutate_modifier {
            let modifier_edge = tagged_keyboard_event(
                ffi::K_CG_EVENT_FLAGS_CHANGED,
                LEFT_CONTROL_KEY_CODE,
                ffi::K_CG_EVENT_FLAG_MASK_CONTROL,
                TEST_RECOVERY_PHYSICAL_MARKER,
            );
            let _ = unsafe {
                event_tap_callback(
                    null_mut(),
                    ffi::K_CG_EVENT_FLAGS_CHANGED,
                    modifier_edge,
                    (&raw mut context).cast(),
                )
            };
            unsafe { ffi::CFRelease(modifier_edge.cast_const()) };
        }
        if invalidate_focus {
            for event_type in [ffi::K_CG_EVENT_KEY_DOWN, ffi::K_CG_EVENT_KEY_UP] {
                let focus_edge =
                    tagged_keyboard_event(event_type, 123, 0, TEST_RECOVERY_PHYSICAL_MARKER);
                let _ = unsafe {
                    event_tap_callback(
                        null_mut(),
                        event_type,
                        focus_edge,
                        (&raw mut context).cast(),
                    )
                };
                unsafe { ffi::CFRelease(focus_edge.cast_const()) };
            }
        }
        let returned = unsafe {
            event_tap_callback(
                null_mut(),
                ffi::K_CG_EVENT_KEY_UP,
                event,
                (&raw mut context).cast(),
            )
        };
        assert!(
            returned.is_null(),
            "authenticated barrier remains helper-owned"
        );
        let authoritative = result
            .result()
            .expect("barrier rejection completes request");
        unsafe { ffi::CFRelease(event.cast_const()) };
        (
            authoritative,
            context
                .observability
                .snapshot()
                .native_paste
                .target_validation_fallbacks,
        )
    }

    assert_eq!(
        run(true, false),
        (failed_paste(PasteFailure::Unavailable), 1),
        "focus/workspace epoch change before barrier up counts one target fallback"
    );
    assert_eq!(
        run(false, true),
        (failed_paste(PasteFailure::ConflictingModifiers), 0),
        "modifier edge at barrier up is not a target fallback"
    );
}
