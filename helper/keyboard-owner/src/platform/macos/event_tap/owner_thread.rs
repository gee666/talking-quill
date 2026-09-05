//! Native source installation and owner run-loop lifetime.

use super::*;

pub(super) fn hook_thread(
    context: CallbackContext,
    ready: Sender<Result<(), PlatformError>>,
    startup_state: Arc<AtomicU8>,
    owner_completion: Sender<()>,
) {
    // Declared first so cleanup completion is published only after all owner
    // resources and callback context have been dropped.
    let _owner_completion = OwnerCompletion(owner_completion);
    let mut context = Box::new(context);
    let injection_identity = match injection::InjectionIdentity::new() {
        Ok(identity) => identity,
        Err(error) => {
            context.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
            context.state.quiescing.store(true, Ordering::Release);
            context.state.stopping.store(true, Ordering::Release);
            context.gate.close();
            let _ = ready.send(Err(error));
            return;
        }
    };
    context.injection_identity = Some(injection_identity);
    let native_events = match injection::NativeEventPool::new(injection_identity) {
        Ok(pool) => pool,
        Err(_) => {
            context.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
            context.state.quiescing.store(true, Ordering::Release);
            context.state.stopping.store(true, Ordering::Release);
            context.gate.close();
            let _ = ready.send(Err(PlatformError::NativeFailure));
            return;
        }
    };
    match context.native_events.get_mut() {
        Ok(slot) => *slot = Some(native_events),
        Err(_) => {
            context.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
            context.state.quiescing.store(true, Ordering::Release);
            context.state.stopping.store(true, Ordering::Release);
            context.gate.close();
            let _ = ready.send(Err(PlatformError::NativeFailure));
            return;
        }
    }
    let callback_context = (&raw mut *context).cast::<c_void>();
    let mask = (1_u64 << ffi::K_CG_EVENT_LEFT_MOUSE_DOWN)
        | (1_u64 << ffi::K_CG_EVENT_LEFT_MOUSE_UP)
        | (1_u64 << ffi::K_CG_EVENT_RIGHT_MOUSE_DOWN)
        | (1_u64 << ffi::K_CG_EVENT_RIGHT_MOUSE_UP)
        | (1_u64 << ffi::K_CG_EVENT_OTHER_MOUSE_DOWN)
        | (1_u64 << ffi::K_CG_EVENT_OTHER_MOUSE_UP)
        | (1_u64 << ffi::K_CG_EVENT_KEY_DOWN)
        | (1_u64 << ffi::K_CG_EVENT_KEY_UP)
        | (1_u64 << ffi::K_CG_EVENT_FLAGS_CHANGED);
    // SAFETY: callback context remains boxed until every source and tap has
    // been removed, invalidated, and released below.
    let tap = unsafe {
        ffi::CGEventTapCreate(
            ffi::K_CG_SESSION_EVENT_TAP,
            ffi::K_CG_HEAD_INSERT_EVENT_TAP,
            event_tap_options(context.suppression_enabled),
            mask,
            Some(event_tap_callback),
            callback_context,
        )
    };
    if tap.is_null() {
        let permissions = permission_snapshot();
        let status = if permissions.input_monitoring == PermissionState::Denied
            || permissions.accessibility == PermissionState::Denied
        {
            HookStatus::PermissionRequired
        } else {
            HookStatus::Unavailable
        };
        context
            .state
            .hook_status
            .store(hook_status_to_u8(status), Ordering::Release);
        context.state.quiescing.store(true, Ordering::Release);
        context.state.stopping.store(true, Ordering::Release);
        context.gate.close();
        let _ = ready.send(Ok(()));
        return;
    }
    context.state.event_tap.store(tap, Ordering::Release);

    // SAFETY: `tap` is a valid CFMachPort returned above.
    let tap_source = unsafe { ffi::CFMachPortCreateRunLoopSource(null(), tap, 0) };
    if tap_source.is_null() {
        cleanup_tap_without_sources(&context, tap);
        context.state.quiescing.store(true, Ordering::Release);
        context.state.stopping.store(true, Ordering::Release);
        context.gate.close();
        let _ = ready.send(Ok(()));
        return;
    }

    let mut source_context = ffi::CFRunLoopSourceContext {
        version: 0,
        info: callback_context,
        retain: None,
        release: None,
        copy_description: None,
        equal: None,
        hash: None,
        schedule: None,
        cancel: None,
        perform: Some(owner_command_perform),
    };
    // SAFETY: Core Foundation copies the version-0 context. Its info pointer
    // remains valid for the source's complete lifetime.
    let command_source = unsafe { ffi::CFRunLoopSourceCreate(null(), 0, &raw mut source_context) };
    if command_source.is_null() {
        // SAFETY: tap sources and taps are owned and no run loop references them.
        unsafe {
            ffi::CFRelease(tap_source.cast_const());
        }
        cleanup_tap_without_sources(&context, tap);
        context.state.quiescing.store(true, Ordering::Release);
        context.state.stopping.store(true, Ordering::Release);
        context.gate.close();
        let _ = ready.send(Ok(()));
        return;
    }

    let mut timer_context = ffi::CFRunLoopTimerContext {
        version: 0,
        info: callback_context,
        retain: None,
        release: None,
        copy_description: None,
    };
    // SAFETY: Core Foundation copies the context and retains no ownership of
    // the boxed callback pointer. The first fire is parked until paste or
    // shutdown work explicitly arms it.
    let maintenance_timer = unsafe {
        ffi::CFRunLoopTimerCreate(
            null(),
            ffi::CFAbsoluteTimeGetCurrent() + PARKED_TIMER_SECONDS,
            MAINTENANCE_INTERVAL_SECONDS,
            0,
            0,
            Some(maintenance_timer_callback),
            &raw mut timer_context,
        )
    };
    if maintenance_timer.is_null() {
        // SAFETY: neither source is installed in a run loop yet.
        unsafe {
            ffi::CFRunLoopSourceInvalidate(command_source);
            ffi::CFRelease(command_source.cast_const());
            ffi::CFRelease(tap_source.cast_const());
        }
        cleanup_tap_without_sources(&context, tap);
        context.state.quiescing.store(true, Ordering::Release);
        context.state.stopping.store(true, Ordering::Release);
        context.gate.close();
        let _ = ready.send(Ok(()));
        return;
    }
    context
        .state
        .maintenance_timer
        .store(maintenance_timer, Ordering::Release);

    // SAFETY: called on the future run-loop owner thread.
    let run_loop = unsafe { ffi::CFRunLoopGetCurrent() };
    // SAFETY: all objects are valid and remain alive through CFRunLoopRun.
    unsafe {
        ffi::CFRunLoopAddSource(run_loop, tap_source, ffi::kCFRunLoopCommonModes);
        ffi::CFRunLoopAddSource(run_loop, command_source, ffi::kCFRunLoopCommonModes);
        ffi::CFRunLoopAddTimer(run_loop, maintenance_timer, ffi::kCFRunLoopCommonModes);
    }
    context.target_cache = TargetCache::start().ok();
    if let Ok(keyboard) = context.keyboard.get_mut() {
        keyboard.seed_from_state(native_key_is_down);
        keyboard.transactional = TransactionEngine::with_physical_snapshot(
            CompiledActivationConfig::default(),
            physical_snapshot(keyboard),
        )
        .with_menu_neutralization_policy(MenuNeutralizationPolicy::NotRequired);
        keyboard.dispatcher.initialize_process_epoch();
    } else {
        context.terminal.trigger(TerminalReason::ReducerPoisoned);
    }
    // Retain one permanent wake reference to each startup resource. The owner
    // may invalidate its operational references after drain, but request_stop
    // can always signal valid CF objects without a lock/use-after-free race.
    unsafe {
        ffi::CFRetain(run_loop.cast_const());
        ffi::CFRetain(command_source.cast_const());
        ffi::CFRetain(maintenance_timer.cast_const());
    }
    context
        .state
        .owner_run_loop
        .store(run_loop, Ordering::Release);
    context
        .state
        .owner_command_source
        .store(command_source, Ordering::Release);
    context
        .state
        .owner_wake_timer
        .store(maintenance_timer, Ordering::Release);
    #[cfg(feature = "transactional-shortcuts-dev")]
    if context.test_tap_disable_request.is_some() {
        unsafe {
            ffi::CFRunLoopTimerSetNextFireDate(
                maintenance_timer,
                ffi::CFAbsoluteTimeGetCurrent() + MAINTENANCE_INTERVAL_SECONDS,
            );
        }
    }
    let _resources = OwnerNativeResourceGuard {
        context: (&raw mut *context),
        run_loop,
        tap,
        tap_source,
        command_source,
        maintenance_timer,
    };
    // The AX cache worker and process epoch are initialized before the tap can
    // invoke the latency-sensitive callback. Catch every owner-loop unwind
    // while the RAII guard still owns a valid callback refcon and resources.
    let owner_lifecycle = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        unsafe {
            ffi::CGEventTapEnable(tap, true);
        };
        if claim_startup(&startup_state) {
            context.state.hook_status.store(
                hook_status_to_u8(HookStatus::InstalledUnobserved),
                Ordering::Release,
            );
            if ready.send(Ok(())).is_ok() && !context.state.stopping.load(Ordering::Acquire) {
                unsafe { ffi::CFRunLoopRun() };
            } else {
                context.gate.close();
            }
        } else {
            context.gate.close();
        }

        while pending_native_work(&context) {
            arm_maintenance_timer(&context);
            unsafe { ffi::CFRunLoopRun() };
        }
    }));
    if owner_lifecycle.is_err() {
        enter_owner_lifecycle_recovery(&context);
        // A second owner/recovery unwind is contained and retried using the
        // permanent timer/source. Resources stay owned until drain proves safe.
        while pending_native_work(&context) {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // Recheck after every attempt. A failed attempt may have a
                // submitted effect in flight; it must not fall through into a
                // second control/shutdown turn.
                let recovered = attempt_pending_recovery(&context);
                if recovered {
                    begin_owner_shutdown(&context);
                }
                arm_maintenance_timer(&context);
                unsafe { ffi::CFRunLoopRun() };
            }));
        }
    }

    context.gate.close();
    context
        .state
        .session_capture_mode
        .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
    while let Ok(command) = context.owner_commands.try_recv() {
        let _ = cancel_owner_command(&command.state);
        let _ = command
            .acknowledgement
            .try_send(Err(PlatformError::ThreadStopped));
    }
    let _ = resolve_pending_activation(&context, PendingActivationResolution::FailDelivery);
    cancel_pending_pastes(&context);
    while let Ok(command) = context.paste_commands.try_recv() {
        let _ = cancel_paste_command(&command.state);
        let _ = command
            .result
            .publish(failed_paste(PasteFailure::Unavailable));
        let _ = command.acknowledgement.try_send(());
    }
    if !context.state.stopping.load(Ordering::Acquire) {
        context.terminal.trigger(TerminalReason::HookStopped);
    }
    // `_resources` drops here before `context`, enforcing ordered native
    // teardown on clean return and every contained owner unwind.
}
