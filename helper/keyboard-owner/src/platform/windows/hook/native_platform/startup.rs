//! Start the auxiliary monitor and arbitrate keyboard-owner readiness.
use super::*;

impl NativePlatform {
    pub(super) fn start_owner(
        outbound: Sender<NativeEvent>,
        gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
        capture_gate: ActivationCaptureGate,
    ) -> Result<Self, PlatformError> {
        // DPI awareness improves target geometry but is not keyboard authority.
        // Windows can reject this when process policy established awareness
        // earlier; registered keybindings must remain available regardless.
        // SAFETY: this documented pseudo-handle selects process DPI policy;
        // it is not dereferenced or owned by the caller.
        let _ = DPI_AWARENESS_READY.get_or_init(|| unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) != 0
        });

        let state = Arc::new(SharedState::new());
        let observability = Arc::new(TransactionObservability::new());
        // Core Audio is auxiliary. A missing endpoint service or COM failure
        // must not prevent installation of the keyboard hook.
        let (_audio_terminal_gate, audio_terminal) = isolated_audio_terminal();
        // Publication shares the global gate so shutdown closes every callback
        // class atomically. Audio failures use the separate terminal above and
        // therefore cannot close keyboard admission.
        let mut audio_monitor =
            AudioDeviceMonitor::start(outbound.clone(), Arc::clone(&gate), audio_terminal).ok();
        let Some(injection_markers) = injection::InjectionMarkers::generate() else {
            if let Some(monitor) = audio_monitor.as_mut() {
                let _ = monitor.shutdown();
            }
            return Err(PlatformError::HookUnavailable);
        };
        let context = CallbackContext {
            state: Arc::clone(&state),
            keyboard: Mutex::new(CallbackKeyboard::default()),
            owner_epoch: Instant::now(),
            suppression_enabled: capture_gate.is_open(),
            outbound,
            gate: Arc::clone(&gate),
            terminal: Arc::clone(&terminal),
            observability: Arc::clone(&observability),
            injection_markers,
            replay_sender: None,
            replay_accepted: Arc::new(AtomicU64::new(0)),
        };
        let (ready_tx, ready_rx) = bounded(1);
        let startup_state = Arc::new(AtomicU8::new(StartupState::Pending as u8));
        let (owner_completion_tx, owner_completion) = bounded(1);
        let (owner_commands, owner_command_receiver) = bounded(8);
        let (paste_commands, paste_command_receiver) = bounded(1);
        let owner_startup_state = Arc::clone(&startup_state);
        let thread = match thread::Builder::new()
            .name("talking-quill-helper-win-hook".into())
            .spawn(move || {
                hook_thread(
                    context,
                    ready_tx,
                    owner_startup_state,
                    owner_command_receiver,
                    paste_command_receiver,
                    owner_completion_tx,
                );
            }) {
            Ok(thread) => thread,
            Err(_) => {
                if let Some(monitor) = audio_monitor.as_mut() {
                    let _ = monitor.shutdown();
                }
                return Err(PlatformError::ThreadStopped);
            }
        };

        let thread_id = match ready_rx.recv_timeout(OWNER_COMPLETION_TIMEOUT) {
            Ok(Ok(thread_id)) => thread_id,
            Ok(Err(error)) => {
                if let Some(monitor) = audio_monitor.as_mut() {
                    monitor.begin_shutdown();
                }
                let _ = join_completed_owner(thread, &owner_completion, OWNER_COMPLETION_TIMEOUT);
                if let Some(monitor) = audio_monitor.as_mut() {
                    let _ = monitor.shutdown();
                }
                return Err(error);
            }
            Err(_) => {
                gate.close();
                if let Some(monitor) = audio_monitor.as_mut() {
                    monitor.begin_shutdown();
                }
                state.stopping.store(true, Ordering::Release);
                state.hook_status.store(
                    hook_status_to_u8(HookStatus::Unavailable),
                    Ordering::Release,
                );
                if cancel_startup(&startup_state) == StartupState::Running
                    && let Ok(Ok(late_thread_id)) = ready_rx.recv_timeout(OWNER_COMPLETION_TIMEOUT)
                {
                    let _ = post_owner_message(late_thread_id, WM_QUIT);
                }
                drop(ready_rx);
                let _ = join_completed_owner(thread, &owner_completion, OWNER_COMPLETION_TIMEOUT);
                if let Some(monitor) = audio_monitor.as_mut() {
                    let _ = monitor.shutdown();
                }
                return Err(PlatformError::ThreadStopped);
            }
        };

        if terminal.is_triggered() {
            gate.close();
            if let Some(monitor) = audio_monitor.as_mut() {
                monitor.begin_shutdown();
            }
            let _ = post_owner_message(thread_id, WM_QUIT);
            let _ = join_completed_owner(thread, &owner_completion, OWNER_COMPLETION_TIMEOUT);
            if let Some(monitor) = audio_monitor.as_mut() {
                let _ = monitor.shutdown();
            }
            return Err(PlatformError::NativeFailure);
        }

        let platform = Self {
            state,
            gate,
            terminal,
            observability,
            audio_monitor,
            owner_commands,
            paste_commands,
            thread_id,
            owner_completion,
            thread: Some(thread),
            shutdown_completed: false,
            shutdown_observability_quiescent: true,
            capture_gate,
        };
        #[cfg(test)]
        if std::env::var_os("TALKING_QUILL_TEST_UNMATCHED_DOWN").is_some() {
            platform.inject_test_unmatched_down()?;
        }
        Ok(platform)
    }
}
