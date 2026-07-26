//! Talking Quill native-helper protocol (JSON-RPC 2.0, protocol version 2).
//!
//! # Transport and framing
//!
//! Stdin and stdout carry a sequence of frames. Each frame is a four-byte
//! unsigned big-endian payload length followed by exactly that many JSON bytes.
//! Payload lengths from 1 through 16,384 bytes (16 KiB) are accepted; zero,
//! oversized, and truncated frames are terminal framing errors. Each payload is
//! one UTF-8 JSON value. JSON-RPC batches are not supported: both empty and
//! nonempty top-level arrays are Invalid Request errors.
//!
//! Stdout is protocol-pure: it is owned by one writer and contains only framed
//! JSON-RPC responses and helper notifications. It never contains logs or other
//! unframed text. Process-level diagnostics are written to stderr. Serialized
//! outbound values are checked before writing: oversized request results become
//! a bounded Response too large error, while any unexpected encoding overflow
//! is terminal and never writes a partial frame.
//!
//! # Request envelope, IDs, and notifications
//!
//! A command envelope has exactly these fields:
//!
//! ```text
//! {"jsonrpc":"2.0","id":ID,"method":METHOD,"params":OBJECT}
//! ```
//!
//! `jsonrpc` must be the string `"2.0"`; `method` must be a nonempty string of
//! at most 64 UTF-8 bytes; and `params` is required and must be an object.
//! Unknown or duplicate envelope fields are invalid. An ID is either a JSON
//! integer in `0..=9_007_199_254_740_991` or a nonempty string of at most 64
//! UTF-8 bytes. Negative, fractional, boolean, null, array, object, empty-string,
//! oversized, and otherwise unsupported IDs are invalid. Valid IDs are echoed
//! unchanged; the helper does not assign IDs or enforce uniqueness, so the host
//! owns correlation. Invalid envelopes receive Invalid Request with a null
//! response ID; malformed JSON receives Parse error with a null response ID.
//!
//! An envelope with no `id` is considered a notification only after all other
//! envelope invariants above have passed. A valid notification is ignored
//! before method dispatch and parameter-schema decoding: it never executes,
//! initializes, shuts down, or emits a response. Consequently an invalid
//! version without an ID is still Invalid Request with a null ID. Explicit
//! `"id":null` is invalid rather than a notification.
//!
//! # Lifecycle and inbound methods
//!
//! The exact method allowlist is listed below. Object fields are required
//! unless shown as `{}`, and unknown or duplicate params fields are Invalid
//! params. Missing `params`, or null/scalar/array `params`, violate the envelope
//! and are Invalid Request.
//!
//! - `initialize`
//!   - Params: `{"protocolVersion":2}`.
//!   - Result: `{"protocolVersion":2,"helperVersion":string,
//!     "platform":string,"architecture":string,
//!     "defaultActivationKey":"Z","hookStatus":HOOK_STATUS,
//!     "permissions":PERMISSIONS}`.
//! - `activation.configure`
//!   - Params/result: `{"enabled":boolean,"bindings":[{"key":"A".."Z","shift":boolean}]}`.
//!     At most ten distinct exact bindings are accepted. Enabling on macOS is
//!     rejected unless required permissions and the event tap are ready;
//!     disabling remains available during permission loss. On Windows the helper
//!     retains unchanged no-repeat registrations and transactionally adds/removes
//!     changed chords. Any conflict leaves the prior configuration unchanged.
//! - `session.set_capture`
//!   - Params: `{"active":boolean}`.
//!   - Result: `{"active":boolean}`.
//! - `paste.inject`
//!   - Params: `{}`.
//!   - Result: `{"submitted":boolean}` on success, or
//!     `{"submitted":false,"reason":PASTE_FAILURE}` on failure. Before native
//!     dispatch the stdout writer must acquire a dedicated delivery. A submitted
//!     paste emits `paste.committed` with the request ID immediately before its
//!     matching response as one non-interleaved batch. On macOS, active Secure
//!     Event Input returns `"secure_input"` and posts no event.
//! - `front_app.get`
//!   - Params: `{}`.
//!   - Result: `{"processName":string,"windowTitle":string}`. Native strings
//!     are UTF-8-safely truncated using worst-case JSON escaping bounds so the
//!     complete response, including the largest valid request ID, fits 16 KiB.
//! - `permissions.get`
//!   - Params: `{}`.
//!   - Result: `PERMISSIONS`. On macOS, a denied permission snapshot disables
//!     activation and session capture before the response is queued.
//! - `ping`
//!   - Params: `{}`.
//!   - Result: `{"ok":true,"hookStatus":HOOK_STATUS}`.
//! - `shutdown`
//!   - Params: `{}`.
//!   - Result: `{}`. Before this response is queued, the callback gate is
//!     closed and the native hook is disabled and joined, so no callback can
//!     enqueue or swallow input after the shutdown response.
//!
//! `HOOK_STATUS` is one of `"ready"`, `"permission_required"`,
//! `"unavailable"`, or `"stopped"`. `PERMISSIONS` is
//! `{"accessibility":PERMISSION,"inputMonitoring":PERMISSION,
//! "eventPost":PERMISSION}`, where `PERMISSION` is `"granted"`, `"denied"`,
//! `"unknown"`, or `"not_applicable"`. `PASTE_FAILURE` is
//! `"permission_denied"`, `"conflicting_modifiers"`, `"secure_input"`,
//! `"os_rejected"`, or `"unavailable"`.
//!
//! `initialize` with `protocolVersion` exactly 2 must be the first successful
//! command. Before it succeeds, other allowlisted commands return Invalid
//! helper state. A failed or incompatible initialization leaves the helper
//! uninitialized. After success, every later `initialize` returns Invalid
//! helper state, regardless of its params. Unknown methods return Method not
//! found and never bypass lifecycle checks. The callback gate opens only after
//! the successful initialization response is queued. Activation capture starts
//! disabled and `initialize` does not enable it; the host must explicitly send
//! `activation.configure` with `"enabled":true`.
//!
//! # Outbound keyboard notifications
//!
//! While initialized and capture is enabled as appropriate, the helper may
//! emit these ID-less JSON-RPC notifications. Activation requires the configured
//! A-Z key with Alt/Option and optional Shift, with no Control/Ctrl, Command, or
//! Windows key. Modifier events themselves always pass through. On Windows,
//! WM_HOTKEY is the sole activation-down source and the low-level hook only
//! balances the matching activation-key up; unrelated Alt chords and all letter
//! downs remain unmodified by the hook. Session Escape/Enter capture is
//! independent of activation configuration/modifiers.
//!
//! - `activation.event` params:
//!   `{"phase":"down"|"up","key":"A".."Z","shift":boolean}`.
//! - `session.key` params:
//!   `{"key":"escape"|"enter","phase":"down"|"up"}`.
//!
//! # Responses and errors
//!
//! Success responses are
//! `{"jsonrpc":"2.0","id":ID,"result":OBJECT}`. Error responses are
//! `{"jsonrpc":"2.0","id":ID_OR_NULL,"error":{"code":CODE,
//! "message":MESSAGE}}`; no `data` member is emitted. The defined errors are:
//!
//! - `-32700`, `Parse error`
//! - `-32600`, `Invalid Request`
//! - `-32601`, `Method not found`
//! - `-32602`, `Invalid params`
//! - `-32603`, `Internal error`
//! - `-32001`, `Incompatible protocol version`
//! - `-32002`, `Invalid helper state`
//! - `-32003`, `Native operation unavailable`
//! - `-32004`, `Response too large`
//!
//! # EOF, shutdown, and terminal failures
//!
//! Clean stdin EOF is a clean shutdown and has no response. An accepted
//! `shutdown` first quiesces native input, then queues its success response and
//! gives the stdout writer a finite two-second window to flush queued frames.
//! Clean EOF uses the same bounded writer drain. If stdout remains blocked, the
//! writer is detached after the deadline and process exit terminates it; native
//! input has already failed open and stopped, so exit cannot leave keys captured.
//! Framing failures, stdout disconnection or queue/encoding failure, native
//! callback failures, reducer failure, and unexpected hook termination are
//! terminal. On macOS, user-input tap disablement, failed timeout recovery, or
//! a second consecutive timeout disablement is also terminal; one timeout may
//! recover only after Core Graphics confirms the tap is enabled. All terminal
//! failures close the callback gate immediately, stop native capture, and
//! terminate the helper. A terminal transport failure cannot be relied upon to produce a
//! final protocol response. A stdin reader blocked in the OS may be detached so
//! it cannot delay terminal shutdown.

mod messages;
mod server;

#[cfg(test)]
pub(crate) use messages::RpcResponse;
pub use messages::{
    INBOUND_METHODS, Outbound, OutboundEncodingError, RequestId, encode_outbound, parse_request,
};
pub(crate) use server::HandleOutcome;
pub use server::Server;

pub const PROTOCOL_VERSION: u16 = 2;
