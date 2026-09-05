use super::client::OwnerCaptureClient;
use crate::gateway::{
    KeyboardOwnerSnapshot, KeyboardOwnerState, PasteFailure, PermissionState, Permissions,
    PlatformError,
};
use talking_quill_keyboard_core::{
    ActivationBinding, ActivationBindings, ActivationKey, ProfileId, Shortcut, ShortcutModifiers,
};
use talking_quill_owner_protocol::{Bytes32, schema as wire};

pub(super) fn decode_opaque32(value: &str) -> Option<Bytes32> {
    if value.len() != 64 {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (index, output) in bytes.iter_mut().enumerate() {
        *output = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(Bytes32::new(bytes))
}

pub(super) fn snapshot_from_owner(owner: &OwnerCaptureClient) -> KeyboardOwnerSnapshot {
    let health = owner.health();
    KeyboardOwnerSnapshot {
        model: "out_of_process",
        protocol_version: 1,
        state: match health.reported_state {
            wire::OwnerReportedState::Starting => KeyboardOwnerState::SafeDisabled,
            wire::OwnerReportedState::IdleNeutral => KeyboardOwnerState::Idle,
            wire::OwnerReportedState::LeaseDisabled => KeyboardOwnerState::LeasedDisabled,
            wire::OwnerReportedState::LeaseEnabled => KeyboardOwnerState::LeasedEnabled,
            wire::OwnerReportedState::LeaseDraining
            | wire::OwnerReportedState::OrphanCancelling
            | wire::OwnerReportedState::OrphanDraining => KeyboardOwnerState::Draining,
            wire::OwnerReportedState::MaintenanceDraining
            | wire::OwnerReportedState::MaintenanceReady => KeyboardOwnerState::Maintenance,
            wire::OwnerReportedState::DegradedDraining => KeyboardOwnerState::Degraded,
            wire::OwnerReportedState::Stopping => KeyboardOwnerState::Unavailable,
        },
        instance_id: encode_opaque(health.owner_instance_id.as_bytes()),
        build_id: owner.build_id().to_owned(),
        lease_epoch: Some(owner.lease_epoch()),
        authenticated: true,
        keyboard_build_eligible: health.keyboard_build_eligible,
        permissions_eligible: health.permissions_eligible,
        hook_healthy: health.hook_healthy
            && health.process_state == wire::ProcessState::Healthy
            && matches!(
                health.reported_state,
                wire::OwnerReportedState::IdleNeutral
                    | wire::OwnerReportedState::LeaseDisabled
                    | wire::OwnerReportedState::LeaseEnabled
                    | wire::OwnerReportedState::LeaseDraining
            ),
        rollback_latched: health.rollback_latched,
    }
}

pub(super) fn wire_bindings(bindings: ActivationBindings) -> Result<wire::Bindings, PlatformError> {
    let values = bindings
        .iter()
        .map(|binding| {
            let shortcut = binding.shortcut();
            let modifiers = shortcut.modifiers();
            let keys = shortcut
                .keys()
                .iter()
                .map(|key| wire_letter(*key))
                .collect();
            Ok(wire::Binding::new(
                wire::ProfileId::new(binding.profile_id().as_str().to_owned())
                    .map_err(|_| PlatformError::NativeFailure)?,
                wire::BindingShortcut::new(
                    wire::Modifiers::new(
                        modifiers.ctrl,
                        modifiers.alt,
                        modifiers.shift,
                        modifiers.meta,
                    ),
                    keys,
                )
                .map_err(|_| PlatformError::NativeFailure)?,
            ))
        })
        .collect::<Result<Vec<_>, PlatformError>>()?;
    wire::Bindings::new(values).map_err(|_| PlatformError::NativeFailure)
}

pub(super) fn core_binding(event: &wire::ActivationEvent) -> Option<ActivationBinding> {
    let (ctrl, alt, shift, meta) = event.shortcut.modifiers().values();
    let keys = event
        .shortcut
        .keys()
        .iter()
        .map(|key| ActivationKey::from_index(*key as u8))
        .collect::<Option<Vec<_>>>()?;
    Some(ActivationBinding::new(
        ProfileId::new(event.profile_id.as_str()).ok()?,
        Shortcut::new(
            ShortcutModifiers {
                ctrl,
                alt,
                shift,
                meta,
            },
            &keys,
        )
        .ok()?,
    ))
}

fn wire_letter(key: ActivationKey) -> wire::Letter {
    const LETTERS: [wire::Letter; 26] = [
        wire::Letter::A,
        wire::Letter::B,
        wire::Letter::C,
        wire::Letter::D,
        wire::Letter::E,
        wire::Letter::F,
        wire::Letter::G,
        wire::Letter::H,
        wire::Letter::I,
        wire::Letter::J,
        wire::Letter::K,
        wire::Letter::L,
        wire::Letter::M,
        wire::Letter::N,
        wire::Letter::O,
        wire::Letter::P,
        wire::Letter::Q,
        wire::Letter::R,
        wire::Letter::S,
        wire::Letter::T,
        wire::Letter::U,
        wire::Letter::V,
        wire::Letter::W,
        wire::Letter::X,
        wire::Letter::Y,
        wire::Letter::Z,
    ];
    LETTERS[usize::from(key.index())]
}
pub(super) fn permissions_from_wire(value: &wire::PermissionsResult) -> Permissions {
    Permissions {
        accessibility: permission(value.accessibility),
        input_monitoring: permission(value.input_monitoring),
        event_post: permission(value.event_post),
    }
}
fn permission(value: wire::PermissionState) -> PermissionState {
    match value {
        wire::PermissionState::Granted => PermissionState::Granted,
        wire::PermissionState::Denied => PermissionState::Denied,
        wire::PermissionState::Unknown => PermissionState::Unknown,
        wire::PermissionState::NotRequired => PermissionState::NotApplicable,
    }
}
pub(super) fn map_paste_error(value: wire::PasteRefusalReason) -> PasteFailure {
    match value {
        wire::PasteRefusalReason::PermissionDenied => PasteFailure::PermissionDenied,
        wire::PasteRefusalReason::ConflictingModifiers => PasteFailure::ConflictingModifiers,
        wire::PasteRefusalReason::SecureInput => PasteFailure::SecureInput,
        wire::PasteRefusalReason::NativeRejected => PasteFailure::OsRejected,
        wire::PasteRefusalReason::TargetUnavailable
        | wire::PasteRefusalReason::ClipboardChanged
        | wire::PasteRefusalReason::NativeUnavailable => PasteFailure::Unavailable,
    }
}
fn encode_opaque(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
