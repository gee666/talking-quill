//! Modifier tracking, activation dispatch, and observation-only matching.
use super::*;

mod modifiers;
pub(super) use modifiers::*;

mod dispatch;
pub(super) use dispatch::*;

mod observation;
pub(super) use observation::*;

#[cfg(all(test, feature = "windows-native-test-input"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum InjectionKind {
    Physical,
    External,
    Helper,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct HookObservation {
    pub(super) observed_at_ms: u64,
    pub(super) native_modifiers: Option<ModifierTracker>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DesktopIdentity {
    pub(super) name: [u16; 64],
    pub(super) len: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TransactionalHookRecord {
    pub(super) virtual_key: u16,
    pub(super) scan_code: u32,
    pub(super) extended: bool,
    pub(super) platform_flags: u32,
    pub(super) phase: KeyPhase,
    pub(super) source: InputSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CompletedHookRecord {
    pub(super) identity: KeyIdentity,
    pub(super) virtual_key: u16,
    pub(super) phase: KeyPhase,
    pub(super) repeat: bool,
    pub(super) enter_source: Option<EnterSource>,
    pub(super) physical: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum EnterSource {
    Main,
    Numpad,
}

#[derive(Debug, Default, Eq, PartialEq)]
pub(super) struct WindowsPhysicalTracker {
    pub(super) common: PhysicalKeyTracker,
    pub(super) main_enter_held: bool,
    pub(super) numpad_enter_held: bool,
}

impl WindowsPhysicalTracker {
    pub(super) fn observe(
        &mut self,
        key: PhysicalKey,
        enter_source: Option<EnterSource>,
        phase: KeyPhase,
    ) -> bool {
        if key != PhysicalKey::Enter {
            return self.common.observe(key, phase);
        }
        let held = match enter_source {
            Some(EnterSource::Main) => &mut self.main_enter_held,
            Some(EnterSource::Numpad) => &mut self.numpad_enter_held,
            None => return false,
        };
        match phase {
            KeyPhase::Down => {
                let repeat = *held;
                *held = true;
                repeat
            }
            KeyPhase::Up => {
                *held = false;
                false
            }
        }
    }

    pub(super) fn seed_enter_preheld(&mut self) {
        // GetAsyncKeyState exposes both physical Enter sources as VK_RETURN.
        // Conservatively fence each source until its own observed up.
        self.main_enter_held = true;
        self.numpad_enter_held = true;
    }

    pub(super) fn held_letter_bits(&self) -> u32 {
        self.common.held_letter_bits()
    }
}

// Both variants are fixed-capacity reducer authority. Boxing would allocate in
// the low-level callback and would weaken panic recovery.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub(super) enum TransactionAuthority {
    Turn(Turn),
    AwaitingDeferredReplay,
    Resume {
        continuation: Continuation,
        outcome: EffectOutcome,
    },
}

#[derive(Clone, Debug)]
pub(super) struct DeferredCallbackReplay {
    pub(super) continuation: Continuation,
    pub(super) record: Option<CompletedHookRecord>,
    pub(super) outcome: DeferredEffectOutcome,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum DeferredEffectOutcome {
    Replay,
    MenuNeutralization,
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, Debug)]
pub(super) enum ReplayWork {
    Replay {
        batch: talking_quill_keyboard_core::transactional::ReplayBatch,
        target: CandidateTargetEvidence,
        desktop: DesktopIdentity,
    },
    NeutralizeMenu {
        modifiers: talking_quill_keyboard_core::transactional::MenuModifiers,
        target: CandidateTargetEvidence,
        desktop: DesktopIdentity,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(super) enum CallbackDisposition {
    Pass = 0,
    Capture = 1,
}
