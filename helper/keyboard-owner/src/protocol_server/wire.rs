//! Convert internal owner state into validated protocol messages.
use super::*;

pub(super) fn adapter_rejection(event: BrokerEvent, error: &ServerError) -> AdapterEventRejection {
    match error {
        ServerError::EventScope
            if matches!(
                event,
                BrokerEvent::Keyboard(_)
                    | BrokerEvent::RegisteredObservation { .. }
                    | BrokerEvent::AudioInputDevicesChanged
            ) =>
        {
            AdapterEventRejection::AdmissionClosed
        }
        ServerError::EventScope => AdapterEventRejection::StaleScope,
        ServerError::EventCapacity => AdapterEventRejection::Capacity,
        ServerError::EventDelivery
        | ServerError::TerminalRoute
        | ServerError::Session(_)
        | ServerError::Transport(_)
        | ServerError::TransportBackpressure => AdapterEventRejection::DeliveryFailed,
        ServerError::StartupSnapshot
        | ServerError::ConnectionCapacity
        | ServerError::DuplicateConnection
        | ServerError::UnknownConnection
        | ServerError::TransportAuthenticationBoundary
        | ServerError::CapabilityProtocol
        | ServerError::CloseRetryExhausted
        | ServerError::ExecutorContract
        | ServerError::State(_) => AdapterEventRejection::InvalidTransition,
    }
}

pub(super) fn require_native_success(summary: DriveSummary) -> Result<(), DispatchError> {
    if summary.native_failure.is_some() {
        Err(DispatchError::Semantic(ErrorCode::NativeFailure))
    } else {
        Ok(())
    }
}

pub(super) fn maintenance_request(
    params: &talking_quill_owner_protocol::schema::MaintenanceAcquireParams,
) -> Option<MaintenanceRequest> {
    let transaction = MaintenanceTransactionId::new(*params.transaction_id().as_bytes())?;
    let source = BuildDigest::new(*params.source_build_digest().as_bytes())?;
    let target = params
        .target_build_digest()
        .and_then(|value| BuildDigest::new(*value.as_bytes()));
    let target_owner = params
        .target_owner_sha256()
        .and_then(|value| BuildDigest::new(*value.as_bytes()));
    let owner_handoff = Bytes32::random()
        .ok()
        .and_then(|value| MaintenanceHandoff::new(*value.as_bytes()))?;
    MaintenanceRequest::new(
        transaction,
        maintenance_operation(params.operation()),
        source,
        target,
        target_owner,
        owner_handoff,
    )
}

pub(super) const fn maintenance_operation(
    operation: WireMaintenanceOperation,
) -> MaintenanceOperation {
    match operation {
        WireMaintenanceOperation::Update => MaintenanceOperation::Update,
        WireMaintenanceOperation::Uninstall => MaintenanceOperation::Uninstall,
        WireMaintenanceOperation::Rollback => MaintenanceOperation::Rollback,
    }
}

pub(super) const fn state_session_mode(mode: SessionMode) -> SessionCaptureMode {
    match mode {
        SessionMode::Off => SessionCaptureMode::Off,
        SessionMode::Recording => SessionCaptureMode::Recording,
        SessionMode::CancelOnly => SessionCaptureMode::CancelOnly,
    }
}

pub(super) fn core_bindings(
    bindings: &talking_quill_owner_protocol::schema::Bindings,
) -> Result<ActivationBindings, ()> {
    let converted = bindings
        .as_slice()
        .iter()
        .map(|binding| {
            let profile = ProfileId::new(binding.profile_id().as_str()).map_err(|_| ())?;
            let (ctrl, alt, shift, meta) = binding.shortcut().modifiers().values();
            let keys = binding
                .shortcut()
                .keys()
                .iter()
                .map(|letter| ActivationKey::from_index(*letter as u8).ok_or(()))
                .collect::<Result<Vec<_>, _>>()?;
            let shortcut = Shortcut::new(
                ShortcutModifiers {
                    ctrl,
                    alt,
                    shift,
                    meta,
                },
                &keys,
            )
            .map_err(|_| ())?;
            Ok(ActivationBinding::new(profile, shortcut))
        })
        .collect::<Result<Vec<_>, ()>>()?;
    ActivationBindings::new(&converted).map_err(|_| ())
}

pub(super) fn wire_activation_event(
    owner_instance: OwnerInstanceId,
    capture_epoch: u64,
    binding: ActivationBinding,
    target_token: Option<NativeTargetToken>,
    generation: u64,
    phase: EventPhase,
    held_ms: Option<u64>,
) -> Result<ActivationEvent, ServerError> {
    let profile_id = WireProfileId::new(binding.profile_id().as_str().to_owned())
        .map_err(|_| ServerError::EventScope)?;
    let shortcut = binding.shortcut();
    let modifiers = shortcut.modifiers();
    let keys = shortcut
        .keys()
        .iter()
        .copied()
        .map(wire_letter)
        .collect::<Vec<_>>();
    let shortcut = BindingShortcut::new(
        Modifiers::new(
            modifiers.ctrl,
            modifiers.alt,
            modifiers.shift,
            modifiers.meta,
        ),
        keys,
    )
    .map_err(|_| ServerError::EventScope)?;
    let target_token = target_token
        .map(|token| WireToken::new(token.as_str().to_owned()))
        .transpose()
        .map_err(|_| ServerError::EventScope)?;
    Ok(ActivationEvent {
        capture_lease_epoch: wire_u64(capture_epoch),
        owner_instance_id: Bytes32::new(*owner_instance.as_bytes()),
        profile_id,
        shortcut,
        activation_generation: wire_u64(generation),
        target_token,
        phase: wire_phase(phase),
        // The v1 duration field uses a nonzero decimal scalar. Both edges of
        // a fast tap can share one millisecond tick; round that duration up
        // instead of panicking and terminating the keyboard owner.
        held_ms: held_ms.map(|elapsed| wire_u64(elapsed.max(1))),
    })
}

pub(super) const fn wire_letter(
    key: ActivationKey,
) -> talking_quill_owner_protocol::schema::Letter {
    use talking_quill_owner_protocol::schema::Letter;
    match key {
        ActivationKey::A => Letter::A,
        ActivationKey::B => Letter::B,
        ActivationKey::C => Letter::C,
        ActivationKey::D => Letter::D,
        ActivationKey::E => Letter::E,
        ActivationKey::F => Letter::F,
        ActivationKey::G => Letter::G,
        ActivationKey::H => Letter::H,
        ActivationKey::I => Letter::I,
        ActivationKey::J => Letter::J,
        ActivationKey::K => Letter::K,
        ActivationKey::L => Letter::L,
        ActivationKey::M => Letter::M,
        ActivationKey::N => Letter::N,
        ActivationKey::O => Letter::O,
        ActivationKey::P => Letter::P,
        ActivationKey::Q => Letter::Q,
        ActivationKey::R => Letter::R,
        ActivationKey::S => Letter::S,
        ActivationKey::T => Letter::T,
        ActivationKey::U => Letter::U,
        ActivationKey::V => Letter::V,
        ActivationKey::W => Letter::W,
        ActivationKey::X => Letter::X,
        ActivationKey::Y => Letter::Y,
        ActivationKey::Z => Letter::Z,
    }
}

pub(super) const fn wire_phase(phase: EventPhase) -> Phase {
    match phase {
        EventPhase::Down => Phase::Down,
        EventPhase::Up => Phase::Up,
    }
}

pub(super) fn terminal_ownership(ownership: NativeOwnership) -> Option<TerminalOwnership> {
    let mut present = Vec::with_capacity(5);
    if ownership.candidate() != CandidateOwnership::None {
        present.push(TerminalOwnership::Candidate);
    }
    if ownership.activation_drain_keys() != 0 {
        present.push(TerminalOwnership::Activation);
    }
    if ownership.session_drain_keys() != 0 {
        present.push(TerminalOwnership::Session);
    }
    if ownership.replay_cleanup_edges() != 0 {
        present.push(TerminalOwnership::ReplayCleanup);
    }
    if ownership.paste() != PasteOwnership::None {
        present.push(TerminalOwnership::Paste);
    }
    match present.as_slice() {
        [] => None,
        [only] => Some(*only),
        _ => Some(TerminalOwnership::Multiple),
    }
}

pub(super) fn wire_terminal_event(
    offer: PredecessorTerminalOffer,
) -> talking_quill_owner_protocol::schema::PredecessorTerminalEvent {
    use talking_quill_owner_protocol::schema::{
        LeaseDisposition as WireDisposition, OwnershipKind, PredecessorTerminalEvent as WireEvent,
        RevocationReason, TerminalUnavailableReason as WireUnavailable,
    };
    let lease_id = Bytes32::new(*offer.authority().id().as_bytes());
    let lease_epoch = wire_u64(offer.authority().epoch().get());
    let terminal_sequence = wire_u64(offer.terminal_sequence());
    match offer.event() {
        PredecessorTerminalEvent::LeaseRevoked(reason) => WireEvent::LeaseRevoked {
            capture_lease_id: lease_id,
            capture_lease_epoch: lease_epoch,
            terminal_sequence,
            reason: match reason {
                TerminalReason::Eof => RevocationReason::Eof,
                TerminalReason::Heartbeat => RevocationReason::Heartbeat,
                TerminalReason::Maintenance => RevocationReason::Maintenance,
                TerminalReason::Release => RevocationReason::Release,
                TerminalReason::Protocol => RevocationReason::Protocol,
                TerminalReason::Rollback => RevocationReason::Rollback,
            },
        },
        PredecessorTerminalEvent::LeaseDraining(ownership) => WireEvent::LeaseDraining {
            capture_lease_id: lease_id,
            capture_lease_epoch: lease_epoch,
            terminal_sequence,
            ownership: match ownership {
                TerminalOwnership::Candidate => OwnershipKind::Candidate,
                TerminalOwnership::Activation => OwnershipKind::Activation,
                TerminalOwnership::Session => OwnershipKind::Session,
                TerminalOwnership::ReplayCleanup => OwnershipKind::ReplayCleanup,
                TerminalOwnership::Paste => OwnershipKind::Paste,
                TerminalOwnership::Multiple => OwnershipKind::Multiple,
            },
        },
        PredecessorTerminalEvent::LeaseNeutral => WireEvent::LeaseNeutral {
            capture_lease_id: lease_id,
            capture_lease_epoch: lease_epoch,
            terminal_sequence,
            disposition: WireDisposition::Neutral,
        },
        PredecessorTerminalEvent::LeaseUnavailable(reason) => WireEvent::LeaseUnavailable {
            capture_lease_id: lease_id,
            capture_lease_epoch: lease_epoch,
            terminal_sequence,
            reason: match reason {
                TerminalUnavailableReason::NativeFault => WireUnavailable::NativeFault,
                TerminalUnavailableReason::OwnershipUnknown => WireUnavailable::OwnershipUnknown,
            },
        },
    }
}

pub(super) const fn wire_reported_state(state: ReportedState) -> OwnerReportedState {
    match state {
        ReportedState::Starting => OwnerReportedState::Starting,
        ReportedState::IdleNeutral => OwnerReportedState::IdleNeutral,
        ReportedState::LeaseDisabled => OwnerReportedState::LeaseDisabled,
        ReportedState::LeaseEnabled => OwnerReportedState::LeaseEnabled,
        ReportedState::LeaseDraining => OwnerReportedState::LeaseDraining,
        ReportedState::OrphanCancelling => OwnerReportedState::OrphanCancelling,
        ReportedState::OrphanDraining => OwnerReportedState::OrphanDraining,
        ReportedState::MaintenanceDraining => OwnerReportedState::MaintenanceDraining,
        ReportedState::DegradedDraining => OwnerReportedState::DegradedDraining,
        ReportedState::MaintenanceReady => OwnerReportedState::MaintenanceReady,
        ReportedState::Stopping => OwnerReportedState::Stopping,
    }
}

pub(super) const fn wire_process_state(state: ProcessState) -> WireProcessState {
    match state {
        ProcessState::Starting => WireProcessState::Starting,
        ProcessState::Healthy => WireProcessState::Healthy,
        ProcessState::RollbackLatched => WireProcessState::RollbackLatched,
        ProcessState::Degraded => WireProcessState::Degraded,
        ProcessState::StoppingNative => WireProcessState::StoppingNative,
        ProcessState::FlushingResponse => WireProcessState::FlushingResponse,
        ProcessState::Exiting => WireProcessState::Exiting,
    }
}

pub(super) const fn wire_disposition(disposition: LeaseDisposition) -> WireLeaseDisposition {
    match disposition {
        LeaseDisposition::Neutral => WireLeaseDisposition::Neutral,
        LeaseDisposition::Draining => WireLeaseDisposition::Draining,
    }
}

pub(super) fn wire_counter(value: u64) -> Counter {
    Counter::new(value.min(9_007_199_254_740_991)).expect("counter is JS-safe")
}

pub(super) fn wire_u64(value: u64) -> U64String {
    U64String::try_from(value).expect("state identities are nonzero")
}

pub(super) const fn error_code(kind: TransitionErrorKind) -> ErrorCode {
    match kind {
        TransitionErrorKind::Busy => ErrorCode::Busy,
        TransitionErrorKind::Draining => ErrorCode::Draining,
        TransitionErrorKind::RollbackLatched => ErrorCode::Rollback,
        TransitionErrorKind::Degraded
        | TransitionErrorKind::NativeReadinessRequired
        | TransitionErrorKind::Starting
        | TransitionErrorKind::StartupSnapshotRequired => ErrorCode::Unavailable,
        TransitionErrorKind::ProtocolFault | TransitionErrorKind::WrongController => {
            ErrorCode::SecurityFault
        }
        TransitionErrorKind::ActionIdExhausted
        | TransitionErrorKind::NativeConfirmationMismatch
        | TransitionErrorKind::InvalidOwnershipTransition
        | TransitionErrorKind::ResponseFlushMismatch => ErrorCode::NativeFailure,
        TransitionErrorKind::PasteUnavailable => ErrorCode::Unavailable,
        TransitionErrorKind::MaintenanceSealed
        | TransitionErrorKind::LeaseMustBeDisabled
        | TransitionErrorKind::AdmissionTransitionPending
        | TransitionErrorKind::ConfigurationRequired
        | TransitionErrorKind::SessionOffReconciliationRequired
        | TransitionErrorKind::InvalidConfigurationRevision
        | TransitionErrorKind::PasteScopeMismatch
        | TransitionErrorKind::MaintenanceTransactionMismatch
        | TransitionErrorKind::MaintenanceOperationMismatch
        | TransitionErrorKind::EpochExhausted
        | TransitionErrorKind::Stopping => ErrorCode::InvalidState,
    }
}

#[cfg(test)]
mod duration_tests {
    use super::*;

    #[test]
    fn same_tick_activation_complete_has_a_valid_wire_duration() {
        let binding = ActivationBinding::new(
            talking_quill_keyboard_core::ProfileId::GENERAL,
            talking_quill_keyboard_core::Shortcut::new(
                talking_quill_keyboard_core::ShortcutModifiers {
                    ctrl: false,
                    alt: true,
                    shift: false,
                    meta: false,
                },
                &[talking_quill_keyboard_core::ActivationKey::X],
            )
            .unwrap(),
        );
        for elapsed in [0, 1, 599, 600, u64::MAX] {
            let event = wire_activation_event(
                OwnerInstanceId::new([1; 32]).unwrap(),
                1,
                binding,
                None,
                1,
                EventPhase::Up,
                Some(elapsed),
            )
            .unwrap();
            assert_eq!(event.held_ms.unwrap().get(), elapsed.max(1));
        }
    }
}
