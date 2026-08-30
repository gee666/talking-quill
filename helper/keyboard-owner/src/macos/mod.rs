//! Local macOS keyboard-owner identity and credential policy.
//!
//! Personal builds may be ad-hoc signed or use a locally generated,
//! user-trusted self-signed certificate. Developer ID and notarization are not
//! authorization inputs. Runtime transport and LoginItem wiring consume these
//! policies in R5-M/R8-M; this module itself never prompts for TCC or Keychain
//! access and never grants privacy permissions.

#[cfg(target_os = "macos")]
mod config;
#[cfg(target_os = "macos")]
mod endpoint;
mod identity;
mod keychain;
#[cfg(target_os = "macos")]
mod native_cms;
#[cfg(target_os = "macos")]
mod native_identity;
#[cfg(target_os = "macos")]
mod native_keychain;
#[cfg(target_os = "macos")]
mod removal;
mod resilience;
#[cfg(target_os = "macos")]
mod runtime_directory;
#[cfg(target_os = "macos")]
mod service_management;
#[cfg(target_os = "macos")]
mod signals;
#[cfg(target_os = "macos")]
mod singleton;

#[cfg(target_os = "macos")]
pub use config::MacosEndpointConfig;
#[cfg(target_os = "macos")]
pub use endpoint::{MacosAuthenticatedConnectionSource, run_native_auth_broker_child};
pub use identity::{
    AuditToken, AuthorizationPurpose, CodeIdentity, GATEWAY_SIGNING_IDENTIFIER, IdentityError,
    LocalSigningIdentity, MacosPeerPolicy, OWNER_SIGNING_IDENTIFIER, PeerCredentials, PeerEvidence,
    PeerEvidenceProvider, PeerRole, RequirementHash, RolePolicy, TccAuthorization,
    TccPersistenceExpectation, VerifiedPeer, ad_hoc_designated_requirement,
    audit_token_connection_binding, peer_policy_digest, requirement_digest,
    self_signed_designated_requirement,
};
pub use keychain::{
    HANDSHAKE_SECRET_ACCOUNT, KEYCHAIN_SECRET_BYTES, KEYCHAIN_SERVICE, KeychainAccess,
    KeychainError, KeychainItem, KeychainPolicy, KeychainSharingMode, KeychainStore,
    MAINTENANCE_LATCH_ACCOUNT, MAX_MAINTENANCE_LATCH_BYTES,
};
#[cfg(target_os = "macos")]
pub use native_identity::{NativePeerEvidence, current_audit_token};
#[cfg(target_os = "macos")]
pub use native_keychain::{
    NativeKeychainStore, clear_maintenance_record_without_ui, delete_after_unregistration,
    fixed_item_query_statuses_without_ui, read_maintenance_record_without_ui,
};
#[cfg(target_os = "macos")]
pub use removal::{
    finish_poisoned_without_bundle, finish_removed_install, prepare_removed_install,
    removal_poisoned,
};
#[cfg(target_os = "macos")]
pub use runtime_directory::MacosRuntimeDirectory;
#[cfg(target_os = "macos")]
pub use service_management::{RemovalServiceBridge, UnregisteredLoginItem};
#[cfg(target_os = "macos")]
pub use signals::{MacosLifecycleHandle, MacosRuntimeSignals};
#[cfg(target_os = "macos")]
pub use singleton::MacosFlockSingleton;

#[cfg(target_os = "macos")]
#[derive(Debug, thiserror::Error)]
pub enum MacosEndpointError {
    #[error("macOS owner runtime directory is unavailable or unsafe")]
    RuntimeDirectory,
    #[error("macOS owner endpoint configuration is unavailable or invalid")]
    Configuration,
    #[error("macOS owner authentication handshake is invalid")]
    Handshake,
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error(transparent)]
    Keychain(#[from] KeychainError),
    #[error(transparent)]
    Authentication(#[from] talking_quill_owner_protocol::AuthenticationError),
    #[error(transparent)]
    Schema(#[from] talking_quill_owner_protocol::schema::SchemaError),
    #[error(transparent)]
    Protocol(#[from] talking_quill_owner_protocol::schema::ProtocolSelectionError),
    #[error(transparent)]
    Framing(#[from] talking_quill_owner_protocol::framing::FramingError),
    #[error(transparent)]
    Envelope(#[from] talking_quill_owner_protocol::envelope::EnvelopeError),
    #[error(transparent)]
    Transport(#[from] talking_quill_owner_protocol::TransportError),
    #[error(transparent)]
    Session(#[from] talking_quill_owner_protocol::SessionCodecError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub const MACOS_LOCAL_IDENTITY_MODE_MARKER: &str =
    "TALKING_QUILL_MACOS_OWNER_IDENTITY=LOCAL_ADHOC_OR_TRUSTED_SELF_SIGNED";
