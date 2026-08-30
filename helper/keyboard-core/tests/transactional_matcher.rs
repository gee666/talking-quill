use proptest::prelude::*;
use talking_quill_keyboard_core::{
    ActivationBinding, ActivationBindings, ActivationKey, ModifierMask, ProfileId, Shortcut,
    ShortcutModifiers,
    transactional::{
        CompiledActivationConfig, CompiledMatcher, ConfigRevision, JOURNAL_CAPACITY, MatchClass,
    },
};

fn modifiers(mask: u8) -> ShortcutModifiers {
    ShortcutModifiers {
        ctrl: mask & 1 != 0,
        alt: mask & 2 != 0,
        shift: mask & 4 != 0,
        meta: mask & 8 != 0,
    }
}

fn custom_profile(index: usize) -> ProfileId {
    ProfileId::new(&format!("00000000-0000-4000-8000-{index:012x}")).unwrap()
}

fn binding(profile: ProfileId, mask: u8, keys: &[ActivationKey]) -> ActivationBinding {
    ActivationBinding::new(profile, Shortcut::new(modifiers(mask), keys).unwrap())
}

fn canonical_family() -> ActivationBindings {
    ActivationBindings::new(&[
        binding(ProfileId::GENERAL, 2, &[ActivationKey::X]),
        binding(ProfileId::PROMPT, 2, &[ActivationKey::X, ActivationKey::P]),
        binding(
            ProfileId::PROMPT_TO_ENGLISH,
            2,
            &[ActivationKey::X, ActivationKey::Q],
        ),
        binding(
            ProfileId::MARKDOWN,
            2,
            &[ActivationKey::X, ActivationKey::M],
        ),
        binding(
            ProfileId::TRANSLATE_TO_ENGLISH,
            2,
            &[ActivationKey::X, ActivationKey::T],
        ),
    ])
    .unwrap()
}

#[test]
fn every_nonempty_modifier_mask_is_matched_exactly() {
    for mask in 1_u8..16 {
        let configured =
            ActivationBindings::new(&[binding(ProfileId::GENERAL, mask, &[ActivationKey::A])])
                .unwrap();
        let matcher = CompiledMatcher::compile(configured).unwrap();
        for actual in 0_u8..16 {
            let result = matcher.start(ModifierMask::from(modifiers(actual)), ActivationKey::A);
            assert_eq!(
                matches!(result, MatchClass::Exact { .. }),
                actual == mask,
                "configured={mask:04b} actual={actual:04b}"
            );
        }
    }
}

#[test]
fn canonical_shared_prefix_and_longer_suffixes_are_classified() {
    let matcher = CompiledMatcher::compile(canonical_family()).unwrap();
    let alt = ModifierMask::from(modifiers(2));
    let MatchClass::ExactWithLonger { cursor, binding } = matcher.start(alt, ActivationKey::X)
    else {
        panic!("Alt+X must be exact and have descendants");
    };
    assert_eq!(binding.profile_id(), ProfileId::GENERAL);
    assert_eq!(cursor.depth(), 1);
    assert_eq!(cursor.candidate_bits().count_ones(), 5);

    let MatchClass::Exact { binding, cursor } = matcher.advance(cursor, ActivationKey::P) else {
        panic!("Alt+X+P must resolve immediately");
    };
    assert_eq!(binding.profile_id(), ProfileId::PROMPT);
    assert_eq!(cursor.depth(), 2);

    let MatchClass::ExactWithLonger { cursor, .. } = matcher.start(alt, ActivationKey::X) else {
        unreachable!()
    };
    assert_eq!(
        matcher.advance(cursor, ActivationKey::Y),
        MatchClass::NoCandidate
    );
    assert_eq!(
        matcher.start(ModifierMask::default(), ActivationKey::X),
        MatchClass::NoCandidate
    );
}

#[test]
fn maximum_configuration_and_sequence_fit_matcher_and_journal_bounds() {
    let mut keys: Vec<_> = (0..26)
        .map(|index| ActivationKey::from_index(index).unwrap())
        .collect();
    let longest = binding(ProfileId::GENERAL, 4, &keys);
    assert!(longest.shortcut().keys().len() * 2 + 2 <= JOURNAL_CAPACITY);

    let mut values = vec![longest];
    for index in 1..13 {
        keys.rotate_left(1);
        values.push(binding(custom_profile(index), 1, &[keys[0]]));
    }
    // Prefix validation applies only within one mask, so use distinct Ctrl
    // single keys and one Shift maximum-length sequence.
    let configured = ActivationBindings::new(&values).unwrap();
    let matcher = CompiledMatcher::compile(configured).unwrap();
    assert_eq!(matcher.binding_count(), 13);

    let mut result = matcher.start(ModifierMask::from(modifiers(4)), ActivationKey::A);
    for key in keys_for_longest().into_iter().skip(1) {
        result = matcher.advance(result.cursor().unwrap(), key);
    }
    assert!(matches!(result, MatchClass::Exact { .. }));
}

fn keys_for_longest() -> Vec<ActivationKey> {
    (0..26)
        .map(|index| ActivationKey::from_index(index).unwrap())
        .collect()
}

#[test]
fn revision_increment_never_wraps() {
    assert_eq!(
        ConfigRevision::new(41).checked_next(),
        Some(ConfigRevision::new(42))
    );
    assert_eq!(ConfigRevision::new(u64::MAX).checked_next(), None);
}

#[test]
fn compiled_configuration_preserves_exact_bounded_snapshot() {
    let bindings = canonical_family();
    let config = CompiledActivationConfig::compile(ConfigRevision::new(7), true, bindings).unwrap();
    assert_eq!(config.revision(), ConfigRevision::new(7));
    assert!(config.enabled());
    assert_eq!(config.bindings(), bindings);
}

proptest! {
    #[test]
    fn compiled_single_step_matcher_equals_naive_scan(
        mut raw in prop::collection::vec((1_u8..16, 0_u8..26), 1..=13),
        actual_mask in 0_u8..16,
        actual_key in 0_u8..26,
    ) {
        raw.retain(|(mask, key)| !(*mask == 2 && *key == ActivationKey::X.index()));
        if raw.is_empty() {
            raw.push((1, ActivationKey::A.index()));
        }
        raw.sort_unstable();
        raw.dedup();
        raw.truncate(13);
        let values: Vec<_> = raw.iter().enumerate().map(|(index, (mask, key))| {
            binding(custom_profile(index), *mask, &[ActivationKey::from_index(*key).unwrap()])
        }).collect();
        let configured = ActivationBindings::new(&values).unwrap();
        let matcher = CompiledMatcher::compile(configured).unwrap();
        let key = ActivationKey::from_index(actual_key).unwrap();
        let result = matcher.start(ModifierMask::from(modifiers(actual_mask)), key);
        let naive = values.iter().find(|candidate| {
            candidate.shortcut().modifiers() == modifiers(actual_mask)
                && candidate.shortcut().keys() == [key]
        }).copied();
        match (result, naive) {
            (MatchClass::Exact { binding, cursor }, Some(expected)) => {
                prop_assert_eq!(binding, expected);
                prop_assert_eq!(cursor.depth(), 1);
                prop_assert_eq!(cursor.candidate_bits().count_ones(), 1);
            }
            (MatchClass::NoCandidate, None) => {}
            (actual, expected) => prop_assert!(false, "actual={actual:?} expected={expected:?}"),
        }
    }

    #[test]
    fn binding_wire_order_does_not_change_semantic_results(
        mut raw in prop::collection::vec((1_u8..16, 0_u8..26), 1..=13),
        actual_mask in 0_u8..16,
        actual_key in 0_u8..26,
    ) {
        raw.retain(|(mask, key)| !(*mask == 2 && *key == ActivationKey::X.index()));
        if raw.is_empty() {
            raw.push((1, ActivationKey::A.index()));
        }
        raw.sort_unstable();
        raw.dedup();
        raw.truncate(13);
        let forward: Vec<_> = raw.iter().enumerate().map(|(index, (mask, key))| {
            binding(custom_profile(index), *mask, &[ActivationKey::from_index(*key).unwrap()])
        }).collect();
        let mut reverse = forward.clone();
        reverse.reverse();
        let first = CompiledMatcher::compile(ActivationBindings::new(&forward).unwrap()).unwrap();
        let second = CompiledMatcher::compile(ActivationBindings::new(&reverse).unwrap()).unwrap();
        let mask = ModifierMask::from(modifiers(actual_mask));
        let key = ActivationKey::from_index(actual_key).unwrap();
        let first_binding = match first.start(mask, key) {
            MatchClass::Exact { binding, .. } => Some(binding),
            _ => None,
        };
        let second_binding = match second.start(mask, key) {
            MatchClass::Exact { binding, .. } => Some(binding),
            _ => None,
        };
        prop_assert_eq!(first_binding, second_binding);
    }
}
