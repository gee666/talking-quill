//! Bounded recovery journal storage; all buffers stay owner-run-loop local.

use super::*;

pub(super) const RECOVERY_PHYSICAL_SOURCE_SLOTS: usize = 2;
pub(super) const RECOVERY_EXTERNAL_SOURCE_SLOTS: usize = 8;
pub(super) const RECOVERY_SOURCE_SLOTS: usize =
    RECOVERY_PHYSICAL_SOURCE_SLOTS + RECOVERY_EXTERNAL_SOURCE_SLOTS;
pub(super) const OVERFLOW_BALANCE_CAPACITY: usize = RECOVERY_SOURCE_SLOTS * (128 + 32);

pub(super) struct RecoveryEdgeJournal {
    pub(super) pending: [injection::DeferredEvent; injection::DEFERRED_EDGE_CAPACITY],
    pub(super) pending_len: usize,
    pub(super) tail: [injection::DeferredEvent; injection::DEFERRED_EDGE_CAPACITY],
    pub(super) tail_len: usize,
    pub(super) overflow_balances: [injection::DeferredEvent; OVERFLOW_BALANCE_CAPACITY],
    pub(super) overflow_balance_head: usize,
    pub(super) overflow_balance_len: usize,
    pub(super) submitted_len: usize,
    pub(super) observed: usize,
    pub(super) token: Option<injection::OperationToken>,
    pub(super) submission_deadline: Option<Instant>,
    pub(super) external_collection_deadline: Option<Instant>,
    pub(super) next_pool_bank: usize,
    pub(super) overflow: bool,
    pub(super) external_source_pids: [i64; RECOVERY_EXTERNAL_SOURCE_SLOTS],
    pub(super) modifier_bits: [u8; RECOVERY_SOURCE_SLOTS],
    pub(super) modifier_known: [u8; RECOVERY_SOURCE_SLOTS],
    pub(super) held_keys: [[bool; 128]; RECOVERY_SOURCE_SLOTS],
    pub(super) key_origins: [[u8; 128]; RECOVERY_SOURCE_SLOTS],
    pub(super) key_generations: [[u32; 128]; RECOVERY_SOURCE_SLOTS],
    pub(super) owned_release_generation: [u32; 128],
    pub(super) held_mouse_buttons: [u32; RECOVERY_SOURCE_SLOTS],
    pub(super) mouse_foreground_origins: [u32; RECOVERY_SOURCE_SLOTS],
    pub(super) exposed_keys: [[bool; 128]; RECOVERY_SOURCE_SLOTS],
    pub(super) exposed_mouse_buttons: [u32; RECOVERY_SOURCE_SLOTS],
    pub(super) discard_key_fences: [[bool; 128]; RECOVERY_SOURCE_SLOTS],
    pub(super) discard_mouse_fences: [u32; RECOVERY_SOURCE_SLOTS],
}

impl Default for RecoveryEdgeJournal {
    fn default() -> Self {
        Self {
            pending: [injection::DeferredEvent::EMPTY; injection::DEFERRED_EDGE_CAPACITY],
            pending_len: 0,
            tail: [injection::DeferredEvent::EMPTY; injection::DEFERRED_EDGE_CAPACITY],
            tail_len: 0,
            overflow_balances: [injection::DeferredEvent::EMPTY; OVERFLOW_BALANCE_CAPACITY],
            overflow_balance_head: 0,
            overflow_balance_len: 0,
            submitted_len: 0,
            observed: 0,
            token: None,
            submission_deadline: None,
            external_collection_deadline: None,
            next_pool_bank: 0,
            overflow: false,
            external_source_pids: [0; RECOVERY_EXTERNAL_SOURCE_SLOTS],
            modifier_bits: [0; RECOVERY_SOURCE_SLOTS],
            modifier_known: [0; RECOVERY_SOURCE_SLOTS],
            held_keys: [[false; 128]; RECOVERY_SOURCE_SLOTS],
            key_origins: [[0; 128]; RECOVERY_SOURCE_SLOTS],
            key_generations: [[0; 128]; RECOVERY_SOURCE_SLOTS],
            owned_release_generation: [0; 128],
            held_mouse_buttons: [0; RECOVERY_SOURCE_SLOTS],
            mouse_foreground_origins: [0; RECOVERY_SOURCE_SLOTS],
            exposed_keys: [[false; 128]; RECOVERY_SOURCE_SLOTS],
            exposed_mouse_buttons: [0; RECOVERY_SOURCE_SLOTS],
            discard_key_fences: [[false; 128]; RECOVERY_SOURCE_SLOTS],
            discard_mouse_fences: [0; RECOVERY_SOURCE_SLOTS],
        }
    }
}
