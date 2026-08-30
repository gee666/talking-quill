//! Out-of-process native keyboard-owner library.
//!
//! The `local-unsigned-owner` feature enables an explicit local personal-use
//! runtime. It is independent of paid signing and native test seams. Windows
//! receives its private connection from the adjacent gateway, while macOS uses
//! the installed local endpoint.

#[cfg(all(
    not(debug_assertions),
    any(
        feature = "transactional-shortcuts-dev",
        feature = "windows-native-test-input"
    )
))]
compile_error!("native keyboard test seams cannot be included in an optimized owner build");

pub mod adapter;
pub mod build_mode;
mod capture_gate;
pub mod executor;
#[cfg(any(target_os = "macos", test))]
pub mod macos;
#[cfg(any(
    talking_quill_unoptimized_test_support,
    feature = "local-unsigned-owner"
))]
#[cfg_attr(not(feature = "local-unsigned-owner"), doc(hidden))]
pub mod platform;
#[cfg(any(
    talking_quill_unoptimized_test_support,
    feature = "local-unsigned-owner"
))]
#[cfg_attr(not(feature = "local-unsigned-owner"), doc(hidden))]
pub mod platform_adapter;
pub mod protocol_server;
#[cfg(any(
    talking_quill_unoptimized_test_support,
    feature = "local-unsigned-owner"
))]
#[cfg_attr(not(feature = "local-unsigned-owner"), doc(hidden))]
pub mod runtime;
pub mod state;
#[cfg(all(windows, feature = "local-unsigned-owner"))]
mod windows_connection;
#[cfg(all(windows, feature = "local-unsigned-owner"))]
mod windows_runtime;

pub use adapter::{
    AdapterEvent, AdapterEventDisposition, AdapterEventId, AdapterEventRejection, BrokerEvent,
    CapabilityIdSource, NativeAdapter, NativeAdapterExecutor, NativeAdapterPump, NativeEffect,
    NativeEffectKind, NativeEffectResult, OrphanRetirementPolicy, PasteCommitOutcome, PasteRefusal,
};
pub use build_mode::{OWNER_BUILD_MODE, OWNER_MODE_MARKER, OwnerBuildMode, local_owner_profile};
pub use capture_gate::ActivationCaptureGate;
pub use executor::{
    ExecutorCommand, ExecutorResult, OwnerExecutor, PasteExecutorRequest, RecordingFakeExecutor,
};
#[cfg(any(
    talking_quill_unoptimized_test_support,
    feature = "local-unsigned-owner"
))]
pub use platform_adapter::PlatformAdapter;
pub use protocol_server::{OwnerProtocolServer, ServerError, ServerPump};
#[cfg(any(
    talking_quill_unoptimized_test_support,
    feature = "local-unsigned-owner"
))]
pub use runtime::{
    AuthenticatedConnection, AuthenticatedConnectionSource, ConnectionSourceError,
    NoAuthenticatedConnections, NoRuntimeSignals, OsCapabilityIds, OsRuntimeSignals, OwnerRuntime,
    ProcessSingleton, ProductionRuntime, RuntimeError, RuntimeSignal, RuntimeSignalSource,
    RuntimeStep, SingletonCoordinator, SingletonError,
};
#[cfg(all(windows, feature = "local-unsigned-owner"))]
pub use windows_connection::WindowsNamedPipeConnectionSource;
#[cfg(all(windows, feature = "local-unsigned-owner"))]
pub use windows_runtime::{WindowsSessionRuntimeSignals, WindowsSessionSingleton};
