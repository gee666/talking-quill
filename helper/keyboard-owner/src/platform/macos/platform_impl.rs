//! Public platform adapter and bounded native shutdown.
use super::*;

impl Platform for NativePlatform {
    fn start(
        outbound: Sender<NativeEvent>,
        gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
        capture_gate: ActivationCaptureGate,
    ) -> Result<Self, PlatformError> {
        let state = Arc::new(SharedState::new());
        let observability = Arc::new(TransactionObservability::new());
        let (owner_commands, owner_command_receiver) = bounded(8);
        let (paste_commands, paste_command_receiver) = bounded(1);
        let (thread, owner_completion) = event_tap::start_hook(
            Arc::clone(&state),
            owner_command_receiver,
            paste_command_receiver,
            outbound,
            Arc::clone(&gate),
            Arc::clone(&terminal),
            Arc::clone(&observability),
            capture_gate.is_open(),
        )?;
        Ok(Self {
            state,
            gate,
            terminal,
            observability,
            owner_commands,
            paste_commands,
            owner_completion,
            thread: Some(thread),
            shutdown_observability_quiescent: true,
            capture_gate,
        })
    }

    fn hook_status(&self) -> HookStatus {
        if self.terminal.is_triggered() {
            HookStatus::Unavailable
        } else {
            hook_status_from_u8(self.state.hook_status.load(Ordering::Acquire))
        }
    }

    fn configure_activation(
        &self,
        enabled: bool,
        bindings: ActivationBindings,
    ) -> Result<(), PlatformError> {
        let enabled = self.capture_gate.filter_enabled(enabled);
        if !matches!(
            self.hook_status(),
            HookStatus::InstalledUnobserved | HookStatus::PhysicalObserved
        ) {
            return if enabled {
                Err(PlatformError::HookUnavailable)
            } else {
                Ok(())
            };
        }
        if enabled && !permissions_allow_native_input(accessibility::permission_snapshot()) {
            if self.suspend_native_input().is_err() {
                self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
                return Err(PlatformError::NativeFailure);
            }
            return Err(PlatformError::PermissionDenied);
        }
        self.submit_owner_mutation(
            OwnerMutation::configure(ActivationConfig { enabled, bindings }),
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
        if mode == SessionCaptureMode::Off && !self.gate.is_open() {
            self.state.quiescing.store(true, Ordering::Release);
        }
        if !matches!(
            self.hook_status(),
            HookStatus::InstalledUnobserved | HookStatus::PhysicalObserved
        ) {
            if mode == SessionCaptureMode::Off {
                return Ok(());
            }
            return Err(PlatformError::HookUnavailable);
        }
        if mode != SessionCaptureMode::Off
            && !permissions_allow_native_input(accessibility::permission_snapshot())
        {
            if self.suspend_native_input().is_err() {
                self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
                return Err(PlatformError::NativeFailure);
            }
            return Err(PlatformError::PermissionDenied);
        }
        self.submit_owner_mutation(
            OwnerMutation::set_session_capture(mode),
            TerminalReason::OwnerThreadUnresponsive,
        )
    }

    fn inject_paste(&self) -> PasteResult {
        // Protocol v8 requires an activation context. Never downgrade to an
        // untargeted paste when a caller uses the legacy trait boundary.
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
        accessibility::front_app()
    }

    fn permissions(&self) -> Permissions {
        let permissions = accessibility::permission_snapshot();
        // PermissionRequired means startup never installed a tap and therefore
        // owns no native edge. Polling must remain a side-effect-free retry
        // path; attempting owner suspension here would incorrectly terminalize
        // the still-live helper.
        if permission_poll_requires_suspension(self.hook_status(), permissions) {
            // The owner acknowledgement is the fail-open linearization point.
            // Failure is itself terminal; never return as though suspension
            // succeeded while native ownership may remain.
            if self.suspend_native_input().is_err() {
                self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
            }
        }
        permissions
    }

    fn transaction_observability(&self) -> TransactionObservabilitySnapshot {
        self.observability.snapshot()
    }

    fn native_work_pending(&self) -> bool {
        self.state.pending_native_work.load(Ordering::Acquire)
    }

    fn shutdown(&mut self) -> PlatformShutdown {
        self.gate.close();
        self.state
            .session_capture_mode
            .store(SessionCaptureMode::Off.as_u8(), Ordering::Release);
        let Some(thread) = self.thread.take() else {
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
        };

        // PermissionRequired/no-tap startup owns no native edge and normally
        // finishes before shutdown is requested. Treat that completed owner as
        // clean even though no run-loop wake endpoint was ever published.
        let already_completed = owner_is_already_quiescent(&self.owner_completion, &thread);
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
        if !already_completed {
            let _ = event_tap::request_stop(&self.state);
        }
        let completion_deadline = deadline + OWNER_TERMINAL_COMPLETION_MARGIN;
        let completed = already_completed
            || owner_completed(
                &self.owner_completion,
                completion_deadline.saturating_duration_since(Instant::now()),
            )
            || thread.is_finished();
        while completed && !thread.is_finished() && Instant::now() < completion_deadline {
            std::thread::yield_now();
        }
        if completed && thread.is_finished() {
            if thread.join().is_ok() {
                self.state
                    .hook_status
                    .store(hook_status_to_u8(HookStatus::Stopped), Ordering::Release);
            } else {
                self.mark_owner_failure(TerminalReason::HookStopped);
            }
        } else {
            // A missing endpoint is clean only when the already-quiescent owner
            // actually completed. Live owners still get exactly one deadline.
            self.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
            self.shutdown_observability_quiescent = false;
            self.mark_owner_failure(TerminalReason::OwnerThreadUnresponsive);
            drop(thread);
        }
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
