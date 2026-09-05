//! Modifier tracking, activation dispatch, and observation-only matching.
use super::*;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ModifierSides {
    pub(super) left: bool,
    pub(super) right: bool,
    pub(super) generic: bool,
}

impl ModifierSides {
    pub(super) fn from_state(
        left: u16,
        right: u16,
        generic: Option<u16>,
        is_down: &mut impl FnMut(u16) -> bool,
    ) -> Self {
        let left_down = is_down(left);
        let right_down = is_down(right);
        Self {
            left: left_down,
            right: right_down,
            generic: !left_down && !right_down && generic.is_some_and(is_down),
        }
    }

    pub(super) fn observe_left(&mut self, phase: KeyPhase) {
        self.generic = false;
        self.left = phase == KeyPhase::Down;
    }

    pub(super) fn observe_right(&mut self, phase: KeyPhase) {
        self.generic = false;
        self.right = phase == KeyPhase::Down;
    }

    pub(super) fn observe_generic(&mut self, phase: KeyPhase) {
        self.generic = phase == KeyPhase::Down;
    }

    pub(super) const fn is_down(self) -> bool {
        self.left || self.right || self.generic
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ModifierTracker {
    pub(super) ctrl: ModifierSides,
    pub(super) alt: ModifierSides,
    pub(super) shift: ModifierSides,
    pub(super) meta: ModifierSides,
}

impl ModifierTracker {
    pub(super) fn from_state(mut is_down: impl FnMut(u16) -> bool) -> Self {
        Self {
            ctrl: ModifierSides::from_state(
                VK_LCONTROL,
                VK_RCONTROL,
                Some(VK_CONTROL),
                &mut is_down,
            ),
            alt: ModifierSides::from_state(VK_LMENU, VK_RMENU, Some(VK_MENU), &mut is_down),
            shift: ModifierSides::from_state(VK_LSHIFT, VK_RSHIFT, Some(VK_SHIFT), &mut is_down),
            meta: ModifierSides::from_state(VK_LWIN, VK_RWIN, None, &mut is_down),
        }
    }

    pub(super) fn observe(
        &mut self,
        virtual_key: u16,
        scan_code: u32,
        extended: bool,
        phase: KeyPhase,
    ) -> bool {
        match virtual_key {
            VK_LCONTROL => self.ctrl.observe_left(phase),
            VK_RCONTROL => self.ctrl.observe_right(phase),
            VK_CONTROL if scan_code == 0x1D && extended => self.ctrl.observe_right(phase),
            VK_CONTROL if scan_code == 0x1D => self.ctrl.observe_left(phase),
            VK_CONTROL => self.ctrl.observe_generic(phase),
            VK_LMENU => self.alt.observe_left(phase),
            VK_RMENU => self.alt.observe_right(phase),
            VK_MENU if scan_code == 0x38 && extended => self.alt.observe_right(phase),
            VK_MENU if scan_code == 0x38 => self.alt.observe_left(phase),
            VK_MENU => self.alt.observe_generic(phase),
            VK_LSHIFT => self.shift.observe_left(phase),
            VK_RSHIFT => self.shift.observe_right(phase),
            VK_SHIFT if scan_code == 0x2A => self.shift.observe_left(phase),
            VK_SHIFT if scan_code == 0x36 => self.shift.observe_right(phase),
            VK_SHIFT => self.shift.observe_generic(phase),
            VK_LWIN => self.meta.observe_left(phase),
            VK_RWIN => self.meta.observe_right(phase),
            _ => return false,
        }
        true
    }

    pub(super) const fn mask(self) -> ModifierMask {
        ModifierMask::new(
            self.ctrl.is_down(),
            self.alt.is_down(),
            self.shift.is_down(),
            self.meta.is_down(),
        )
    }

    pub(super) const fn transactional_sides(self) -> TransactionalModifierSides {
        let mut bits = 0_u8;
        if self.ctrl.left || self.ctrl.generic {
            bits |= ModifierSide::LeftCtrl.bit();
        }
        if self.ctrl.right {
            bits |= ModifierSide::RightCtrl.bit();
        }
        if self.alt.left || self.alt.generic {
            bits |= ModifierSide::LeftAlt.bit();
        }
        if self.alt.right {
            bits |= ModifierSide::RightAlt.bit();
        }
        if self.shift.left || self.shift.generic {
            bits |= ModifierSide::LeftShift.bit();
        }
        if self.shift.right {
            bits |= ModifierSide::RightShift.bit();
        }
        if self.meta.left {
            bits |= ModifierSide::LeftMeta.bit();
        }
        if self.meta.right {
            bits |= ModifierSide::RightMeta.bit();
        }
        TransactionalModifierSides::from_bits(bits)
    }

    pub(super) const fn is_neutral(self) -> bool {
        !self.mask().any()
    }
}

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ActiveActivationContext {
    pub(super) binding: ActivationBinding,
    pub(super) context: ActivationContext,
}

#[derive(Debug)]
pub(super) struct ActivationDispatcher {
    pub(super) next_generation: Option<ActivationGeneration>,
    pub(super) active: Option<ActiveActivationContext>,
    pub(super) targets: TargetRegistry,
}

impl ActivationDispatcher {
    pub(super) fn deliver(
        &mut self,
        outbound: &Sender<NativeEvent>,
        terminal: &TerminalSignal,
        context_observability: &TransactionObservability,
        notice: ActivationNotice,
        candidate_target: Option<CandidateTargetEvidence>,
    ) -> bool {
        match notice {
            ActivationNotice::Down { binding } => {
                context_observability.record_registered_match_callback();
                if self.active.is_some() {
                    return false;
                }
                let Some(context) = self.take_context(candidate_target) else {
                    return false;
                };
                let event = KeyboardEvent::Activation {
                    binding,
                    context,
                    phase: EventPhase::Down,
                };
                if deliver_callback_event(outbound, terminal, event) {
                    context_observability.record_callback_channel_accepted();
                    self.active = Some(ActiveActivationContext { binding, context });
                    true
                } else {
                    context_observability.record_callback_channel_rejected();
                    self.targets.remove(context);
                    false
                }
            }
            ActivationNotice::Up { binding, .. } => {
                context_observability.record_registered_release_callback();
                let Some(active) = self.active.take() else {
                    return false;
                };
                if active.binding != binding {
                    self.targets.remove(active.context);
                    return false;
                }
                let delivered = deliver_callback_event(
                    outbound,
                    terminal,
                    KeyboardEvent::Activation {
                        binding,
                        context: active.context,
                        phase: EventPhase::Up,
                    },
                );
                if delivered {
                    context_observability.record_callback_channel_accepted();
                } else {
                    context_observability.record_callback_channel_rejected();
                }
                delivered
            }
            ActivationNotice::Complete { binding, held_ms } => {
                context_observability.record_registered_match_callback();
                context_observability.record_registered_release_callback();
                if self.active.is_some() {
                    return false;
                }
                let Some(context) = self.take_context(candidate_target) else {
                    return false;
                };
                let event = KeyboardEvent::ActivationComplete {
                    binding,
                    context,
                    held_ms,
                };
                if deliver_callback_event(outbound, terminal, event) {
                    context_observability.record_callback_channel_accepted();
                    true
                } else {
                    context_observability.record_callback_channel_rejected();
                    self.targets.remove(context);
                    false
                }
            }
        }
    }

    pub(super) fn take_context(
        &mut self,
        candidate_target: Option<CandidateTargetEvidence>,
    ) -> Option<ActivationContext> {
        let generation = self.next_generation?;
        self.next_generation = if generation == ActivationGeneration::MAX {
            None
        } else {
            ActivationGeneration::new(generation.get() + 1)
        };
        Some(self.targets.capture_context(
            generation,
            candidate_target.and_then(CandidateTargetEvidence::paste_evidence),
        ))
    }
}

impl Default for ActivationDispatcher {
    fn default() -> Self {
        Self {
            next_generation: Some(ActivationGeneration::FIRST),
            active: None,
            targets: TargetRegistry::new(),
        }
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

#[derive(Clone, Copy, Debug)]
pub(super) struct ShadowCandidate {
    pub(super) generation: u64,
    pub(super) revision: ConfigRevision,
    pub(super) expected_modifiers: TransactionalModifierSides,
    pub(super) cursor: MatchCursor,
    pub(super) held_letters: u32,
    pub(super) pending_exact: Option<(ActivationBinding, ActivationKey)>,
}

/// Non-owning mirror of the production matcher. It consumes the same compiled
/// prefix family and physical snapshots, but has no disposition/effect API.
#[derive(Clone, Copy, Debug)]
pub(super) struct RegisteredObservationShadow {
    pub(super) candidate: Option<ShadowCandidate>,
    pub(super) next_generation: u64,
}

impl Default for RegisteredObservationShadow {
    fn default() -> Self {
        Self {
            candidate: None,
            next_generation: 1,
        }
    }
}

impl RegisteredObservationShadow {
    pub(super) fn reset(&mut self) {
        self.candidate = None;
    }

    pub(super) fn cancel(&mut self) {
        self.candidate = None;
    }

    pub(super) fn observe(
        &mut self,
        observability: &TransactionObservability,
        config: CompiledActivationConfig,
        snapshot: PhysicalSnapshot,
        identity: KeyIdentity,
        phase: PhysicalPhase,
    ) -> Option<u64> {
        let Some(mut candidate) = self.candidate else {
            let KeyIdentity::Letter(letter) = identity else {
                return None;
            };
            let bit = 1_u32 << letter.index();
            if phase != PhysicalPhase::Down
                || snapshot.alt_gr_active
                || snapshot.held_letters != bit
            {
                return None;
            }
            let result = config
                .matcher()
                .start(snapshot.modifiers.combined(), letter);
            let cursor = result.cursor()?;
            let generation = self.next_generation;
            self.next_generation = self
                .next_generation
                .saturating_add(1)
                .clamp(1, crate::platform::observability::MAX_OBSERVABILITY_COUNTER);
            observability.record_registered_candidate_callback();
            let pending_exact = shadow_pending_exact(result, letter);
            if pending_exact.is_some() {
                observability.record_registered_match_callback();
            }
            self.candidate = Some(ShadowCandidate {
                generation,
                revision: config.revision(),
                expected_modifiers: snapshot.modifiers,
                cursor,
                held_letters: bit,
                pending_exact,
            });
            return None;
        };

        if candidate.revision != config.revision()
            || snapshot.alt_gr_active
            || snapshot.modifiers != candidate.expected_modifiers
        {
            self.cancel();
            return None;
        }
        let KeyIdentity::Letter(letter) = identity else {
            if !matches!(identity, KeyIdentity::Modifier(_)) {
                self.cancel();
            }
            return None;
        };
        let bit = 1_u32 << letter.index();
        match phase {
            PhysicalPhase::Repeat => {
                if candidate.held_letters & bit == 0 {
                    self.cancel();
                }
            }
            PhysicalPhase::Down => {
                if candidate.held_letters & bit != 0 {
                    self.cancel();
                    return None;
                }
                let result = config.matcher().advance(candidate.cursor, letter);
                let Some(cursor) = result.cursor() else {
                    self.cancel();
                    return None;
                };
                candidate.cursor = cursor;
                candidate.held_letters |= bit;
                candidate.pending_exact = shadow_pending_exact(result, letter);
                if candidate.pending_exact.is_some() {
                    observability.record_registered_match_callback();
                }
                self.candidate = Some(candidate);
            }
            PhysicalPhase::Up => {
                if candidate.held_letters & bit == 0 {
                    self.cancel();
                    return None;
                }
                candidate.held_letters &= !bit;
                if candidate
                    .pending_exact
                    .is_some_and(|(_, trigger)| trigger == letter)
                {
                    observability.record_registered_release_callback();
                    self.reset();
                    return Some(candidate.generation);
                }
                self.cancel();
            }
        }
        None
    }
}

pub(super) fn shadow_pending_exact(
    result: MatchClass,
    trigger: ActivationKey,
) -> Option<(ActivationBinding, ActivationKey)> {
    match result {
        MatchClass::Exact { binding, .. } | MatchClass::ExactWithLonger { binding, .. } => {
            Some((binding, trigger))
        }
        MatchClass::Prefix(_) | MatchClass::NoCandidate => None,
    }
}
