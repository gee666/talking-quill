use crate::gateway::ClipboardTextHash;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use talking_quill_keyboard_core::{
    ActivationBindings, ActivationContext, ActivationGeneration, NativeTargetToken,
    SessionCaptureMode,
};

/// Protocol-v10 `activation.configure` params schema.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConfigureActivationParams {
    pub(super) enabled: bool,
    pub(super) bindings: ActivationBindings,
}

/// `session.set_capture` params schema: `{ "mode": "off" | "recording" | "cancel-only" }`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SetCaptureParams {
    pub(super) mode: SessionCaptureMode,
}

/// Protocol-v10 `paste.inject` params schema. The opaque target token is validated
/// and forwarded without interpretation or diagnostic exposure.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct PasteInjectParams {
    activation_generation: u64,
    target_token: Value,
    expected_clipboard_sha256: String,
}

impl PasteInjectParams {
    pub(super) fn into_context(self) -> Option<(ActivationContext, ClipboardTextHash)> {
        let generation = ActivationGeneration::new(self.activation_generation)?;
        let expected_hash = ClipboardTextHash::from_lower_hex(&self.expected_clipboard_sha256)?;
        let context = ActivationContext::target_unavailable(generation);
        let context = match self.target_token {
            Value::Null => context,
            Value::String(token) => context.with_target_token(NativeTargetToken::new(&token).ok()?),
            _ => return None,
        };
        Some((context, expected_hash))
    }
}

/// Params schema for parameterless methods. Params are still required and must
/// be exactly `{}` so misspelled or future fields fail closed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EmptyParams {}

#[derive(Debug, Serialize)]
pub(super) struct SetCaptureResult {
    pub(super) mode: SessionCaptureMode,
}

#[derive(Debug, Serialize)]
pub(super) struct PingResult {
    pub(super) ok: bool,
    #[serde(rename = "hookStatus")]
    pub(super) hook_status: crate::gateway::HookStatus,
    #[serde(rename = "keyboardOwner")]
    pub(super) keyboard_owner: crate::gateway::KeyboardOwnerSnapshot,
}
