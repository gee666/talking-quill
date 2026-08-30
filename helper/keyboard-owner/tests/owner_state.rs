use proptest::prelude::*;
use talking_quill_keyboard_core::{
    ACTIVATION_KEY_CAPACITY, ActivationBindings, COMBINED_PHYSICAL_DRAIN_CAPACITY,
    OWNER_ADMITTED_EFFECT_CAPACITY, REPLAY_CLEANUP_EDGE_CAPACITY, SESSION_KEY_CAPACITY,
    SessionCaptureMode,
};
use talking_quill_keyboard_owner::state::{
    AdmissionState, BuildDigest, CandidateOwnership, CapabilityId, CapabilityRef, CaptureCommand,
    CommandSequence, ConfigurationRequest, ConfigurationRevision, ConnectionId, ControllerState,
    ExitPhase, KeyboardOwnerState, LeaseDisposition, MaintenanceCommand, MaintenanceHandoff,
    MaintenanceOperation, MaintenancePhase, MaintenanceRequest, MaintenanceTransactionId,
    NativeActionFailure, NativeActionToken, NativeOwnership, NativeOwnershipObservation,
    NativeReadiness, OwnerActivationGeneration, OwnerInstanceId, PasteAuthorization,
    PasteOperationId, PasteOwnership, PredecessorTerminalEvent, ProcessState, ReportedState,
    RequiredAction, RequiredActionKind, ResponseCorrelation, ResponseStage, TerminalOwnership,
    Transition, TransitionErrorKind,
};

const READY: NativeReadiness = NativeReadiness {
    keyboard_build_eligible: true,
    paste_ready: true,
    permissions_eligible: true,
    hook_healthy: true,
};
const SAFE_DISABLED_PASTE_READY: NativeReadiness = NativeReadiness {
    keyboard_build_eligible: false,
    paste_ready: true,
    permissions_eligible: false,
    hook_healthy: false,
};
const KEYBOARD_NOT_READY: NativeReadiness = NativeReadiness {
    keyboard_build_eligible: true,
    paste_ready: true,
    permissions_eligible: false,
    hook_healthy: true,
};

fn owner(value: u8) -> OwnerInstanceId {
    OwnerInstanceId::new([value; 32]).unwrap()
}

fn connection(value: u64) -> ConnectionId {
    ConnectionId::new(value).unwrap()
}

fn capability(value: u8) -> CapabilityId {
    CapabilityId::new([value; 32]).unwrap()
}

fn transaction(value: u8) -> MaintenanceTransactionId {
    MaintenanceTransactionId::new([value; 32]).unwrap()
}

fn digest(value: u8) -> BuildDigest {
    BuildDigest::new([value; 32]).unwrap()
}

fn operation(value: u8) -> PasteOperationId {
    PasteOperationId::new([value; 32]).unwrap()
}

fn revision(value: u64) -> ConfigurationRevision {
    ConfigurationRevision::new(value).unwrap()
}

fn maintenance_request(value: u8, operation: MaintenanceOperation) -> MaintenanceRequest {
    match operation {
        MaintenanceOperation::Uninstall => MaintenanceRequest::new(
            transaction(value),
            operation,
            digest(value + 1),
            None,
            None,
            MaintenanceHandoff::new([value.wrapping_add(4); 32]).unwrap(),
        ),
        MaintenanceOperation::Update | MaintenanceOperation::Rollback => MaintenanceRequest::new(
            transaction(value),
            operation,
            digest(value + 1),
            Some(digest(value + 2)),
            Some(digest(value + 3)),
            MaintenanceHandoff::new([value.wrapping_add(4); 32]).unwrap(),
        ),
    }
    .unwrap()
}

fn action_kinds(transition: &Transition) -> Vec<RequiredActionKind> {
    transition
        .actions()
        .as_slice()
        .iter()
        .map(|action| action.kind())
        .collect()
}

fn action_token(transition: &Transition, kind: RequiredActionKind) -> NativeActionToken {
    transition
        .actions()
        .as_slice()
        .iter()
        .find(|action| action.kind() == kind)
        .and_then(|action| action.token())
        .unwrap_or_else(|| panic!("missing {kind:?} from {:?}", action_kinds(transition)))
}

fn configuration_request(transition: &Transition) -> ConfigurationRequest {
    transition
        .actions()
        .as_slice()
        .iter()
        .find_map(|action| match action {
            RequiredAction::ApplyConfiguration { request, .. } => Some(*request),
            _ => None,
        })
        .expect("configuration action")
}

fn healthy(readiness: NativeReadiness) -> KeyboardOwnerState {
    let mut state = KeyboardOwnerState::new(owner(1));
    state.confirm_startup_snapshot_seeded().unwrap();
    state.startup_completed().unwrap();
    state.observe_native_readiness(readiness).unwrap();
    state
}

fn lease(readiness: NativeReadiness) -> (KeyboardOwnerState, ConnectionId, CapabilityRef) {
    let mut state = healthy(readiness);
    let connection = connection(1);
    state.authenticate_observer(connection).unwrap();
    state
        .acquire_capture_lease(connection, capability(1))
        .unwrap();
    let ControllerState::CaptureLeaseDisabled { authority, .. } = state.controller() else {
        panic!("disabled capture lease expected");
    };
    (state, connection, authority)
}

fn reconcile_disabled(
    state: &mut KeyboardOwnerState,
    controller: ConnectionId,
    authority: CapabilityRef,
) -> u64 {
    let session = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::FIRST,
            CaptureCommand::ReconcileSessionOff,
        )
        .unwrap();
    state
        .confirm_session_mode_applied(
            action_token(&session, RequiredActionKind::ApplySessionMode),
            SessionCaptureMode::Off,
        )
        .unwrap();
    let configuration = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(2).unwrap(),
            CaptureCommand::ReplaceConfiguration {
                revision: revision(1),
                bindings: ActivationBindings::default(),
            },
        )
        .unwrap();
    let request = configuration_request(&configuration);
    state
        .confirm_configuration_applied(
            action_token(&configuration, RequiredActionKind::ApplyConfiguration),
            request,
        )
        .unwrap();
    2
}

fn enabled() -> (KeyboardOwnerState, ConnectionId, CapabilityRef, u64) {
    let (mut state, controller, authority) = lease(READY);
    let sequence = reconcile_disabled(&mut state, controller, authority);
    let opening = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(sequence + 1).unwrap(),
            CaptureCommand::Enable,
        )
        .unwrap();
    state
        .confirm_admission_opened(action_token(
            &opening,
            RequiredActionKind::OpenFreshAdmission,
        ))
        .unwrap();
    (state, controller, authority, sequence + 1)
}

fn paste_authorization(
    state: &KeyboardOwnerState,
    authority: CapabilityRef,
    value: u8,
) -> PasteAuthorization {
    PasteAuthorization::new(
        operation(value),
        state.owner_instance(),
        authority.epoch(),
        OwnerActivationGeneration::new(u64::from(value)).unwrap(),
    )
}

fn begin_waiting_paste(
    state: &mut KeyboardOwnerState,
    controller: ConnectionId,
    authority: CapabilityRef,
    sequence: u64,
    value: u8,
) -> (u64, PasteAuthorization) {
    let authorization = paste_authorization(state, authority, value);
    let admit = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(sequence + 1).unwrap(),
            CaptureCommand::BeginPaste(authorization),
        )
        .unwrap();
    state
        .confirm_paste_waiting(
            action_token(&admit, RequiredActionKind::AdmitPaste),
            authorization,
        )
        .unwrap();
    (sequence + 1, authorization)
}

fn persisted_maintenance(
    state: &mut KeyboardOwnerState,
    maintainer: ConnectionId,
    request: MaintenanceRequest,
) -> (CapabilityRef, Transition) {
    let acquire = state
        .acquire_maintenance(maintainer, capability(9), request)
        .unwrap();
    let persist = if acquire
        .actions()
        .contains_kind(RequiredActionKind::PersistMaintenanceRecord)
    {
        acquire.clone()
    } else {
        let close = action_token(&acquire, RequiredActionKind::CloseFreshAdmission);
        state.confirm_admission_closed(close).unwrap()
    };
    let persisted = state
        .confirm_maintenance_persisted(
            action_token(&persist, RequiredActionKind::PersistMaintenanceRecord),
            request,
        )
        .unwrap();
    let ControllerState::MaintenanceExclusive { authority, .. } = state.controller() else {
        panic!("maintenance capability expected only after persistence");
    };
    (authority, persisted)
}

#[test]
fn exact_initial_state_is_closed_safe_and_requires_snapshot() {
    let mut state = KeyboardOwnerState::new(owner(1));
    assert_eq!(state.process(), ProcessState::Starting);
    assert_eq!(state.admission(), AdmissionState::Closed);
    assert_eq!(state.ownership(), NativeOwnership::NEUTRAL);
    assert!(!state.can_open_keyboard());
    assert!(!state.can_begin_paste());
    assert_eq!(
        state.startup_completed().unwrap_err().kind(),
        TransitionErrorKind::StartupSnapshotRequired
    );
    state.confirm_startup_snapshot_seeded().unwrap();
    state.startup_completed().unwrap();
    assert_eq!(state.reported_state(), ReportedState::IdleNeutral);
}

#[test]
fn shared_owner_bounds_accept_exact_max_and_reject_max_plus_one() {
    assert_eq!(ACTIVATION_KEY_CAPACITY, 26);
    assert_eq!(SESSION_KEY_CAPACITY, 2);
    assert_eq!(COMBINED_PHYSICAL_DRAIN_CAPACITY, 28);
    assert_eq!(REPLAY_CLEANUP_EDGE_CAPACITY, 26);
    assert_eq!(OWNER_ADMITTED_EFFECT_CAPACITY, 8);

    assert!(
        NativeOwnership::new(
            CandidateOwnership::None,
            ACTIVATION_KEY_CAPACITY as u8,
            SESSION_KEY_CAPACITY as u8,
            REPLAY_CLEANUP_EDGE_CAPACITY as u8,
            PasteOwnership::None,
            OWNER_ADMITTED_EFFECT_CAPACITY as u8,
        )
        .is_ok()
    );
    assert!(
        NativeOwnership::new(
            CandidateOwnership::None,
            ACTIVATION_KEY_CAPACITY as u8 + 1,
            0,
            0,
            PasteOwnership::None,
            0,
        )
        .is_err()
    );
    assert!(
        NativeOwnership::new(
            CandidateOwnership::None,
            0,
            SESSION_KEY_CAPACITY as u8 + 1,
            0,
            PasteOwnership::None,
            0,
        )
        .is_err()
    );
    assert!(
        NativeOwnership::new(
            CandidateOwnership::None,
            0,
            0,
            REPLAY_CLEANUP_EDGE_CAPACITY as u8 + 1,
            PasteOwnership::None,
            0,
        )
        .is_err()
    );
    assert!(
        NativeOwnership::new(
            CandidateOwnership::None,
            0,
            0,
            0,
            PasteOwnership::None,
            OWNER_ADMITTED_EFFECT_CAPACITY as u8 + 1,
        )
        .is_err()
    );
}

#[test]
fn safe_disabled_build_still_admits_one_shot_paste_with_keyboard_closed() {
    let (mut state, controller, authority) = lease(SAFE_DISABLED_PASTE_READY);
    assert!(!state.can_open_keyboard());
    assert!(state.can_begin_paste());
    let authorization = paste_authorization(&state, authority, 1);
    let admit = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::FIRST,
            CaptureCommand::BeginPaste(authorization),
        )
        .unwrap();
    assert_eq!(state.admission(), AdmissionState::Closed);
    assert_eq!(action_kinds(&admit), vec![RequiredActionKind::AdmitPaste]);
    state
        .confirm_paste_waiting(
            action_token(&admit, RequiredActionKind::AdmitPaste),
            authorization,
        )
        .unwrap();
    state.confirm_paste_claimed(authorization).unwrap();
    state.confirm_paste_indeterminate(authorization).unwrap();
    assert!(!state.can_begin_paste());
    state.confirm_paste_completed(authorization).unwrap();
    assert!(state.can_begin_paste());
}

#[test]
fn paste_preclaim_refusal_clears_operation_without_opening_keyboard() {
    let (mut state, controller, authority) = lease(SAFE_DISABLED_PASTE_READY);
    let authorization = paste_authorization(&state, authority, 2);
    let admit = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::FIRST,
            CaptureCommand::BeginPaste(authorization),
        )
        .unwrap();
    state
        .confirm_paste_refused(
            action_token(&admit, RequiredActionKind::AdmitPaste),
            authorization,
        )
        .unwrap();
    assert_eq!(state.ownership().paste(), PasteOwnership::None);
    assert_eq!(state.admission(), AdmissionState::Closed);
}

#[test]
fn paste_scope_and_readiness_are_exact_and_independent_of_build_mode() {
    let (mut state, controller, authority) = lease(SAFE_DISABLED_PASTE_READY);
    let wrong_owner = PasteAuthorization::new(
        operation(3),
        owner(9),
        authority.epoch(),
        OwnerActivationGeneration::new(3).unwrap(),
    );
    assert_eq!(
        state
            .apply_capture_command(
                controller,
                authority,
                CommandSequence::FIRST,
                CaptureCommand::BeginPaste(wrong_owner),
            )
            .unwrap_err()
            .kind(),
        TransitionErrorKind::PasteScopeMismatch
    );

    state
        .observe_native_readiness(NativeReadiness {
            paste_ready: false,
            ..SAFE_DISABLED_PASTE_READY
        })
        .unwrap();
    let exact = paste_authorization(&state, authority, 4);
    assert_eq!(
        state
            .apply_capture_command(
                controller,
                authority,
                CommandSequence::new(2).unwrap(),
                CaptureCommand::BeginPaste(exact),
            )
            .unwrap_err()
            .kind(),
        TransitionErrorKind::PasteUnavailable
    );
}

#[test]
fn disconnect_before_paste_claim_cancels_but_claimed_paste_is_retained() {
    let (mut waiting, controller, authority) = lease(SAFE_DISABLED_PASTE_READY);
    let (_, authorization) = begin_waiting_paste(&mut waiting, controller, authority, 0, 5);
    let loss = waiting.controller_disconnected(controller).unwrap();
    assert_eq!(
        action_kinds(&loss),
        vec![RequiredActionKind::CancelWaitingPaste]
    );
    waiting
        .confirm_waiting_paste_cancelled(
            action_token(&loss, RequiredActionKind::CancelWaitingPaste),
            authorization,
        )
        .unwrap();
    assert_eq!(waiting.ownership().paste(), PasteOwnership::None);

    let (mut claimed, controller, authority) = lease(SAFE_DISABLED_PASTE_READY);
    let (_, authorization) = begin_waiting_paste(&mut claimed, controller, authority, 0, 6);
    claimed.confirm_paste_claimed(authorization).unwrap();
    let loss = claimed.controller_disconnected(controller).unwrap();
    assert!(
        !loss
            .actions()
            .contains_kind(RequiredActionKind::CancelWaitingPaste)
    );
    assert_eq!(claimed.ownership().paste(), PasteOwnership::Claimed);
}

#[test]
fn disconnect_during_paste_admission_cancels_immediately_after_waiting_confirmation() {
    let (mut state, controller, authority) = lease(SAFE_DISABLED_PASTE_READY);
    let authorization = paste_authorization(&state, authority, 7);
    let admit = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::FIRST,
            CaptureCommand::BeginPaste(authorization),
        )
        .unwrap();
    state.controller_disconnected(controller).unwrap();
    let cancellation = state
        .confirm_paste_waiting(
            action_token(&admit, RequiredActionKind::AdmitPaste),
            authorization,
        )
        .unwrap();
    assert_eq!(
        action_kinds(&cancellation),
        vec![RequiredActionKind::CancelWaitingPaste]
    );
}

#[test]
fn rollback_is_priority_over_pending_open_and_never_reenables() {
    let (mut state, controller, authority) = lease(READY);
    let sequence = reconcile_disabled(&mut state, controller, authority);
    let open = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(sequence + 1).unwrap(),
            CaptureCommand::Enable,
        )
        .unwrap();
    let rollback = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(sequence + 2).unwrap(),
            CaptureCommand::RuntimeRollback,
        )
        .unwrap();
    assert!(
        rollback
            .actions()
            .contains_kind(RequiredActionKind::CloseFreshAdmission)
    );
    assert!(state.status().rollback_latched);
    state
        .confirm_admission_opened(action_token(&open, RequiredActionKind::OpenFreshAdmission))
        .unwrap();
    assert_ne!(
        state.controller(),
        ControllerState::CaptureLeaseEnabled {
            connection: controller,
            authority,
        }
    );
    state
        .confirm_admission_closed(action_token(
            &rollback,
            RequiredActionKind::CloseFreshAdmission,
        ))
        .unwrap();
    assert_eq!(state.process(), ProcessState::RollbackLatched);
}

#[test]
fn rollback_supersedes_pending_session_configuration_paste_and_existing_close() {
    // Session action.
    let (mut session_state, controller, authority) = lease(READY);
    let session = session_state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::FIRST,
            CaptureCommand::ReconcileSessionOff,
        )
        .unwrap();
    session_state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(2).unwrap(),
            CaptureCommand::RuntimeRollback,
        )
        .unwrap();
    session_state
        .confirm_session_mode_applied(
            action_token(&session, RequiredActionKind::ApplySessionMode),
            SessionCaptureMode::Off,
        )
        .unwrap();
    assert_eq!(session_state.applied_session_mode(), None);

    // Configuration action and its high-water survive supersession.
    let (mut config_state, controller, authority) = lease(READY);
    let config = config_state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::FIRST,
            CaptureCommand::ReplaceConfiguration {
                revision: revision(11),
                bindings: ActivationBindings::default(),
            },
        )
        .unwrap();
    let request = configuration_request(&config);
    config_state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(2).unwrap(),
            CaptureCommand::RuntimeRollback,
        )
        .unwrap();
    config_state
        .confirm_configuration_applied(
            action_token(&config, RequiredActionKind::ApplyConfiguration),
            request,
        )
        .unwrap();
    assert_eq!(config_state.applied_configuration(), None);
    assert_eq!(config_state.configuration_high_water(), Some(revision(11)));

    // Paste admission becomes cancel-after-admit.
    let (mut paste_state, controller, authority) = lease(SAFE_DISABLED_PASTE_READY);
    let authorization = paste_authorization(&paste_state, authority, 8);
    let admit = paste_state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::FIRST,
            CaptureCommand::BeginPaste(authorization),
        )
        .unwrap();
    paste_state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(2).unwrap(),
            CaptureCommand::RuntimeRollback,
        )
        .unwrap();
    let cancel = paste_state
        .confirm_paste_waiting(
            action_token(&admit, RequiredActionKind::AdmitPaste),
            authorization,
        )
        .unwrap();
    assert!(
        cancel
            .actions()
            .contains_kind(RequiredActionKind::CancelWaitingPaste)
    );

    // An existing close is merged instead of rejected or duplicated.
    let (mut close_state, controller, authority, sequence) = enabled();
    let close = close_state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(sequence + 1).unwrap(),
            CaptureCommand::Disable,
        )
        .unwrap();
    let rollback = close_state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(sequence + 2).unwrap(),
            CaptureCommand::RuntimeRollback,
        )
        .unwrap();
    assert!(rollback.actions().as_slice().is_empty());
    close_state
        .confirm_admission_closed(action_token(
            &close,
            RequiredActionKind::CloseFreshAdmission,
        ))
        .unwrap();
    assert!(close_state.status().rollback_latched);
}

#[test]
fn degraded_process_forbids_new_config_session_paste_open_and_renew() {
    let commands = [
        CaptureCommand::Renew,
        CaptureCommand::ReconcileSessionOff,
        CaptureCommand::SetSessionMode(SessionCaptureMode::Recording),
        CaptureCommand::ReplaceConfiguration {
            revision: revision(2),
            bindings: ActivationBindings::default(),
        },
        CaptureCommand::Enable,
    ];
    for command in commands {
        let (mut state, controller, authority) = lease(READY);
        state.recoverable_native_fault().unwrap();
        assert_eq!(
            state
                .apply_capture_command(controller, authority, CommandSequence::FIRST, command,)
                .unwrap_err()
                .kind(),
            TransitionErrorKind::Degraded
        );
    }
    let (mut state, controller, authority) = lease(READY);
    state.recoverable_native_fault().unwrap();
    let authorization = paste_authorization(&state, authority, 9);
    assert_eq!(
        state
            .apply_capture_command(
                controller,
                authority,
                CommandSequence::FIRST,
                CaptureCommand::BeginPaste(authorization),
            )
            .unwrap_err()
            .kind(),
        TransitionErrorKind::Degraded
    );
}

#[test]
fn configuration_revision_high_water_survives_readiness_loss_failure_and_exhaustion() {
    let (mut state, controller, authority) = lease(READY);
    let first = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::FIRST,
            CaptureCommand::ReplaceConfiguration {
                revision: revision(4),
                bindings: ActivationBindings::default(),
            },
        )
        .unwrap();
    let first_request = configuration_request(&first);
    state
        .confirm_configuration_applied(
            action_token(&first, RequiredActionKind::ApplyConfiguration),
            first_request,
        )
        .unwrap();
    state.observe_native_readiness(KEYBOARD_NOT_READY).unwrap();
    assert_eq!(state.configuration_high_water(), Some(revision(4)));
    assert_eq!(
        state
            .apply_capture_command(
                controller,
                authority,
                CommandSequence::new(2).unwrap(),
                CaptureCommand::ReplaceConfiguration {
                    revision: revision(4),
                    bindings: ActivationBindings::default(),
                },
            )
            .unwrap_err()
            .kind(),
        TransitionErrorKind::InvalidConfigurationRevision
    );
    let higher = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(3).unwrap(),
            CaptureCommand::ReplaceConfiguration {
                revision: revision(5),
                bindings: ActivationBindings::default(),
            },
        )
        .unwrap();
    state
        .fail_native_action(
            action_token(&higher, RequiredActionKind::ApplyConfiguration),
            NativeActionFailure::Indeterminate,
        )
        .unwrap();
    assert_eq!(state.configuration_high_water(), Some(revision(5)));
}

#[test]
fn new_capture_epoch_restarts_revision_and_old_epoch_completion_is_rejected() {
    let (mut first, controller, authority) = lease(READY);
    let action = first
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::FIRST,
            CaptureCommand::ReplaceConfiguration {
                revision: revision(10),
                bindings: ActivationBindings::default(),
            },
        )
        .unwrap();
    let old_token = action_token(&action, RequiredActionKind::ApplyConfiguration);
    let old_request = configuration_request(&action);
    let loss = first.controller_disconnected(controller).unwrap();
    if let Some(offer) = loss.terminal_offer() {
        first.fail_predecessor_terminal_write(offer).unwrap();
    }
    first
        .confirm_configuration_applied(old_token, old_request)
        .unwrap();

    let next_connection = connection(2);
    first.authenticate_observer(next_connection).unwrap();
    first
        .acquire_capture_lease(next_connection, capability(2))
        .unwrap();
    let ControllerState::CaptureLeaseDisabled {
        authority: next_authority,
        ..
    } = first.controller()
    else {
        panic!();
    };
    let next = first
        .apply_capture_command(
            next_connection,
            next_authority,
            CommandSequence::FIRST,
            CaptureCommand::ReplaceConfiguration {
                revision: revision(1),
                bindings: ActivationBindings::default(),
            },
        )
        .unwrap();
    let next_request = configuration_request(&next);
    assert_ne!(next_request.identity().capture_epoch(), authority.epoch());
    assert_eq!(
        first
            .confirm_configuration_applied(old_token, old_request)
            .unwrap_err()
            .kind(),
        TransitionErrorKind::NativeConfirmationMismatch
    );
}

#[test]
fn dependent_actions_are_strictly_candidate_then_paste_then_config_then_drain() {
    let (mut state, controller, authority, mut sequence) = enabled();
    state
        .observe_native_ownership(
            NativeOwnership::new(CandidateOwnership::Active, 1, 0, 0, PasteOwnership::None, 0)
                .unwrap(),
        )
        .unwrap();
    let paste = paste_authorization(&state, authority, 10);
    let admit = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(sequence + 1).unwrap(),
            CaptureCommand::BeginPaste(paste),
        )
        .unwrap();
    sequence += 1;
    state
        .confirm_paste_waiting(action_token(&admit, RequiredActionKind::AdmitPaste), paste)
        .unwrap();
    let close = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(sequence + 1).unwrap(),
            CaptureCommand::ReplaceConfiguration {
                revision: revision(2),
                bindings: ActivationBindings::default(),
            },
        )
        .unwrap();
    let after_close = state
        .confirm_admission_closed(action_token(
            &close,
            RequiredActionKind::CloseFreshAdmission,
        ))
        .unwrap();
    assert_eq!(
        action_kinds(&after_close),
        vec![RequiredActionKind::CancelCandidate]
    );
    let after_candidate = state
        .confirm_candidate_cancelled(
            action_token(&after_close, RequiredActionKind::CancelCandidate),
            NativeOwnership::new(
                CandidateOwnership::None,
                1,
                0,
                0,
                PasteOwnership::Waiting,
                0,
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        action_kinds(&after_candidate),
        vec![RequiredActionKind::CancelWaitingPaste]
    );
    let after_paste = state
        .confirm_waiting_paste_cancelled(
            action_token(&after_candidate, RequiredActionKind::CancelWaitingPaste),
            paste,
        )
        .unwrap();
    assert_eq!(
        action_kinds(&after_paste),
        vec![RequiredActionKind::ApplyConfiguration]
    );
    let request = configuration_request(&after_paste);
    let after_config = state
        .confirm_configuration_applied(
            action_token(&after_paste, RequiredActionKind::ApplyConfiguration),
            request,
        )
        .unwrap();
    assert_eq!(
        action_kinds(&after_config),
        vec![RequiredActionKind::ContinueNativeDrain]
    );
}

#[test]
fn uncertain_cancellation_never_dispatches_later_paste_or_configuration() {
    for failure in [
        NativeActionFailure::FailedNotApplied,
        NativeActionFailure::Indeterminate,
    ] {
        let (mut state, controller, authority, sequence) = enabled();
        state
            .observe_native_ownership(
                NativeOwnership::new(CandidateOwnership::Active, 0, 0, 0, PasteOwnership::None, 0)
                    .unwrap(),
            )
            .unwrap();
        let close = state
            .apply_capture_command(
                controller,
                authority,
                CommandSequence::new(sequence + 1).unwrap(),
                CaptureCommand::ReplaceConfiguration {
                    revision: revision(2),
                    bindings: ActivationBindings::default(),
                },
            )
            .unwrap();
        let cancel = state
            .confirm_admission_closed(action_token(
                &close,
                RequiredActionKind::CloseFreshAdmission,
            ))
            .unwrap();
        let failed = state
            .fail_native_action(
                action_token(&cancel, RequiredActionKind::CancelCandidate),
                failure,
            )
            .unwrap();
        assert!(
            !failed
                .actions()
                .contains_kind(RequiredActionKind::ApplyConfiguration)
        );
        assert!(state.status().native_state_unknown);
        assert_eq!(state.process(), ProcessState::Degraded);
    }
}

#[test]
fn maintenance_acquire_seals_then_persists_then_installs_capability() {
    let (mut state, _controller, _authority, _sequence) = enabled();
    state
        .observe_native_ownership(
            NativeOwnership::new(CandidateOwnership::Active, 0, 0, 0, PasteOwnership::None, 0)
                .unwrap(),
        )
        .unwrap();
    let request = maintenance_request(1, MaintenanceOperation::Update);
    let acquire = state
        .acquire_maintenance(connection(2), capability(9), request)
        .unwrap();
    assert_eq!(state.maintenance_phase(), MaintenancePhase::Sealing);
    assert!(!matches!(
        state.controller(),
        ControllerState::MaintenanceExclusive { .. }
    ));
    let revoked = acquire
        .terminal_offer()
        .expect("best-effort predecessor route");
    assert_eq!(
        revoked.event(),
        PredecessorTerminalEvent::LeaseRevoked(
            talking_quill_keyboard_owner::state::TerminalReason::Maintenance
        )
    );
    let after_close = state
        .confirm_admission_closed(action_token(
            &acquire,
            RequiredActionKind::CloseFreshAdmission,
        ))
        .unwrap();
    assert_eq!(
        action_kinds(&after_close),
        vec![RequiredActionKind::CancelCandidate]
    );
    let persistence = state
        .confirm_candidate_cancelled(
            action_token(&after_close, RequiredActionKind::CancelCandidate),
            NativeOwnership::NEUTRAL,
        )
        .unwrap();
    assert_eq!(state.maintenance_phase(), MaintenancePhase::Persisting);
    assert_eq!(
        action_kinds(&persistence),
        vec![RequiredActionKind::PersistMaintenanceRecord]
    );
    let installed = state
        .confirm_maintenance_persisted(
            action_token(&persistence, RequiredActionKind::PersistMaintenanceRecord),
            request,
        )
        .unwrap();
    assert_eq!(
        installed.response_stage(),
        Some(ResponseStage::MaintenanceAcquireReady)
    );
    assert_eq!(state.maintenance_phase(), MaintenancePhase::Exclusive);
}

#[test]
fn terminal_route_is_nonblocking_best_effort_immutable_and_one_shot_revocation() {
    let (mut state, _controller, _authority, _sequence) = enabled();
    state
        .observe_native_ownership(
            NativeOwnership::new(CandidateOwnership::None, 1, 0, 0, PasteOwnership::None, 0)
                .unwrap(),
        )
        .unwrap();
    let request = maintenance_request(2, MaintenanceOperation::Rollback);
    let acquire = state
        .acquire_maintenance(connection(2), capability(9), request)
        .unwrap();
    let revoked = acquire.terminal_offer().unwrap();
    // Native close proceeds while the writer effect remains outstanding.
    let after_close = state
        .confirm_admission_closed(action_token(
            &acquire,
            RequiredActionKind::CloseFreshAdmission,
        ))
        .unwrap();
    assert!(
        after_close
            .actions()
            .contains_kind(RequiredActionKind::PersistMaintenanceRecord)
    );
    assert!(
        state
            .offer_predecessor_terminal(PredecessorTerminalEvent::LeaseRevoked(
                talking_quill_keyboard_owner::state::TerminalReason::Maintenance,
            ))
            .is_none()
    );
    state.confirm_predecessor_terminal_written(revoked).unwrap();
    let draining = state
        .offer_predecessor_terminal(PredecessorTerminalEvent::LeaseDraining(
            TerminalOwnership::Activation,
        ))
        .unwrap();
    assert!(revoked.same_route_as(draining));
    state
        .confirm_predecessor_terminal_written(draining)
        .unwrap();
    state
        .confirm_maintenance_persisted(
            action_token(&after_close, RequiredActionKind::PersistMaintenanceRecord),
            request,
        )
        .unwrap();
    state
        .observe_native_ownership(NativeOwnership::NEUTRAL)
        .unwrap();
    let final_offer = state
        .offer_predecessor_terminal(PredecessorTerminalEvent::LeaseNeutral)
        .unwrap();
    assert!(revoked.same_route_as(final_offer));
    state.fail_predecessor_terminal_write(final_offer).unwrap();
    assert!(
        state
            .offer_predecessor_terminal(PredecessorTerminalEvent::LeaseNeutral)
            .is_none()
    );
}

#[test]
fn maintenance_persistence_failure_remains_sealed_degraded_without_capability() {
    let mut state = healthy(READY);
    let request = maintenance_request(3, MaintenanceOperation::Uninstall);
    let acquire = state
        .acquire_maintenance(connection(2), capability(9), request)
        .unwrap();
    state
        .fail_native_action(
            action_token(&acquire, RequiredActionKind::PersistMaintenanceRecord),
            NativeActionFailure::FailedNotApplied,
        )
        .unwrap();
    assert_eq!(state.maintenance_phase(), MaintenancePhase::SealedFailed);
    assert_eq!(state.process(), ProcessState::Degraded);
    assert!(!matches!(
        state.controller(),
        ControllerState::MaintenanceExclusive { .. }
    ));
    assert_eq!(state.admission(), AdmissionState::Closed);
}

#[test]
fn maintenance_disconnect_stays_sealed_and_exact_transaction_reacquires_new_epoch() {
    let mut state = healthy(READY);
    let maintainer = connection(2);
    let request = maintenance_request(5, MaintenanceOperation::Rollback);
    let (authority, _) = persisted_maintenance(&mut state, maintainer, request);
    state.controller_disconnected(maintainer).unwrap();
    assert_eq!(state.maintenance_phase(), MaintenancePhase::Sealed);
    assert_eq!(
        state
            .acquire_maintenance(
                connection(3),
                capability(10),
                maintenance_request(6, MaintenanceOperation::Rollback),
            )
            .unwrap_err()
            .kind(),
        TransitionErrorKind::MaintenanceTransactionMismatch
    );
    let reacquired = state
        .acquire_maintenance(connection(3), capability(10), request)
        .unwrap();
    assert_eq!(
        reacquired.response_stage(),
        Some(ResponseStage::MaintenanceAcquireReady)
    );
    let ControllerState::MaintenanceExclusive {
        authority: next, ..
    } = state.controller()
    else {
        panic!();
    };
    assert!(next.epoch() > authority.epoch());
}

#[test]
fn maintenance_prepare_stops_then_creates_response_then_flushes_then_exits() {
    let mut state = healthy(READY);
    let maintainer = connection(2);
    let request = maintenance_request(7, MaintenanceOperation::Update);
    let (authority, _) = persisted_maintenance(&mut state, maintainer, request);
    let correlation = ResponseCorrelation::new(41).unwrap();
    let stop = state
        .apply_maintenance_command(
            maintainer,
            authority,
            CommandSequence::FIRST,
            MaintenanceCommand::Prepare {
                operation: MaintenanceOperation::Update,
                response_correlation: correlation,
            },
        )
        .unwrap();
    assert_eq!(state.exit_phase(), ExitPhase::NativeStopPending);
    assert_eq!(stop.response_stage(), None);
    let ready = state
        .confirm_native_stopped(action_token(&stop, RequiredActionKind::StopNativeAdapter))
        .unwrap();
    assert_eq!(
        ready.response_stage(),
        Some(ResponseStage::FinalResponseReady)
    );
    assert!(!ready.actions().contains_kind(RequiredActionKind::ExitOwner));
    state.begin_final_response_flush(correlation).unwrap();
    assert_eq!(state.process(), ProcessState::FlushingResponse);
    let exit = state.confirm_final_response_flushed(correlation).unwrap();
    assert_eq!(action_kinds(&exit), vec![RequiredActionKind::ExitOwner]);
    assert_eq!(state.process(), ProcessState::Exiting);
}

#[test]
fn maintenance_prepare_connection_loss_never_creates_or_exits_on_success_response() {
    let mut state = healthy(READY);
    let maintainer = connection(2);
    let request = maintenance_request(8, MaintenanceOperation::Update);
    let (authority, _) = persisted_maintenance(&mut state, maintainer, request);
    let stop = state
        .apply_maintenance_command(
            maintainer,
            authority,
            CommandSequence::FIRST,
            MaintenanceCommand::Prepare {
                operation: MaintenanceOperation::Update,
                response_correlation: ResponseCorrelation::new(51).unwrap(),
            },
        )
        .unwrap();
    state.controller_disconnected(maintainer).unwrap();
    let stopped = state
        .confirm_native_stopped(action_token(&stop, RequiredActionKind::StopNativeAdapter))
        .unwrap();
    assert_eq!(stopped.response_stage(), None);
    assert!(
        !stopped
            .actions()
            .contains_kind(RequiredActionKind::ExitOwner)
    );
    assert_eq!(state.exit_phase(), ExitPhase::NativeStoppedSealed);
    let exit = state.maintenance_guard_lost().unwrap();
    assert_eq!(action_kinds(&exit), vec![RequiredActionKind::ExitOwner]);
}

#[test]
fn all_maintenance_mutations_and_renewals_reject_during_stop_response_and_flush_phases() {
    let mut state = healthy(READY);
    let maintainer = connection(2);
    let request = maintenance_request(9, MaintenanceOperation::Update);
    let (authority, _) = persisted_maintenance(&mut state, maintainer, request);
    let correlation = ResponseCorrelation::new(61).unwrap();
    let stop = state
        .apply_maintenance_command(
            maintainer,
            authority,
            CommandSequence::FIRST,
            MaintenanceCommand::Prepare {
                operation: MaintenanceOperation::Update,
                response_correlation: correlation,
            },
        )
        .unwrap();
    assert_eq!(
        state
            .apply_maintenance_command(
                maintainer,
                authority,
                CommandSequence::new(2).unwrap(),
                MaintenanceCommand::Renew,
            )
            .unwrap_err()
            .kind(),
        TransitionErrorKind::Stopping
    );
    state
        .confirm_native_stopped(action_token(&stop, RequiredActionKind::StopNativeAdapter))
        .unwrap();
    assert_eq!(
        state
            .apply_maintenance_command(
                maintainer,
                authority,
                CommandSequence::new(2).unwrap(),
                MaintenanceCommand::Renew,
            )
            .unwrap_err()
            .kind(),
        TransitionErrorKind::Stopping
    );
    state.begin_final_response_flush(correlation).unwrap();
    assert_eq!(
        state
            .apply_maintenance_command(
                maintainer,
                authority,
                CommandSequence::new(2).unwrap(),
                MaintenanceCommand::Renew,
            )
            .unwrap_err()
            .kind(),
        TransitionErrorKind::Stopping
    );
}

#[test]
fn rollback_status_remains_orthogonal_after_degradation() {
    let (mut state, _controller, _authority, _sequence) = enabled();
    let rollback = state.latch_runtime_rollback().unwrap();
    state
        .confirm_admission_closed(action_token(
            &rollback,
            RequiredActionKind::CloseFreshAdmission,
        ))
        .unwrap();
    state.recoverable_native_fault().unwrap();
    let status = state.status();
    assert!(status.rollback_latched);
    assert_eq!(status.process_state, ProcessState::Degraded);
    assert_eq!(status.reported_state, ReportedState::DegradedDraining);
    assert!(!state.can_open_keyboard());
    assert!(!state.can_begin_paste());
}

#[test]
fn stale_token_from_another_owner_instance_is_rejected_even_when_action_ids_match() {
    let (mut first, first_connection, first_authority) = lease(READY);
    let first_action = first
        .apply_capture_command(
            first_connection,
            first_authority,
            CommandSequence::FIRST,
            CaptureCommand::ReconcileSessionOff,
        )
        .unwrap();
    let stale = action_token(&first_action, RequiredActionKind::ApplySessionMode);

    let mut second = KeyboardOwnerState::new(owner(2));
    second.confirm_startup_snapshot_seeded().unwrap();
    second.startup_completed().unwrap();
    second.observe_native_readiness(READY).unwrap();
    let second_connection = connection(1);
    second.authenticate_observer(second_connection).unwrap();
    second
        .acquire_capture_lease(second_connection, capability(1))
        .unwrap();
    let ControllerState::CaptureLeaseDisabled {
        authority: second_authority,
        ..
    } = second.controller()
    else {
        panic!();
    };
    second
        .apply_capture_command(
            second_connection,
            second_authority,
            CommandSequence::FIRST,
            CaptureCommand::ReconcileSessionOff,
        )
        .unwrap();
    assert_eq!(
        second
            .confirm_session_mode_applied(stale, SessionCaptureMode::Off)
            .unwrap_err()
            .kind(),
        TransitionErrorKind::NativeConfirmationMismatch
    );
    assert_eq!(second.process(), ProcessState::Degraded);
}

#[test]
fn all_authority_and_state_debug_output_is_redacted() {
    let (mut state, controller, authority) = lease(READY);
    let action = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::FIRST,
            CaptureCommand::ReplaceConfiguration {
                revision: revision(777),
                bindings: ActivationBindings::default(),
            },
        )
        .unwrap();
    let token = action_token(&action, RequiredActionKind::ApplyConfiguration);
    let output = format!(
        "{state:#?} {:?} {authority:#?} {token:#?} {action:#?}",
        state.controller()
    );
    assert!(output.contains("<redacted>"));
    for forbidden in ["777", "[1, 1", "capture_epoch", "last_action_id"] {
        assert!(!output.contains(forbidden), "leaked {forbidden}: {output}");
    }
}

#[test]
fn maintenance_disconnect_during_sealing_or_persistence_never_installs_dead_authority() {
    let (mut sealing, _controller, _authority, _sequence) = enabled();
    let maintainer = connection(2);
    let request = maintenance_request(20, MaintenanceOperation::Update);
    let acquire = sealing
        .acquire_maintenance(maintainer, capability(9), request)
        .unwrap();
    sealing.controller_disconnected(maintainer).unwrap();
    let persistence = sealing
        .confirm_admission_closed(action_token(
            &acquire,
            RequiredActionKind::CloseFreshAdmission,
        ))
        .unwrap();
    let completed = sealing
        .confirm_maintenance_persisted(
            action_token(&persistence, RequiredActionKind::PersistMaintenanceRecord),
            request,
        )
        .unwrap();
    assert_eq!(completed.response_stage(), None);
    assert_eq!(sealing.maintenance_phase(), MaintenancePhase::Sealed);
    assert!(matches!(
        sealing.controller(),
        ControllerState::NoController
    ));

    let mut persisting = healthy(READY);
    let request = maintenance_request(21, MaintenanceOperation::Uninstall);
    let persistence = persisting
        .acquire_maintenance(maintainer, capability(10), request)
        .unwrap();
    assert_eq!(persisting.maintenance_phase(), MaintenancePhase::Persisting);
    persisting.controller_disconnected(maintainer).unwrap();
    let completed = persisting
        .confirm_maintenance_persisted(
            action_token(&persistence, RequiredActionKind::PersistMaintenanceRecord),
            request,
        )
        .unwrap();
    assert_eq!(completed.response_stage(), None);
    assert_eq!(persisting.maintenance_phase(), MaintenancePhase::Sealed);
}

#[test]
fn maintenance_waits_for_pending_paste_admission_and_cancellation_before_persistence() {
    let (mut state, controller, authority) = lease(SAFE_DISABLED_PASTE_READY);
    let authorization = paste_authorization(&state, authority, 22);
    let admit = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::FIRST,
            CaptureCommand::BeginPaste(authorization),
        )
        .unwrap();
    let request = maintenance_request(22, MaintenanceOperation::Update);
    let acquire = state
        .acquire_maintenance(connection(2), capability(9), request)
        .unwrap();
    assert!(
        !acquire
            .actions()
            .contains_kind(RequiredActionKind::PersistMaintenanceRecord)
    );
    let cancel = state
        .confirm_paste_waiting(
            action_token(&admit, RequiredActionKind::AdmitPaste),
            authorization,
        )
        .unwrap();
    assert_eq!(
        action_kinds(&cancel),
        vec![RequiredActionKind::CancelWaitingPaste]
    );
    let persistence = state
        .confirm_waiting_paste_cancelled(
            action_token(&cancel, RequiredActionKind::CancelWaitingPaste),
            authorization,
        )
        .unwrap();
    assert_eq!(
        action_kinds(&persistence),
        vec![RequiredActionKind::PersistMaintenanceRecord]
    );
}

#[test]
fn repeated_close_and_rollback_never_duplicate_pending_or_uncertain_cancellation() {
    let (mut state, controller, authority, sequence) = enabled();
    state
        .observe_native_ownership(
            NativeOwnership::new(CandidateOwnership::Active, 0, 0, 0, PasteOwnership::None, 0)
                .unwrap(),
        )
        .unwrap();
    let close = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(sequence + 1).unwrap(),
            CaptureCommand::Disable,
        )
        .unwrap();
    let cancel = state
        .confirm_admission_closed(action_token(
            &close,
            RequiredActionKind::CloseFreshAdmission,
        ))
        .unwrap();
    let rollback = state.latch_runtime_rollback().unwrap();
    assert!(
        !rollback
            .actions()
            .contains_kind(RequiredActionKind::CancelCandidate)
    );
    let completed = state
        .confirm_candidate_cancelled(
            action_token(&cancel, RequiredActionKind::CancelCandidate),
            NativeOwnership::NEUTRAL,
        )
        .unwrap();
    assert!(
        !completed
            .actions()
            .contains_kind(RequiredActionKind::CancelCandidate)
    );

    let (mut uncertain, controller, authority, sequence) = enabled();
    uncertain
        .observe_native_ownership(
            NativeOwnership::new(CandidateOwnership::Active, 0, 0, 0, PasteOwnership::None, 0)
                .unwrap(),
        )
        .unwrap();
    let close = uncertain
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(sequence + 1).unwrap(),
            CaptureCommand::Disable,
        )
        .unwrap();
    let cancel = uncertain
        .confirm_admission_closed(action_token(
            &close,
            RequiredActionKind::CloseFreshAdmission,
        ))
        .unwrap();
    uncertain
        .fail_native_action(
            action_token(&cancel, RequiredActionKind::CancelCandidate),
            NativeActionFailure::Indeterminate,
        )
        .unwrap();
    let rollback = uncertain.latch_runtime_rollback().unwrap();
    assert!(
        !rollback
            .actions()
            .contains_kind(RequiredActionKind::CancelCandidate)
    );
}

#[test]
fn retained_predecessor_route_blocks_new_lease_until_final_or_writer_failure() {
    let (mut state, controller, authority) = lease(READY);
    let release = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::FIRST,
            CaptureCommand::Release,
        )
        .unwrap();
    let revoked = release.terminal_offer().unwrap();
    state.confirm_predecessor_terminal_written(revoked).unwrap();
    let next = connection(2);
    state.authenticate_observer(next).unwrap();
    assert_eq!(
        state
            .acquire_capture_lease(next, capability(2))
            .unwrap_err()
            .kind(),
        TransitionErrorKind::Draining
    );
    let final_offer = state
        .offer_predecessor_terminal(PredecessorTerminalEvent::LeaseNeutral)
        .unwrap();
    state
        .confirm_predecessor_terminal_written(final_offer)
        .unwrap();
    state.acquire_capture_lease(next, capability(2)).unwrap();
}

#[test]
fn every_independent_keyboard_readiness_loss_invalidates_reconciliation() {
    let (mut state, controller, authority) = lease(READY);
    reconcile_disabled(&mut state, controller, authority);
    state
        .observe_native_readiness(NativeReadiness {
            keyboard_build_eligible: false,
            ..READY
        })
        .unwrap();
    let replacement = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(3).unwrap(),
            CaptureCommand::ReplaceConfiguration {
                revision: revision(2),
                bindings: ActivationBindings::default(),
            },
        )
        .unwrap();
    let request = configuration_request(&replacement);
    state
        .confirm_configuration_applied(
            action_token(&replacement, RequiredActionKind::ApplyConfiguration),
            request,
        )
        .unwrap();
    assert!(state.applied_configuration().is_some());
    state
        .observe_native_readiness(NativeReadiness {
            keyboard_build_eligible: false,
            hook_healthy: false,
            ..READY
        })
        .unwrap();
    assert_eq!(state.applied_configuration(), None);
    assert_eq!(state.configuration_high_water(), Some(revision(2)));
}

#[test]
fn conservative_native_work_can_rise_while_open_and_must_drain_before_neutral() {
    let (mut state, _controller, _authority, _sequence) = enabled();
    let pending = NativeOwnershipObservation {
        candidate: CandidateOwnership::None,
        activation_drain_keys: 0,
        session_drain_keys: 0,
        replay_cleanup_edges: 0,
        paste: PasteOwnership::None,
        conservative_native_work: true,
        admitted_effects: 0,
    };
    state.observe_native_observation(pending).unwrap();
    assert!(!state.ownership().is_native_neutral());
    state
        .observe_native_observation(NativeOwnershipObservation {
            conservative_native_work: false,
            ..pending
        })
        .unwrap();
    assert!(state.ownership().is_native_neutral());
}

#[test]
fn impossible_native_observation_is_retained_and_forces_degraded_closure() {
    let (mut state, _controller, _authority, _sequence) = enabled();
    let error = state
        .observe_native_observation(NativeOwnershipObservation {
            candidate: CandidateOwnership::None,
            activation_drain_keys: ACTIVATION_KEY_CAPACITY as u16 + 1,
            session_drain_keys: 0,
            replay_cleanup_edges: 0,
            paste: PasteOwnership::None,
            conservative_native_work: false,
            admitted_effects: 0,
        })
        .unwrap_err();
    assert_eq!(
        error.kind(),
        TransitionErrorKind::InvalidOwnershipTransition
    );
    assert!(
        error
            .actions()
            .contains_kind(RequiredActionKind::CloseFreshAdmission)
    );
    assert!(state.has_impossible_native_observation());
    assert_eq!(state.process(), ProcessState::Degraded);
    assert!(state.status().native_state_unknown);
}

#[test]
fn rollback_revokes_capture_and_returns_an_immediate_draining_disposition() {
    let (mut state, controller, authority, sequence) = enabled();
    let rollback = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(sequence + 1).unwrap(),
            CaptureCommand::RuntimeRollback,
        )
        .unwrap();
    assert_eq!(
        rollback.lease_disposition(),
        Some(LeaseDisposition::Draining)
    );
    assert!(matches!(state.controller(), ControllerState::NoController));
    assert_eq!(
        state
            .apply_capture_command(
                controller,
                authority,
                CommandSequence::new(sequence + 2).unwrap(),
                CaptureCommand::Disable,
            )
            .unwrap_err()
            .kind(),
        TransitionErrorKind::WrongController
    );
}

#[test]
fn release_during_paste_admission_cancels_after_waiting_confirmation() {
    let (mut state, controller, authority) = lease(SAFE_DISABLED_PASTE_READY);
    let authorization = paste_authorization(&state, authority, 23);
    let admit = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::FIRST,
            CaptureCommand::BeginPaste(authorization),
        )
        .unwrap();
    let release = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(2).unwrap(),
            CaptureCommand::Release,
        )
        .unwrap();
    assert_eq!(
        release.lease_disposition(),
        Some(LeaseDisposition::Draining)
    );
    let cancel = state
        .confirm_paste_waiting(
            action_token(&admit, RequiredActionKind::AdmitPaste),
            authorization,
        )
        .unwrap();
    assert_eq!(
        action_kinds(&cancel),
        vec![RequiredActionKind::CancelWaitingPaste]
    );
}

#[test]
fn indeterminate_paste_admission_and_recoverable_fault_both_close_and_cancel() {
    let (mut indeterminate, controller, authority, sequence) = enabled();
    let authorization = paste_authorization(&indeterminate, authority, 28);
    let admit = indeterminate
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(sequence + 1).unwrap(),
            CaptureCommand::BeginPaste(authorization),
        )
        .unwrap();
    let failure = indeterminate
        .fail_native_action(
            action_token(&admit, RequiredActionKind::AdmitPaste),
            NativeActionFailure::Indeterminate,
        )
        .unwrap();
    assert!(
        failure
            .actions()
            .contains_kind(RequiredActionKind::CloseFreshAdmission)
    );
    assert_eq!(indeterminate.admission(), AdmissionState::Closing);
    assert_eq!(
        indeterminate.ownership().paste(),
        PasteOwnership::Indeterminate
    );

    let (mut recoverable, controller, authority, sequence) = enabled();
    let authorization = paste_authorization(&recoverable, authority, 29);
    let admit = recoverable
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(sequence + 1).unwrap(),
            CaptureCommand::BeginPaste(authorization),
        )
        .unwrap();
    let close = recoverable.recoverable_native_fault().unwrap();
    assert!(
        close
            .actions()
            .contains_kind(RequiredActionKind::CloseFreshAdmission)
    );
    recoverable
        .confirm_admission_closed(action_token(
            &close,
            RequiredActionKind::CloseFreshAdmission,
        ))
        .unwrap();
    let cancel = recoverable
        .confirm_paste_waiting(
            action_token(&admit, RequiredActionKind::AdmitPaste),
            authorization,
        )
        .unwrap();
    assert!(
        cancel
            .actions()
            .contains_kind(RequiredActionKind::CancelWaitingPaste)
    );
}

#[test]
fn uncertain_candidate_cancellation_retires_queued_paste_config_and_drain() {
    let (mut state, controller, authority, mut sequence) = enabled();
    state
        .observe_native_ownership(
            NativeOwnership::new(CandidateOwnership::Active, 1, 0, 0, PasteOwnership::None, 0)
                .unwrap(),
        )
        .unwrap();
    let paste = paste_authorization(&state, authority, 24);
    let admit = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(sequence + 1).unwrap(),
            CaptureCommand::BeginPaste(paste),
        )
        .unwrap();
    sequence += 1;
    state
        .confirm_paste_waiting(action_token(&admit, RequiredActionKind::AdmitPaste), paste)
        .unwrap();
    let close = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::new(sequence + 1).unwrap(),
            CaptureCommand::ReplaceConfiguration {
                revision: revision(2),
                bindings: ActivationBindings::default(),
            },
        )
        .unwrap();
    let cancel = state
        .confirm_admission_closed(action_token(
            &close,
            RequiredActionKind::CloseFreshAdmission,
        ))
        .unwrap();
    state
        .fail_native_action(
            action_token(&cancel, RequiredActionKind::CancelCandidate),
            NativeActionFailure::Indeterminate,
        )
        .unwrap();
    let readiness = state.observe_native_readiness(KEYBOARD_NOT_READY).unwrap();
    assert!(readiness.actions().as_slice().is_empty());
    let rollback = state.latch_runtime_rollback().unwrap();
    assert!(
        !rollback
            .actions()
            .contains_kind(RequiredActionKind::CancelWaitingPaste)
    );
    assert!(
        !rollback
            .actions()
            .contains_kind(RequiredActionKind::ApplyConfiguration)
    );
    assert!(
        !rollback
            .actions()
            .contains_kind(RequiredActionKind::ContinueNativeDrain)
    );
}

#[test]
fn maintenance_dispatches_drain_before_persistence_and_prepare_waits_for_neutrality() {
    let (mut state, _controller, _authority, _sequence) = enabled();
    state
        .observe_native_ownership(
            NativeOwnership::new(CandidateOwnership::None, 1, 0, 0, PasteOwnership::None, 0)
                .unwrap(),
        )
        .unwrap();
    let maintainer = connection(2);
    let request = maintenance_request(25, MaintenanceOperation::Update);
    let acquire = state
        .acquire_maintenance(maintainer, capability(9), request)
        .unwrap();
    let after_close = state
        .confirm_admission_closed(action_token(
            &acquire,
            RequiredActionKind::CloseFreshAdmission,
        ))
        .unwrap();
    assert_eq!(
        action_kinds(&after_close),
        vec![
            RequiredActionKind::ContinueNativeDrain,
            RequiredActionKind::PersistMaintenanceRecord,
        ]
    );
    state
        .confirm_maintenance_persisted(
            action_token(&after_close, RequiredActionKind::PersistMaintenanceRecord),
            request,
        )
        .unwrap();
    if let Some(offer) = acquire.terminal_offer() {
        state.fail_predecessor_terminal_write(offer).unwrap();
    }
    let ControllerState::MaintenanceExclusive { authority, .. } = state.controller() else {
        panic!();
    };
    assert_eq!(
        state
            .apply_maintenance_command(
                maintainer,
                authority,
                CommandSequence::FIRST,
                MaintenanceCommand::Prepare {
                    operation: MaintenanceOperation::Update,
                    response_correlation: ResponseCorrelation::new(70).unwrap(),
                },
            )
            .unwrap_err()
            .kind(),
        TransitionErrorKind::Draining
    );
    state
        .observe_native_ownership(NativeOwnership::NEUTRAL)
        .unwrap();
    let stop = state
        .apply_maintenance_command(
            maintainer,
            authority,
            CommandSequence::new(2).unwrap(),
            MaintenanceCommand::Prepare {
                operation: MaintenanceOperation::Update,
                response_correlation: ResponseCorrelation::new(71).unwrap(),
            },
        )
        .unwrap();
    assert!(
        stop.actions()
            .contains_kind(RequiredActionKind::StopNativeAdapter)
    );
}

#[test]
fn failed_maintenance_persistence_cannot_reacquire_a_success_capability() {
    let mut state = healthy(READY);
    let request = maintenance_request(26, MaintenanceOperation::Uninstall);
    let persistence = state
        .acquire_maintenance(connection(2), capability(9), request)
        .unwrap();
    state
        .fail_native_action(
            action_token(&persistence, RequiredActionKind::PersistMaintenanceRecord),
            NativeActionFailure::FailedNotApplied,
        )
        .unwrap();
    assert_eq!(
        state
            .acquire_maintenance(connection(3), capability(10), request)
            .unwrap_err()
            .kind(),
        TransitionErrorKind::Degraded
    );
}

#[test]
fn neutral_release_disposition_ignores_later_best_effort_terminal_route() {
    let (mut state, controller, authority) = lease(READY);
    let release = state
        .apply_capture_command(
            controller,
            authority,
            CommandSequence::FIRST,
            CaptureCommand::Release,
        )
        .unwrap();
    assert_eq!(release.lease_disposition(), Some(LeaseDisposition::Neutral));
    assert!(release.terminal_offer().is_some());
}

#[test]
fn terminal_final_status_must_match_authoritative_native_state() {
    let (mut state, controller, _authority, _sequence) = enabled();
    state
        .observe_native_ownership(
            NativeOwnership::new(CandidateOwnership::None, 1, 0, 0, PasteOwnership::None, 0)
                .unwrap(),
        )
        .unwrap();
    let loss = state.controller_disconnected(controller).unwrap();
    let revoked = loss.terminal_offer().unwrap();
    state.confirm_predecessor_terminal_written(revoked).unwrap();
    assert!(
        state
            .offer_predecessor_terminal(PredecessorTerminalEvent::LeaseNeutral)
            .is_none()
    );
    assert!(
        state
            .offer_predecessor_terminal(PredecessorTerminalEvent::LeaseUnavailable(
                talking_quill_keyboard_owner::state::TerminalUnavailableReason::OwnershipUnknown,
            ))
            .is_none()
    );
    assert!(
        state
            .offer_predecessor_terminal(PredecessorTerminalEvent::LeaseDraining(
                TerminalOwnership::Candidate,
            ))
            .is_none()
    );
    let draining = state
        .offer_predecessor_terminal(PredecessorTerminalEvent::LeaseDraining(
            TerminalOwnership::Activation,
        ))
        .unwrap();
    state
        .confirm_predecessor_terminal_written(draining)
        .unwrap();
    state
        .observe_native_ownership(NativeOwnership::NEUTRAL)
        .unwrap();
    state
        .confirm_admission_closed(action_token(&loss, RequiredActionKind::CloseFreshAdmission))
        .unwrap();
    assert!(
        state
            .offer_predecessor_terminal(PredecessorTerminalEvent::LeaseNeutral)
            .is_some()
    );
}

#[test]
fn ownership_unknown_terminal_reason_cannot_be_downgraded_by_later_native_fault() {
    let (mut state, controller, _authority, _sequence) = enabled();
    let close_error = state
        .observe_native_observation(NativeOwnershipObservation {
            candidate: CandidateOwnership::None,
            activation_drain_keys: ACTIVATION_KEY_CAPACITY as u16 + 1,
            session_drain_keys: 0,
            replay_cleanup_edges: 0,
            paste: PasteOwnership::None,
            conservative_native_work: false,
            admitted_effects: 0,
        })
        .unwrap_err();
    assert!(
        close_error
            .actions()
            .contains_kind(RequiredActionKind::CloseFreshAdmission)
    );
    let loss = state.controller_disconnected(controller).unwrap();
    let revoked = loss.terminal_offer().unwrap();
    state.confirm_predecessor_terminal_written(revoked).unwrap();
    state.recoverable_native_fault().unwrap();
    assert!(
        state
            .offer_predecessor_terminal(PredecessorTerminalEvent::LeaseUnavailable(
                talking_quill_keyboard_owner::state::TerminalUnavailableReason::NativeFault,
            ))
            .is_none()
    );
    assert!(
        state
            .offer_predecessor_terminal(PredecessorTerminalEvent::LeaseUnavailable(
                talking_quill_keyboard_owner::state::TerminalUnavailableReason::OwnershipUnknown,
            ))
            .is_some()
    );
}

#[test]
fn owner_generation_supports_full_nonzero_u64_and_actions_expose_exact_executor_fields() {
    let generation = OwnerActivationGeneration::new(u64::MAX).unwrap();
    assert_eq!(generation.get(), u64::MAX);
    let (state, _controller, authority) = lease(SAFE_DISABLED_PASTE_READY);
    let authorization = PasteAuthorization::new(
        operation(27),
        state.owner_instance(),
        authority.epoch(),
        generation,
    );
    assert_eq!(authorization.operation(), operation(27));
    assert_eq!(authorization.owner_instance(), state.owner_instance());
    assert_eq!(authorization.capture_epoch(), authority.epoch());
    assert_eq!(authorization.activation_generation(), generation);

    let request = maintenance_request(27, MaintenanceOperation::Update);
    assert_eq!(request.source_build(), digest(28));
    assert!(request.target_build().is_some());
    assert!(request.target_owner().is_some());
}

proptest! {
    #[test]
    fn generated_valid_ownership_bounds_round_trip(
        activation in 0_u8..=ACTIVATION_KEY_CAPACITY as u8,
        session in 0_u8..=SESSION_KEY_CAPACITY as u8,
        cleanup in 0_u8..=REPLAY_CLEANUP_EDGE_CAPACITY as u8,
        effects in 0_u8..=OWNER_ADMITTED_EFFECT_CAPACITY as u8,
    ) {
        let ownership = NativeOwnership::new(
            CandidateOwnership::None,
            activation,
            session,
            cleanup,
            PasteOwnership::None,
            effects,
        ).unwrap();
        prop_assert_eq!(ownership.activation_drain_keys(), activation);
        prop_assert_eq!(ownership.session_drain_keys(), session);
        prop_assert_eq!(ownership.replay_cleanup_edges(), cleanup);
        prop_assert_eq!(ownership.admitted_effects(), effects);
    }

    #[test]
    fn rollback_race_always_latches_and_removes_enable_availability(
        pending_kind in 0_u8..5,
    ) {
        let (mut state, controller, authority, sequence) = enabled();
        let mut rollback_sequence = sequence + 1;
        match pending_kind {
            0 => {}
            1 => {
                state.apply_capture_command(
                    controller,
                    authority,
                    CommandSequence::new(rollback_sequence).unwrap(),
                    CaptureCommand::Disable,
                ).unwrap();
                rollback_sequence += 1;
            }
            2 => {
                let authorization = paste_authorization(&state, authority, 20);
                state.apply_capture_command(
                    controller,
                    authority,
                    CommandSequence::new(rollback_sequence).unwrap(),
                    CaptureCommand::BeginPaste(authorization),
                ).unwrap();
                rollback_sequence += 1;
            }
            3 => {
                state.observe_native_ownership(
                    NativeOwnership::new(
                        CandidateOwnership::Active,
                        0,
                        0,
                        0,
                        PasteOwnership::None,
                        0,
                    ).unwrap(),
                ).unwrap();
            }
            _ => {
                state.observe_native_readiness(KEYBOARD_NOT_READY).unwrap();
            }
        }
        let result = state.apply_capture_command(
            controller,
            authority,
            CommandSequence::new(rollback_sequence).unwrap(),
            CaptureCommand::RuntimeRollback,
        );
        prop_assert!(result.is_ok());
        prop_assert!(state.status().rollback_latched);
        prop_assert!(!state.can_open_keyboard());
        prop_assert_ne!(state.controller(), ControllerState::CaptureLeaseEnabled {
            connection: controller,
            authority,
        });
    }

    #[test]
    fn semantic_rejections_consume_exact_command_sequence(next_revision in 1_u64..1000) {
        let (mut state, controller, authority) = lease(SAFE_DISABLED_PASTE_READY);
        let first = state.apply_capture_command(
            controller,
            authority,
            CommandSequence::FIRST,
            CaptureCommand::ReplaceConfiguration {
                revision: revision(next_revision),
                bindings: ActivationBindings::default(),
            },
        ).unwrap();
        let request = configuration_request(&first);
        state.confirm_configuration_applied(
            action_token(&first, RequiredActionKind::ApplyConfiguration),
            request,
        ).unwrap();
        let rejected = state.apply_capture_command(
            controller,
            authority,
            CommandSequence::new(2).unwrap(),
            CaptureCommand::ReplaceConfiguration {
                revision: revision(next_revision),
                bindings: ActivationBindings::default(),
            },
        ).unwrap_err();
        prop_assert_eq!(rejected.kind(), TransitionErrorKind::InvalidConfigurationRevision);
        let renew = state.apply_capture_command(
            controller,
            authority,
            CommandSequence::new(3).unwrap(),
            CaptureCommand::Renew,
        );
        prop_assert!(renew.is_ok());
    }
}
