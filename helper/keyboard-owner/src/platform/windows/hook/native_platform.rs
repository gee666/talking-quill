//! Platform lifecycle and serialized public owner commands.
use super::*;

pub struct NativePlatform {
    pub(super) state: Arc<SharedState>,
    pub(super) gate: Arc<CallbackGate>,
    pub(super) terminal: Arc<TerminalSignal>,
    pub(super) observability: Arc<TransactionObservability>,
    pub(super) audio_monitor: Option<AudioDeviceMonitor>,
    pub(super) owner_commands: Sender<OwnerCommand>,
    pub(super) paste_commands: Sender<PasteCommand>,
    pub(super) thread_id: u32,
    pub(super) owner_completion: Receiver<()>,
    pub(super) thread: Option<JoinHandle<()>>,
    pub(super) shutdown_completed: bool,
    pub(super) shutdown_observability_quiescent: bool,
    pub(super) capture_gate: ActivationCaptureGate,
}

impl NativePlatform {
    pub(super) fn submit_owner_mutation(
        &self,
        mutation: OwnerMutation,
        terminal_reason: TerminalReason,
    ) -> Result<(), PlatformError> {
        if self.state.stopping.load(Ordering::Acquire)
            || self.thread.is_none()
            || self.terminal.is_triggered()
        {
            return Err(PlatformError::ThreadStopped);
        }

        let command_state = Arc::new(AtomicU8::new(OwnerCommandState::Pending as u8));
        let (acknowledgement, response) = bounded(1);
        self.owner_commands
            .send_timeout(
                OwnerCommand {
                    mutation,
                    state: Arc::clone(&command_state),
                    acknowledgement,
                },
                OWNER_COMMAND_TIMEOUT,
            )
            .map_err(|_| PlatformError::NativeFailure)?;

        if !post_owner_message(self.thread_id, WM_OWNER_COMMAND)
            && cancel_owner_command(&command_state) == OwnerCommandState::Cancelled
        {
            self.mark_owner_failure(terminal_reason);
            return Err(PlatformError::NativeFailure);
        }

        if let Ok(result) = response.recv_timeout(OWNER_COMMAND_TIMEOUT) {
            return result;
        }

        match cancel_owner_command(&command_state) {
            OwnerCommandState::Applied => Ok(()),
            OwnerCommandState::Applying => {
                if let Ok(result) = response.recv_timeout(OWNER_COMMAND_TIMEOUT) {
                    return result;
                }
                if owner_command_state(&command_state) == OwnerCommandState::Applied {
                    Ok(())
                } else {
                    self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
                    Err(PlatformError::NativeFailure)
                }
            }
            OwnerCommandState::Pending | OwnerCommandState::Cancelled => {
                self.mark_owner_failure(terminal_reason);
                Err(PlatformError::NativeFailure)
            }
        }
    }

    pub(super) fn submit_paste(
        &self,
        context: ActivationContext,
        expected_clipboard_sha256: ClipboardTextHash,
    ) -> PasteResult {
        if self.state.stopping.load(Ordering::Acquire)
            || self.thread.is_none()
            || self.terminal.is_triggered()
            || !self
                .state
                .target_change_evidence_ready
                .load(Ordering::Acquire)
        {
            return failed_paste(PasteFailure::Unavailable);
        }
        let state = Arc::new(AtomicU8::new(PasteCommandState::Pending as u8));
        let result = Arc::new(PasteResultSlot::new());
        let (acknowledgement, response) = bounded(1);
        let submitted_at = Instant::now();
        let injection_deadline = submitted_at + PASTE_NEUTRAL_TIMEOUT;
        let response_deadline = submitted_at + PASTE_COMMAND_TIMEOUT;
        if self
            .paste_commands
            .send_timeout(
                PasteCommand {
                    context,
                    expected_clipboard_sha256,
                    injection_deadline,
                    state: Arc::clone(&state),
                    result: Arc::clone(&result),
                    acknowledgement,
                },
                OWNER_COMMAND_TIMEOUT,
            )
            .is_err()
            || !post_owner_message(self.thread_id, WM_OWNER_PASTE)
        {
            let _ = cancel_paste_command(&state);
            return failed_paste(PasteFailure::Unavailable);
        }
        let _ = response.recv_timeout(response_deadline.saturating_duration_since(Instant::now()));
        if let Some(result) = result.result() {
            return result;
        }
        match cancel_paste_command(&state) {
            PasteCommandState::Injecting => {
                // Once Waiting->Injecting wins, duplicate fallback is unsafe:
                // SendInput has irreversible authority. Wait only for the
                // bounded post-CAS completion budget. If the native call does
                // not return, fail the owner closed and conservatively report
                // submitted so the host cannot issue another insertion.
                let completion = wait_for_claimed_paste_completion(
                    &response,
                    &result,
                    PASTE_POST_CAS_COMPLETION_TIMEOUT,
                );
                if completion.reason == Some(PasteFailure::Indeterminate) {
                    self.state.hook_status.store(
                        hook_status_to_u8(HookStatus::Unavailable),
                        Ordering::Release,
                    );
                    self.state.post_claim_timeout.store(true, Ordering::Release);
                    self.gate.close();
                }
                completion
            }
            PasteCommandState::Committed
            | PasteCommandState::Applied
            | PasteCommandState::ResultReady => {
                if let Some(result) = result.result() {
                    result
                } else {
                    self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
                    failed_paste(PasteFailure::Unavailable)
                }
            }
            PasteCommandState::Pending
            | PasteCommandState::Waiting
            | PasteCommandState::Cancelled => result
                .result()
                .unwrap_or_else(|| failed_paste(PasteFailure::Unavailable)),
        }
    }

    #[cfg(test)]
    pub(super) fn inject_test_unmatched_down(&self) -> Result<(), PlatformError> {
        self.submit_owner_mutation(
            OwnerMutation::inject_test_unmatched_down(),
            TerminalReason::OwnerThreadUnresponsive,
        )
    }

    pub(super) fn mark_owner_failure(&self, reason: TerminalReason) {
        self.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        self.terminal.trigger(reason);
    }
}

pub(super) fn isolated_audio_terminal() -> (Arc<CallbackGate>, Arc<TerminalSignal>) {
    let gate = Arc::new(CallbackGate::new());
    gate.open();
    let (sender, _events) = bounded(1);
    let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), sender));
    (gate, terminal)
}

impl Platform for NativePlatform {
    fn start(
        outbound: Sender<NativeEvent>,
        gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
        capture_gate: ActivationCaptureGate,
    ) -> Result<Self, PlatformError> {
        // DPI awareness improves target geometry but is not keyboard authority.
        // Windows can reject this when process policy established awareness
        // earlier; registered keybindings must remain available regardless.
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

    fn hook_status(&self) -> HookStatus {
        if self.terminal.is_triggered() {
            HookStatus::Unavailable
        } else {
            hook_status_from_u8(self.state.hook_status.load(Ordering::Acquire))
        }
    }

    fn protocol_initialized(&self) {
        self.state
            .protocol_initialized
            .store(true, Ordering::Release);
        if let Some(monitor) = self.audio_monitor.as_ref() {
            monitor.protocol_initialized();
        }
    }

    fn configure_activation(
        &self,
        enabled: bool,
        bindings: ActivationBindings,
    ) -> Result<(), PlatformError> {
        let enabled = self.capture_gate.filter_enabled(enabled);
        self.submit_owner_mutation(
            OwnerMutation::configure(ActivationConfig { enabled, bindings }),
            TerminalReason::ActivationConfigurationUnavailable,
        )
    }

    fn close_activation_admission(
        &self,
        _bindings: ActivationBindings,
    ) -> Result<(), PlatformError> {
        self.submit_owner_mutation(
            OwnerMutation::close_admission(),
            TerminalReason::ActivationConfigurationUnavailable,
        )
    }

    fn cancel_activation_candidate(&self) -> Result<(), PlatformError> {
        self.submit_owner_mutation(
            OwnerMutation::cancel_candidate(),
            TerminalReason::ActivationConfigurationUnavailable,
        )
    }

    fn set_session_capture(&self, mode: SessionCaptureMode) -> Result<(), PlatformError> {
        let mode = self.capture_gate.filter_session_mode(mode);
        if mode == SessionCaptureMode::Off {
            // Fail open at the caller's commit boundary. The owner command acknowledges the
            // mutation without clearing reducer ownership needed to balance an already-held key.
            self.state
                .session_capture_mode
                .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
        }
        self.submit_owner_mutation(
            OwnerMutation::set_session_capture(mode),
            TerminalReason::OwnerThreadUnresponsive,
        )
    }

    fn inject_paste(&self) -> PasteResult {
        failed_paste(PasteFailure::Unavailable)
    }

    fn inject_paste_for_activation(&self, _context: ActivationContext) -> PasteResult {
        failed_paste(PasteFailure::Unavailable)
    }

    fn inject_paste_for_activation_with_clipboard_hash(
        &self,
        context: ActivationContext,
        expected_clipboard_sha256: ClipboardTextHash,
    ) -> PasteResult {
        self.submit_paste(context, expected_clipboard_sha256)
    }

    fn front_app(&self) -> Result<FrontApp, PlatformError> {
        front_app()
    }

    fn permissions(&self) -> Permissions {
        Permissions {
            accessibility: PermissionState::NotApplicable,
            input_monitoring: PermissionState::NotApplicable,
            event_post: PermissionState::NotApplicable,
        }
    }

    fn transaction_observability(&self) -> TransactionObservabilitySnapshot {
        self.observability.snapshot()
    }

    fn record_adapter_dequeued(&self) {
        self.observability.record_adapter_dequeued();
    }

    fn native_work_pending(&self) -> bool {
        self.state.pending_native_work.load(Ordering::Acquire)
    }

    fn shutdown(&mut self) -> PlatformShutdown {
        if self.shutdown_completed {
            return PlatformShutdown {
                terminal_reason: self.terminal.reason(),
                observability_quiescent: self.shutdown_observability_quiescent,
                terminal_incomplete: self
                    .observability
                    .snapshot()
                    .native_paste
                    .shutdown_ownership_deadlines
                    != 0,
            };
        }
        self.gate.close();
        self.state
            .session_capture_mode
            .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
        let deadline = Instant::now() + SHUTDOWN_DRAIN_TIMEOUT;
        match self.state.shutdown_deadline.lock() {
            Ok(mut installed) => {
                let _ = installed.get_or_insert(deadline);
            }
            Err(poisoned) => {
                let _ = poisoned.into_inner().get_or_insert(deadline);
                self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
            }
        }
        self.state.stopping.store(true, Ordering::Release);
        if let Some(monitor) = self.audio_monitor.as_mut() {
            monitor.begin_shutdown();
        }
        if self.thread.is_some() && !post_owner_message(self.thread_id, WM_QUIT) {
            self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
        }

        if let Some(thread) = self.thread.take() {
            let wait = deadline.saturating_duration_since(Instant::now());
            let completed = owner_completed(&self.owner_completion, wait) || thread.is_finished();
            while completed && !thread.is_finished() && Instant::now() < deadline {
                thread::yield_now();
            }
            if completed && thread.is_finished() {
                if thread.join().is_ok() {
                    self.state
                        .hook_status
                        .store(hook_status_to_u8(HookStatus::Stopped), Ordering::Release);
                } else {
                    self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
                }
            } else {
                // The owner has exhausted the single native deadline. Detach
                // rather than blocking process shutdown or inventing recovery.
                self.shutdown_observability_quiescent = false;
                self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
                drop(thread);
            }
        }
        if let Some(monitor) = self.audio_monitor.as_mut() {
            // Audio shutdown cannot revoke otherwise healthy keyboard capture.
            let _ =
                monitor.shutdown_with_timeout(deadline.saturating_duration_since(Instant::now()));
        }
        self.shutdown_completed = true;
        PlatformShutdown {
            terminal_reason: self.terminal.reason(),
            observability_quiescent: self.shutdown_observability_quiescent,
            terminal_incomplete: self
                .observability
                .snapshot()
                .native_paste
                .shutdown_ownership_deadlines
                != 0,
        }
    }
}

impl Drop for NativePlatform {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
