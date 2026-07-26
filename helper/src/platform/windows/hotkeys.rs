use std::{
    ptr::null_mut,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
};

use crossbeam_channel::Sender;
use windows_sys::Win32::{
    Foundation::LPARAM,
    UI::Input::KeyboardAndMouse::{
        MOD_ALT, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey, UnregisterHotKey,
    },
};

use crate::{
    keyboard::{ActivationBindings, ActivationKey},
    platform::PlatformError,
};

const HOTKEY_BANK_A_BASE: i32 = 0x4D00;
const HOTKEY_BANK_B_BASE: i32 = 0x4E00;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ActivationConfig {
    pub(super) enabled: bool,
    pub(super) bindings: ActivationBindings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RegistrationSlot {
    A,
    B,
}
impl RegistrationSlot {
    const fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
    const fn id(self, key: ActivationKey, shift: bool) -> i32 {
        let base = match self {
            Self::A => HOTKEY_BANK_A_BASE,
            Self::B => HOTKEY_BANK_B_BASE,
        };
        base + key.index() as i32 * 2 + shift as i32 + 1
    }
}
fn registration_from_id(id: i32) -> Option<(RegistrationSlot, ActivationKey, bool)> {
    let (slot, offset) = if id > HOTKEY_BANK_A_BASE && id <= HOTKEY_BANK_A_BASE + 52 {
        (RegistrationSlot::A, id - HOTKEY_BANK_A_BASE - 1)
    } else if id > HOTKEY_BANK_B_BASE && id <= HOTKEY_BANK_B_BASE + 52 {
        (RegistrationSlot::B, id - HOTKEY_BANK_B_BASE - 1)
    } else {
        return None;
    };
    Some((
        slot,
        ActivationKey::from_index((offset / 2) as u8)?,
        offset % 2 == 1,
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct HotKeyRegistrationState {
    pub(super) config: ActivationConfig,
    /// Bank preferred for the next newly-added binding. Retained bindings may
    /// remain in either bank so Windows never sees the same chord under two IDs.
    slot: RegistrationSlot,
    bank_b: ActivationBindings,
    pub(super) generation: u64,
}
impl HotKeyRegistrationState {
    const fn active_bindings(self) -> ActivationBindings {
        if self.config.enabled {
            self.config.bindings
        } else {
            ActivationBindings::from_bits(0)
        }
    }

    const fn slot_for(self, key: ActivationKey, shift: bool) -> Option<RegistrationSlot> {
        if !self.config.enabled || !self.config.bindings.contains(key, shift) {
            None
        } else if self.bank_b.contains(key, shift) {
            Some(RegistrationSlot::B)
        } else {
            Some(RegistrationSlot::A)
        }
    }
}
impl Default for HotKeyRegistrationState {
    fn default() -> Self {
        Self {
            config: ActivationConfig::default(),
            slot: RegistrationSlot::A,
            bank_b: ActivationBindings::default(),
            generation: 0,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HotKeyTransactionError {
    Conflict,
    Cancelled,
    RollbackFailed,
}
pub(super) trait HotKeyRegistrar {
    fn register(&mut self, id: i32, modifiers: u32, virtual_key: u32) -> bool;
    fn unregister(&mut self, id: i32) -> bool;
}
pub(super) struct WindowsHotKeyRegistrar;
impl HotKeyRegistrar for WindowsHotKeyRegistrar {
    fn register(&mut self, id: i32, modifiers: u32, virtual_key: u32) -> bool {
        unsafe { RegisterHotKey(null_mut(), id, modifiers, virtual_key) != 0 }
    }
    fn unregister(&mut self, id: i32) -> bool {
        unsafe { UnregisterHotKey(null_mut(), id) != 0 }
    }
}
const fn activation_virtual_key(key: ActivationKey) -> u32 {
    0x41 + key.index() as u32
}
const fn hotkey_modifiers(shift: bool) -> u32 {
    MOD_ALT | MOD_NOREPEAT | if shift { MOD_SHIFT } else { 0 }
}
fn unregister_set(
    registrar: &mut impl HotKeyRegistrar,
    slot: RegistrationSlot,
    bindings: ActivationBindings,
) -> bool {
    bindings.iter().fold(true, |ok, (key, shift)| {
        registrar.unregister(slot.id(key, shift)) && ok
    })
}
fn register_set(
    registrar: &mut impl HotKeyRegistrar,
    slot: RegistrationSlot,
    bindings: ActivationBindings,
) -> Result<(), HotKeyTransactionError> {
    let mut registered = ActivationBindings::default();
    for (key, shift) in bindings.iter() {
        if !registrar.register(
            slot.id(key, shift),
            hotkey_modifiers(shift),
            activation_virtual_key(key),
        ) {
            if !unregister_set(registrar, slot, registered) {
                return Err(HotKeyTransactionError::RollbackFailed);
            }
            return Err(HotKeyTransactionError::Conflict);
        }
        registered = ActivationBindings::from_exact(
            &registered.iter().chain([(key, shift)]).collect::<Vec<_>>(),
        )
        .expect("bounded set");
    }
    Ok(())
}
pub(super) fn configure_hotkeys(
    current: HotKeyRegistrationState,
    requested: ActivationConfig,
    registrar: &mut impl HotKeyRegistrar,
    commit_allowed: impl FnOnce() -> bool,
) -> Result<HotKeyRegistrationState, HotKeyTransactionError> {
    if current.config == requested {
        return Ok(current);
    }
    let current_active = current.active_bindings();
    let requested_active = if requested.enabled {
        requested.bindings
    } else {
        ActivationBindings::default()
    };
    let retained = ActivationBindings::from_bits(current_active.bits() & requested_active.bits());
    let added = ActivationBindings::from_bits(requested_active.bits() & !current_active.bits());
    let removed = ActivationBindings::from_bits(current_active.bits() & !requested_active.bits());
    let candidate = current.slot.other();

    // Register only genuinely new chords. RegisterHotKey rejects a retained
    // chord if it is registered again under an ID in the other bank.
    register_set(registrar, candidate, added)?;
    if !commit_allowed() {
        if !unregister_set(registrar, candidate, added) {
            return Err(HotKeyTransactionError::RollbackFailed);
        }
        return Err(HotKeyTransactionError::Cancelled);
    }

    let mut removed_a = ActivationBindings::default();
    let mut removed_b = ActivationBindings::default();
    for (key, shift) in removed.iter() {
        let slot = current
            .slot_for(key, shift)
            .expect("removed binding was active");
        if !registrar.unregister(slot.id(key, shift)) {
            let additions_removed = unregister_set(registrar, candidate, added);
            let restored_a = register_set(registrar, RegistrationSlot::A, removed_a).is_ok();
            let restored_b = register_set(registrar, RegistrationSlot::B, removed_b).is_ok();
            return if additions_removed && restored_a && restored_b {
                Err(HotKeyTransactionError::Conflict)
            } else {
                Err(HotKeyTransactionError::RollbackFailed)
            };
        }
        let removed_from_slot = if slot == RegistrationSlot::B {
            &mut removed_b
        } else {
            &mut removed_a
        };
        *removed_from_slot = ActivationBindings::from_bits(
            removed_from_slot.bits() | bindings_bit(key, shift).bits(),
        );
    }

    let retained_b = ActivationBindings::from_bits(current.bank_b.bits() & retained.bits());
    let bank_b = if candidate == RegistrationSlot::B {
        ActivationBindings::from_bits(retained_b.bits() | added.bits())
    } else {
        retained_b
    };
    Ok(HotKeyRegistrationState {
        config: requested,
        slot: if added.bits() == 0 {
            current.slot
        } else {
            candidate
        },
        bank_b,
        generation: current.generation.wrapping_add(1),
    })
}

fn bindings_bit(key: ActivationKey, shift: bool) -> ActivationBindings {
    ActivationBindings::from_exact(&[(key, shift)]).expect("one binding is bounded")
}
pub(super) fn unregister_all_hotkeys(registrar: &mut impl HotKeyRegistrar) {
    for slot in [RegistrationSlot::A, RegistrationSlot::B] {
        for index in 0_u8..26 {
            let key = ActivationKey::from_index(index).unwrap();
            let _ = registrar.unregister(slot.id(key, false));
            let _ = registrar.unregister(slot.id(key, true));
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ActivationCandidate {
    generation: u64,
    slot: RegistrationSlot,
    pub(super) key: ActivationKey,
    pub(super) shift: bool,
}
#[cfg(test)]
impl ActivationCandidate {
    const fn id(self) -> i32 {
        self.slot.id(self.key, self.shift)
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum ActivationCandidateState {
    #[default]
    Idle,
    Pressed(ActivationCandidate),
    ReleasedBeforeMessage(ActivationCandidate),
    Accepted(ActivationCandidate),
    ConsumedDown(ActivationCandidate),
    Completed(ActivationCandidate),
}
impl ActivationCandidateState {
    pub(super) const fn blocks_configuration(self) -> bool {
        matches!(
            self,
            Self::Pressed(_)
                | Self::ReleasedBeforeMessage(_)
                | Self::Accepted(_)
                | Self::ConsumedDown(_)
        )
    }
    pub(super) const fn candidate(self) -> Option<ActivationCandidate> {
        match self {
            Self::Idle => None,
            Self::Pressed(v)
            | Self::ReleasedBeforeMessage(v)
            | Self::Accepted(v)
            | Self::ConsumedDown(v)
            | Self::Completed(v) => Some(v),
        }
    }
}
#[derive(Debug, Default)]
pub(super) struct HotKeyRuntime {
    pub(super) registrations: HotKeyRegistrationState,
    pub(super) candidate: ActivationCandidateState,
}
pub(super) fn candidate_for_exact_passive_down(
    registrations: HotKeyRegistrationState,
    current: ActivationCandidateState,
    key: ActivationKey,
    alt: bool,
    shift: bool,
    disallowed_modifiers: bool,
    repeat: bool,
) -> ActivationCandidateState {
    // The hook's tracked Shift state identifies the registered physical
    // candidate. WM_HOTKEY remains authoritative for the exact variant Windows
    // recognized, guarding the boundary if native modifier state ever differs.
    let observed_shift = registrations
        .config
        .bindings
        .contains(key, shift)
        .then_some(shift);
    if !registrations.config.enabled
        || observed_shift.is_none()
        || !alt
        || disallowed_modifiers
        || repeat
    {
        return current;
    }
    match current {
        ActivationCandidateState::Idle | ActivationCandidateState::Completed(_) => {
            let observed_shift = observed_shift.expect("candidate binding was configured");
            ActivationCandidateState::Pressed(ActivationCandidate {
                generation: registrations.generation,
                slot: registrations
                    .slot_for(key, observed_shift)
                    .expect("candidate binding was configured"),
                key,
                shift: observed_shift,
            })
        }
        _ => current,
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CandidateUpAction {
    Pass,
    SwallowAwaitMessage(ActivationCandidate),
    SwallowConsumed(ActivationCandidate),
    BalanceAccepted(ActivationCandidate),
}
pub(super) fn release_activation_candidate(
    state: ActivationCandidateState,
    key: ActivationKey,
) -> (ActivationCandidateState, CandidateUpAction) {
    match state {
        ActivationCandidateState::Pressed(c) if c.key == key => (
            ActivationCandidateState::ReleasedBeforeMessage(c),
            CandidateUpAction::SwallowAwaitMessage(c),
        ),
        ActivationCandidateState::Accepted(c) if c.key == key => {
            (state, CandidateUpAction::BalanceAccepted(c))
        }
        ActivationCandidateState::ConsumedDown(c) if c.key == key => (
            ActivationCandidateState::Completed(c),
            CandidateUpAction::SwallowConsumed(c),
        ),
        _ => (state, CandidateUpAction::Pass),
    }
}
pub(super) fn resolve_activation_candidate_message(
    candidate: ActivationCandidate,
    registrations: HotKeyRegistrationState,
    id: i32,
    l_param: LPARAM,
) -> Option<ActivationCandidate> {
    if candidate.generation != registrations.generation {
        return None;
    }
    let (key, shift) = hotkey_message_activation(registrations, id, l_param)?;
    if key != candidate.key {
        return None;
    }
    Some(ActivationCandidate {
        generation: candidate.generation,
        slot: registrations.slot_for(key, shift)?,
        key,
        shift,
    })
}
#[cfg(test)]
fn candidate_matches_message(
    candidate: ActivationCandidate,
    registrations: HotKeyRegistrationState,
    id: i32,
    l_param: LPARAM,
) -> bool {
    resolve_activation_candidate_message(candidate, registrations, id, l_param) == Some(candidate)
}

fn hotkey_message_activation(
    state: HotKeyRegistrationState,
    id: i32,
    l_param: LPARAM,
) -> Option<(ActivationKey, bool)> {
    if !state.config.enabled {
        return None;
    }
    let (slot, key, shift) = registration_from_id(id)?;
    if state.slot_for(key, shift) != Some(slot) {
        return None;
    }
    let value = l_param as u32;
    let expected = MOD_ALT | if shift { MOD_SHIFT } else { 0 };
    ((value & 0xFFFF) == expected && (value >> 16) == activation_virtual_key(key))
        .then_some((key, shift))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(super) enum ConfigurationHandoff {
    Pending,
    Cancelled,
    Committed,
    RollbackConfirmed,
    TerminalRequested,
    TerminalComplete,
}

pub(super) fn configuration_handoff(value: &AtomicU8) -> ConfigurationHandoff {
    match value.load(Ordering::Acquire) {
        1 => ConfigurationHandoff::Cancelled,
        2 => ConfigurationHandoff::Committed,
        3 => ConfigurationHandoff::RollbackConfirmed,
        4 => ConfigurationHandoff::TerminalRequested,
        5 => ConfigurationHandoff::TerminalComplete,
        _ => ConfigurationHandoff::Pending,
    }
}

fn transition_configuration(
    value: &AtomicU8,
    from: ConfigurationHandoff,
    to: ConfigurationHandoff,
) -> ConfigurationHandoff {
    match value.compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => to,
        Err(_) => configuration_handoff(value),
    }
}

pub(super) fn cancel_configuration(value: &AtomicU8) -> ConfigurationHandoff {
    transition_configuration(
        value,
        ConfigurationHandoff::Pending,
        ConfigurationHandoff::Cancelled,
    )
}

pub(super) fn commit_configuration(value: &AtomicU8) -> bool {
    transition_configuration(
        value,
        ConfigurationHandoff::Pending,
        ConfigurationHandoff::Committed,
    ) == ConfigurationHandoff::Committed
}

pub(super) fn confirm_configuration_rollback(value: &AtomicU8) -> ConfigurationHandoff {
    transition_configuration(
        value,
        ConfigurationHandoff::Cancelled,
        ConfigurationHandoff::RollbackConfirmed,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ConfigurationTimeoutAction {
    Success,
    Failed,
    WakeTerminalCleanup,
}

pub(super) fn configuration_ack_timeout(value: &AtomicU8) -> ConfigurationTimeoutAction {
    match cancel_configuration(value) {
        ConfigurationHandoff::Committed => ConfigurationTimeoutAction::Success,
        ConfigurationHandoff::RollbackConfirmed | ConfigurationHandoff::TerminalComplete => {
            ConfigurationTimeoutAction::Failed
        }
        ConfigurationHandoff::Cancelled => match transition_configuration(
            value,
            ConfigurationHandoff::Cancelled,
            ConfigurationHandoff::TerminalRequested,
        ) {
            ConfigurationHandoff::Committed => ConfigurationTimeoutAction::Success,
            ConfigurationHandoff::RollbackConfirmed | ConfigurationHandoff::TerminalComplete => {
                ConfigurationTimeoutAction::Failed
            }
            ConfigurationHandoff::TerminalRequested => {
                ConfigurationTimeoutAction::WakeTerminalCleanup
            }
            ConfigurationHandoff::Pending | ConfigurationHandoff::Cancelled => {
                ConfigurationTimeoutAction::Failed
            }
        },
        ConfigurationHandoff::Pending | ConfigurationHandoff::TerminalRequested => {
            ConfigurationTimeoutAction::Failed
        }
    }
}

pub(super) struct ActivationCommand {
    pub(super) requested: ActivationConfig,
    pub(super) handoff: Arc<AtomicU8>,
    pub(super) acknowledgement: Sender<Result<(), PlatformError>>,
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::keyboard::{KeyInput, KeyPhase, KeyboardReducer, PhysicalKey};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum RegistrarOperation {
        Register(i32, u32, u32),
        Unregister(i32),
    }

    #[derive(Default)]
    struct FakeRegistrar {
        active: BTreeMap<i32, (u32, u32)>,
        fail_register_once: BTreeSet<i32>,
        fail_unregister_once: BTreeSet<i32>,
        operations: Vec<RegistrarOperation>,
    }

    impl HotKeyRegistrar for FakeRegistrar {
        fn register(&mut self, id: i32, modifiers: u32, virtual_key: u32) -> bool {
            self.operations
                .push(RegistrarOperation::Register(id, modifiers, virtual_key));
            if self.fail_register_once.remove(&id)
                || self.active.contains_key(&id)
                || self
                    .active
                    .values()
                    .any(|chord| *chord == (modifiers, virtual_key))
            {
                return false;
            }
            self.active.insert(id, (modifiers, virtual_key));
            true
        }

        fn unregister(&mut self, id: i32) -> bool {
            self.operations.push(RegistrarOperation::Unregister(id));
            if self.fail_unregister_once.remove(&id) {
                return false;
            }
            self.active.remove(&id).is_some()
        }
    }

    fn bindings(values: &[(ActivationKey, bool)]) -> ActivationBindings {
        ActivationBindings::from_exact(values).unwrap()
    }

    fn config(enabled: bool, key: ActivationKey) -> ActivationConfig {
        ActivationConfig {
            enabled,
            bindings: bindings(&[(key, false), (key, true)]),
        }
    }

    fn hotkey_l_param(modifiers: u32, key: ActivationKey) -> LPARAM {
        ((activation_virtual_key(key) << 16) | modifiers) as LPARAM
    }

    #[test]
    fn hotkey_registration_transaction_covers_lifecycle_and_alternating_ids() {
        let mut registrar = FakeRegistrar::default();
        let disabled = HotKeyRegistrationState::default();
        assert_eq!(disabled.config, ActivationConfig::default());
        assert!(registrar.active.is_empty());

        let enabled = configure_hotkeys(
            disabled,
            config(true, ActivationKey::A),
            &mut registrar,
            || true,
        )
        .unwrap();
        assert_eq!(enabled.slot, RegistrationSlot::B);
        assert_eq!(enabled.config, config(true, ActivationKey::A));
        assert_eq!(
            registrar
                .active
                .get(&RegistrationSlot::B.id(ActivationKey::A, false)),
            Some(&(
                hotkey_modifiers(false),
                activation_virtual_key(ActivationKey::A)
            ))
        );
        assert_eq!(
            registrar
                .active
                .get(&RegistrationSlot::B.id(ActivationKey::A, true)),
            Some(&(
                hotkey_modifiers(true),
                activation_virtual_key(ActivationKey::A)
            ))
        );
        assert!(hotkey_modifiers(false) & MOD_NOREPEAT != 0);
        assert!(hotkey_modifiers(true) & MOD_NOREPEAT != 0);

        let operation_count = registrar.operations.len();
        assert_eq!(
            configure_hotkeys(
                enabled,
                config(true, ActivationKey::A),
                &mut registrar,
                || true,
            ),
            Ok(enabled)
        );
        assert_eq!(registrar.operations.len(), operation_count);

        let changed = configure_hotkeys(
            enabled,
            config(true, ActivationKey::B),
            &mut registrar,
            || true,
        )
        .unwrap();
        assert_eq!(changed.slot, RegistrationSlot::A);
        assert_eq!(changed.config, config(true, ActivationKey::B));
        assert_eq!(registrar.active.len(), 2);
        assert!(
            registrar
                .active
                .contains_key(&RegistrationSlot::A.id(ActivationKey::B, false))
        );
        assert!(
            registrar
                .active
                .contains_key(&RegistrationSlot::A.id(ActivationKey::B, true))
        );

        let disabled_again = configure_hotkeys(
            changed,
            config(false, ActivationKey::C),
            &mut registrar,
            || true,
        )
        .unwrap();
        assert_eq!(disabled_again.config, config(false, ActivationKey::C));
        assert!(registrar.active.is_empty());

        let reenabled = configure_hotkeys(
            disabled_again,
            config(true, ActivationKey::D),
            &mut registrar,
            || true,
        )
        .unwrap();
        assert_eq!(reenabled.slot, RegistrationSlot::B);
        unregister_all_hotkeys(&mut registrar);
        assert!(registrar.active.is_empty());
    }

    #[test]
    fn partial_registration_conflict_and_cancellation_keep_previous_pair() {
        let mut registrar = FakeRegistrar::default();
        let current = configure_hotkeys(
            HotKeyRegistrationState::default(),
            config(true, ActivationKey::A),
            &mut registrar,
            || true,
        )
        .unwrap();
        let previous_active = registrar.active.clone();

        registrar
            .fail_register_once
            .insert(RegistrationSlot::A.id(ActivationKey::B, true));
        assert_eq!(
            configure_hotkeys(
                current,
                config(true, ActivationKey::B),
                &mut registrar,
                || true,
            ),
            Err(HotKeyTransactionError::Conflict)
        );
        assert_eq!(registrar.active, previous_active);

        assert_eq!(
            configure_hotkeys(
                current,
                config(true, ActivationKey::C),
                &mut registrar,
                || false,
            ),
            Err(HotKeyTransactionError::Cancelled)
        );
        assert_eq!(registrar.active, previous_active);
    }

    #[test]
    fn hotkey_id_mapping_and_message_policy_are_exact() {
        for (id, expected) in [
            (
                RegistrationSlot::A.id(ActivationKey::A, false),
                Some((RegistrationSlot::A, ActivationKey::A, false)),
            ),
            (
                RegistrationSlot::A.id(ActivationKey::A, true),
                Some((RegistrationSlot::A, ActivationKey::A, true)),
            ),
            (
                RegistrationSlot::B.id(ActivationKey::A, false),
                Some((RegistrationSlot::B, ActivationKey::A, false)),
            ),
            (
                RegistrationSlot::B.id(ActivationKey::A, true),
                Some((RegistrationSlot::B, ActivationKey::A, true)),
            ),
            (0, None),
        ] {
            assert_eq!(registration_from_id(id), expected);
        }

        let state = HotKeyRegistrationState {
            config: config(true, ActivationKey::A),
            slot: RegistrationSlot::B,
            bank_b: config(true, ActivationKey::A).bindings,
            generation: 1,
        };
        assert_eq!(
            hotkey_message_activation(
                state,
                RegistrationSlot::B.id(ActivationKey::A, false),
                hotkey_l_param(MOD_ALT, ActivationKey::A),
            ),
            Some((ActivationKey::A, false))
        );
        assert_eq!(
            hotkey_message_activation(
                state,
                RegistrationSlot::B.id(ActivationKey::A, true),
                hotkey_l_param(MOD_ALT | MOD_SHIFT, ActivationKey::A),
            ),
            Some((ActivationKey::A, true))
        );

        for (id, l_param) in [
            (
                RegistrationSlot::A.id(ActivationKey::A, false),
                hotkey_l_param(MOD_ALT, ActivationKey::A),
            ),
            (
                RegistrationSlot::B.id(ActivationKey::A, false),
                hotkey_l_param(MOD_ALT | MOD_SHIFT, ActivationKey::A),
            ),
            (
                RegistrationSlot::B.id(ActivationKey::A, true),
                hotkey_l_param(MOD_ALT, ActivationKey::A),
            ),
            (
                RegistrationSlot::B.id(ActivationKey::A, false),
                hotkey_l_param(MOD_ALT | 2, ActivationKey::A),
            ),
            (
                RegistrationSlot::B.id(ActivationKey::A, false),
                hotkey_l_param(MOD_ALT, ActivationKey::B),
            ),
        ] {
            assert_eq!(hotkey_message_activation(state, id, l_param), None);
        }

        let disabled = HotKeyRegistrationState {
            config: config(false, ActivationKey::A),
            slot: RegistrationSlot::B,
            bank_b: ActivationBindings::default(),
            generation: 2,
        };
        assert_eq!(
            hotkey_message_activation(
                disabled,
                RegistrationSlot::B.id(ActivationKey::A, false),
                hotkey_l_param(MOD_ALT, ActivationKey::A),
            ),
            None
        );
    }

    #[test]
    fn candidate_interleavings_preserve_down_then_up_exactly_once() {
        use crate::keyboard::{EventPhase, HelperEvent};

        let registrations = HotKeyRegistrationState {
            config: config(true, ActivationKey::A),
            slot: RegistrationSlot::B,
            bank_b: config(true, ActivationKey::A).bindings,
            generation: 7,
        };
        let pressed = candidate_for_exact_passive_down(
            registrations,
            ActivationCandidateState::Idle,
            ActivationKey::A,
            true,
            false,
            false,
            false,
        );
        let candidate = pressed.candidate().unwrap();
        let (released, action) = release_activation_candidate(pressed, ActivationKey::A);
        assert_eq!(action, CandidateUpAction::SwallowAwaitMessage(candidate));
        assert!(candidate_matches_message(
            candidate,
            registrations,
            RegistrationSlot::B.id(ActivationKey::A, false),
            hotkey_l_param(MOD_ALT, ActivationKey::A),
        ));
        assert!(matches!(
            released,
            ActivationCandidateState::ReleasedBeforeMessage(value) if value == candidate
        ));

        let mut reducer = KeyboardReducer::default();
        let down = reducer.plan_registered_hotkey(ActivationKey::A, false, ActivationKey::A, true);
        assert_eq!(
            down.event(),
            Some(HelperEvent::Activation {
                key: ActivationKey::A,
                phase: EventPhase::Down,
                shift: false,
            })
        );
        assert!(reducer.apply(down, true));
        let up = reducer.plan_passive_hook(
            KeyInput {
                key: PhysicalKey::Letter(ActivationKey::A),
                phase: KeyPhase::Up,
                alt: false,
                shift: false,
                disallowed_modifiers: false,
                repeat: false,
                injected: false,
            },
            ActivationKey::A,
            true,
            false,
        );
        assert_eq!(
            up.event(),
            Some(HelperEvent::Activation {
                key: ActivationKey::A,
                phase: EventPhase::Up,
                shift: false,
            })
        );
        assert!(reducer.apply(up, true));

        // A duplicate or delayed message cannot re-establish the completed
        // sequence; only Pressed/ReleasedBeforeMessage states are accepted.
        let completed = ActivationCandidateState::Completed(candidate);
        assert!(!matches!(
            completed,
            ActivationCandidateState::Pressed(_)
                | ActivationCandidateState::ReleasedBeforeMessage(_)
        ));
    }

    #[test]
    fn candidate_validation_rejects_nonexact_and_stale_sequences() {
        let registrations = HotKeyRegistrationState {
            config: config(true, ActivationKey::A),
            slot: RegistrationSlot::A,
            bank_b: ActivationBindings::default(),
            generation: 11,
        };
        for (key, alt, disallowed, repeat) in [
            (ActivationKey::B, true, false, false),
            (ActivationKey::A, false, false, false),
            (ActivationKey::A, true, true, false),
            (ActivationKey::A, true, false, true),
        ] {
            assert_eq!(
                candidate_for_exact_passive_down(
                    registrations,
                    ActivationCandidateState::Idle,
                    key,
                    alt,
                    false,
                    disallowed,
                    repeat,
                ),
                ActivationCandidateState::Idle
            );
        }

        let pressed = candidate_for_exact_passive_down(
            registrations,
            ActivationCandidateState::Idle,
            ActivationKey::A,
            true,
            true,
            false,
            false,
        );
        let candidate = pressed.candidate().unwrap();
        assert!(candidate.shift);
        assert!(candidate_matches_message(
            candidate,
            registrations,
            RegistrationSlot::A.id(ActivationKey::A, true),
            hotkey_l_param(MOD_ALT | MOD_SHIFT, ActivationKey::A),
        ));
        for stale in [
            HotKeyRegistrationState {
                generation: 12,
                ..registrations
            },
            HotKeyRegistrationState {
                bank_b: registrations.config.bindings,
                ..registrations
            },
            HotKeyRegistrationState {
                config: config(true, ActivationKey::B),
                ..registrations
            },
        ] {
            assert!(!candidate_matches_message(
                candidate,
                stale,
                candidate.id(),
                hotkey_l_param(MOD_ALT | MOD_SHIFT, ActivationKey::A),
            ));
        }
        assert!(!candidate_matches_message(
            candidate,
            registrations,
            RegistrationSlot::A.id(ActivationKey::A, false),
            hotkey_l_param(MOD_ALT, ActivationKey::A),
        ));
    }

    #[test]
    fn candidate_release_balances_consumed_sequences_but_not_other_input() {
        let candidate = ActivationCandidate {
            generation: 3,
            slot: RegistrationSlot::A,
            key: ActivationKey::A,
            shift: false,
        };
        for (state, expected_state, expected_action) in [
            (
                ActivationCandidateState::Accepted(candidate),
                ActivationCandidateState::Accepted(candidate),
                CandidateUpAction::BalanceAccepted(candidate),
            ),
            (
                ActivationCandidateState::ConsumedDown(candidate),
                ActivationCandidateState::Completed(candidate),
                CandidateUpAction::SwallowConsumed(candidate),
            ),
        ] {
            assert_eq!(
                release_activation_candidate(state, ActivationKey::A),
                (expected_state, expected_action)
            );
            assert_eq!(
                release_activation_candidate(state, ActivationKey::B),
                (state, CandidateUpAction::Pass)
            );
        }
    }

    #[test]
    fn configuration_handoff_has_linearizable_commit_and_rollback_paths() {
        let handoff = AtomicU8::new(ConfigurationHandoff::Pending as u8);
        assert!(commit_configuration(&handoff));
        assert_eq!(
            cancel_configuration(&handoff),
            ConfigurationHandoff::Committed
        );

        let handoff = AtomicU8::new(ConfigurationHandoff::Pending as u8);
        assert_eq!(
            cancel_configuration(&handoff),
            ConfigurationHandoff::Cancelled
        );
        assert!(!commit_configuration(&handoff));
        assert_eq!(
            confirm_configuration_rollback(&handoff),
            ConfigurationHandoff::RollbackConfirmed
        );

        let handoff = AtomicU8::new(ConfigurationHandoff::Cancelled as u8);
        assert_eq!(
            transition_configuration(
                &handoff,
                ConfigurationHandoff::Cancelled,
                ConfigurationHandoff::TerminalRequested,
            ),
            ConfigurationHandoff::TerminalRequested
        );
        handoff.store(
            ConfigurationHandoff::TerminalComplete as u8,
            Ordering::Release,
        );
        assert_eq!(
            configuration_handoff(&handoff),
            ConfigurationHandoff::TerminalComplete
        );
    }

    #[test]
    fn configuration_ack_timeout_never_converts_failure_to_success() {
        let pending = AtomicU8::new(ConfigurationHandoff::Pending as u8);
        assert_eq!(
            configuration_ack_timeout(&pending),
            ConfigurationTimeoutAction::WakeTerminalCleanup
        );
        assert_eq!(
            configuration_handoff(&pending),
            ConfigurationHandoff::TerminalRequested
        );

        let committed = AtomicU8::new(ConfigurationHandoff::Committed as u8);
        assert_eq!(
            configuration_ack_timeout(&committed),
            ConfigurationTimeoutAction::Success
        );

        let rolled_back = AtomicU8::new(ConfigurationHandoff::RollbackConfirmed as u8);
        assert_eq!(
            configuration_ack_timeout(&rolled_back),
            ConfigurationTimeoutAction::Failed
        );
    }

    #[test]
    fn multi_binding_bank_registers_and_maps_every_exact_chord() {
        let requested = ActivationConfig {
            enabled: true,
            bindings: bindings(&[
                (ActivationKey::A, false),
                (ActivationKey::A, true),
                (ActivationKey::Q, false),
            ]),
        };
        let mut registrar = FakeRegistrar::default();
        let state = configure_hotkeys(
            HotKeyRegistrationState::default(),
            requested,
            &mut registrar,
            || true,
        )
        .unwrap();
        assert_eq!(registrar.active.len(), 3);
        for (key, shift) in requested.bindings.iter() {
            let id = state.slot.id(key, shift);
            assert_eq!(registration_from_id(id), Some((state.slot, key, shift)));
            assert_eq!(
                hotkey_message_activation(
                    state,
                    id,
                    hotkey_l_param(MOD_ALT | if shift { MOD_SHIFT } else { 0 }, key),
                ),
                Some((key, shift))
            );
        }
    }

    #[test]
    fn overlapping_profile_add_edit_delete_retain_chords_without_duplicate_registration() {
        let mut registrar = FakeRegistrar::default();
        let initial = configure_hotkeys(
            HotKeyRegistrationState::default(),
            ActivationConfig {
                enabled: true,
                bindings: bindings(&[(ActivationKey::A, false)]),
            },
            &mut registrar,
            || true,
        )
        .unwrap();

        let added = configure_hotkeys(
            initial,
            ActivationConfig {
                enabled: true,
                bindings: bindings(&[(ActivationKey::A, false), (ActivationKey::Q, true)]),
            },
            &mut registrar,
            || true,
        )
        .unwrap();
        assert_eq!(registrar.active.len(), 2);
        assert_eq!(
            added.slot_for(ActivationKey::A, false),
            Some(RegistrationSlot::B)
        );
        assert_eq!(
            added.slot_for(ActivationKey::Q, true),
            Some(RegistrationSlot::A)
        );

        let edited = configure_hotkeys(
            added,
            ActivationConfig {
                enabled: true,
                bindings: bindings(&[(ActivationKey::A, false), (ActivationKey::R, true)]),
            },
            &mut registrar,
            || true,
        )
        .unwrap();
        assert_eq!(registrar.active.len(), 2);
        assert_eq!(
            edited.slot_for(ActivationKey::A, false),
            Some(RegistrationSlot::B)
        );
        assert_eq!(
            edited.slot_for(ActivationKey::R, true),
            Some(RegistrationSlot::B)
        );
        assert_eq!(edited.slot_for(ActivationKey::Q, true), None);

        let deleted = configure_hotkeys(
            edited,
            ActivationConfig {
                enabled: true,
                bindings: bindings(&[(ActivationKey::R, true)]),
            },
            &mut registrar,
            || true,
        )
        .unwrap();
        assert_eq!(registrar.active.len(), 1);
        assert_eq!(deleted.slot_for(ActivationKey::A, false), None);
        assert_eq!(
            deleted.slot_for(ActivationKey::R, true),
            Some(RegistrationSlot::B)
        );
        let id = RegistrationSlot::B.id(ActivationKey::R, true);
        assert_eq!(
            hotkey_message_activation(
                deleted,
                id,
                hotkey_l_param(MOD_ALT | MOD_SHIFT, ActivationKey::R),
            ),
            Some((ActivationKey::R, true)),
        );
    }

    #[test]
    fn candidate_bank_conflict_rolls_back_without_changing_active_bank() {
        let current_config = ActivationConfig {
            enabled: true,
            bindings: bindings(&[(ActivationKey::A, false)]),
        };
        let mut registrar = FakeRegistrar::default();
        let current = configure_hotkeys(
            HotKeyRegistrationState::default(),
            current_config,
            &mut registrar,
            || true,
        )
        .unwrap();
        registrar
            .fail_register_once
            .insert(RegistrationSlot::A.id(ActivationKey::Q, true));
        let requested = ActivationConfig {
            enabled: true,
            bindings: bindings(&[(ActivationKey::B, false), (ActivationKey::Q, true)]),
        };
        assert_eq!(
            configure_hotkeys(current, requested, &mut registrar, || true),
            Err(HotKeyTransactionError::Conflict)
        );
        assert_eq!(registrar.active.len(), 1);
        assert!(
            registrar
                .active
                .contains_key(&current.slot.id(ActivationKey::A, false))
        );
    }

    #[test]
    fn shifted_hotkey_message_corrects_a_stale_hook_shift_sample() {
        let registrations = HotKeyRegistrationState {
            config: ActivationConfig {
                enabled: true,
                bindings: bindings(&[(ActivationKey::Z, false), (ActivationKey::Z, true)]),
            },
            slot: RegistrationSlot::B,
            bank_b: bindings(&[(ActivationKey::Z, false), (ActivationKey::Z, true)]),
            generation: 7,
        };
        let observed = candidate_for_exact_passive_down(
            registrations,
            ActivationCandidateState::Idle,
            ActivationKey::Z,
            true,
            false,
            false,
            false,
        )
        .candidate()
        .expect("physical candidate");
        assert!(!observed.shift);

        let resolved = resolve_activation_candidate_message(
            observed,
            registrations,
            RegistrationSlot::B.id(ActivationKey::Z, true),
            hotkey_l_param(MOD_ALT | MOD_SHIFT, ActivationKey::Z),
        )
        .expect("registered hotkey is authoritative");
        assert_eq!(resolved.key, ActivationKey::Z);
        assert!(resolved.shift);
    }

    #[test]
    fn physical_candidate_requires_the_sampled_exact_binding() {
        let state = HotKeyRegistrationState {
            config: ActivationConfig {
                enabled: true,
                bindings: bindings(&[(ActivationKey::Z, true)]),
            },
            slot: RegistrationSlot::A,
            bank_b: ActivationBindings::default(),
            generation: 7,
        };
        assert!(matches!(
            candidate_for_exact_passive_down(
                state,
                ActivationCandidateState::Idle,
                ActivationKey::Z,
                true,
                true,
                false,
                false,
            ),
            ActivationCandidateState::Pressed(_)
        ));
        for (key, sampled_shift) in [(ActivationKey::Z, false), (ActivationKey::Y, true)] {
            assert_eq!(
                candidate_for_exact_passive_down(
                    state,
                    ActivationCandidateState::Idle,
                    key,
                    true,
                    sampled_shift,
                    false,
                    false,
                ),
                ActivationCandidateState::Idle
            );
        }
    }
}
