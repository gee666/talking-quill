use std::collections::VecDeque;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, Ordering},
};
use talking_quill_keyboard_owner::{
    AdapterEvent, AdapterEventDisposition, AdapterEventId, BrokerEvent, CapabilityIdSource,
    NativeAdapter, NativeEffect, NativeEffectKind, NativeEffectResult,
};
use talking_quill_owner_protocol::schema::*;

#[derive(Debug)]
pub(super) struct FakeNativeAdapter {
    events: VecDeque<AdapterEvent>,
    next_event_id: u64,
    neutral_pending: bool,
    neutral_release: Arc<AtomicBool>,
}

impl FakeNativeAdapter {
    pub(super) fn new(neutral_release: Arc<AtomicBool>) -> Self {
        Self {
            events: VecDeque::new(),
            next_event_id: 0,
            neutral_pending: false,
            neutral_release,
        }
    }

    fn emit_ownership(
        &mut self,
        candidate: talking_quill_keyboard_owner::state::CandidateOwnership,
        replay_cleanup_edges: u16,
    ) {
        self.next_event_id += 1;
        self.events.push_back(AdapterEvent::new(
            AdapterEventId::new(self.next_event_id).unwrap(),
            BrokerEvent::OwnershipChanged(
                talking_quill_keyboard_owner::state::NativeOwnershipObservation {
                    candidate,
                    activation_drain_keys: 0,
                    session_drain_keys: 0,
                    replay_cleanup_edges,
                    paste: talking_quill_keyboard_owner::state::PasteOwnership::None,
                    conservative_native_work: false,
                    admitted_effects: 0,
                },
            ),
        ));
    }
}

impl NativeAdapter for FakeNativeAdapter {
    fn seed_startup_physical_snapshot(&mut self) -> bool {
        true
    }
    fn readiness(&self) -> talking_quill_keyboard_owner::state::NativeReadiness {
        talking_quill_keyboard_owner::state::NativeReadiness {
            keyboard_build_eligible: true,
            paste_ready: true,
            permissions_eligible: true,
            hook_healthy: true,
        }
    }
    fn execute(&mut self, effect: NativeEffect) -> NativeEffectResult {
        use talking_quill_keyboard_owner::state::{
            CandidateOwnership, NativeOwnership, PasteOwnership,
        };

        match effect.kind() {
            NativeEffectKind::OpenFreshAdmission => {
                self.emit_ownership(CandidateOwnership::Active, 0);
                NativeEffectResult::Applied
            }
            NativeEffectKind::CloseFreshAdmission
            | NativeEffectKind::EmergencyCloseFreshAdmission => {
                NativeEffectResult::AdmissionClosed {
                    through_event: AdapterEventId::new(self.next_event_id),
                }
            }
            NativeEffectKind::CancelCandidate => {
                self.neutral_pending = true;
                NativeEffectResult::CandidateCancelled(
                    NativeOwnership::new(
                        CandidateOwnership::None,
                        0,
                        0,
                        1,
                        PasteOwnership::None,
                        0,
                    )
                    .unwrap(),
                )
            }
            _ => NativeEffectResult::Applied,
        }
    }
    fn try_next_event(&mut self) -> Option<AdapterEvent> {
        if self.neutral_pending && self.neutral_release.load(Ordering::Acquire) {
            self.neutral_pending = false;
            self.emit_ownership(
                talking_quill_keyboard_owner::state::CandidateOwnership::None,
                0,
            );
        }
        self.events.pop_front()
    }
    fn acknowledge_event(
        &mut self,
        _: talking_quill_keyboard_owner::AdapterEventId,
        _: AdapterEventDisposition,
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

#[derive(Debug, Default)]
pub(super) struct Capabilities(AtomicU8);
impl CapabilityIdSource for Capabilities {
    fn next_capability_id(&mut self) -> Option<talking_quill_keyboard_owner::state::CapabilityId> {
        let value = self.0.fetch_add(1, Ordering::AcqRel).checked_add(1)?;
        let mut bytes = [0_u8; 32];
        bytes[0] = value;
        talking_quill_keyboard_owner::state::CapabilityId::new(bytes)
    }
}
