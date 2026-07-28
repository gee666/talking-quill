use serde_json::{Value, json};
use talking_quill_helper::keyboard::{
    ActivationBinding, ActivationBindings, ActivationKey, ModifierMask, ProfileId, Shortcut,
    ShortcutModifiers, ShortcutValidationError,
};

fn modifiers(ctrl: bool, alt: bool, shift: bool, meta: bool) -> ShortcutModifiers {
    ShortcutModifiers {
        ctrl,
        alt,
        shift,
        meta,
    }
}

fn shortcut(modifiers: ShortcutModifiers, keys: &[ActivationKey]) -> Shortcut {
    Shortcut::new(modifiers, keys).unwrap()
}

fn profile_id(index: usize) -> ProfileId {
    match index {
        0 => ProfileId::GENERAL,
        1 => ProfileId::PROMPT,
        2 => ProfileId::PROMPT_TO_ENGLISH,
        3 => ProfileId::MARKDOWN,
        4 => ProfileId::TRANSLATE_TO_ENGLISH,
        _ => ProfileId::new(&format!("00000000-0000-4000-8000-{index:012x}")).unwrap(),
    }
}

fn binding(index: usize, shortcut: Shortcut) -> ActivationBinding {
    ActivationBinding::new(profile_id(index), shortcut)
}

#[test]
fn shortcut_wire_shape_round_trips_all_modifiers_and_ordered_keys_exactly() {
    let value = json!({
        "modifiers": {"ctrl": true, "alt": false, "shift": true, "meta": true},
        "keys": ["X", "P"]
    });
    let parsed: Shortcut = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), value);
    assert_eq!(parsed.keys(), &[ActivationKey::X, ActivationKey::P]);
    assert_eq!(parsed.trigger(), ActivationKey::P);
    assert_eq!(
        parsed.modifier_mask(),
        ModifierMask::new(true, false, true, true)
    );
}

#[test]
fn shortcut_rejects_empty_modifierless_duplicate_lowercase_missing_and_unknown_fields() {
    for value in [
        json!({"modifiers": {"ctrl": false, "alt": true, "shift": false, "meta": false}, "keys": []}),
        json!({"modifiers": {"ctrl": false, "alt": false, "shift": false, "meta": false}, "keys": ["A"]}),
        json!({"modifiers": {"ctrl": false, "alt": true, "shift": false, "meta": false}, "keys": ["A", "A"]}),
        json!({"modifiers": {"ctrl": false, "alt": true, "shift": false, "meta": false}, "keys": ["a"]}),
        json!({"modifiers": {"alt": true, "shift": false, "meta": false}, "keys": ["A"]}),
        json!({"modifiers": {"ctrl": false, "alt": true, "shift": false, "meta": false, "capsLock": false}, "keys": ["A"]}),
        json!({"modifiers": {"ctrl": false, "alt": true, "shift": false, "meta": false}, "keys": ["A"], "trigger": "A"}),
    ] {
        assert!(serde_json::from_value::<Shortcut>(value).is_err());
    }
}

#[test]
fn shortcut_accepts_exactly_twenty_six_unique_letters_and_rejects_twenty_seven() {
    let keys: Vec<_> = (0..26)
        .map(|index| ActivationKey::from_index(index).unwrap())
        .collect();
    let all = Shortcut::new(modifiers(false, false, true, false), &keys).unwrap();
    assert_eq!(all.keys(), keys);

    let mut too_many = keys;
    too_many.push(ActivationKey::A);
    assert_eq!(
        Shortcut::new(modifiers(false, false, true, false), &too_many),
        Err(ShortcutValidationError::InvalidKeyCount)
    );
}

#[test]
fn bindings_are_bounded_ordered_and_reject_exact_duplicates() {
    let values: Vec<_> = (0..13)
        .map(|index| {
            shortcut(
                modifiers(false, true, false, false),
                &[ActivationKey::from_index(index).unwrap()],
            )
        })
        .collect();
    let binding_values: Vec<_> = values
        .iter()
        .copied()
        .enumerate()
        .map(|(index, shortcut)| binding(index, shortcut))
        .collect();
    let bindings = ActivationBindings::new(&binding_values).unwrap();
    assert_eq!(bindings.len(), 13);
    assert_eq!(bindings.iter().collect::<Vec<_>>(), binding_values);

    let mut too_many = binding_values;
    too_many.push(binding(
        13,
        shortcut(modifiers(false, true, false, false), &[ActivationKey::N]),
    ));
    assert_eq!(
        ActivationBindings::new(&too_many),
        Err(ShortcutValidationError::TooManyBindings)
    );

    let duplicate = shortcut(modifiers(false, true, false, false), &[ActivationKey::A]);
    assert_eq!(
        ActivationBindings::new(&[binding(0, duplicate), binding(1, duplicate)]),
        Err(ShortcutValidationError::DuplicateBinding)
    );
    assert_eq!(
        ActivationBindings::new(&[
            binding(0, duplicate),
            binding(
                0,
                shortcut(modifiers(false, true, false, false), &[ActivationKey::B])
            ),
        ]),
        Err(ShortcutValidationError::DuplicateProfileId)
    );
}

#[test]
fn only_the_exact_built_in_family_allows_same_modifier_prefixes() {
    let alt = modifiers(false, true, false, false);
    let ctrl_shift = modifiers(true, false, true, false);
    let prefix = shortcut(alt, &[ActivationKey::X]);
    let prompt = shortcut(alt, &[ActivationKey::X, ActivationKey::P]);
    let prompt_to_english = shortcut(alt, &[ActivationKey::X, ActivationKey::Q]);
    let markdown = shortcut(alt, &[ActivationKey::X, ActivationKey::M]);
    let translate = shortcut(alt, &[ActivationKey::X, ActivationKey::T]);

    assert!(
        ActivationBindings::new(&[
            binding(0, prefix),
            binding(1, prompt),
            binding(2, prompt_to_english),
            binding(3, markdown),
            binding(4, translate),
        ])
        .is_ok()
    );
    for reserved in [
        binding(5, prefix),
        binding(5, prompt),
        binding(0, shortcut(alt, &[ActivationKey::X, ActivationKey::Q])),
    ] {
        assert_eq!(
            ActivationBindings::new(&[reserved]),
            Err(ShortcutValidationError::ReservedBuiltInFamily),
        );
    }

    let unrelated_prefix = shortcut(alt, &[ActivationKey::A]);
    let unrelated_longer = shortcut(alt, &[ActivationKey::A, ActivationKey::B]);
    assert_eq!(
        ActivationBindings::new(&[binding(5, unrelated_prefix), binding(6, unrelated_longer)]),
        Err(ShortcutValidationError::PrefixConflict),
    );

    let different_modifiers = shortcut(ctrl_shift, &[ActivationKey::X, ActivationKey::P]);
    assert!(
        ActivationBindings::new(&[binding(0, prefix), binding(5, different_modifiers)]).is_ok()
    );
}

#[test]
fn bindings_wire_value_contains_strict_profile_id_and_shortcut_objects() {
    let values = json!([
        {
            "profileId": "general",
            "shortcut": {
                "modifiers": {"ctrl": false, "alt": true, "shift": false, "meta": false},
                "keys": ["X"]
            }
        },
        {
            "profileId": "prompt",
            "shortcut": {
                "modifiers": {"ctrl": false, "alt": true, "shift": false, "meta": false},
                "keys": ["X", "P"]
            }
        },
        {
            "profileId": "prompt-to-english",
            "shortcut": {
                "modifiers": {"ctrl": false, "alt": true, "shift": false, "meta": false},
                "keys": ["X", "Q"]
            }
        },
        {
            "profileId": "markdown",
            "shortcut": {
                "modifiers": {"ctrl": false, "alt": true, "shift": false, "meta": false},
                "keys": ["X", "M"]
            }
        },
        {
            "profileId": "translate-to-english",
            "shortcut": {
                "modifiers": {"ctrl": false, "alt": true, "shift": false, "meta": false},
                "keys": ["X", "T"]
            }
        }
    ]);
    let parsed: ActivationBindings = serde_json::from_value(values.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), values);

    let invalid: Value = json!([values[0].clone(), values[0].clone()]);
    assert!(serde_json::from_value::<ActivationBindings>(invalid).is_err());

    for invalid_profile_id in [
        "",
        "custom",
        "00000000-0000-0000-8000-000000000001",
        "00000000-0000-4000-7000-000000000000",
        "FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF",
    ] {
        let invalid = json!([{
            "profileId": invalid_profile_id,
            "shortcut": {
                "modifiers": {"ctrl": false, "alt": true, "shift": false, "meta": false},
                "keys": ["A"]
            }
        }]);
        assert!(serde_json::from_value::<ActivationBindings>(invalid).is_err());
    }

    for boundary in [
        "00000000-0000-0000-0000-000000000000",
        "ffffffff-ffff-ffff-ffff-ffffffffffff",
    ] {
        assert_eq!(ProfileId::new(boundary).unwrap().as_str(), boundary);
    }
}
