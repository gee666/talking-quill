#![cfg(not(feature = "local-unsigned-owner"))]

use std::{
    collections::VecDeque,
    num::NonZeroU64,
    sync::{
        Arc, Barrier, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
};

use crossbeam_channel::{Receiver, Sender, unbounded};
use proptest::prelude::*;
use talking_quill_keyboard_core::{
    ActivationBinding, ActivationContext, ActivationGeneration, ActivationKey, EventPhase,
    KeyboardEvent, OWNER_ADMITTED_EFFECT_CAPACITY, ProfileId, SessionKey, Shortcut,
    ShortcutModifiers,
};
use talking_quill_keyboard_owner::state::{
    AdmissionState, CandidateOwnership, CapabilityId, ConnectionId, ControllerState,
    NativeActionFailure, NativeOwnership, NativeOwnershipObservation, NativeReadiness,
    OwnerInstanceId, PasteAuthorization, PasteOperationId, PasteOwnership, ProcessState,
    ReportedState, RequiredAction,
};
use talking_quill_keyboard_owner::{
    ActivationCaptureGate, AdapterEvent, AdapterEventDisposition, AdapterEventId,
    AdapterEventRejection, BrokerEvent, CapabilityIdSource, ExecutorCommand, ExecutorResult,
    NativeAdapter, NativeAdapterExecutor, NativeAdapterPump, NativeEffect, NativeEffectKind,
    NativeEffectResult, OwnerExecutor, OwnerProtocolServer, PasteCommitOutcome, PasteRefusal,
    ServerPump,
};
use talking_quill_owner_protocol::{
    Bytes32, FakeAuthenticatedMaterial, GatewayMessage, U64String,
    client::{ClientPoll, OwnerProtocolClient},
    fake_transport::{FakeOrderedEndpoint, fake_ordered_transport_pair},
    schema::{
        AcquireState, Binding, BindingShortcut, Bindings, CaptureCommandParams, Empty, ErrorCode,
        FrontAppResult, Letter, MaintenanceAcquireParams, MaintenanceOperation,
        MaintenancePrepareParams, Modifiers, ObservabilityResult, PasteRefusalReason,
        PermissionState, PermissionsResult, ProfileId as WireProfileId, Purpose,
        ReplaceConfigurationParams, Request, Response, SessionMode, SessionSetModeParams,
        SetEnabledParams, SuccessResult,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OrderEntry {
    Effect(NativeEffectKind),
    Acknowledgement(AdapterEventId),
}

const READY: NativeReadiness = NativeReadiness {
    keyboard_build_eligible: true,
    paste_ready: true,
    permissions_eligible: true,
    hook_healthy: true,
};

#[derive(Default)]
struct SharedFake {
    effects: Mutex<Vec<NativeEffect>>,
    acknowledgements: Mutex<Vec<(AdapterEventId, AdapterEventDisposition)>>,
    order: Mutex<Vec<OrderEntry>>,
    scripted: Mutex<VecDeque<NativeEffectResult>>,
}

#[derive(Clone)]
struct FakeAdapterHandle {
    events: Sender<AdapterEvent>,
    shared: Arc<SharedFake>,
    next_event: Arc<AtomicU64>,
}

impl FakeAdapterHandle {
    fn emit(&self, event: BrokerEvent) -> AdapterEventId {
        let value = self.next_event.fetch_add(1, Ordering::Relaxed);
        let id = AdapterEventId::new(value).expect("positive fake adapter event ID");
        self.events
            .send(AdapterEvent::new(id, event))
            .expect("fake adapter receiver remains alive");
        id
    }

    fn script(&self, result: NativeEffectResult) {
        self.shared.scripted.lock().unwrap().push_back(result);
    }

    fn effect_kinds(&self) -> Vec<NativeEffectKind> {
        self.shared
            .effects
            .lock()
            .unwrap()
            .iter()
            .copied()
            .map(NativeEffect::kind)
            .collect()
    }

    fn acknowledgements(&self) -> Vec<(AdapterEventId, AdapterEventDisposition)> {
        self.shared.acknowledgements.lock().unwrap().clone()
    }

    fn order(&self) -> Vec<OrderEntry> {
        self.shared.order.lock().unwrap().clone()
    }
}

struct FakeAdapter {
    events: Receiver<AdapterEvent>,
    shared: Arc<SharedFake>,
    next_event: Arc<AtomicU64>,
    readiness: NativeReadiness,
}

impl FakeAdapter {
    fn new(readiness: NativeReadiness) -> (Self, FakeAdapterHandle) {
        let (sender, receiver) = unbounded();
        let shared = Arc::new(SharedFake::default());
        let next_event = Arc::new(AtomicU64::new(1));
        (
            Self {
                events: receiver,
                shared: Arc::clone(&shared),
                next_event: Arc::clone(&next_event),
                readiness,
            },
            FakeAdapterHandle {
                events: sender,
                shared,
                next_event,
            },
        )
    }
}

impl NativeAdapter for FakeAdapter {
    fn seed_startup_physical_snapshot(&mut self) -> bool {
        true
    }

    fn readiness(&self) -> NativeReadiness {
        self.readiness
    }

    fn execute(&mut self, effect: NativeEffect) -> NativeEffectResult {
        self.shared.effects.lock().unwrap().push(effect);
        self.shared
            .order
            .lock()
            .unwrap()
            .push(OrderEntry::Effect(effect.kind()));
        if let Some(result) = self.shared.scripted.lock().unwrap().pop_front() {
            return result;
        }
        match effect.kind() {
            NativeEffectKind::CloseFreshAdmission
            | NativeEffectKind::EmergencyCloseFreshAdmission => {
                let high_water = self.next_event.load(Ordering::Acquire) - 1;
                NativeEffectResult::AdmissionClosed {
                    through_event: AdapterEventId::new(high_water),
                }
            }
            NativeEffectKind::AdmitPaste => NativeEffectResult::PasteWaiting,
            NativeEffectKind::CancelCandidate => {
                NativeEffectResult::CandidateCancelled(NativeOwnership::NEUTRAL)
            }
            _ => NativeEffectResult::Applied,
        }
    }

    fn try_next_event(&mut self) -> Option<AdapterEvent> {
        self.events.try_recv().ok()
    }

    fn acknowledge_event(&mut self, id: AdapterEventId, disposition: AdapterEventDisposition) {
        self.shared
            .acknowledgements
            .lock()
            .unwrap()
            .push((id, disposition));
        self.shared
            .order
            .lock()
            .unwrap()
            .push(OrderEntry::Acknowledgement(id));
    }

    fn permissions(&self) -> PermissionsResult {
        PermissionsResult {
            accessibility: PermissionState::NotRequired,
            input_monitoring: PermissionState::NotRequired,
            event_post: PermissionState::NotRequired,
        }
    }

    fn front_app(&self) -> FrontAppResult {
        FrontAppResult {
            available: false,
            application_token: None,
        }
    }

    fn observability(&self) -> ObservabilityResult {
        ObservabilityResult::default()
    }
}

struct FakeCapabilities(u64);

impl CapabilityIdSource for FakeCapabilities {
    fn next_capability_id(&mut self) -> Option<CapabilityId> {
        let value = self.0;
        self.0 = value.checked_add(1)?;
        let mut bytes = [0_u8; 32];
        bytes[..8].copy_from_slice(&value.to_be_bytes());
        CapabilityId::new(bytes)
    }
}

type TestExecutor = NativeAdapterExecutor<FakeAdapter, FakeCapabilities>;

fn owner(value: u8) -> OwnerInstanceId {
    OwnerInstanceId::new([value; 32]).unwrap()
}

fn connection(value: u64) -> ConnectionId {
    ConnectionId::new(value).unwrap()
}

fn wire_u64(value: u64) -> U64String {
    U64String::new(NonZeroU64::new(value).unwrap())
}

fn material(value: u8, purpose: Purpose) -> FakeAuthenticatedMaterial {
    FakeAuthenticatedMaterial::new(
        Bytes32::new([value; 32]),
        purpose,
        [value.wrapping_add(1); 32],
        [value.wrapping_add(2); 32],
    )
}

fn open_server(
    value: u8,
) -> (
    OwnerProtocolServer<'static, TestExecutor>,
    FakeAdapterHandle,
) {
    let (adapter, handle) = FakeAdapter::new(READY);
    let executor = NativeAdapterExecutor::new_for_test(
        adapter,
        FakeCapabilities(1),
        ActivationCaptureGate::open_for_test_harness(),
    );
    (
        OwnerProtocolServer::start_fake(owner(value), executor).unwrap(),
        handle,
    )
}

#[cfg(not(any(
    feature = "local-unsigned-owner",
    feature = "transactional-shortcuts-dev",
    feature = "windows-native-test-input"
)))]
fn safe_server(
    value: u8,
) -> (
    OwnerProtocolServer<'static, TestExecutor>,
    FakeAdapterHandle,
) {
    let (adapter, handle) = FakeAdapter::new(READY);
    let executor = NativeAdapterExecutor::new(adapter, FakeCapabilities(1));
    (
        OwnerProtocolServer::start_fake(owner(value), executor).unwrap(),
        handle,
    )
}

fn attach<'a>(
    server: &mut OwnerProtocolServer<'a, TestExecutor>,
    material: &'a FakeAuthenticatedMaterial,
    id: ConnectionId,
) -> OwnerProtocolClient<'a> {
    let (client_codec, server_codec) = material.codecs().unwrap();
    let (client_endpoint, server_endpoint): (FakeOrderedEndpoint, FakeOrderedEndpoint) =
        fake_ordered_transport_pair();
    server
        .attach_connection(id, server_endpoint, server_codec)
        .unwrap();
    OwnerProtocolClient::new(client_endpoint, client_codec).unwrap()
}

fn request_response(
    server: &mut OwnerProtocolServer<'_, TestExecutor>,
    client: &mut OwnerProtocolClient<'_>,
    id: ConnectionId,
    request: &Request,
) -> Response {
    let expected = client.send_request(request).unwrap();
    assert_eq!(server.pump(id).unwrap(), ServerPump::Processed);
    loop {
        match client.poll().unwrap() {
            ClientPoll::Message(GatewayMessage::Response {
                correlation_sequence,
                response,
            }) => {
                assert_eq!(correlation_sequence, expected);
                return response;
            }
            ClientPoll::Message(_) => {}
            ClientPoll::Empty => panic!("response missing"),
            ClientPoll::PeerClosed => panic!("peer closed before response"),
        }
    }
}

#[derive(Clone, Copy)]
struct CaptureWire {
    id: Bytes32,
    epoch: U64String,
    next_sequence: u64,
}

impl CaptureWire {
    fn command(&mut self) -> CaptureCommandParams {
        let command_sequence = wire_u64(self.next_sequence);
        self.next_sequence += 1;
        CaptureCommandParams {
            capture_lease_id: self.id,
            capture_lease_epoch: self.epoch,
            command_sequence,
        }
    }
}

fn acquire_capture(
    server: &mut OwnerProtocolServer<'_, TestExecutor>,
    client: &mut OwnerProtocolClient<'_>,
    id: ConnectionId,
) -> CaptureWire {
    let Response::Success(SuccessResult::LeaseAcquire(acquired)) =
        request_response(server, client, id, &Request::LeaseAcquire(Empty {}))
    else {
        panic!("capture acquire failed")
    };
    assert_eq!(acquired.state, AcquireState::Disabled);
    CaptureWire {
        id: acquired.capture_lease_id,
        epoch: acquired.capture_lease_epoch,
        next_sequence: 1,
    }
}

fn one_binding() -> Bindings {
    Bindings::new(vec![Binding::new(
        WireProfileId::new("123e4567-e89b-12d3-a456-426614174000".to_owned()).unwrap(),
        BindingShortcut::new(Modifiers::new(true, false, false, false), vec![Letter::A]).unwrap(),
    )])
    .unwrap()
}

fn core_binding() -> ActivationBinding {
    ActivationBinding::new(
        ProfileId::new("123e4567-e89b-12d3-a456-426614174000").unwrap(),
        Shortcut::new(
            ShortcutModifiers {
                ctrl: true,
                alt: false,
                shift: false,
                meta: false,
            },
            &[ActivationKey::A],
        )
        .unwrap(),
    )
}

fn reconcile_and_enable(
    server: &mut OwnerProtocolServer<'_, TestExecutor>,
    client: &mut OwnerProtocolClient<'_>,
    id: ConnectionId,
    capture: &mut CaptureWire,
) {
    let off = capture.command();
    assert!(matches!(
        request_response(server, client, id, &Request::SessionReconcileOff(off)),
        Response::Success(SuccessResult::SessionMode(_))
    ));
    let configuration = ReplaceConfigurationParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        revision: wire_u64(1),
        bindings: one_binding(),
    };
    capture.next_sequence += 1;
    assert!(matches!(
        request_response(
            server,
            client,
            id,
            &Request::CaptureReplaceConfiguration(configuration),
        ),
        Response::Success(SuccessResult::Configuration(_))
    ));
    let enable = SetEnabledParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        enabled: true,
    };
    capture.next_sequence += 1;
    assert!(matches!(
        request_response(server, client, id, &Request::CaptureSetEnabled(enable)),
        Response::Success(SuccessResult::Enabled(_))
    ));
}

fn disable(
    server: &mut OwnerProtocolServer<'_, TestExecutor>,
    client: &mut OwnerProtocolClient<'_>,
    id: ConnectionId,
    capture: &mut CaptureWire,
) -> Response {
    let request = SetEnabledParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        enabled: false,
    };
    capture.next_sequence += 1;
    request_response(server, client, id, &Request::CaptureSetEnabled(request))
}

fn authorization(
    server: &OwnerProtocolServer<'_, TestExecutor>,
    operation: [u8; 32],
) -> PasteAuthorization {
    let (ControllerState::CaptureLeaseEnabled { authority, .. }
    | ControllerState::CaptureLeaseDisabled { authority, .. }) = server.state().controller()
    else {
        panic!("capture capability required")
    };
    PasteAuthorization::new(
        PasteOperationId::new(operation).unwrap(),
        server.state().owner_instance(),
        authority.epoch(),
        talking_quill_keyboard_owner::state::OwnerActivationGeneration::new(1).unwrap(),
    )
}

fn neutral_observation(candidate: CandidateOwnership) -> NativeOwnershipObservation {
    NativeOwnershipObservation {
        candidate,
        activation_drain_keys: 0,
        session_drain_keys: 0,
        replay_cleanup_edges: 0,
        paste: PasteOwnership::None,
        conservative_native_work: false,
        admitted_effects: 0,
    }
}

#[test]
fn emergency_close_maps_to_adapter_while_exit_remains_an_outer_loop_directive() {
    let (adapter, handle) = FakeAdapter::new(READY);
    let mut executor = NativeAdapterExecutor::new_for_test(
        adapter,
        FakeCapabilities(1),
        ActivationCaptureGate::open_for_test_harness(),
    );
    assert_eq!(
        executor.execute(ExecutorCommand::State(
            RequiredAction::EmergencyCloseFreshAdmission,
        )),
        ExecutorResult::AdmissionClosed {
            through_event: None,
        }
    );
    assert_eq!(
        executor.execute(ExecutorCommand::State(RequiredAction::ExitOwner)),
        ExecutorResult::Applied
    );
    assert_eq!(
        handle.effect_kinds(),
        vec![NativeEffectKind::EmergencyCloseFreshAdmission]
    );
}

#[cfg(not(any(
    feature = "local-unsigned-owner",
    feature = "transactional-shortcuts-dev",
    feature = "windows-native-test-input"
)))]
#[test]
fn production_constructor_masks_adapter_readiness_and_never_dispatches_open() {
    let material = material(1, Purpose::Capture);
    let (mut server, handle) = safe_server(1);
    assert!(!server.state().readiness().keyboard_build_eligible);
    let id = connection(1);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);

    let event = handle.emit(BrokerEvent::ReadinessChanged(READY));
    assert_eq!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    );
    assert_eq!(
        handle.acknowledgements(),
        vec![(event, AdapterEventDisposition::Accepted)]
    );
    assert!(!server.state().readiness().keyboard_build_eligible);

    let off = capture.command();
    request_response(
        &mut server,
        &mut client,
        id,
        &Request::SessionReconcileOff(off),
    );
    let configuration = ReplaceConfigurationParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        revision: wire_u64(1),
        bindings: one_binding(),
    };
    capture.next_sequence += 1;
    request_response(
        &mut server,
        &mut client,
        id,
        &Request::CaptureReplaceConfiguration(configuration),
    );
    let enable = SetEnabledParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        enabled: true,
    };
    let response = request_response(
        &mut server,
        &mut client,
        id,
        &Request::CaptureSetEnabled(enable),
    );
    assert!(matches!(
        response,
        Response::Error(ref error) if error.code() == ErrorCode::Unavailable
    ));
    assert!(
        !handle
            .effect_kinds()
            .contains(&NativeEffectKind::OpenFreshAdmission)
    );
    assert_eq!(server.state().admission(), AdmissionState::Closed);
}

#[test]
fn conservative_platform_work_round_trips_through_adapter_server_validation() {
    let material = material(42, Purpose::Capture);
    let (mut server, handle) = open_server(42);
    let id = connection(42);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    reconcile_and_enable(&mut server, &mut client, id, &mut capture);

    let pending = handle.emit(BrokerEvent::OwnershipChanged(NativeOwnershipObservation {
        conservative_native_work: true,
        ..neutral_observation(CandidateOwnership::None)
    }));
    assert_eq!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    );
    assert!(server.state().ownership().conservative_native_work());
    assert_eq!(
        handle.acknowledgements().last(),
        Some(&(pending, AdapterEventDisposition::Accepted))
    );

    let neutral = handle.emit(BrokerEvent::OwnershipChanged(neutral_observation(
        CandidateOwnership::None,
    )));
    assert_eq!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    );
    assert!(!server.state().ownership().conservative_native_work());
    assert_eq!(
        handle.acknowledgements().last(),
        Some(&(neutral, AdapterEventDisposition::Accepted))
    );
}

#[test]
fn effects_map_exact_payload_classes_and_preserve_dependency_order() {
    let material = material(2, Purpose::Capture);
    let (mut server, handle) = open_server(2);
    let id = connection(2);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    reconcile_and_enable(&mut server, &mut client, id, &mut capture);

    let recording = SessionSetModeParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        mode: SessionMode::Recording,
    };
    capture.next_sequence += 1;
    request_response(
        &mut server,
        &mut client,
        id,
        &Request::SessionSetMode(recording),
    );

    let active = handle.emit(BrokerEvent::OwnershipChanged(neutral_observation(
        CandidateOwnership::Active,
    )));
    assert_eq!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    );
    assert_eq!(handle.acknowledgements().last().unwrap().0, active);

    let release = capture.command();
    let response = request_response(
        &mut server,
        &mut client,
        id,
        &Request::LeaseRelease(release),
    );
    assert!(matches!(
        response,
        Response::Success(SuccessResult::Release(_))
    ));
    assert_eq!(server.state().reported_state(), ReportedState::IdleNeutral);

    assert_eq!(
        handle.effect_kinds(),
        vec![
            NativeEffectKind::ApplySessionMode,
            NativeEffectKind::ApplyConfiguration,
            NativeEffectKind::OpenFreshAdmission,
            NativeEffectKind::ApplySessionMode,
            NativeEffectKind::ContinueNativeDrain,
            NativeEffectKind::CloseFreshAdmission,
            NativeEffectKind::CancelCandidate,
        ]
    );
    let effects = handle.shared.effects.lock().unwrap();
    assert!(matches!(
        effects[1],
        NativeEffect::ApplyConfiguration { request, .. }
            if request.bindings().iter().eq([core_binding()])
    ));
}

#[test]
fn semantic_activation_audio_and_session_events_keep_acceptance_and_wire_order() {
    let material = material(3, Purpose::Capture);
    let (mut server, handle) = open_server(3);
    let id = connection(3);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    reconcile_and_enable(&mut server, &mut client, id, &mut capture);
    let recording = SessionSetModeParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        mode: SessionMode::Recording,
    };
    request_response(
        &mut server,
        &mut client,
        id,
        &Request::SessionSetMode(recording),
    );

    let context = ActivationContext::target_unavailable(ActivationGeneration::FIRST);
    let ids = [
        handle.emit(BrokerEvent::Keyboard(KeyboardEvent::Activation {
            binding: core_binding(),
            context,
            phase: EventPhase::Down,
        })),
        handle.emit(BrokerEvent::AudioInputDevicesChanged),
        handle.emit(BrokerEvent::Keyboard(KeyboardEvent::SessionKey {
            key: SessionKey::Escape,
            phase: EventPhase::Down,
        })),
        handle.emit(BrokerEvent::Keyboard(KeyboardEvent::SessionKey {
            key: SessionKey::Escape,
            phase: EventPhase::Up,
        })),
        handle.emit(BrokerEvent::Keyboard(KeyboardEvent::Activation {
            binding: core_binding(),
            context,
            phase: EventPhase::Up,
        })),
    ];
    for _ in ids {
        assert_eq!(
            server.pump_native_adapter(),
            NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
        );
    }
    server.flush_admitted_events().unwrap();
    assert_eq!(
        handle.acknowledgements(),
        ids.into_iter()
            .map(|id| (id, AdapterEventDisposition::Accepted))
            .collect::<Vec<_>>()
    );

    let mut observed = Vec::new();
    while let ClientPoll::Message(GatewayMessage::Event(event)) = client.poll().unwrap() {
        observed.push(match event {
            talking_quill_owner_protocol::schema::Event::Activation(_) => "activation",
            talking_quill_owner_protocol::schema::Event::AudioDevicesChanged(_) => "audio",
            talking_quill_owner_protocol::schema::Event::SessionKey(_) => "session",
            _ => "other",
        });
    }
    assert_eq!(
        observed,
        vec!["activation", "audio", "session", "session", "activation"]
    );
}

#[test]
fn readiness_loss_closes_before_acknowledgement_and_later_input_is_rejected() {
    let material = material(4, Purpose::Capture);
    let (mut server, handle) = open_server(4);
    let id = connection(4);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    reconcile_and_enable(&mut server, &mut client, id, &mut capture);

    let unavailable = NativeReadiness {
        hook_healthy: false,
        ..READY
    };
    let readiness = handle.emit(BrokerEvent::ReadinessChanged(unavailable));
    let preclose_audio = handle.emit(BrokerEvent::AudioInputDevicesChanged);
    assert_eq!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    );
    assert_eq!(server.state().admission(), AdmissionState::Closed);
    assert!(!server.state().status().native_state_unknown);
    assert_eq!(
        &handle.acknowledgements()[..2],
        &[
            (readiness, AdapterEventDisposition::Accepted),
            (preclose_audio, AdapterEventDisposition::Accepted),
        ]
    );
    assert!(matches!(
        client.poll().unwrap(),
        ClientPoll::Message(GatewayMessage::Event(
            talking_quill_owner_protocol::schema::Event::AudioDevicesChanged(_)
        ))
    ));

    let event = handle.emit(BrokerEvent::AudioInputDevicesChanged);
    assert_eq!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Rejected(
            AdapterEventRejection::AdmissionClosed,
        ))
    );
    assert_eq!(
        handle.acknowledgements().last().copied(),
        Some((
            event,
            AdapterEventDisposition::Rejected(AdapterEventRejection::AdmissionClosed),
        ))
    );
}

#[test]
fn consumed_readiness_event_is_accepted_when_close_recovery_is_deferred() {
    let material = material(27, Purpose::Capture);
    let (mut server, handle) = open_server(27);
    let id = connection(27);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    reconcile_and_enable(&mut server, &mut client, id, &mut capture);
    for _ in 0..8 {
        handle.script(NativeEffectResult::Failed(
            NativeActionFailure::FailedNotApplied,
        ));
    }
    let event = handle.emit(BrokerEvent::ReadinessChanged(NativeReadiness {
        hook_healthy: false,
        ..READY
    }));
    assert_eq!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    );
    assert_eq!(
        handle.acknowledgements().last().copied(),
        Some((event, AdapterEventDisposition::Accepted))
    );
    assert!(!server.state().readiness().hook_healthy);
    assert!(server.has_deferred_close());
    let later = handle.emit(BrokerEvent::AudioInputDevicesChanged);
    let acknowledgement_count = handle.acknowledgements().len();
    assert_eq!(
        server.pump_native_adapter(),
        NativeAdapterPump::ClosePending
    );
    assert_eq!(handle.acknowledgements().len(), acknowledgement_count + 1);
    assert_eq!(
        handle.acknowledgements().last().copied(),
        Some((later, AdapterEventDisposition::Accepted))
    );
    assert_eq!(server.state().admission(), AdmissionState::Closed);
    assert_eq!(server.pump_native_adapter(), NativeAdapterPump::Empty);
}

#[test]
fn duplicate_or_skipped_adapter_event_id_latches_fault_and_never_dispatches_event() {
    for duplicate in [true, false] {
        let value = if duplicate { 20 } else { 21 };
        let material = material(value, Purpose::Capture);
        let (mut server, handle) = open_server(value);
        let id = connection(u64::from(value));
        let mut client = attach(&mut server, &material, id);
        let mut capture = acquire_capture(&mut server, &mut client, id);
        reconcile_and_enable(&mut server, &mut client, id, &mut capture);

        if duplicate {
            handle.emit(BrokerEvent::AudioInputDevicesChanged);
            assert_eq!(
                server.pump_native_adapter(),
                NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
            );
            server.flush_admitted_events().unwrap();
        }
        let invalid_id = AdapterEventId::new(if duplicate { 1 } else { 2 }).unwrap();
        handle
            .events
            .send(AdapterEvent::new(
                invalid_id,
                BrokerEvent::AudioInputDevicesChanged,
            ))
            .unwrap();
        assert_eq!(
            server.pump_native_adapter(),
            NativeAdapterPump::Processed(AdapterEventDisposition::Rejected(
                AdapterEventRejection::InvalidTransition,
            ))
        );
        assert_eq!(server.state().process(), ProcessState::Degraded);
        assert!(server.state().status().native_state_unknown);
        assert_ne!(server.state().reported_state(), ReportedState::IdleNeutral);
        assert_eq!(
            handle.acknowledgements().last().copied(),
            Some((
                invalid_id,
                AdapterEventDisposition::Rejected(AdapterEventRejection::InvalidTransition),
            ))
        );
    }
}

#[test]
fn queue_capacity_accepts_exactly_eight_then_rejects_and_closes_before_overflow() {
    let material = material(5, Purpose::Capture);
    let (mut server, handle) = open_server(5);
    let id = connection(5);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    reconcile_and_enable(&mut server, &mut client, id, &mut capture);

    for _ in 0..OWNER_ADMITTED_EFFECT_CAPACITY {
        handle.emit(BrokerEvent::AudioInputDevicesChanged);
        assert_eq!(
            server.pump_native_adapter(),
            NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
        );
    }
    assert_eq!(server.queued_event_count(), OWNER_ADMITTED_EFFECT_CAPACITY);
    handle.emit(BrokerEvent::AudioInputDevicesChanged);
    assert_eq!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Rejected(
            AdapterEventRejection::Capacity,
        ))
    );
    assert_eq!(server.queued_event_count(), 0);
    assert_eq!(server.state().ownership().admitted_effects(), 0);
    assert_eq!(server.state().process(), ProcessState::Degraded);
    assert_eq!(server.state().admission(), AdmissionState::Closed);
    assert_eq!(
        handle
            .acknowledgements()
            .iter()
            .filter(|(_, disposition)| *disposition == AdapterEventDisposition::Accepted)
            .count(),
        OWNER_ADMITTED_EFFECT_CAPACITY
    );
}

#[test]
fn raw_max_plus_one_ownership_is_not_masked_and_forces_unknown_degraded_close() {
    let material = material(6, Purpose::Capture);
    let (mut server, handle) = open_server(6);
    let id = connection(6);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    reconcile_and_enable(&mut server, &mut client, id, &mut capture);

    let mut impossible = neutral_observation(CandidateOwnership::None);
    impossible.activation_drain_keys = 27;
    handle.emit(BrokerEvent::OwnershipChanged(impossible));
    assert_eq!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    );
    assert!(server.state().status().native_state_unknown);
    assert_eq!(server.state().process(), ProcessState::Degraded);
    assert_eq!(server.state().admission(), AdmissionState::Closed);
    assert!(server.state().has_impossible_native_observation());
}

#[test]
fn paste_claim_commit_indeterminate_and_late_resolution_are_one_shot() {
    let material = material(7, Purpose::Capture);
    let (mut server, handle) = open_server(7);
    let id = connection(7);
    let mut client = attach(&mut server, &material, id);
    let capture = acquire_capture(&mut server, &mut client, id);
    let operation = [70; 32];
    let auth = authorization(&server, operation);
    let paste = talking_quill_owner_protocol::schema::PasteInjectParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(1),
        operation_id: Bytes32::new(operation),
        owner_instance_id: Bytes32::new(*server.state().owner_instance().as_bytes()),
        activation_generation: wire_u64(1),
        target_token: None,
        fallback_text_sha256: Bytes32::new([71; 32]),
    };
    assert!(matches!(
        request_response(&mut server, &mut client, id, &Request::PasteInject(paste)),
        Response::Success(SuccessResult::Paste(_))
    ));

    handle.emit(BrokerEvent::PasteClaimed(auth));
    handle.emit(BrokerEvent::PasteFinished {
        authorization: auth,
        outcome: PasteCommitOutcome::Indeterminate,
    });
    assert!(matches!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    ));
    assert!(matches!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    ));
    assert_eq!(
        server.state().ownership().paste(),
        PasteOwnership::Indeterminate
    );

    handle.emit(BrokerEvent::PasteIndeterminateResolved(auth));
    assert!(matches!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    ));
    assert_eq!(server.state().ownership().paste(), PasteOwnership::None);
    handle.emit(BrokerEvent::PasteIndeterminateResolved(auth));
    assert!(matches!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Rejected(
            AdapterEventRejection::StaleScope,
        ))
    ));
}

#[test]
fn pending_paste_claim_linearizes_before_preflush_controller_loss() {
    let material = material(35, Purpose::Capture);
    let (mut server, handle) = open_server(35);
    let id = connection(35);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    reconcile_and_enable(&mut server, &mut client, id, &mut capture);
    let operation = [35; 32];
    let auth = authorization(&server, operation);
    let paste = talking_quill_owner_protocol::schema::PasteInjectParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        operation_id: Bytes32::new(operation),
        owner_instance_id: Bytes32::new(*server.state().owner_instance().as_bytes()),
        activation_generation: wire_u64(1),
        target_token: None,
        fallback_text_sha256: Bytes32::new([36; 32]),
    };
    request_response(&mut server, &mut client, id, &Request::PasteInject(paste));
    handle.emit(BrokerEvent::AudioInputDevicesChanged);
    assert!(matches!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    ));
    client.abort();
    let claim = handle.emit(BrokerEvent::PasteClaimed(auth));
    assert_eq!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    );
    assert_eq!(handle.acknowledgements().last().unwrap().0, claim);
    assert_eq!(server.state().ownership().paste(), PasteOwnership::Claimed);
    handle.emit(BrokerEvent::PasteFinished {
        authorization: auth,
        outcome: PasteCommitOutcome::Committed,
    });
    assert!(matches!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    ));
    assert_eq!(server.state().ownership().paste(), PasteOwnership::None);
}

#[test]
fn claimed_paste_completion_is_consumed_when_notification_writer_fails() {
    let material = material(29, Purpose::Capture);
    let (mut server, handle) = open_server(29);
    let id = connection(29);
    let mut client = attach(&mut server, &material, id);
    let capture = acquire_capture(&mut server, &mut client, id);
    let operation = [29; 32];
    let auth = authorization(&server, operation);
    let paste = talking_quill_owner_protocol::schema::PasteInjectParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        operation_id: Bytes32::new(operation),
        owner_instance_id: Bytes32::new(*server.state().owner_instance().as_bytes()),
        activation_generation: wire_u64(1),
        target_token: None,
        fallback_text_sha256: Bytes32::new([28; 32]),
    };
    request_response(&mut server, &mut client, id, &Request::PasteInject(paste));
    handle.emit(BrokerEvent::PasteClaimed(auth));
    assert!(matches!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    ));
    client.abort();
    let completion = handle.emit(BrokerEvent::PasteFinished {
        authorization: auth,
        outcome: PasteCommitOutcome::Committed,
    });
    assert_eq!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    );
    assert_eq!(
        handle.acknowledgements().last().copied(),
        Some((completion, AdapterEventDisposition::Accepted))
    );
    assert_eq!(server.state().ownership().paste(), PasteOwnership::None);
}

#[test]
fn claimed_paste_finishes_once_after_every_capture_authority_loss_boundary() {
    #[derive(Clone, Copy)]
    enum Boundary {
        Disconnect,
        Release,
        Rollback,
        Maintenance,
    }

    for (index, boundary) in [
        Boundary::Disconnect,
        Boundary::Release,
        Boundary::Rollback,
        Boundary::Maintenance,
    ]
    .into_iter()
    .enumerate()
    {
        let value = 30 + index as u8;
        let capture_material = material(value, Purpose::Capture);
        let maintenance_material = material(value + 40, Purpose::Maintenance);
        let (mut server, handle) = open_server(value);
        let capture_id = connection(u64::from(value));
        let mut capture_client = attach(&mut server, &capture_material, capture_id);
        let mut capture = acquire_capture(&mut server, &mut capture_client, capture_id);
        let operation = [value; 32];
        let auth = authorization(&server, operation);
        let paste = talking_quill_owner_protocol::schema::PasteInjectParams {
            capture_lease_id: capture.id,
            capture_lease_epoch: capture.epoch,
            command_sequence: wire_u64(capture.next_sequence),
            operation_id: Bytes32::new(operation),
            owner_instance_id: Bytes32::new(*server.state().owner_instance().as_bytes()),
            activation_generation: wire_u64(1),
            target_token: None,
            fallback_text_sha256: Bytes32::new([value + 1; 32]),
        };
        capture.next_sequence += 1;
        request_response(
            &mut server,
            &mut capture_client,
            capture_id,
            &Request::PasteInject(paste),
        );
        handle.emit(BrokerEvent::PasteClaimed(auth));
        assert_eq!(
            server.pump_native_adapter(),
            NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
        );

        match boundary {
            Boundary::Disconnect => {
                capture_client.abort();
                assert_eq!(server.pump(capture_id).unwrap(), ServerPump::PeerClosed);
            }
            Boundary::Release => {
                request_response(
                    &mut server,
                    &mut capture_client,
                    capture_id,
                    &Request::LeaseRelease(capture.command()),
                );
            }
            Boundary::Rollback => {
                request_response(
                    &mut server,
                    &mut capture_client,
                    capture_id,
                    &Request::RuntimeRollback(capture.command()),
                );
            }
            Boundary::Maintenance => {
                let maintenance_id = connection(u64::from(value) + 100);
                let mut maintenance = attach(&mut server, &maintenance_material, maintenance_id);
                let response = request_response(
                    &mut server,
                    &mut maintenance,
                    maintenance_id,
                    &Request::MaintenanceAcquire(MaintenanceAcquireParams::Uninstall {
                        transaction_id: Bytes32::new([value + 2; 32]),
                        source_build_digest: Bytes32::new([value + 3; 32]),
                    }),
                );
                assert!(matches!(
                    response,
                    Response::Success(SuccessResult::MaintenanceAcquire(
                        talking_quill_owner_protocol::schema::MaintenanceAcquireResult {
                            state: talking_quill_owner_protocol::schema::MaintenanceAcquireState::Draining,
                            ..
                        }
                    ))
                ));
            }
        }
        assert_eq!(server.state().ownership().paste(), PasteOwnership::Claimed);
        handle.emit(BrokerEvent::PasteFinished {
            authorization: auth,
            outcome: PasteCommitOutcome::Committed,
        });
        assert_eq!(
            server.pump_native_adapter(),
            NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
        );
        assert_eq!(server.state().ownership().paste(), PasteOwnership::None);
        assert!(matches!(
            server.pump_native_adapter(),
            NativeAdapterPump::Empty
        ));
    }
}

#[test]
fn disconnect_before_paste_claim_maps_to_exact_waiting_paste_cancellation() {
    let material = material(16, Purpose::Capture);
    let (mut server, handle) = open_server(16);
    let id = connection(16);
    let mut client = attach(&mut server, &material, id);
    let capture = acquire_capture(&mut server, &mut client, id);
    let operation = [160; 32];
    let paste = talking_quill_owner_protocol::schema::PasteInjectParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(1),
        operation_id: Bytes32::new(operation),
        owner_instance_id: Bytes32::new(*server.state().owner_instance().as_bytes()),
        activation_generation: wire_u64(1),
        target_token: None,
        fallback_text_sha256: Bytes32::new([161; 32]),
    };
    request_response(&mut server, &mut client, id, &Request::PasteInject(paste));
    client.abort();
    assert_eq!(server.pump(id).unwrap(), ServerPump::PeerClosed);
    assert_eq!(server.state().ownership().paste(), PasteOwnership::None);
    assert!(handle.effect_kinds().ends_with(&[
        NativeEffectKind::AdmitPaste,
        NativeEffectKind::CancelWaitingPaste,
    ]));
    let effects = handle.shared.effects.lock().unwrap();
    assert!(matches!(
        effects[0],
        NativeEffect::AdmitPaste { request, .. }
            if request.authorization.operation().as_bytes() == &operation
                && request.fallback_text_sha256 == Bytes32::new([161; 32])
    ));
}

#[test]
fn release_retains_closing_scope_and_orders_preclose_key_before_terminal_response() {
    let material = material(28, Purpose::Capture);
    let (mut server, handle) = open_server(28);
    let id = connection(28);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    reconcile_and_enable(&mut server, &mut client, id, &mut capture);
    let recording = SessionSetModeParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        mode: SessionMode::Recording,
    };
    capture.next_sequence += 1;
    request_response(
        &mut server,
        &mut client,
        id,
        &Request::SessionSetMode(recording),
    );
    let key = handle.emit(BrokerEvent::Keyboard(KeyboardEvent::SessionKey {
        key: SessionKey::Escape,
        phase: EventPhase::Down,
    }));
    let ownership = handle.emit(BrokerEvent::OwnershipChanged(NativeOwnershipObservation {
        session_drain_keys: 1,
        ..neutral_observation(CandidateOwnership::None)
    }));
    let sequence = client
        .send_request(&Request::LeaseRelease(capture.command()))
        .unwrap();
    assert_eq!(server.pump(id).unwrap(), ServerPump::Processed);
    assert_eq!(
        &handle.acknowledgements()[..2],
        &[
            (key, AdapterEventDisposition::Accepted),
            (ownership, AdapterEventDisposition::Accepted),
        ]
    );
    assert!(matches!(
        client.poll().unwrap(),
        ClientPoll::Message(GatewayMessage::Event(
            talking_quill_owner_protocol::schema::Event::SessionKey(_)
        ))
    ));
    assert!(matches!(
        client.poll().unwrap(),
        ClientPoll::Message(GatewayMessage::PredecessorTerminal(
            talking_quill_owner_protocol::schema::PredecessorTerminalEvent::LeaseRevoked { .. }
        ))
    ));
    assert!(matches!(
        client.poll().unwrap(),
        ClientPoll::Message(GatewayMessage::PredecessorTerminal(
            talking_quill_owner_protocol::schema::PredecessorTerminalEvent::LeaseDraining { .. }
        ))
    ));
    assert!(matches!(
        client.poll().unwrap(),
        ClientPoll::Message(GatewayMessage::Response {
            correlation_sequence,
            response: Response::Success(SuccessResult::Release(_)),
        }) if correlation_sequence == sequence
    ));
    assert_eq!(
        server.state().reported_state(),
        ReportedState::OrphanDraining
    );
    assert!(handle.effect_kinds().ends_with(&[
        NativeEffectKind::CloseFreshAdmission,
        NativeEffectKind::ContinueNativeDrain,
    ]));
    let order = handle.order();
    let close = order
        .iter()
        .rposition(|entry| *entry == OrderEntry::Effect(NativeEffectKind::CloseFreshAdmission))
        .unwrap();
    let key_ack = order
        .iter()
        .position(|entry| *entry == OrderEntry::Acknowledgement(key))
        .unwrap();
    let ownership_ack = order
        .iter()
        .position(|entry| *entry == OrderEntry::Acknowledgement(ownership))
        .unwrap();
    let drain = order
        .iter()
        .rposition(|entry| *entry == OrderEntry::Effect(NativeEffectKind::ContinueNativeDrain))
        .unwrap();
    assert!(close < key_ack && key_ack < ownership_ack && ownership_ack < drain);
}

#[test]
fn cancellation_cleanup_blocks_reacquire_until_authoritative_neutral_drain() {
    let first_material = material(17, Purpose::Capture);
    let second_material = material(18, Purpose::Capture);
    let (mut server, handle) = open_server(17);
    let first_id = connection(17);
    let second_id = connection(18);
    let mut first = attach(&mut server, &first_material, first_id);
    let mut capture = acquire_capture(&mut server, &mut first, first_id);
    reconcile_and_enable(&mut server, &mut first, first_id, &mut capture);
    handle.emit(BrokerEvent::OwnershipChanged(neutral_observation(
        CandidateOwnership::Active,
    )));
    assert!(matches!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    ));
    handle.script(NativeEffectResult::AdmissionClosed {
        through_event: AdapterEventId::new(1),
    });
    handle.script(NativeEffectResult::CandidateCancelled(
        NativeOwnership::new(CandidateOwnership::None, 0, 0, 1, PasteOwnership::None, 0).unwrap(),
    ));
    let response = request_response(
        &mut server,
        &mut first,
        first_id,
        &Request::LeaseRelease(capture.command()),
    );
    assert!(matches!(
        response,
        Response::Success(SuccessResult::Release(
            talking_quill_owner_protocol::schema::ReleaseResult {
                disposition: talking_quill_owner_protocol::schema::LeaseDisposition::Draining,
            }
        ))
    ));
    assert_eq!(
        server.state().reported_state(),
        ReportedState::OrphanDraining
    );

    let mut second = attach(&mut server, &second_material, second_id);
    let response = request_response(
        &mut server,
        &mut second,
        second_id,
        &Request::LeaseAcquire(Empty {}),
    );
    assert!(matches!(
        response,
        Response::Error(ref error) if error.code() == ErrorCode::Draining
    ));

    handle.emit(BrokerEvent::OwnershipChanged(neutral_observation(
        CandidateOwnership::None,
    )));
    assert_eq!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    );
    assert_eq!(server.state().reported_state(), ReportedState::IdleNeutral);
    assert!(matches!(
        request_response(
            &mut server,
            &mut second,
            second_id,
            &Request::LeaseAcquire(Empty {}),
        ),
        Response::Success(SuccessResult::LeaseAcquire(_))
    ));
}

#[test]
fn local_event_acceptance_precedes_writer_failure_and_controller_drain() {
    let material = material(19, Purpose::Capture);
    let (mut server, handle) = open_server(19);
    let id = connection(19);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    reconcile_and_enable(&mut server, &mut client, id, &mut capture);
    let event = handle.emit(BrokerEvent::AudioInputDevicesChanged);
    assert_eq!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    );
    client.abort();
    assert!(server.flush_admitted_events().is_err());
    assert_eq!(
        handle.acknowledgements().last().copied(),
        Some((event, AdapterEventDisposition::Accepted))
    );
    assert!(!matches!(
        server.state().reported_state(),
        ReportedState::LeaseEnabled | ReportedState::LeaseDisabled
    ));
}

#[test]
fn elapsed_heartbeat_deadline_revokes_a_live_open_transport() {
    let material = material(69, Purpose::Capture);
    let (mut server, _handle) = open_server(69);
    server.set_heartbeat_timeout_for_test(std::time::Duration::from_millis(2));
    let id = connection(69);
    let mut client = attach(&mut server, &material, id);
    let _capture = acquire_capture(&mut server, &mut client, id);
    std::thread::sleep(std::time::Duration::from_millis(10));

    assert_eq!(server.pump(id).unwrap(), ServerPump::FatalClosed);
    assert!(matches!(
        server.state().controller(),
        ControllerState::NoController
    ));
}

#[test]
fn heartbeat_expiry_retires_queued_events_before_same_id_can_be_reused() {
    for queued in [1, OWNER_ADMITTED_EFFECT_CAPACITY] {
        let original_material = material(70 + queued as u8, Purpose::Capture);
        let replacement_material = material(90 + queued as u8, Purpose::Capture);
        let (mut server, handle) = open_server(70 + queued as u8);
        let id = connection(70 + queued as u64);
        let mut client = attach(&mut server, &original_material, id);
        let mut capture = acquire_capture(&mut server, &mut client, id);
        reconcile_and_enable(&mut server, &mut client, id, &mut capture);
        for _ in 0..queued {
            handle.emit(BrokerEvent::AudioInputDevicesChanged);
            assert!(matches!(
                server.pump_native_adapter(),
                NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
            ));
        }
        assert_eq!(server.queued_event_count(), queued);
        server.expire_connection(id).unwrap();
        assert_eq!(server.queued_event_count(), 0);
        assert_eq!(server.state().ownership().admitted_effects(), 0);

        let mut replacement = attach(&mut server, &replacement_material, id);
        assert!(matches!(
            request_response(
                &mut server,
                &mut replacement,
                id,
                &Request::LeaseAcquire(Empty {}),
            ),
            Response::Success(SuccessResult::LeaseAcquire(_))
        ));
        assert_eq!(replacement.poll().unwrap(), ClientPoll::Empty);
    }
}

#[test]
fn teardown_runs_controller_loss_even_when_close_recovery_fails() {
    let original_material = material(98, Purpose::Capture);
    let (mut server, handle) = open_server(98);
    let id = connection(98);
    let mut client = attach(&mut server, &original_material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    reconcile_and_enable(&mut server, &mut client, id, &mut capture);
    handle.emit(BrokerEvent::AudioInputDevicesChanged);
    assert!(matches!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    ));
    for _ in 0..8 {
        handle.script(NativeEffectResult::Failed(
            NativeActionFailure::FailedNotApplied,
        ));
    }
    assert!(server.expire_connection(id).is_err());
    assert_eq!(server.queued_event_count(), 0);
    assert!(matches!(
        server.state().controller(),
        ControllerState::NoController
    ));
    assert!(server.has_deferred_close());

    let replacement_material = material(97, Purpose::Capture);
    let (_client_codec, server_codec) = replacement_material.codecs().unwrap();
    let (_client_endpoint, server_endpoint) = fake_ordered_transport_pair();
    assert!(matches!(
        server.attach_connection(id, server_endpoint, server_codec),
        Err(talking_quill_keyboard_owner::ServerError::DuplicateConnection)
    ));

    assert!(server.service_deferred_close().unwrap());
    assert_eq!(server.state().admission(), AdmissionState::Closed);
    let mut replacement = attach(&mut server, &replacement_material, id);
    assert!(matches!(
        request_response(
            &mut server,
            &mut replacement,
            id,
            &Request::LeaseAcquire(Empty {}),
        ),
        Response::Success(SuccessResult::LeaseAcquire(_))
    ));
}

#[test]
fn known_native_fault_finalizes_predecessor_with_unavailable_not_neutral() {
    let material = material(99, Purpose::Capture);
    let (mut server, handle) = open_server(99);
    let id = connection(99);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    reconcile_and_enable(&mut server, &mut client, id, &mut capture);
    handle.emit(BrokerEvent::OwnershipChanged(NativeOwnershipObservation {
        activation_drain_keys: 1,
        ..neutral_observation(CandidateOwnership::None)
    }));
    assert!(matches!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    ));
    request_response(
        &mut server,
        &mut client,
        id,
        &Request::LeaseRelease(capture.command()),
    );
    handle.emit(BrokerEvent::RecoverableNativeFault);
    assert_eq!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    );
    assert!(matches!(
        client.poll().unwrap(),
        ClientPoll::Message(GatewayMessage::PredecessorTerminal(
            talking_quill_owner_protocol::schema::PredecessorTerminalEvent::LeaseUnavailable {
                reason:
                    talking_quill_owner_protocol::schema::TerminalUnavailableReason::NativeFault,
                ..
            }
        ))
    ));
}

#[test]
fn mismatched_and_failed_effect_completions_fail_closed_without_dispatching_dependents() {
    let capture_material = material(8, Purpose::Capture);
    let (mut server, handle) = open_server(8);
    let id = connection(8);
    let mut client = attach(&mut server, &capture_material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    request_response(
        &mut server,
        &mut client,
        id,
        &Request::SessionReconcileOff(capture.command()),
    );
    handle.script(NativeEffectResult::PasteWaiting);
    let configuration = ReplaceConfigurationParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        revision: wire_u64(1),
        bindings: one_binding(),
    };
    let response = request_response(
        &mut server,
        &mut client,
        id,
        &Request::CaptureReplaceConfiguration(configuration),
    );
    assert!(matches!(
        response,
        Response::Error(ref error) if error.code() == ErrorCode::NativeFailure
    ));
    assert_eq!(server.state().process(), ProcessState::Degraded);
    assert!(
        !handle
            .effect_kinds()
            .contains(&NativeEffectKind::OpenFreshAdmission)
    );

    let material = material(9, Purpose::Capture);
    let (mut server, handle) = open_server(9);
    let id = connection(9);
    let mut client = attach(&mut server, &material, id);
    let capture = acquire_capture(&mut server, &mut client, id);
    handle.script(NativeEffectResult::Failed(
        NativeActionFailure::FailedNotApplied,
    ));
    let auth = authorization(&server, [90; 32]);
    let paste = talking_quill_owner_protocol::schema::PasteInjectParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(1),
        operation_id: Bytes32::new(*auth.operation().as_bytes()),
        owner_instance_id: Bytes32::new(*server.state().owner_instance().as_bytes()),
        activation_generation: wire_u64(1),
        target_token: None,
        fallback_text_sha256: Bytes32::new([91; 32]),
    };
    assert!(matches!(
        request_response(&mut server, &mut client, id, &Request::PasteInject(paste)),
        Response::Success(SuccessResult::Paste(
            talking_quill_owner_protocol::schema::PasteResult::ClipboardOnly {
                reason: PasteRefusalReason::NativeRejected,
            }
        ))
    ));
}

#[test]
fn invalid_candidate_confirmation_drives_attached_fail_closed_actions() {
    let material = material(23, Purpose::Capture);
    let (mut server, handle) = open_server(23);
    let id = connection(23);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    reconcile_and_enable(&mut server, &mut client, id, &mut capture);
    handle.emit(BrokerEvent::OwnershipChanged(neutral_observation(
        CandidateOwnership::Active,
    )));
    assert!(matches!(
        server.pump_native_adapter(),
        NativeAdapterPump::Processed(AdapterEventDisposition::Accepted)
    ));
    handle.script(NativeEffectResult::AdmissionClosed {
        through_event: AdapterEventId::new(1),
    });
    handle.script(NativeEffectResult::CandidateCancelled(
        NativeOwnership::new(CandidateOwnership::Active, 0, 0, 0, PasteOwnership::None, 0).unwrap(),
    ));
    let response = request_response(
        &mut server,
        &mut client,
        id,
        &Request::LeaseRelease(capture.command()),
    );
    assert!(matches!(
        response,
        Response::Error(ref error) if error.code() == ErrorCode::NativeFailure
    ));
    assert!(server.state().status().native_state_unknown);
    assert_ne!(server.state().reported_state(), ReportedState::IdleNeutral);
    let kinds = handle.effect_kinds();
    assert!(
        kinds.ends_with(&[
            NativeEffectKind::CloseFreshAdmission,
            NativeEffectKind::CancelCandidate,
        ]),
        "{kinds:?}",
    );
}

#[test]
fn indeterminate_close_retry_drains_watermark_backlog_while_admission_is_unknown() {
    let material = material(36, Purpose::Capture);
    let (mut server, handle) = open_server(36);
    let id = connection(36);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    reconcile_and_enable(&mut server, &mut client, id, &mut capture);
    let recording = SessionSetModeParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        mode: SessionMode::Recording,
    };
    capture.next_sequence += 1;
    request_response(
        &mut server,
        &mut client,
        id,
        &Request::SessionSetMode(recording),
    );
    let key = handle.emit(BrokerEvent::Keyboard(KeyboardEvent::SessionKey {
        key: SessionKey::Escape,
        phase: EventPhase::Down,
    }));
    let ownership = handle.emit(BrokerEvent::OwnershipChanged(NativeOwnershipObservation {
        session_drain_keys: 1,
        ..neutral_observation(CandidateOwnership::None)
    }));
    handle.script(NativeEffectResult::Failed(
        NativeActionFailure::Indeterminate,
    ));
    let response = disable(&mut server, &mut client, id, &mut capture);
    assert!(matches!(
        response,
        Response::Error(ref error) if error.code() == ErrorCode::NativeFailure
    ));
    assert_eq!(server.state().admission(), AdmissionState::Closed);
    assert!(server.state().status().native_state_unknown);
    assert_eq!(
        &handle.acknowledgements()[..2],
        &[
            (key, AdapterEventDisposition::Accepted),
            (ownership, AdapterEventDisposition::Accepted),
        ]
    );
    assert!(handle.effect_kinds().ends_with(&[
        NativeEffectKind::CloseFreshAdmission,
        NativeEffectKind::CloseFreshAdmission,
        NativeEffectKind::ContinueNativeDrain,
    ]));
}

#[test]
fn close_failure_retries_before_response_and_preserves_effect_order() {
    let material = material(10, Purpose::Capture);
    let (mut server, handle) = open_server(10);
    let id = connection(10);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    reconcile_and_enable(&mut server, &mut client, id, &mut capture);
    handle.script(NativeEffectResult::Failed(
        NativeActionFailure::FailedNotApplied,
    ));
    handle.script(NativeEffectResult::AdmissionClosed {
        through_event: None,
    });
    let response = disable(&mut server, &mut client, id, &mut capture);
    assert!(matches!(
        response,
        Response::Error(ref error) if error.code() == ErrorCode::NativeFailure
    ));
    assert_eq!(server.state().admission(), AdmissionState::Closed);
    assert!(handle.effect_kinds().ends_with(&[
        NativeEffectKind::CloseFreshAdmission,
        NativeEffectKind::CloseFreshAdmission,
    ]));
}

#[test]
fn maintenance_persistence_failure_finalizes_predecessor_as_native_unavailable() {
    let capture_material = material(24, Purpose::Capture);
    let maintenance_material = material(25, Purpose::Maintenance);
    let (mut server, handle) = open_server(24);
    let capture_id = connection(24);
    let maintenance_id = connection(25);
    let mut capture = attach(&mut server, &capture_material, capture_id);
    acquire_capture(&mut server, &mut capture, capture_id);
    let mut maintenance = attach(&mut server, &maintenance_material, maintenance_id);
    handle.script(NativeEffectResult::Failed(
        NativeActionFailure::FailedNotApplied,
    ));
    let response = request_response(
        &mut server,
        &mut maintenance,
        maintenance_id,
        &Request::MaintenanceAcquire(MaintenanceAcquireParams::Uninstall {
            transaction_id: Bytes32::new([24; 32]),
            source_build_digest: Bytes32::new([25; 32]),
        }),
    );
    assert!(matches!(
        response,
        Response::Error(ref error) if error.code() == ErrorCode::NativeFailure
    ));
    assert!(matches!(
        capture.poll().unwrap(),
        ClientPoll::Message(GatewayMessage::PredecessorTerminal(
            talking_quill_owner_protocol::schema::PredecessorTerminalEvent::LeaseRevoked { .. }
        ))
    ));
    assert!(matches!(
        capture.poll().unwrap(),
        ClientPoll::Message(GatewayMessage::PredecessorTerminal(
            talking_quill_owner_protocol::schema::PredecessorTerminalEvent::LeaseUnavailable {
                reason:
                    talking_quill_owner_protocol::schema::TerminalUnavailableReason::NativeFault,
                ..
            }
        ))
    ));
}

#[test]
fn maintenance_maps_persist_then_stop_and_keeps_process_exit_outside_adapter() {
    let material = material(11, Purpose::Maintenance);
    let (mut server, handle) = open_server(11);
    let id = connection(11);
    let mut client = attach(&mut server, &material, id);
    let transaction = Bytes32::new([110; 32]);
    let Response::Success(SuccessResult::MaintenanceAcquire(acquired)) = request_response(
        &mut server,
        &mut client,
        id,
        &Request::MaintenanceAcquire(MaintenanceAcquireParams::Uninstall {
            transaction_id: transaction,
            source_build_digest: Bytes32::new([111; 32]),
        }),
    ) else {
        panic!("maintenance acquire failed")
    };
    let prepare = MaintenancePrepareParams {
        maintenance_capability_id: acquired.maintenance_capability_id,
        maintenance_capability_epoch: acquired.maintenance_capability_epoch,
        command_sequence: wire_u64(1),
        transaction_id: transaction,
        operation: MaintenanceOperation::Uninstall,
    };
    assert!(matches!(
        request_response(
            &mut server,
            &mut client,
            id,
            &Request::MaintenancePrepare(prepare),
        ),
        Response::Success(SuccessResult::MaintenancePrepare(_))
    ));
    assert_eq!(
        handle.effect_kinds(),
        vec![
            NativeEffectKind::PersistMaintenanceRecord,
            NativeEffectKind::StopNativeAdapter,
        ]
    );
    assert!(server.exit_requested());
    assert_eq!(server.state().process(), ProcessState::Exiting);
}

#[test]
fn maintenance_stop_failure_never_flushes_success_or_exits() {
    let material = material(26, Purpose::Maintenance);
    let (mut server, handle) = open_server(26);
    let id = connection(26);
    let mut client = attach(&mut server, &material, id);
    let transaction = Bytes32::new([26; 32]);
    let Response::Success(SuccessResult::MaintenanceAcquire(acquired)) = request_response(
        &mut server,
        &mut client,
        id,
        &Request::MaintenanceAcquire(MaintenanceAcquireParams::Uninstall {
            transaction_id: transaction,
            source_build_digest: Bytes32::new([27; 32]),
        }),
    ) else {
        panic!("maintenance acquire failed")
    };
    handle.script(NativeEffectResult::Failed(
        NativeActionFailure::FailedNotApplied,
    ));
    let response = request_response(
        &mut server,
        &mut client,
        id,
        &Request::MaintenancePrepare(MaintenancePrepareParams {
            maintenance_capability_id: acquired.maintenance_capability_id,
            maintenance_capability_epoch: acquired.maintenance_capability_epoch,
            command_sequence: wire_u64(1),
            transaction_id: transaction,
            operation: MaintenanceOperation::Uninstall,
        }),
    );
    assert!(matches!(
        response,
        Response::Error(ref error) if error.code() == ErrorCode::NativeFailure
    ));
    assert_eq!(server.state().process(), ProcessState::Degraded);
    assert!(!server.exit_requested());
}

#[test]
fn all_boundary_debug_output_redacts_authority_target_hash_binding_and_event_values() {
    let material = material(12, Purpose::Capture);
    let (mut server, handle) = open_server(12);
    let id = connection(12);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    reconcile_and_enable(&mut server, &mut client, id, &mut capture);
    let event = AdapterEvent::new(
        AdapterEventId::new(9_876_543).unwrap(),
        BrokerEvent::Keyboard(KeyboardEvent::Activation {
            binding: core_binding(),
            context: ActivationContext::target_unavailable(ActivationGeneration::FIRST),
            phase: EventPhase::Down,
        }),
    );
    let event_debug = format!("{event:?}");
    assert_eq!(event_debug, "AdapterEvent(<redacted>)");
    assert!(!event_debug.contains("9876543"));

    for effect in handle.shared.effects.lock().unwrap().iter() {
        let debug = format!("{effect:?}");
        assert!(debug.contains("redacted") || *effect == NativeEffect::ContinueNativeDrain);
        for forbidden in [
            "123e4567-e89b-12d3-a456-426614174000",
            "9876543",
            "target",
            "sha256",
        ] {
            assert!(!debug.contains(forbidden));
        }
    }
    assert_eq!(
        format!(
            "{:?}",
            BrokerEvent::OwnershipChanged(neutral_observation(CandidateOwnership::None))
        ),
        "BrokerEvent::OwnershipChanged(<redacted>)"
    );
    assert_eq!(
        format!(
            "{:?}",
            NativeEffectResult::PasteRefused(PasteRefusal::SecureInput)
        ),
        "NativeEffectResult(<redacted>)"
    );
}

#[test]
fn every_preclaim_paste_refusal_has_an_exhaustive_stable_wire_mapping() {
    for (reason, wire) in [
        (
            PasteRefusal::PermissionDenied,
            PasteRefusalReason::PermissionDenied,
        ),
        (
            PasteRefusal::ConflictingModifiers,
            PasteRefusalReason::ConflictingModifiers,
        ),
        (PasteRefusal::SecureInput, PasteRefusalReason::SecureInput),
        (
            PasteRefusal::TargetUnavailable,
            PasteRefusalReason::TargetUnavailable,
        ),
        (
            PasteRefusal::ClipboardChanged,
            PasteRefusalReason::ClipboardChanged,
        ),
        (
            PasteRefusal::NativeUnavailable,
            PasteRefusalReason::NativeUnavailable,
        ),
        (
            PasteRefusal::NativeRejected,
            PasteRefusalReason::NativeRejected,
        ),
    ] {
        assert_eq!(reason.wire_reason(), wire);
    }
}

#[test]
fn event_and_disable_race_linearizes_to_accept_before_close_or_reject_after_close() {
    for event_first in [true, false] {
        let value = if event_first { 13 } else { 14 };
        let material = material(value, Purpose::Capture);
        let (mut server, handle) = open_server(value);
        let id = connection(u64::from(value));
        let mut client = attach(&mut server, &material, id);
        let mut capture = acquire_capture(&mut server, &mut client, id);
        reconcile_and_enable(&mut server, &mut client, id, &mut capture);

        let barrier = Arc::new(Barrier::new(2));
        let producer_barrier = Arc::clone(&barrier);
        let producer = handle.clone();
        let race = thread::spawn(move || {
            producer_barrier.wait();
            producer.emit(BrokerEvent::AudioInputDevicesChanged)
        });
        let disposition = if event_first {
            barrier.wait();
            let emitted = race.join().unwrap();
            let pumped = server.pump_native_adapter();
            disable(&mut server, &mut client, id, &mut capture);
            assert_eq!(handle.acknowledgements().last().unwrap().0, emitted);
            pumped
        } else {
            disable(&mut server, &mut client, id, &mut capture);
            barrier.wait();
            let emitted = race.join().unwrap();
            let pumped = server.pump_native_adapter();
            assert_eq!(handle.acknowledgements().last().unwrap().0, emitted);
            pumped
        };
        assert_eq!(
            disposition,
            NativeAdapterPump::Processed(if event_first {
                AdapterEventDisposition::Accepted
            } else {
                AdapterEventDisposition::Rejected(AdapterEventRejection::AdmissionClosed)
            })
        );
        assert_eq!(server.state().admission(), AdmissionState::Closed);
    }
}

#[test]
fn close_watermark_drains_and_acknowledges_unpumped_physical_event_before_response() {
    let material = material(22, Purpose::Capture);
    let (mut server, handle) = open_server(22);
    let id = connection(22);
    let mut client = attach(&mut server, &material, id);
    let mut capture = acquire_capture(&mut server, &mut client, id);
    reconcile_and_enable(&mut server, &mut client, id, &mut capture);
    let recording = SessionSetModeParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        mode: SessionMode::Recording,
    };
    capture.next_sequence += 1;
    request_response(
        &mut server,
        &mut client,
        id,
        &Request::SessionSetMode(recording),
    );
    let event = handle.emit(BrokerEvent::Keyboard(KeyboardEvent::SessionKey {
        key: SessionKey::Escape,
        phase: EventPhase::Down,
    }));
    let close = SetEnabledParams {
        capture_lease_id: capture.id,
        capture_lease_epoch: capture.epoch,
        command_sequence: wire_u64(capture.next_sequence),
        enabled: false,
    };
    client
        .send_request(&Request::CaptureSetEnabled(close))
        .unwrap();
    assert_eq!(server.pump(id).unwrap(), ServerPump::Processed);
    assert_eq!(server.state().admission(), AdmissionState::Closed);
    assert!(!server.state().status().native_state_unknown);
    assert_eq!(server.pump_native_adapter(), NativeAdapterPump::Empty);
    assert_eq!(
        handle.acknowledgements().last().copied(),
        Some((event, AdapterEventDisposition::Accepted))
    );
    assert!(matches!(
        client.poll().unwrap(),
        ClientPoll::Message(GatewayMessage::Event(
            talking_quill_owner_protocol::schema::Event::SessionKey(_)
        ))
    ));
    assert!(matches!(
        client.poll().unwrap(),
        ClientPoll::Message(GatewayMessage::Response { .. })
    ));
}

proptest! {
    #[test]
    fn arbitrary_event_disable_interleavings_never_accept_after_close(
        operations in prop::collection::vec(any::<bool>(), 1..=64),
    ) {
        let material = material(15, Purpose::Capture);
        let (mut server, handle) = open_server(15);
        let id = connection(15);
        let mut client = attach(&mut server, &material, id);
        let mut capture = acquire_capture(&mut server, &mut client, id);
        reconcile_and_enable(&mut server, &mut client, id, &mut capture);
        let mut closed = false;
        let mut expected = Vec::new();
        for close in operations {
            if close && !closed {
                disable(&mut server, &mut client, id, &mut capture);
                closed = true;
            } else {
                let event = handle.emit(BrokerEvent::AudioInputDevicesChanged);
                let disposition = if closed {
                    AdapterEventDisposition::Rejected(AdapterEventRejection::AdmissionClosed)
                } else {
                    AdapterEventDisposition::Accepted
                };
                prop_assert_eq!(
                    server.pump_native_adapter(),
                    NativeAdapterPump::Processed(disposition),
                );
                if !closed {
                    server.flush_admitted_events().unwrap();
                    prop_assert!(matches!(
                        client.poll().unwrap(),
                        ClientPoll::Message(GatewayMessage::Event(_))
                    ));
                }
                expected.push((event, disposition));
            }
        }
        prop_assert_eq!(handle.acknowledgements(), expected);
        prop_assert_eq!(server.state().admission() == AdmissionState::Closed, closed);
    }

    #[test]
    fn arbitrary_refusal_selection_preserves_its_exact_wire_category(index in 0_usize..7) {
        let reasons = [
            (PasteRefusal::PermissionDenied, PasteRefusalReason::PermissionDenied),
            (PasteRefusal::ConflictingModifiers, PasteRefusalReason::ConflictingModifiers),
            (PasteRefusal::SecureInput, PasteRefusalReason::SecureInput),
            (PasteRefusal::TargetUnavailable, PasteRefusalReason::TargetUnavailable),
            (PasteRefusal::ClipboardChanged, PasteRefusalReason::ClipboardChanged),
            (PasteRefusal::NativeUnavailable, PasteRefusalReason::NativeUnavailable),
            (PasteRefusal::NativeRejected, PasteRefusalReason::NativeRejected),
        ];
        prop_assert_eq!(reasons[index].0.wire_reason(), reasons[index].1);
    }
}
