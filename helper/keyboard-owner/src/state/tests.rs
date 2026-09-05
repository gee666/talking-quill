use super::*;

fn owner(value: u8) -> OwnerInstanceId {
    OwnerInstanceId::new([value; AUTHORITY_BYTES]).unwrap()
}

fn capability(value: u8) -> CapabilityId {
    CapabilityId::new([value; AUTHORITY_BYTES]).unwrap()
}

fn connection(value: u64) -> ConnectionId {
    ConnectionId::new(value).unwrap()
}

fn capture_state(enabled: bool) -> (KeyboardOwnerState, ConnectionId, CapabilityRef) {
    let mut state = KeyboardOwnerState::new(owner(1));
    state.process_health = ProcessHealth::Healthy;
    state.startup_snapshot_seeded = true;
    state.readiness = NativeReadiness {
        keyboard_build_eligible: true,
        paste_ready: true,
        permissions_eligible: true,
        hook_healthy: true,
    };
    let connection = connection(1);
    let authority = CapabilityRef {
        id: capability(1),
        epoch: CapabilityEpoch(NonZeroU64::MIN),
    };
    let capability = CapabilityState {
        authority,
        last_command_sequence: 0,
    };
    state.controller = if enabled {
        state.admission = AdmissionState::Open;
        ControllerInternal::CaptureLeaseEnabled {
            connection,
            capability,
        }
    } else {
        ControllerInternal::CaptureLeaseDisabled {
            connection,
            capability,
        }
    };
    (state, connection, authority)
}

#[test]
fn action_exhaustion_reserves_configuration_high_water_and_degrades() {
    let (mut state, connection, authority) = capture_state(false);
    state.last_action_id = u64::MAX;
    let error = state
        .apply_capture_command(
            connection,
            authority,
            CommandSequence::FIRST,
            CaptureCommand::ReplaceConfiguration {
                revision: ConfigurationRevision::new(8).unwrap(),
                bindings: ActivationBindings::default(),
            },
        )
        .unwrap_err();
    assert_eq!(error.kind(), TransitionErrorKind::ActionIdExhausted);
    assert_eq!(
        state.configuration_high_water,
        ConfigurationRevision::new(8)
    );
    assert_eq!(state.process(), ProcessState::Degraded);
    assert!(state.native_state_unknown);
}

#[test]
fn rollback_action_exhaustion_preserves_priority_latch() {
    let (mut state, _, _) = capture_state(true);
    state.last_action_id = u64::MAX;
    let error = state.latch_runtime_rollback().unwrap_err();
    assert_eq!(error.kind(), TransitionErrorKind::ActionIdExhausted);
    assert!(
        error
            .actions()
            .contains_kind(RequiredActionKind::EmergencyCloseFreshAdmission)
    );
    assert!(state.rollback_latched);
    assert_eq!(state.process(), ProcessState::Degraded);
    assert_ne!(state.admission, AdmissionState::Open);
    assert!(matches!(state.controller, ControllerInternal::NoController));
}

#[test]
fn maintenance_post_seal_action_exhaustion_never_restores_capture() {
    let (mut state, _, _) = capture_state(true);
    state.last_action_id = u64::MAX;
    let request = MaintenanceRequest::new(
        MaintenanceTransactionId::new([3; AUTHORITY_BYTES]).unwrap(),
        MaintenanceOperation::Update,
        BuildDigest::new([4; AUTHORITY_BYTES]).unwrap(),
        Some(BuildDigest::new([5; AUTHORITY_BYTES]).unwrap()),
        Some(BuildDigest::new([6; AUTHORITY_BYTES]).unwrap()),
        MaintenanceHandoff::new([7; AUTHORITY_BYTES]).unwrap(),
    )
    .unwrap();
    let error = state
        .acquire_maintenance(connection(2), capability(2), request)
        .unwrap_err();
    assert_eq!(error.kind(), TransitionErrorKind::ActionIdExhausted);
    assert!(
        error
            .actions()
            .contains_kind(RequiredActionKind::EmergencyCloseFreshAdmission)
    );
    assert_eq!(state.maintenance_phase(), MaintenancePhase::SealedFailed);
    assert_eq!(state.process(), ProcessState::Degraded);
    assert!(!matches!(
        state.controller,
        ControllerInternal::CaptureLeaseEnabled { .. }
    ));
}

#[test]
fn mismatch_exhaustion_preserves_and_confirms_emergency_close() {
    let (mut state, _, _) = capture_state(true);
    state.last_action_id = u64::MAX;
    let stale = NativeActionToken {
        owner_instance: state.owner_instance,
        id: NonZeroU64::MIN,
        scope: ActionScope::Process,
    };
    let error = state.confirm_admission_opened(stale).unwrap_err();
    assert_eq!(
        error.kind(),
        TransitionErrorKind::NativeConfirmationMismatch
    );
    assert!(
        error
            .actions()
            .contains_kind(RequiredActionKind::EmergencyCloseFreshAdmission)
    );
    assert!(state.emergency_close_pending);
    state.confirm_emergency_admission_closed().unwrap();
    assert_eq!(state.admission, AdmissionState::Closed);
    assert!(!state.emergency_close_pending);
}

#[test]
fn controller_loss_exhaustion_returns_retrievable_predecessor_offer() {
    let (mut state, connection, _) = capture_state(true);
    state.last_action_id = u64::MAX;
    let error = state.controller_disconnected(connection).unwrap_err();
    assert_eq!(error.kind(), TransitionErrorKind::ActionIdExhausted);
    assert!(
        error
            .actions()
            .contains_kind(RequiredActionKind::EmergencyCloseFreshAdmission)
    );
    let offer = error.terminal_offer().expect("predecessor offer retained");
    state.fail_predecessor_terminal_write(offer).unwrap();
    state.confirm_emergency_admission_closed().unwrap();
}

#[test]
fn persistence_allocation_exhaustion_returns_prior_drain_directive() {
    let (mut state, _, _) = capture_state(true);
    state.ownership =
        NativeOwnership::new(CandidateOwnership::None, 1, 0, 0, PasteOwnership::None, 0).unwrap();
    state.last_action_id = u64::MAX - 1;
    let request = MaintenanceRequest::new(
        MaintenanceTransactionId::new([9; AUTHORITY_BYTES]).unwrap(),
        MaintenanceOperation::Update,
        BuildDigest::new([10; AUTHORITY_BYTES]).unwrap(),
        Some(BuildDigest::new([11; AUTHORITY_BYTES]).unwrap()),
        Some(BuildDigest::new([12; AUTHORITY_BYTES]).unwrap()),
        MaintenanceHandoff::new([13; AUTHORITY_BYTES]).unwrap(),
    )
    .unwrap();
    let acquire = state
        .acquire_maintenance(connection(2), capability(2), request)
        .unwrap();
    let error = state
        .confirm_admission_closed(
            acquire
                .actions()
                .as_slice()
                .iter()
                .find_map(|action| action.token())
                .unwrap(),
        )
        .unwrap_err();
    assert_eq!(error.kind(), TransitionErrorKind::ActionIdExhausted);
    assert!(
        error
            .actions()
            .contains_kind(RequiredActionKind::ContinueNativeDrain)
    );
    assert_eq!(state.maintenance_phase(), MaintenancePhase::SealedFailed);
}

#[test]
fn capture_and_maintenance_epoch_exhaustion_never_wraps() {
    let mut capture = KeyboardOwnerState::new(owner(1));
    capture.process_health = ProcessHealth::Healthy;
    capture.controller = ControllerInternal::AuthenticatedObserver {
        connection: connection(1),
    };
    capture.last_capture_epoch = u64::MAX;
    assert_eq!(
        capture
            .acquire_capture_lease(connection(1), capability(1))
            .unwrap_err()
            .kind(),
        TransitionErrorKind::EpochExhausted
    );

    let mut maintenance = KeyboardOwnerState::new(owner(2));
    maintenance.process_health = ProcessHealth::Healthy;
    maintenance.last_maintenance_epoch = u64::MAX;
    let request = MaintenanceRequest::new(
        MaintenanceTransactionId::new([7; AUTHORITY_BYTES]).unwrap(),
        MaintenanceOperation::Uninstall,
        BuildDigest::new([8; AUTHORITY_BYTES]).unwrap(),
        None,
        None,
        MaintenanceHandoff::new([9; AUTHORITY_BYTES]).unwrap(),
    )
    .unwrap();
    assert_eq!(
        maintenance
            .acquire_maintenance(connection(2), capability(2), request)
            .unwrap_err()
            .kind(),
        TransitionErrorKind::EpochExhausted
    );
    assert_eq!(maintenance.maintenance_phase(), MaintenancePhase::None);
}
