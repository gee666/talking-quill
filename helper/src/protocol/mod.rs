//! Talking Quill owner-gateway protocol (strict JSON-RPC 2.0, version 10).
//!
//! The Electron boundary retains four-byte big-endian, nonempty 16-KiB frames,
//! strict request IDs/objects, no batches, and deny-unknown-fields schemas.
//! Version 10 adds the authenticated out-of-process keyboard-owner snapshot to
//! `initialize`, `ping`, and aggregate observability; shutdown reports the
//! detached owner's `neutral|draining` disposition. The strict
//! `owner.prepare_maintenance` operation uses a separate maintenance-authenticated
//! owner connection and disjoint capability. Activation/session/paste/front-app
//! request and semantic notification shapes remain compatible with v8.
//!
//! Capture is available only when the gateway has an authenticated exact-build
//! owner lease whose build, permissions, and native health are eligible. Every
//! connection begins disabled, reconciles session capture off, replaces a full
//! configuration revision, and only then enables. Owner-protocol faults and
//! uncertain mutations close the association; they are never retransmitted.
//! The gateway contains no native suppression or injection implementation.

mod messages;
mod server;

#[cfg(test)]
pub(crate) use messages::RpcResponse;
pub use messages::{
    INBOUND_METHODS, Outbound, OutboundEncodingError, RequestId, encode_outbound, parse_request,
};
pub(crate) use server::HandleOutcome;
pub use server::Server;

pub const PROTOCOL_VERSION: u16 = 10;
