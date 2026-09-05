//! Bounded activation-to-handle registry and process-scoped tokens.

use super::*;

pub(super) struct TargetEntry {
    pub(super) generation: ActivationGeneration,
    pub(super) token: NativeTargetToken,
    pub(super) target: TargetHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ProcessEpoch(pub(super) [u8; 16]);

/// Fixed generation-to-target ring. The process epoch is sourced from the
/// operating system CSPRNG on owner startup. Without it, registry insertion is
/// disabled and every activation is conservatively targetless.
pub(in crate::platform::macos) struct TargetRegistry {
    pub(super) entries: [Option<TargetEntry>; TARGET_REGISTRY_CAPACITY],
    pub(super) process_epoch: Option<ProcessEpoch>,
}

impl TargetRegistry {
    pub(in crate::platform::macos) fn new() -> Self {
        Self {
            entries: std::array::from_fn(|_| None),
            process_epoch: None,
        }
    }

    pub(in crate::platform::macos) fn initialize_process_epoch(&mut self) -> bool {
        let mut bytes = [0_u8; 16];
        // SAFETY: a null random source requests kSecRandomDefault and `bytes`
        // is valid writable storage for the exact supplied length.
        let status =
            unsafe { ffi::SecRandomCopyBytes(null_mut(), bytes.len(), bytes.as_mut_ptr().cast()) };
        if status != 0 {
            self.process_epoch = None;
            false
        } else {
            self.process_epoch = Some(ProcessEpoch(bytes));
            true
        }
    }

    pub(in crate::platform::macos) fn bind_context(
        &mut self,
        generation: ActivationGeneration,
        target: Option<TargetHandle>,
    ) -> ActivationContext {
        let Some(epoch) = self.process_epoch else {
            return ActivationContext::target_unavailable(generation);
        };
        let Some(target) = target else {
            return ActivationContext::target_unavailable(generation);
        };
        self.record(generation, target, epoch)
    }

    pub(in crate::platform::macos) fn take(
        &mut self,
        context: ActivationContext,
    ) -> Option<TargetHandle> {
        let token = context.target_token()?;
        let slot = slot(context.activation_generation());
        let entry = self.entries[slot].take()?;
        if entry.generation == context.activation_generation() && entry.token == token {
            Some(entry.target)
        } else {
            self.entries[slot] = Some(entry);
            None
        }
    }

    pub(in crate::platform::macos) fn remove(&mut self, context: ActivationContext) {
        let slot = slot(context.activation_generation());
        if self.entries[slot].as_ref().is_some_and(|entry| {
            entry.generation == context.activation_generation()
                && Some(entry.token) == context.target_token()
        }) {
            self.entries[slot] = None;
        }
    }

    pub(super) fn record(
        &mut self,
        generation: ActivationGeneration,
        target: TargetHandle,
        epoch: ProcessEpoch,
    ) -> ActivationContext {
        let token = token_for(epoch, generation);
        self.entries[slot(generation)] = Some(TargetEntry {
            generation,
            token,
            target,
        });
        ActivationContext::target_unavailable(generation).with_target_token(token)
    }

    #[cfg(test)]
    pub(super) fn with_epoch(epoch: [u8; 16]) -> Self {
        Self {
            entries: std::array::from_fn(|_| None),
            process_epoch: Some(ProcessEpoch(epoch)),
        }
    }
}

#[cfg(test)]
pub(in crate::platform::macos) const fn activation_reservation_for_test(
    publication_id: i32,
    notification_epoch: u64,
) -> ActivationReservation {
    ActivationReservation {
        notification_epoch,
        boundary_epoch: 1,
        selected_range_epoch: 1,
        publication_id: publication_id as u64,
    }
}

#[cfg(test)]
pub(in crate::platform::macos) fn validation_request_for_test(
    request_id: u64,
    start_epoch: u64,
) -> ValidationRequest {
    Arc::new(ValidationPool::new())
        .acquire(ValidationTicket {
            request_id,
            start_epoch,
            start_boundary_epoch: 1,
        })
        .expect("test validation slot")
        .0
}

impl Default for TargetRegistry {
    fn default() -> Self {
        Self::new()
    }
}
pub(super) const fn slot(generation: ActivationGeneration) -> usize {
    generation.get() as usize % TARGET_REGISTRY_CAPACITY
}

pub(super) fn token_for(
    epoch: ProcessEpoch,
    generation: ActivationGeneration,
) -> NativeTargetToken {
    const PREFIX: &[u8] = b"mac-v8:";
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; 56];
    bytes[..PREFIX.len()].copy_from_slice(PREFIX);
    let mut index = 0;
    while index < epoch.0.len() {
        bytes[PREFIX.len() + index * 2] = HEX[(epoch.0[index] >> 4) as usize];
        bytes[PREFIX.len() + index * 2 + 1] = HEX[(epoch.0[index] & 0x0f) as usize];
        index += 1;
    }
    let separator = PREFIX.len() + 32;
    bytes[separator] = b':';
    let mut nibble = 0;
    while nibble < 16 {
        let shift = (15 - nibble) * 4;
        bytes[separator + 1 + nibble] = HEX[((generation.get() >> shift) & 0xF) as usize];
        nibble += 1;
    }
    let value = std::str::from_utf8(&bytes).expect("token alphabet is ASCII");
    NativeTargetToken::new(value).expect("fixed macOS target token is bounded")
}
