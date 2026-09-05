use std::collections::VecDeque;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

// These two tests cross the real process-global Windows hook startup
// boundary; keep them from racing each other under the parallel test runner.
static PRODUCTION_SETUP_TEST_LOCK: Mutex<()> = Mutex::new(());

use talking_quill_owner_protocol::FakeAuthenticatedMaterial;
use talking_quill_owner_protocol::fake_transport::{
    FakeOrderedEndpoint, fake_ordered_transport_pair,
};
use talking_quill_owner_protocol::schema::{
    FrontAppResult, ObservabilityResult, PermissionState, PermissionsResult, Purpose,
};

use super::*;
use crate::state::{NativeReadiness, OwnerInstanceId};
use crate::{
    AdapterEvent, AdapterEventDisposition, NativeEffect, NativeEffectKind, NativeEffectResult,
};

#[derive(Debug, Default)]
struct FakeAdapter {
    events: VecDeque<AdapterEvent>,
    stopped: bool,
}

impl NativeAdapter for FakeAdapter {
    fn seed_startup_physical_snapshot(&mut self) -> bool {
        true
    }

    fn readiness(&self) -> NativeReadiness {
        NativeReadiness {
            keyboard_build_eligible: true,
            paste_ready: true,
            permissions_eligible: true,
            hook_healthy: !self.stopped,
        }
    }

    fn execute(&mut self, effect: NativeEffect) -> NativeEffectResult {
        match effect.kind() {
            NativeEffectKind::CloseFreshAdmission
            | NativeEffectKind::EmergencyCloseFreshAdmission => {
                NativeEffectResult::AdmissionClosed {
                    through_event: None,
                }
            }
            NativeEffectKind::StopNativeAdapter => {
                self.stopped = true;
                NativeEffectResult::Applied
            }
            _ => NativeEffectResult::Applied,
        }
    }

    fn try_next_event(&mut self) -> Option<AdapterEvent> {
        self.events.pop_front()
    }

    fn acknowledge_event(
        &mut self,
        _id: crate::AdapterEventId,
        _disposition: AdapterEventDisposition,
    ) {
    }

    fn permissions(&self) -> PermissionsResult {
        PermissionsResult {
            accessibility: PermissionState::Granted,
            input_monitoring: PermissionState::Granted,
            event_post: PermissionState::Granted,
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

#[derive(Debug)]
struct FakeCapabilities(u8);

impl CapabilityIdSource for FakeCapabilities {
    fn next_capability_id(&mut self) -> Option<CapabilityId> {
        self.0 = self.0.checked_add(1)?;
        let mut bytes = [0_u8; 32];
        bytes[0] = self.0;
        CapabilityId::new(bytes)
    }
}

#[derive(Debug)]
struct Signals(VecDeque<RuntimeSignal>);

impl RuntimeSignalSource for Signals {
    fn poll_signal(&mut self) -> Option<RuntimeSignal> {
        self.0.pop_front()
    }
}

#[derive(Debug)]
struct OrderedSource(Arc<AtomicU8>);

impl AuthenticatedConnectionSource for OrderedSource {
    fn poll_authenticated(
        &mut self,
    ) -> Result<Option<AuthenticatedConnection>, ConnectionSourceError> {
        Ok(None)
    }

    fn shutdown_endpoint(&mut self) -> Result<(), ConnectionSourceError> {
        assert_eq!(self.0.swap(1, Ordering::AcqRel), 0);
        Ok(())
    }
}

#[derive(Debug)]
struct SetupFailureSource(Arc<AtomicU8>);

impl AuthenticatedConnectionSource for SetupFailureSource {
    fn bind_owner_instance(
        &mut self,
        _owner: OwnerInstanceId,
    ) -> Result<(), ConnectionSourceError> {
        Err(ConnectionSourceError)
    }

    fn poll_authenticated(
        &mut self,
    ) -> Result<Option<AuthenticatedConnection>, ConnectionSourceError> {
        Ok(None)
    }

    fn shutdown_endpoint(&mut self) -> Result<(), ConnectionSourceError> {
        assert_eq!(self.0.swap(1, Ordering::AcqRel), 0);
        Ok(())
    }
}

#[derive(Debug)]
struct OrderedSingleton(Arc<AtomicU8>);

impl SingletonCoordinator for OrderedSingleton {
    fn try_acquire(&mut self) -> Result<bool, SingletonError> {
        Ok(true)
    }

    fn release(&mut self) {
        assert_eq!(self.0.swap(2, Ordering::AcqRel), 1);
    }
}

#[derive(Debug)]
struct CleanupFailureSource;

impl AuthenticatedConnectionSource for CleanupFailureSource {
    fn bind_owner_instance(
        &mut self,
        _owner: OwnerInstanceId,
    ) -> Result<(), ConnectionSourceError> {
        Err(ConnectionSourceError)
    }

    fn poll_authenticated(
        &mut self,
    ) -> Result<Option<AuthenticatedConnection>, ConnectionSourceError> {
        Ok(None)
    }

    fn shutdown_endpoint(&mut self) -> Result<(), ConnectionSourceError> {
        Err(ConnectionSourceError)
    }
}

#[derive(Debug)]
struct PoisonTrackingSingleton(Arc<AtomicU8>);

impl SingletonCoordinator for PoisonTrackingSingleton {
    fn try_acquire(&mut self) -> Result<bool, SingletonError> {
        Ok(true)
    }

    fn release(&mut self) {
        self.0.store(2, Ordering::Release);
    }

    fn preserve_process_lifetime(&mut self) {
        self.0.store(3, Ordering::Release);
    }
}

#[derive(Debug, Clone)]
struct SharedSingleton(Arc<AtomicBool>, bool);

impl SingletonCoordinator for SharedSingleton {
    fn try_acquire(&mut self) -> Result<bool, SingletonError> {
        if self.1 {
            return Ok(true);
        }
        self.1 = self
            .0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        Ok(self.1)
    }

    fn release(&mut self) {
        if self.1 {
            self.1 = false;
            self.0.store(false, Ordering::Release);
        }
    }
}

fn server(
    value: u8,
) -> OwnerProtocolServer<'static, NativeAdapterExecutor<FakeAdapter, FakeCapabilities>> {
    let mut bytes = [0_u8; 32];
    bytes[0] = value;
    OwnerProtocolServer::start(
        OwnerInstanceId::new(bytes).unwrap(),
        NativeAdapterExecutor::new(FakeAdapter::default(), FakeCapabilities(0)),
    )
    .unwrap()
}

#[cfg(windows)]
#[test]
fn windows_session_event_wait_unblocks_on_drain_and_obeys_deadline() {
    let complete = Arc::new(AtomicBool::new(false));
    let writer = Arc::clone(&complete);
    let join = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(10));
        writer.store(true, Ordering::Release);
    });
    assert!(wait_for_windows_drain(&complete, Duration::from_secs(1)));
    join.join().unwrap();

    complete.store(false, Ordering::Release);
    let started = std::time::Instant::now();
    assert!(!wait_for_windows_drain(
        &complete,
        Duration::from_millis(10)
    ));
    assert!(started.elapsed() < Duration::from_millis(250));
}

#[test]
fn fragmented_fixed_nonce_and_partial_write_injection_are_exact() {
    let expected = [7_u8; 32];
    let mut buffer = [0_u8; 32];
    let mut received = 0;
    assert_eq!(
        accumulate_fixed_32(&mut buffer, &mut received, &[7; 3], &expected),
        Fixed32AccumulatorOutcome::Pending
    );
    assert_eq!(
        accumulate_fixed_32(&mut buffer, &mut received, &[7; 5], &expected),
        Fixed32AccumulatorOutcome::Pending
    );
    assert_eq!(
        accumulate_fixed_32(&mut buffer, &mut received, &[7; 24], &expected),
        Fixed32AccumulatorOutcome::Match
    );
    assert_eq!(received, 32);
    let mut wrong = [0_u8; 32];
    let mut wrong_received = 0;
    assert_eq!(
        accumulate_fixed_32(&mut wrong, &mut wrong_received, &[8; 32], &expected),
        Fixed32AccumulatorOutcome::Mismatch
    );
    assert_eq!(
        accumulate_fixed_32(&mut wrong, &mut wrong_received, &[], &expected),
        Fixed32AccumulatorOutcome::Terminal
    );

    let mut offset = 0;
    advance_bounded_write(&mut offset, 32, 1).unwrap();
    advance_bounded_write(&mut offset, 32, 7).unwrap();
    advance_bounded_write(&mut offset, 32, 24).unwrap();
    assert_eq!(offset, 32);
    assert!(advance_bounded_write(&mut offset, 32, 1).is_err());
}

#[test]
fn os_capability_ids_are_nonzero_and_not_reused() {
    let mut source = OsCapabilityIds;
    let first = source.next_capability_id().expect("OS random capability");
    let second = source.next_capability_id().expect("OS random capability");
    assert_ne!(first.as_bytes(), second.as_bytes());
}

#[test]
fn shutdown_signal_detaches_authority_drains_and_exits() {
    let singleton = Arc::new(AtomicBool::new(false));
    let mut runtime = OwnerRuntime::from_parts(
        server(1),
        NoAuthenticatedConnections,
        Signals(VecDeque::from([RuntimeSignal::Shutdown])),
        SharedSingleton(Arc::clone(&singleton), false),
    )
    .unwrap();
    assert_eq!(runtime.step().unwrap(), RuntimeStep::Exit);
    assert!(!singleton.load(Ordering::Acquire));
}

#[test]
fn gateway_loss_detaches_authority_and_releases_the_singleton() {
    let singleton = Arc::new(AtomicBool::new(false));
    let mut runtime = OwnerRuntime::from_parts(
        server(2),
        NoAuthenticatedConnections,
        Signals(VecDeque::from([RuntimeSignal::AuthorityLost])),
        SharedSingleton(Arc::clone(&singleton), false),
    )
    .unwrap();
    assert_eq!(runtime.step().unwrap(), RuntimeStep::Exit);
    assert!(!singleton.load(Ordering::Acquire));
}

#[test]
fn authority_loss_during_runtime_rollback_finishes_the_existing_stop() {
    let singleton = Arc::new(AtomicBool::new(false));
    let mut runtime = OwnerRuntime::from_parts(
        server(3),
        NoAuthenticatedConnections,
        Signals(VecDeque::from([
            RuntimeSignal::RuntimeRollback,
            RuntimeSignal::AuthorityLost,
        ])),
        SharedSingleton(Arc::clone(&singleton), false),
    )
    .unwrap();
    for _ in 0..16 {
        if runtime.step().unwrap() == RuntimeStep::Exit {
            assert!(!singleton.load(Ordering::Acquire));
            return;
        }
    }
    panic!("owner did not exit after rollback authority loss");
}

#[test]
fn endpoint_is_destroyed_before_singleton_release() {
    let order = Arc::new(AtomicU8::new(0));
    let mut runtime = OwnerRuntime::from_parts(
        server(9),
        OrderedSource(Arc::clone(&order)),
        Signals(VecDeque::from([RuntimeSignal::Shutdown])),
        OrderedSingleton(Arc::clone(&order)),
    )
    .unwrap();
    assert_eq!(runtime.step().unwrap(), RuntimeStep::Exit);
    assert_eq!(order.load(Ordering::Acquire), 2);
}

#[test]
fn production_setup_failure_quiesces_endpoint_before_singleton_release() {
    let _serial = PRODUCTION_SETUP_TEST_LOCK.lock().unwrap();
    let order = Arc::new(AtomicU8::new(0));
    let result = ProductionRuntime::production(
        SetupFailureSource(Arc::clone(&order)),
        Signals(VecDeque::new()),
        OrderedSingleton(Arc::clone(&order)),
    );
    assert!(matches!(result, Err(RuntimeError::ConnectionSource)));
    assert_eq!(order.load(Ordering::Acquire), 2);
}

#[test]
fn drop_quiesces_endpoint_before_singleton_release() {
    let order = Arc::new(AtomicU8::new(0));
    let runtime = OwnerRuntime::from_parts(
        server(10),
        OrderedSource(Arc::clone(&order)),
        Signals(VecDeque::new()),
        OrderedSingleton(Arc::clone(&order)),
    )
    .unwrap();
    drop(runtime);
    assert_eq!(order.load(Ordering::Acquire), 2);
}

#[test]
fn cleanup_failure_poison_prevents_release_on_exit_and_drop() {
    let state = Arc::new(AtomicU8::new(0));
    let mut runtime = OwnerRuntime::from_parts(
        server(11),
        CleanupFailureSource,
        Signals(VecDeque::from([RuntimeSignal::Shutdown])),
        PoisonTrackingSingleton(Arc::clone(&state)),
    )
    .unwrap();
    assert!(matches!(
        runtime.step(),
        Err(RuntimeError::ConnectionSource)
    ));
    assert_eq!(state.load(Ordering::Acquire), 3);
    drop(runtime);
    assert_eq!(state.load(Ordering::Acquire), 3);

    let drop_state = Arc::new(AtomicU8::new(0));
    let runtime = OwnerRuntime::from_parts(
        server(12),
        CleanupFailureSource,
        Signals(VecDeque::new()),
        PoisonTrackingSingleton(Arc::clone(&drop_state)),
    )
    .unwrap();
    drop(runtime);
    assert_eq!(drop_state.load(Ordering::Acquire), 3);
}

#[test]
fn setup_cleanup_failure_poison_prevents_singleton_release() {
    let _serial = PRODUCTION_SETUP_TEST_LOCK.lock().unwrap();
    let state = Arc::new(AtomicU8::new(0));
    let result = ProductionRuntime::production(
        CleanupFailureSource,
        Signals(VecDeque::new()),
        PoisonTrackingSingleton(Arc::clone(&state)),
    );
    assert!(matches!(result, Err(RuntimeError::ConnectionSource)));
    assert_eq!(state.load(Ordering::Acquire), 3);
}

#[test]
fn removal_poison_failure_drains_but_retains_singleton_until_process_exit() {
    let state = Arc::new(AtomicU8::new(0));
    let mut runtime = OwnerRuntime::from_parts(
        server(13),
        NoAuthenticatedConnections,
        Signals(VecDeque::from([RuntimeSignal::RemovalPoisonFailed])),
        PoisonTrackingSingleton(Arc::clone(&state)),
    )
    .unwrap();
    for _ in 0..16 {
        if runtime.step().unwrap() == RuntimeStep::Exit {
            assert_eq!(state.load(Ordering::Acquire), 3);
            return;
        }
    }
    panic!("removal poison failure did not reach retained exit");
}

#[test]
fn runtime_rollback_is_latched_without_stopping_the_detached_owner() {
    let singleton = Arc::new(AtomicBool::new(false));
    let mut runtime = OwnerRuntime::from_parts(
        server(2),
        NoAuthenticatedConnections,
        Signals(VecDeque::from([RuntimeSignal::RuntimeRollback])),
        SharedSingleton(singleton, false),
    )
    .unwrap();
    assert_ne!(runtime.step().unwrap(), RuntimeStep::Exit);
    assert!(runtime.server().state().status().rollback_latched);
}

#[test]
fn singleton_collision_is_atomic_and_release_allows_reacquire() {
    let shared = Arc::new(AtomicBool::new(false));
    let first = OwnerRuntime::from_parts(
        server(3),
        NoAuthenticatedConnections,
        Signals(VecDeque::new()),
        SharedSingleton(Arc::clone(&shared), false),
    )
    .unwrap();
    let collision = OwnerRuntime::from_parts(
        server(4),
        NoAuthenticatedConnections,
        Signals(VecDeque::new()),
        SharedSingleton(Arc::clone(&shared), false),
    );
    assert!(matches!(collision, Err(RuntimeError::SingletonBusy)));
    drop(first);
    assert!(
        OwnerRuntime::from_parts(
            server(5),
            NoAuthenticatedConnections,
            Signals(VecDeque::new()),
            SharedSingleton(shared, false),
        )
        .is_ok()
    );
}

#[derive(Debug)]
struct CapacitySource {
    pending: VecDeque<AuthenticatedConnection>,
    peers: Vec<FakeOrderedEndpoint>,
    polls: Arc<std::sync::atomic::AtomicUsize>,
}

impl AuthenticatedConnectionSource for CapacitySource {
    fn poll_authenticated(
        &mut self,
    ) -> Result<Option<AuthenticatedConnection>, ConnectionSourceError> {
        self.polls.fetch_add(1, Ordering::AcqRel);
        Ok(self.pending.pop_front())
    }

    fn shutdown_endpoint(&mut self) -> Result<(), ConnectionSourceError> {
        Ok(())
    }
}

fn capacity_source(owner: u8, count: u8) -> CapacitySource {
    let mut pending = VecDeque::new();
    let mut peers = Vec::new();
    for value in 1..=count {
        let mut session = [0_u8; 32];
        session[0] = value;
        let mut owner_instance = [0_u8; 32];
        owner_instance[0] = owner;
        let material = FakeAuthenticatedMaterial::new_with_owner_instance(
            Bytes32::new(session),
            Bytes32::new(owner_instance),
            Purpose::Observe,
            [value; 32],
            [value.saturating_add(1); 32],
        );
        let (_, codec) = material.codecs().unwrap();
        let (runtime_endpoint, peer) = fake_ordered_transport_pair();
        pending.push_back(AuthenticatedConnection::new(
            Box::new(runtime_endpoint),
            codec,
        ));
        peers.push(peer);
    }
    CapacitySource {
        pending,
        peers,
        polls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    }
}

#[test]
fn retiring_source_releases_singleton_after_last_authenticated_peer_disconnects() {
    #[derive(Debug)]
    struct RetiringSource(CapacitySource);
    impl AuthenticatedConnectionSource for RetiringSource {
        fn poll_authenticated(
            &mut self,
        ) -> Result<Option<AuthenticatedConnection>, ConnectionSourceError> {
            self.0.poll_authenticated()
        }
        fn retire_after_disconnect(&self) -> bool {
            true
        }
        fn shutdown_endpoint(&mut self) -> Result<(), ConnectionSourceError> {
            Ok(())
        }
    }
    let mut source = capacity_source(16, 1);
    let mut peer = source.peers.pop().unwrap();
    let singleton = Arc::new(AtomicBool::new(false));
    let mut runtime = OwnerRuntime::from_parts(
        server(16),
        RetiringSource(source),
        Signals(VecDeque::new()),
        SharedSingleton(Arc::clone(&singleton), false),
    )
    .unwrap();
    assert_eq!(runtime.step().unwrap(), RuntimeStep::Progress);
    assert!(singleton.load(Ordering::Acquire));
    peer.abort();
    assert_eq!(runtime.step().unwrap(), RuntimeStep::Exit);
    assert!(!singleton.load(Ordering::Acquire));
}

#[test]
fn full_connection_set_backpressures_source_without_poll_or_id_spin() {
    let source = capacity_source(7, 9);
    let polls = Arc::clone(&source.polls);
    let singleton = Arc::new(AtomicBool::new(false));
    let mut runtime = OwnerRuntime::from_parts(
        server(7),
        source,
        Signals(VecDeque::new()),
        SharedSingleton(singleton, false),
    )
    .unwrap();
    for _ in 0..8 {
        assert!(matches!(
            runtime.step().unwrap(),
            RuntimeStep::Progress | RuntimeStep::Idle
        ));
    }
    let before = polls.load(Ordering::Acquire);
    assert_eq!(runtime.step().unwrap(), RuntimeStep::Backpressured);
    assert_eq!(polls.load(Ordering::Acquire), before);
    assert_eq!(runtime.source.pending.len(), 1);
    assert_eq!(runtime.source.peers.len(), 9);
}

#[derive(Debug)]
struct GraceSource {
    pending: Option<AuthenticatedConnection>,
    grace: Duration,
}

impl AuthenticatedConnectionSource for GraceSource {
    fn poll_authenticated(
        &mut self,
    ) -> Result<Option<AuthenticatedConnection>, ConnectionSourceError> {
        Ok(self.pending.take())
    }

    fn initial_authentication_grace(&self) -> Option<Duration> {
        Some(self.grace)
    }

    fn shutdown_endpoint(&mut self) -> Result<(), ConnectionSourceError> {
        Ok(())
    }
}

#[test]
fn initial_authentication_grace_retires_an_unclaimed_owner() {
    let singleton = Arc::new(AtomicBool::new(false));
    let mut runtime = OwnerRuntime::from_parts(
        server(14),
        GraceSource {
            pending: None,
            grace: Duration::ZERO,
        },
        Signals(VecDeque::new()),
        SharedSingleton(Arc::clone(&singleton), false),
    )
    .unwrap();
    assert_eq!(runtime.step().unwrap(), RuntimeStep::Exit);
    assert!(!singleton.load(Ordering::Acquire));
}

#[test]
fn first_authentication_permanently_cancels_initial_grace() {
    let mut source = capacity_source(15, 1);
    let connection = source.pending.pop_front().unwrap();
    let _peer = source.peers.pop().unwrap();
    let singleton = Arc::new(AtomicBool::new(false));
    let mut runtime = OwnerRuntime::from_parts(
        server(15),
        GraceSource {
            pending: Some(connection),
            grace: Duration::ZERO,
        },
        Signals(VecDeque::new()),
        SharedSingleton(singleton, false),
    )
    .unwrap();
    assert_eq!(runtime.step().unwrap(), RuntimeStep::Progress);
    assert!(runtime.initial_authentication_deadline.is_none());
    assert_ne!(runtime.step().unwrap(), RuntimeStep::Exit);
}

#[derive(Debug)]
struct FailedSource;

impl AuthenticatedConnectionSource for FailedSource {
    fn poll_authenticated(
        &mut self,
    ) -> Result<Option<AuthenticatedConnection>, ConnectionSourceError> {
        Err(ConnectionSourceError)
    }

    fn shutdown_endpoint(&mut self) -> Result<(), ConnectionSourceError> {
        Ok(())
    }
}

#[test]
fn endpoint_failure_enters_explicit_fatal_recovery_and_never_reenables() {
    let shared = Arc::new(AtomicBool::new(false));
    let mut runtime = OwnerRuntime::from_parts(
        server(6),
        FailedSource,
        Signals(VecDeque::new()),
        SharedSingleton(shared, false),
    )
    .unwrap();
    assert!(matches!(
        runtime.step().unwrap(),
        RuntimeStep::Exit | RuntimeStep::FatalRecovery
    ));
    assert!(!runtime.server().state().can_open_keyboard());
}
