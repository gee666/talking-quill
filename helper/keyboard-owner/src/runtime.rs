//! Serialized production owner host.
//!
//! R4 owns process lifetime and pump ordering only. R5-W/R5-M provide the
//! authenticated platform endpoint, session signal, and OS singleton
//! implementations through the traits in this module; no unauthenticated
//! socket/pipe fallback exists here.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{Duration, Instant};

use talking_quill_owner_protocol::{Bytes32, OrderedTransport, OwnerSessionCodec};
use thiserror::Error;

use crate::adapter::{BoundedShutdownOutcome, OrphanRetirementPolicy};
use crate::state::{CapabilityId, ConnectionId, OwnerInstanceId, TransitionErrorKind};
use crate::{
    CapabilityIdSource, NativeAdapter, NativeAdapterExecutor, NativeAdapterPump,
    OwnerProtocolServer, PlatformAdapter, ServerError, ServerPump,
};

const RANDOM_RETRIES: usize = 8;
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(2);

/// Cryptographically random capability IDs. Failure is reported instead of
/// substituting counters, timestamps, process IDs, or weak randomness.
#[derive(Debug, Default)]
pub struct OsCapabilityIds;

impl CapabilityIdSource for OsCapabilityIds {
    fn next_capability_id(&mut self) -> Option<CapabilityId> {
        random_opaque_id(CapabilityId::new)
    }
}

fn random_owner_instance() -> Result<OwnerInstanceId, RuntimeError> {
    random_opaque_id(OwnerInstanceId::new).ok_or(RuntimeError::Randomness)
}

fn random_opaque_id<T>(constructor: impl Fn([u8; 32]) -> Option<T>) -> Option<T> {
    for _ in 0..RANDOM_RETRIES {
        let random = Bytes32::random().ok()?;
        if let Some(value) = constructor(*random.as_bytes()) {
            return Some(value);
        }
    }
    None
}

/// One connection whose OS peer/code/session authentication has already
/// completed. Constructing this value is the exact R5-W/R5-M attach boundary.
pub struct AuthenticatedConnection {
    transport: Box<dyn OrderedTransport>,
    codec: OwnerSessionCodec,
}

impl fmt::Debug for AuthenticatedConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AuthenticatedConnection(<redacted>)")
    }
}

impl AuthenticatedConnection {
    #[must_use]
    pub fn new(transport: Box<dyn OrderedTransport>, codec: OwnerSessionCodec) -> Self {
        Self { transport, codec }
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Fixed32AccumulatorOutcome {
    Pending,
    Match,
    Mismatch,
    Terminal,
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn accumulate_fixed_32(
    buffer: &mut [u8; 32],
    received: &mut usize,
    chunk: &[u8],
    expected: &[u8; 32],
) -> Fixed32AccumulatorOutcome {
    if chunk.is_empty() || chunk.len() > buffer.len().saturating_sub(*received) {
        return Fixed32AccumulatorOutcome::Terminal;
    }
    buffer[*received..*received + chunk.len()].copy_from_slice(chunk);
    *received += chunk.len();
    if *received < buffer.len() {
        Fixed32AccumulatorOutcome::Pending
    } else if buffer == expected {
        Fixed32AccumulatorOutcome::Match
    } else {
        Fixed32AccumulatorOutcome::Mismatch
    }
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn advance_bounded_write(
    offset: &mut usize,
    total: usize,
    written: usize,
) -> Result<(), ConnectionSourceError> {
    if written == 0 || written > total.saturating_sub(*offset) {
        return Err(ConnectionSourceError);
    }
    *offset += written;
    Ok(())
}

/// R5-W/R5-M endpoint providers implement nonblocking accept + authentication
/// and return only authenticated connections. `poll_authenticated` must never
/// perform native input work or call the protocol server reentrantly.
pub trait AuthenticatedConnectionSource: fmt::Debug {
    /// Bind the process-scoped owner instance before the endpoint begins its
    /// handshake. Platform sources must reject a second binding.
    fn bind_owner_instance(
        &mut self,
        _owner: OwnerInstanceId,
    ) -> Result<(), ConnectionSourceError> {
        Ok(())
    }

    fn poll_authenticated(
        &mut self,
    ) -> Result<Option<AuthenticatedConnection>, ConnectionSourceError>;

    /// True when this one-use source can never produce another connection.
    /// A runtime with no attached connection then drains and exits rather than
    /// retaining the singleton as an orphan.
    fn permanently_exhausted(&self) -> bool {
        false
    }

    /// Process-backed owners can retire after their last controller disconnects.
    /// A replacement controller then creates a fresh owner after native drain.
    fn retire_after_disconnect(&self) -> bool {
        false
    }

    /// Bounds only the interval before the first authenticated controller.
    /// Once authentication succeeds, reconnect behavior remains stable for the
    /// rest of the process lifetime.
    fn initial_authentication_grace(&self) -> Option<Duration> {
        None
    }

    /// Close the discoverable endpoint, cancel and join every accepted
    /// authentication worker, and remove only the endpoint created by this
    /// source. Success is the quiescence barrier that permits singleton
    /// release. Implementations must also enforce the barrier from `Drop`.
    fn shutdown_endpoint(&mut self) -> Result<(), ConnectionSourceError>;
}

/// Deliberate R4 stub: no discoverable endpoint and therefore no authority.
#[derive(Debug, Default)]
pub struct NoAuthenticatedConnections;

impl AuthenticatedConnectionSource for NoAuthenticatedConnections {
    fn poll_authenticated(
        &mut self,
    ) -> Result<Option<AuthenticatedConnection>, ConnectionSourceError> {
        Ok(None)
    }

    fn shutdown_endpoint(&mut self) -> Result<(), ConnectionSourceError> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
#[error("authenticated owner endpoint failed")]
pub struct ConnectionSourceError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeSignal {
    Shutdown,
    SessionEnded,
    AuthorityLost,
    RuntimeRollback,
    RemovalPoisonFailed,
}

/// R5-W supplies authority/session/control notifications; R5-M supplies
/// audit-session/login-item/signals. Polling is serialized with both pumps.
pub trait RuntimeSignalSource: fmt::Debug {
    fn poll_signal(&mut self) -> Option<RuntimeSignal>;
}

#[derive(Debug, Default)]
pub struct NoRuntimeSignals;

impl RuntimeSignalSource for NoRuntimeSignals {
    fn poll_signal(&mut self) -> Option<RuntimeSignal> {
        None
    }
}

static OS_SIGNAL: AtomicU8 = AtomicU8::new(0);
static OS_SIGNALS_INSTALLED: AtomicBool = AtomicBool::new(false);
static OS_DRAIN_COMPLETE: AtomicBool = AtomicBool::new(false);

/// Minimal async-signal-safe process/session notification source. R5 endpoint
/// providers may combine this with their stronger authority/audit-session
/// notifications, but normal executable shutdown never depends on stdio.
#[derive(Debug, Default)]
pub struct OsRuntimeSignals;

impl OsRuntimeSignals {
    pub fn install() -> Result<Self, RuntimeError> {
        OS_DRAIN_COMPLETE.store(false, Ordering::Release);
        if !OS_SIGNALS_INSTALLED.swap(true, Ordering::AcqRel) && !install_os_signal_handlers() {
            OS_SIGNALS_INSTALLED.store(false, Ordering::Release);
            return Err(RuntimeError::SignalInstall);
        }
        Ok(Self)
    }
}

impl RuntimeSignalSource for OsRuntimeSignals {
    fn poll_signal(&mut self) -> Option<RuntimeSignal> {
        match OS_SIGNAL.swap(0, Ordering::AcqRel) {
            1 => Some(RuntimeSignal::Shutdown),
            2 => Some(RuntimeSignal::SessionEnded),
            _ => None,
        }
    }
}

#[cfg(windows)]
unsafe extern "system" fn console_control_handler(control: u32) -> i32 {
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
    };
    let signal = match control {
        CTRL_C_EVENT | CTRL_BREAK_EVENT => 1,
        CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => 2,
        _ => return 0,
    };
    OS_SIGNAL.store(signal, Ordering::Release);
    if signal == 2 {
        // Windows invokes this handler on a dedicated thread. Keep it alive
        // briefly so the serialized loop can reach neutral drain before the
        // outer session deadline (session destruction remains a hard limit).
        wait_for_windows_drain(&OS_DRAIN_COMPLETE, Duration::from_secs(4));
    }
    1
}

#[cfg(windows)]
fn wait_for_windows_drain(complete: &AtomicBool, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while !complete.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    complete.load(Ordering::Acquire)
}

#[cfg(windows)]
fn install_os_signal_handlers() -> bool {
    // SAFETY: the registered function has the required system ABI, accesses
    // only a lock-free atomic, and remains at a static address for process life.
    unsafe {
        windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(console_control_handler), 1)
            != 0
    }
}

#[cfg(target_os = "macos")]
extern "C" fn posix_signal_handler(_signal: libc::c_int) {
    // SIGHUP is only a process shutdown hint. macOS session-end authority is
    // derived by MacosRuntimeSignals from audit/console-session polling.
    OS_SIGNAL.store(1, Ordering::Release);
}

#[cfg(target_os = "macos")]
fn install_os_signal_handlers() -> bool {
    // SAFETY: `signal` installs one static C-ABI handler which performs only an
    // atomic store. SIGKILL remains outside the documented guarantee boundary.
    unsafe {
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            if libc::signal(
                signal,
                posix_signal_handler as *const () as libc::sighandler_t,
            ) == libc::SIG_ERR
            {
                return false;
            }
        }
    }
    true
}

#[cfg(not(any(windows, target_os = "macos")))]
fn install_os_signal_handlers() -> bool {
    false
}

/// Stable per-login-session singleton policy. R5 implementations must replace
/// `ProcessSingleton` with their protected named-object/launchd+lock policy.
pub trait SingletonCoordinator: fmt::Debug {
    fn try_acquire(&mut self) -> Result<bool, SingletonError>;
    fn release(&mut self);
    /// Transfers held authority into process-lifetime poison storage. Called
    /// when endpoint cleanup cannot be proven; implementations must not make
    /// the singleton acquirable again in this process lifetime.
    fn preserve_process_lifetime(&mut self) {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
#[error("keyboard-owner singleton coordination failed")]
pub struct SingletonError;

static PROCESS_SINGLETON: AtomicBool = AtomicBool::new(false);

/// Process-local R4 coordination stub. It prevents duplicate hosts in one
/// process but deliberately makes no cross-process security claim.
#[derive(Debug, Default)]
pub struct ProcessSingleton {
    held: bool,
}

impl SingletonCoordinator for ProcessSingleton {
    fn try_acquire(&mut self) -> Result<bool, SingletonError> {
        if self.held {
            return Ok(true);
        }
        self.held = PROCESS_SINGLETON
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        Ok(self.held)
    }

    fn release(&mut self) {
        if self.held {
            self.held = false;
            PROCESS_SINGLETON.store(false, Ordering::Release);
        }
    }

    fn preserve_process_lifetime(&mut self) {
        self.held = false;
        PROCESS_SINGLETON.store(true, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeStep {
    Idle,
    Progress,
    Backpressured,
    Draining,
    FatalRecovery,
    Exit,
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("operating-system cryptographic randomness is unavailable")]
    Randomness,
    #[error("another keyboard owner already holds the singleton")]
    SingletonBusy,
    #[error(transparent)]
    Singleton(#[from] SingletonError),
    #[error("native keyboard owner startup failed")]
    NativeStartup,
    #[error("authenticated owner endpoint startup failed")]
    ConnectionSource,
    #[error("operating-system shutdown signal installation failed")]
    SignalInstall,
    #[error(transparent)]
    Server(#[from] ServerError),
    #[error("owner connection identity space is exhausted")]
    ConnectionIdsExhausted,
}

/// Single-threaded coordinator. Protocol, transport flushes, adapter events,
/// shutdown, and recovery are linearized by `step`; endpoint threads may only
/// enqueue fully authenticated connections into their source.
pub struct OwnerRuntime<A, C, S, G, Q>
where
    A: NativeAdapter,
    C: CapabilityIdSource,
    S: AuthenticatedConnectionSource,
    G: RuntimeSignalSource,
    Q: SingletonCoordinator,
{
    server: OwnerProtocolServer<'static, NativeAdapterExecutor<A, C>>,
    source: S,
    signals: G,
    singleton: Q,
    connections: Vec<ConnectionId>,
    next_connection_id: u64,
    shutdown_requested: bool,
    orphaned_authority: bool,
    terminal_incomplete_shutdown: bool,
    fatal_recovery: bool,
    singleton_held: bool,
    retain_singleton_until_exit: bool,
    initial_authentication_deadline: Option<Instant>,
    poll_interval: Duration,
}

impl<A, C, S, G, Q> fmt::Debug for OwnerRuntime<A, C, S, G, Q>
where
    A: NativeAdapter,
    C: CapabilityIdSource,
    S: AuthenticatedConnectionSource,
    G: RuntimeSignalSource,
    Q: SingletonCoordinator,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OwnerRuntime(<redacted>)")
    }
}

impl<A, C, S, G, Q> OwnerRuntime<A, C, S, G, Q>
where
    A: NativeAdapter,
    C: CapabilityIdSource,
    S: AuthenticatedConnectionSource,
    G: RuntimeSignalSource,
    Q: SingletonCoordinator,
{
    pub fn from_parts(
        server: OwnerProtocolServer<'static, NativeAdapterExecutor<A, C>>,
        source: S,
        signals: G,
        mut singleton: Q,
    ) -> Result<Self, RuntimeError> {
        if !singleton.try_acquire()? {
            return Err(RuntimeError::SingletonBusy);
        }
        let initial_authentication_deadline = source
            .initial_authentication_grace()
            .and_then(|grace| Instant::now().checked_add(grace));
        Ok(Self {
            server,
            source,
            signals,
            singleton,
            connections: Vec::new(),
            next_connection_id: 1,
            shutdown_requested: false,
            orphaned_authority: false,
            terminal_incomplete_shutdown: false,
            fatal_recovery: false,
            singleton_held: true,
            retain_singleton_until_exit: false,
            initial_authentication_deadline,
            poll_interval: DEFAULT_POLL_INTERVAL,
        })
    }

    #[must_use]
    pub const fn server(&self) -> &OwnerProtocolServer<'static, NativeAdapterExecutor<A, C>> {
        &self.server
    }

    #[must_use]
    pub const fn terminal_incomplete_shutdown(&self) -> bool {
        self.terminal_incomplete_shutdown
    }

    pub fn request_shutdown(&mut self) {
        self.shutdown_requested = true;
        // Teardown drives any returned fail-closed transition before reporting
        // an error. Authority loss while rollback is already stopping native
        // capture can therefore report Stopping even though no route remains.
        // Keep pumping the existing drain instead of converting that expected
        // overlap into a permanent fatal-recovery process.
        match self.server.detach_all_for_shutdown() {
            Ok(()) => self.connections.clear(),
            // State transitions are fail-closed and may report the already
            // latched rollback, degraded drain, or stopping phase. None means
            // that an authority route remains usable. Executor and transport
            // errors below still enter fatal containment.
            Err(ServerError::State(_)) => self.connections.clear(),
            Err(_) => self.enter_fatal_recovery(),
        }
    }

    pub fn step(&mut self) -> Result<RuntimeStep, RuntimeError> {
        if self.server.exit_requested() {
            self.release_singleton()?;
            return Ok(RuntimeStep::Exit);
        }

        if let Some(signal) = self.signals.poll_signal() {
            match signal {
                RuntimeSignal::RuntimeRollback => {
                    if self.server.latch_runtime_rollback().is_err() {
                        self.enter_fatal_recovery();
                    }
                }
                RuntimeSignal::Shutdown | RuntimeSignal::SessionEnded => self.request_shutdown(),
                RuntimeSignal::RemovalPoisonFailed => {
                    self.retain_singleton_until_exit = true;
                    self.request_shutdown();
                }
                RuntimeSignal::AuthorityLost => {
                    self.orphaned_authority = true;
                    self.request_shutdown();
                }
            }
        }

        let mut progress = false;
        let mut backpressured = false;
        if !self.shutdown_requested
            && !self.fatal_recovery
            && !self.server.planned_exit_when_neutral()
            && self.server.has_connection_capacity()
        {
            match self.source.poll_authenticated() {
                Ok(Some(connection)) => {
                    let id = self.allocate_connection_id()?;
                    match self.server.attach_boxed_connection(
                        id,
                        connection.transport,
                        connection.codec,
                    ) {
                        Ok(()) => {
                            self.connections.push(id);
                            self.initial_authentication_deadline = None;
                        }
                        Err(ServerError::ConnectionCapacity) => {
                            // Reject only this fully authenticated peer. Existing
                            // authorities and their pending flushes remain live.
                        }
                        Err(_) => self.enter_fatal_recovery(),
                    }
                    progress = true;
                }
                Ok(None) => {}
                Err(_) => self.enter_fatal_recovery(),
            }
        } else if !self.shutdown_requested
            && !self.fatal_recovery
            && !self.server.has_connection_capacity()
        {
            backpressured = true;
        }

        if !self.shutdown_requested
            && self.connections.is_empty()
            && self
                .initial_authentication_deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.initial_authentication_deadline = None;
            self.request_shutdown();
            progress = true;
        }

        let had_connections = !self.connections.is_empty();
        let mut retained = Vec::with_capacity(self.connections.len());
        for connection in self.connections.iter().copied() {
            match self.server.pump(connection) {
                Ok(ServerPump::Empty) => retained.push(connection),
                Ok(ServerPump::Backpressured) => {
                    retained.push(connection);
                    backpressured = true;
                }
                Ok(ServerPump::Processed) => {
                    retained.push(connection);
                    progress = true;
                }
                Ok(ServerPump::PeerClosed | ServerPump::FatalClosed) => progress = true,
                Err(ServerError::UnknownConnection) => progress = true,
                Err(error) => {
                    eprintln!("keyboard-owner connection pump failed: {error:?}");
                    retained.push(connection);
                    self.enter_fatal_recovery();
                    break;
                }
            }
        }
        self.connections = retained;
        if self.server.planned_exit_route_complete() {
            self.server.detach_all_for_shutdown()?;
            self.connections.clear();
            self.request_shutdown();
        }
        if self.connections.is_empty()
            && (self.source.permanently_exhausted()
                || (had_connections && self.source.retire_after_disconnect()))
        {
            // A one-use source cannot authenticate another controller. Its loss
            // enters the normal cancellation and native-drain path.
            self.orphaned_authority = true;
            self.request_shutdown();
        }

        match self.server.pump_native_adapter() {
            NativeAdapterPump::Empty => {}
            NativeAdapterPump::ClosePending => backpressured = true,
            NativeAdapterPump::Processed(_) => progress = true,
        }

        if self.fatal_recovery && !self.server.exit_requested() {
            // Authority loss can race a rollback/native-close transition. The
            // first containment request may correctly report Stopping. Retry
            // after each adapter pump until degraded neutral exit is admitted.
            let _ = self.server.recover_fatal_runtime_fault();
        }

        if self.shutdown_requested && !self.server.exit_requested() {
            match self.server.request_idle_exit() {
                Ok(()) => progress = true,
                Err(ServerError::State(error))
                    if matches!(
                        error.kind(),
                        TransitionErrorKind::Busy
                            | TransitionErrorKind::Draining
                            | TransitionErrorKind::Degraded
                            | TransitionErrorKind::Stopping
                    ) => {}
                Err(_) => self.enter_fatal_recovery(),
            }
        }

        if self.orphaned_authority
            && !self.server.exit_requested()
            && self.server.orphan_retirement_policy()
                == OrphanRetirementPolicy::BoundedNativeShutdown
        {
            match self.server.bounded_orphan_shutdown() {
                BoundedShutdownOutcome::Quiescent => {
                    self.release_singleton()?;
                    return Ok(RuntimeStep::Exit);
                }
                BoundedShutdownOutcome::TerminalIncomplete => {
                    self.terminal_incomplete_shutdown = true;
                    self.release_singleton()?;
                    return Ok(RuntimeStep::Exit);
                }
                BoundedShutdownOutcome::Failed => self.enter_fatal_recovery(),
            }
        }

        if self.server.exit_requested() {
            self.release_singleton()?;
            Ok(RuntimeStep::Exit)
        } else if self.fatal_recovery {
            Ok(RuntimeStep::FatalRecovery)
        } else if self.shutdown_requested {
            Ok(RuntimeStep::Draining)
        } else if backpressured {
            Ok(RuntimeStep::Backpressured)
        } else if progress {
            Ok(RuntimeStep::Progress)
        } else {
            Ok(RuntimeStep::Idle)
        }
    }

    pub fn run(&mut self) -> Result<(), RuntimeError> {
        loop {
            match self.step()? {
                RuntimeStep::Exit => return Ok(()),
                RuntimeStep::Progress => {}
                RuntimeStep::Idle
                | RuntimeStep::Backpressured
                | RuntimeStep::Draining
                | RuntimeStep::FatalRecovery => std::thread::sleep(self.poll_interval),
            }
        }
    }

    fn allocate_connection_id(&mut self) -> Result<ConnectionId, RuntimeError> {
        let id = ConnectionId::new(self.next_connection_id)
            .ok_or(RuntimeError::ConnectionIdsExhausted)?;
        self.next_connection_id = self
            .next_connection_id
            .checked_add(1)
            .ok_or(RuntimeError::ConnectionIdsExhausted)?;
        Ok(id)
    }

    fn enter_fatal_recovery(&mut self) {
        if !self.fatal_recovery {
            eprintln!("keyboard-owner runtime entered recovery");
            self.fatal_recovery = true;
            self.shutdown_requested = true;
            if self.server.detach_all_for_shutdown().is_ok() {
                self.connections.clear();
            }
            // If containment itself cannot be confirmed, remain alive in this
            // state and keep pumping native drain; never claim a clean exit.
            let _ = self.server.recover_fatal_runtime_fault();
        }
    }

    fn release_singleton(&mut self) -> Result<(), RuntimeError> {
        if self.singleton_held {
            if self.retain_singleton_until_exit {
                self.singleton.preserve_process_lifetime();
                self.singleton_held = false;
                return Ok(());
            }
            if self.orphaned_authority && !self.server.exit_requested() {
                self.singleton.preserve_process_lifetime();
                self.singleton_held = false;
                return Ok(());
            }
            if self.source.shutdown_endpoint().is_err() {
                self.singleton.preserve_process_lifetime();
                self.singleton_held = false;
                return Err(RuntimeError::ConnectionSource);
            }
            self.singleton.release();
            self.singleton_held = false;
            OS_DRAIN_COMPLETE.store(true, Ordering::Release);
        }
        Ok(())
    }
}

impl<A, C, S, G, Q> Drop for OwnerRuntime<A, C, S, G, Q>
where
    A: NativeAdapter,
    C: CapabilityIdSource,
    S: AuthenticatedConnectionSource,
    G: RuntimeSignalSource,
    Q: SingletonCoordinator,
{
    fn drop(&mut self) {
        // On failure, declaration-order field destruction drops/quiesces the
        // source before the singleton coordinator can release its OS locks.
        let _ = self.release_singleton();
    }
}

pub type ProductionRuntime<
    S = NoAuthenticatedConnections,
    G = NoRuntimeSignals,
    Q = ProcessSingleton,
> = OwnerRuntime<PlatformAdapter<crate::platform::NativePlatform>, OsCapabilityIds, S, G, Q>;

impl<S, G, Q> ProductionRuntime<S, G, Q>
where
    S: AuthenticatedConnectionSource,
    G: RuntimeSignalSource,
    Q: SingletonCoordinator,
{
    pub fn production(mut source: S, signals: G, mut singleton: Q) -> Result<Self, RuntimeError> {
        if !singleton.try_acquire()? {
            return Err(RuntimeError::SingletonBusy);
        }
        let setup = (|| {
            let owner_instance = random_owner_instance()?;
            // Bind the authenticated endpoint before starting native input, as
            // required by the lifecycle contract. The server is not attached
            // yet, so peers cannot acquire capture while native startup runs.
            source
                .bind_owner_instance(owner_instance)
                .map_err(|_| RuntimeError::ConnectionSource)?;
            let adapter = PlatformAdapter::start().map_err(|_| RuntimeError::NativeStartup)?;
            let executor = NativeAdapterExecutor::new(adapter, OsCapabilityIds);
            let mut server = OwnerProtocolServer::start(owner_instance, executor)?;
            if ActivationCaptureGate::for_process().runtime_rollback_active() {
                server.latch_runtime_rollback()?;
            }
            Ok(server)
        })();
        match setup {
            Ok(server) => {
                let initial_authentication_deadline = source
                    .initial_authentication_grace()
                    .and_then(|grace| Instant::now().checked_add(grace));
                Ok(Self {
                    server,
                    source,
                    signals,
                    singleton,
                    connections: Vec::new(),
                    next_connection_id: 1,
                    shutdown_requested: false,
                    orphaned_authority: false,
                    terminal_incomplete_shutdown: false,
                    fatal_recovery: false,
                    singleton_held: true,
                    retain_singleton_until_exit: false,
                    initial_authentication_deadline,
                    poll_interval: DEFAULT_POLL_INTERVAL,
                })
            }
            Err(error) => {
                let native_startup_failed = matches!(error, RuntimeError::NativeStartup);
                let shutdown = source.shutdown_endpoint();
                // Function arguments otherwise drop in reverse order. Force
                // source Drop/quiescence before releasing singleton locks.
                drop(source);
                if native_startup_failed || shutdown.is_err() {
                    // A timed-out platform startup can still own a native
                    // thread. Keep the singleton until process exit rather than
                    // permitting a replacement owner beside that thread.
                    singleton.preserve_process_lifetime();
                    if shutdown.is_err() {
                        Err(RuntimeError::ConnectionSource)
                    } else {
                        Err(error)
                    }
                } else {
                    singleton.release();
                    Err(error)
                }
            }
        }
    }
}

use crate::ActivationCaptureGate;

#[cfg(test)]
mod tests;
