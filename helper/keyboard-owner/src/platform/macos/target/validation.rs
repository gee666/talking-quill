//! Bounded generation-tagged validation slots and scalar replies.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::platform::macos) struct ValidationTicket {
    pub(super) request_id: u64,
    pub(super) start_epoch: u64,
    pub(super) start_boundary_epoch: u64,
}

impl ValidationTicket {
    pub(in crate::platform::macos) const fn start_epoch(self) -> u64 {
        self.start_epoch
    }

    pub(in crate::platform::macos) const fn start_boundary_epoch(self) -> u64 {
        self.start_boundary_epoch
    }
}

pub(super) const VALIDATION_SLOT_FREE: u64 = 0;
pub(super) const VALIDATION_SLOT_PENDING: u64 = 1;
pub(super) const VALIDATION_SLOT_READY: u64 = 2;
pub(super) const VALIDATION_SLOT_STATE_BITS: u32 = 2;
pub(super) const VALIDATION_SLOT_STATE_MASK: u64 = (1 << VALIDATION_SLOT_STATE_BITS) - 1;
pub(super) const VALIDATION_SLOT_MAX_GENERATION: u64 = u64::MAX >> VALIDATION_SLOT_STATE_BITS;

pub(super) fn validation_slot_word(generation: u64, state: u64) -> u64 {
    (generation << VALIDATION_SLOT_STATE_BITS) | state
}

pub(super) fn validation_slot_generation(word: u64) -> u64 {
    word >> VALIDATION_SLOT_STATE_BITS
}

pub(super) fn validation_slot_state(word: u64) -> u64 {
    word & VALIDATION_SLOT_STATE_MASK
}

pub(super) struct ValidationSlot {
    pub(super) word: AtomicU64,
    pub(super) response: Mutex<Option<ValidationResponse>>,
}

impl ValidationSlot {
    pub(super) fn new() -> Self {
        Self {
            word: AtomicU64::new(validation_slot_word(0, VALIDATION_SLOT_FREE)),
            response: Mutex::new(None),
        }
    }
}

pub(super) struct ValidationPool {
    pub(super) slots: [ValidationSlot; VALIDATION_QUEUE_CAPACITY],
    pub(super) stopped: AtomicBool,
}

impl ValidationPool {
    pub(super) fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| ValidationSlot::new()),
            stopped: AtomicBool::new(false),
        }
    }

    #[cfg(test)]
    pub(super) fn acquire(
        self: &Arc<Self>,
        ticket: ValidationTicket,
    ) -> Option<(ValidationRequest, ValidationWork)> {
        self.acquire_expected(ticket, 0)
    }

    pub(super) fn acquire_expected(
        self: &Arc<Self>,
        ticket: ValidationTicket,
        expected_publication_id: u64,
    ) -> Option<(ValidationRequest, ValidationWork)> {
        if self.stopped.load(Ordering::Acquire) {
            return None;
        }
        for (index, slot) in self.slots.iter().enumerate() {
            let current = slot.word.load(Ordering::Acquire);
            if validation_slot_state(current) != VALIDATION_SLOT_FREE {
                continue;
            }
            let generation = validation_slot_generation(current);
            if generation == VALIDATION_SLOT_MAX_GENERATION {
                continue;
            }
            let generation = generation + 1;
            if slot
                .word
                .compare_exchange(
                    current,
                    validation_slot_word(generation, VALIDATION_SLOT_PENDING),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                if self.stopped.load(Ordering::Acquire) {
                    let _ = slot.word.compare_exchange(
                        validation_slot_word(generation, VALIDATION_SLOT_PENDING),
                        validation_slot_word(generation, VALIDATION_SLOT_FREE),
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    );
                    return None;
                }
                let index = u8::try_from(index).expect("validation slot capacity fits u8");
                return Some((
                    ValidationRequest {
                        ticket,
                        slot_index: index,
                        slot_generation: generation,
                        pool: Arc::clone(self),
                        active: true,
                    },
                    ValidationWork {
                        ticket,
                        slot_index: index,
                        slot_generation: generation,
                        expected_publication_id,
                    },
                ));
            }
        }
        None
    }

    pub(super) fn is_pending(&self, work: ValidationWork) -> bool {
        if self.stopped.load(Ordering::Acquire) {
            return false;
        }
        self.slots[usize::from(work.slot_index)]
            .word
            .load(Ordering::Acquire)
            == validation_slot_word(work.slot_generation, VALIDATION_SLOT_PENDING)
    }

    pub(super) fn publish(&self, work: ValidationWork, response: ValidationResponse) -> bool {
        if self.stopped.load(Ordering::Acquire) {
            return false;
        }
        let slot = &self.slots[usize::from(work.slot_index)];
        let pending = validation_slot_word(work.slot_generation, VALIDATION_SLOT_PENDING);
        if slot.word.load(Ordering::Acquire) != pending {
            return false;
        }
        let mut stored = slot
            .response
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.word.load(Ordering::Acquire) != pending {
            return false;
        }
        *stored = Some(response);
        slot.word
            .compare_exchange(
                pending,
                validation_slot_word(work.slot_generation, VALIDATION_SLOT_READY),
                Ordering::Release,
                Ordering::Acquire,
            )
            .is_ok()
    }

    pub(super) fn release(&self, slot_index: u8, slot_generation: u64) {
        let slot = &self.slots[usize::from(slot_index)];
        for state in [VALIDATION_SLOT_PENDING, VALIDATION_SLOT_READY] {
            if slot
                .word
                .compare_exchange(
                    validation_slot_word(slot_generation, state),
                    validation_slot_word(slot_generation, VALIDATION_SLOT_FREE),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return;
            }
        }
    }

    pub(super) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }
}

pub(in crate::platform::macos) struct ValidationRequest {
    pub(super) ticket: ValidationTicket,
    pub(super) slot_index: u8,
    pub(super) slot_generation: u64,
    pub(super) pool: Arc<ValidationPool>,
    pub(super) active: bool,
}

impl ValidationRequest {
    pub(in crate::platform::macos) const fn ticket(&self) -> ValidationTicket {
        self.ticket
    }

    pub(in crate::platform::macos) fn try_response(&mut self) -> Option<ValidationResponse> {
        if !self.active {
            return None;
        }
        let slot = &self.pool.slots[usize::from(self.slot_index)];
        let ready = validation_slot_word(self.slot_generation, VALIDATION_SLOT_READY);
        if slot.word.load(Ordering::Acquire) != ready {
            return None;
        }
        let mut stored = match slot.response.try_lock() {
            Ok(stored) => stored,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        if slot.word.load(Ordering::Acquire) != ready {
            return None;
        }
        let response = stored.take()?;
        if slot
            .word
            .compare_exchange(
                ready,
                validation_slot_word(self.slot_generation, VALIDATION_SLOT_FREE),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            *stored = Some(response);
            return None;
        }
        self.active = false;
        Some(response)
    }
}

impl Drop for ValidationRequest {
    fn drop(&mut self) {
        if self.active {
            self.pool.release(self.slot_index, self.slot_generation);
            self.active = false;
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct ValidationWork {
    pub(super) ticket: ValidationTicket,
    pub(super) slot_index: u8,
    pub(super) slot_generation: u64,
    pub(super) expected_publication_id: u64,
}

pub(in crate::platform::macos) struct ValidationResponse {
    pub(super) ticket: ValidationTicket,
    pub(super) handle: Option<TargetHandle>,
    pub(super) observed_epoch: u64,
    pub(super) observed_boundary_epoch: u64,
    pub(super) observed_selected_range_epoch: u64,
}

impl ValidationResponse {
    pub(in crate::platform::macos) fn into_current_handle(
        self,
        expected: ValidationTicket,
        current_epoch: u64,
        current_boundary_epoch: u64,
        current_selected_range_epoch: u64,
    ) -> Option<(TargetHandle, u64, u64, u64, bool)> {
        (self.ticket == expected
            && self.ticket.start_epoch == self.observed_epoch
            && self.observed_epoch == current_epoch
            && self.ticket.start_boundary_epoch == self.observed_boundary_epoch
            && self.observed_boundary_epoch == current_boundary_epoch
            && self.observed_selected_range_epoch == current_selected_range_epoch)
            .then_some(self.handle.map(|handle| {
                (
                    handle,
                    self.observed_epoch,
                    self.observed_boundary_epoch,
                    self.observed_selected_range_epoch,
                    true,
                )
            }))
            .flatten()
    }
}
