//! Hook startup handshake and permanent cross-thread stop signalling.

use super::*;

pub(super) const fn event_tap_options(suppression_enabled: bool) -> u32 {
    if suppression_enabled {
        ffi::K_CG_EVENT_TAP_OPTION_DEFAULT
    } else {
        ffi::K_CG_EVENT_TAP_OPTION_LISTEN_ONLY
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::platform::macos) fn start_hook(
    state: Arc<SharedState>,
    owner_commands: Receiver<OwnerCommand>,
    paste_commands: Receiver<PasteCommand>,
    outbound: Sender<NativeEvent>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
    observability: Arc<TransactionObservability>,
    suppression_enabled: bool,
) -> Result<(JoinHandle<()>, Receiver<()>), PlatformError> {
    let stop_state = Arc::clone(&state);
    let stop_gate = Arc::clone(&gate);
    let context = CallbackContext {
        state,
        suppression_enabled,
        keyboard: RecoveringMutex::new(CallbackKeyboard::default()),
        pending_activation: RecoveringMutex::new(None),
        native_events: RecoveringMutex::new(None),
        injection_identity: None,
        test_physical_seam_enabled: cfg!(feature = "transactional-shortcuts-dev")
            && std::env::var_os("TALKING_QUILL_MACOS_TEST_PHYSICAL_SEAM").as_deref()
                == Some(std::ffi::OsStr::new("1")),
        callback_proxy: AtomicPtr::new(null_mut()),
        current_edge_disposition: AtomicU8::new(CurrentEdgeDisposition::Pass as u8),
        owner_commands,
        paste_commands,
        pending_paste: RecoveringMutex::new(None),
        recovery_edges: RecoveringMutex::new(RecoveryEdgeJournal::default()),
        target_cache: None,
        #[cfg(feature = "transactional-shortcuts-dev")]
        test_tap_disable_request: std::env::var_os("TALKING_QUILL_MACOS_TEST_TAP_DISABLE_REQUEST")
            .map(std::path::PathBuf::from),
        #[cfg(feature = "transactional-shortcuts-dev")]
        test_paste_barrier_paused: std::env::var_os(
            "TALKING_QUILL_MACOS_TEST_PASTE_BARRIER_PAUSED",
        )
        .map(std::path::PathBuf::from),
        #[cfg(feature = "transactional-shortcuts-dev")]
        test_paste_barrier_release: std::env::var_os(
            "TALKING_QUILL_MACOS_TEST_PASTE_BARRIER_RELEASE",
        )
        .map(std::path::PathBuf::from),
        #[cfg(feature = "transactional-shortcuts-dev")]
        test_paste_barrier_split_active: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "transactional-shortcuts-dev")]
        test_paste_barrier_down_observed: std::sync::atomic::AtomicBool::new(false),
        #[cfg(feature = "transactional-shortcuts-dev")]
        test_paste_barrier_pause_announced: std::sync::atomic::AtomicBool::new(false),
        #[cfg(test)]
        forced_activation_reservation: None,
        outbound,
        gate,
        terminal,
        observability,
    };
    let (ready_tx, ready_rx) = bounded(1);
    let startup_state = Arc::new(AtomicU8::new(StartupState::Pending as u8));
    let owner_startup_state = Arc::clone(&startup_state);
    let (owner_completion_tx, owner_completion) = bounded(1);
    let thread = thread::Builder::new()
        .name("talking-quill-helper-macos-hook".into())
        .spawn(move || {
            hook_thread(context, ready_tx, owner_startup_state, owner_completion_tx);
        })
        .map_err(|_| PlatformError::ThreadStopped)?;
    match ready_rx.recv_timeout(OWNER_COMPLETION_TIMEOUT) {
        Ok(Ok(())) => Ok((thread, owner_completion)),
        Ok(Err(error)) => {
            stop_gate.close();
            stop_state.quiescing.store(true, Ordering::Release);
            stop_state.stopping.store(true, Ordering::Release);
            if owner_completed(&owner_completion, OWNER_COMPLETION_TIMEOUT) && thread.is_finished()
            {
                let _ = thread.join();
            } else {
                drop(thread);
            }
            Err(error)
        }
        Err(_) => {
            stop_gate.close();
            stop_state.quiescing.store(true, Ordering::Release);
            stop_state.stopping.store(true, Ordering::Release);
            if cancel_startup(&startup_state) == StartupState::Running {
                let _ = ready_rx.recv_timeout(OWNER_COMPLETION_TIMEOUT);
            }
            request_stop(&stop_state);
            if owner_completed(&owner_completion, OWNER_COMPLETION_TIMEOUT) && thread.is_finished()
            {
                let _ = thread.join();
            } else {
                drop(thread);
            }
            Err(PlatformError::ThreadStopped)
        }
    }
}

pub(in crate::platform::macos) fn request_stop(state: &Arc<SharedState>) -> bool {
    state.quiescing.store(true, Ordering::Release);
    state.stopping.store(true, Ordering::Release);
    state
        .session_capture_mode
        .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);

    let source = state.owner_command_source.load(Ordering::Acquire);
    let timer = state.owner_wake_timer.load(Ordering::Acquire);
    let run_loop = state.owner_run_loop.load(Ordering::Acquire);
    // SAFETY: all three objects are permanent owner-startup resources,
    // published before Ready and cleared only after pending native work drains.
    // Signalling and wake-up are thread-safe and require no lock/allocation.
    unsafe {
        if !source.is_null() {
            ffi::CFRunLoopSourceSignal(source);
        }
        if !timer.is_null() {
            ffi::CFRunLoopTimerSetNextFireDate(
                timer,
                ffi::CFAbsoluteTimeGetCurrent() + MAINTENANCE_INTERVAL_SECONDS,
            );
        }
        if !run_loop.is_null() {
            ffi::CFRunLoopWakeUp(run_loop);
        }
    }
    !run_loop.is_null() && (!source.is_null() || !timer.is_null())
}
