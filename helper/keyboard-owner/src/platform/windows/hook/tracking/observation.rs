//! Non-owning observation of exact registered shortcut matches and releases.
use super::*;

#[derive(Clone, Copy, Debug)]
pub(in crate::platform::windows::hook) struct ShadowCandidate {
    pub(in crate::platform::windows::hook) generation: u64,
    pub(in crate::platform::windows::hook) revision: ConfigRevision,
    pub(in crate::platform::windows::hook) expected_modifiers: TransactionalModifierSides,
    pub(in crate::platform::windows::hook) cursor: MatchCursor,
    pub(in crate::platform::windows::hook) held_letters: u32,
    pub(in crate::platform::windows::hook) pending_exact:
        Option<(ActivationBinding, ActivationKey)>,
}

/// Non-owning mirror of the production matcher. It consumes the same compiled
/// prefix family and physical snapshots, but has no disposition/effect API.
#[derive(Clone, Copy, Debug)]
pub(in crate::platform::windows::hook) struct RegisteredObservationShadow {
    pub(in crate::platform::windows::hook) candidate: Option<ShadowCandidate>,
    pub(in crate::platform::windows::hook) next_generation: u64,
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
    pub(in crate::platform::windows::hook) fn reset(&mut self) {
        self.candidate = None;
    }

    pub(in crate::platform::windows::hook) fn cancel(&mut self) {
        self.reset();
    }

    pub(in crate::platform::windows::hook) fn observe(
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

pub(in crate::platform::windows::hook) fn shadow_pending_exact(
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
