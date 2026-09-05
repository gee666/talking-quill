//! Platform lifecycle and serialized public owner commands.
use super::*;

mod startup;
mod submission;

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
        Self::start_owner(outbound, gate, terminal, capture_gate)
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
