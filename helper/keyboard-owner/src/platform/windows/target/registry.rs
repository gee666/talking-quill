//! Bound generation-scoped target tokens and consume each retained target once.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TargetEntry {
    generation: ActivationGeneration,
    token: NativeTargetToken,
    evidence: TargetEvidence,
}

/// Fixed generation-to-target ring. Eviction, a stale generation, or a token
/// mismatch is deliberately indistinguishable from an unavailable target.
#[derive(Clone, Debug)]
pub(in crate::platform::windows) struct TargetRegistry {
    pub(super) epoch: Option<u64>,
    entries: [Option<TargetEntry>; TARGET_REGISTRY_CAPACITY],
}

impl TargetRegistry {
    pub(in crate::platform::windows) fn new() -> Self {
        Self {
            epoch: random_epoch(),
            entries: [None; TARGET_REGISTRY_CAPACITY],
        }
    }

    /// Registers immutable candidate-start paste evidence at the accepted
    /// activation boundary. Missing focus evidence deliberately yields a
    /// targetless activation and therefore clipboard-only insertion.
    pub(in crate::platform::windows) fn capture_context(
        &mut self,
        generation: ActivationGeneration,
        evidence: Option<TargetEvidence>,
    ) -> ActivationContext {
        evidence.map_or_else(
            || ActivationContext::target_unavailable(generation),
            |evidence| self.record(generation, evidence),
        )
    }

    pub(in crate::platform::windows) fn take(
        &mut self,
        context: ActivationContext,
    ) -> Option<TargetEvidence> {
        let token = context.target_token()?;
        let slot = slot(context.activation_generation());
        let entry = self.entries[slot]?;
        if entry.generation != context.activation_generation() || entry.token != token {
            return None;
        }
        self.entries[slot] = None;
        Some(entry.evidence)
    }

    pub(in crate::platform::windows) fn remove(&mut self, context: ActivationContext) {
        let slot = slot(context.activation_generation());
        if self.entries[slot].is_some_and(|entry| {
            entry.generation == context.activation_generation()
                && Some(entry.token) == context.target_token()
        }) {
            self.entries[slot] = None;
        }
    }

    pub(super) fn record(
        &mut self,
        generation: ActivationGeneration,
        evidence: TargetEvidence,
    ) -> ActivationContext {
        let Some(epoch) = self.epoch else {
            return ActivationContext::target_unavailable(generation);
        };
        let token = token_for(epoch, generation);
        self.entries[slot(generation)] = Some(TargetEntry {
            generation,
            token,
            evidence,
        });
        ActivationContext::target_unavailable(generation).with_target_token(token)
    }
}

impl Default for TargetRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub(super) fn random_epoch() -> Option<u64> {
    let mut bytes = [0_u8; 8];
    // SAFETY: a null algorithm handle with BCRYPT_USE_SYSTEM_PREFERRED_RNG is
    // the documented system CSPRNG form; the output buffer is fully writable.
    let status = unsafe {
        BCryptGenRandom(
            null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    (status == 0).then(|| u64::from_le_bytes(bytes))
}

const fn slot(generation: ActivationGeneration) -> usize {
    generation.get() as usize % TARGET_REGISTRY_CAPACITY
}

pub(super) fn token_for(epoch: u64, generation: ActivationGeneration) -> NativeTargetToken {
    const PREFIX: &[u8] = b"win-v8:";
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; 40];
    let mut index = 0;
    while index < PREFIX.len() {
        bytes[index] = PREFIX[index];
        index += 1;
    }
    let mut nibble = 0;
    while nibble < 16 {
        let shift = (15 - nibble) * 4;
        bytes[PREFIX.len() + nibble] = HEX[((epoch >> shift) & 0xF) as usize];
        nibble += 1;
    }
    bytes[PREFIX.len() + 16] = b':';
    nibble = 0;
    while nibble < 16 {
        let shift = (15 - nibble) * 4;
        bytes[PREFIX.len() + 17 + nibble] = HEX[((generation.get() >> shift) & 0xF) as usize];
        nibble += 1;
    }
    let value = std::str::from_utf8(&bytes).expect("token alphabet is ASCII");
    NativeTargetToken::new(value).expect("fixed Windows target token is bounded")
}
