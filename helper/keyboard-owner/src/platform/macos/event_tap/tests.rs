use super::*;
use crate::platform::macos::{OwnerMutation, owner_command_state};
use talking_quill_keyboard_core::{
    ActivationBinding, ActivationBindings, ActivationContext, ActivationGeneration, EventPhase,
    ProfileId, Shortcut, ShortcutModifiers,
    transactional::{EventJournal, JOURNAL_CAPACITY},
};

mod support_context;
use support_context::*;
mod support_native_events;
use support_native_events::*;
mod support_transactions;
use support_transactions::*;

mod activation_dispatch;
mod activation_unwind;
mod activation_validation;
mod binding_revisions;
mod candidate_ordering;
mod delivery_failure;
mod exact_observation;
mod external_ordering;
mod fresh_generation;
mod gap_tombstones;
mod insertion_safety;
mod lossless_repost;
mod modifier_sources;
mod ordered_shortcuts;
mod overflow;
mod owned_release;
mod owner_commands;
mod paste_barrier;
mod paste_safety;
mod physical;
mod policy_fences;
mod process_gate;
mod recovery_observation;
mod recovery_ordering;
mod replay_unwind;
mod resource_lifetime;
mod session_capture;
mod session_recovery;
mod shutdown;
mod source_retention;
mod target_replay;
mod terminal_replay;
mod unwind_authority;
