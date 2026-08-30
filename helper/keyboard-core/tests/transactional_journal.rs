use talking_quill_keyboard_core::{
    ActivationKey,
    transactional::{
        EventJournal, JOURNAL_CAPACITY, JournalDisposition, JournalError, KeyIdentity, NativeKey,
        PhysicalPhase, ReplayRecord,
    },
};

fn record(key: ActivationKey, phase: PhysicalPhase, order: u64) -> ReplayRecord {
    ReplayRecord {
        key: KeyIdentity::Letter(key),
        native: NativeKey {
            virtual_key: u16::from(key.index()) + 0x41,
            scan_code: u32::from(key.index()) + 1,
            extended: key == ActivationKey::Z,
            platform_flags: order,
        },
        phase,
        observed_at_ms: 1_000 + order,
    }
}

#[test]
fn append_boundaries_are_exact_and_failed_append_does_not_mutate() {
    let mut journal = EventJournal::new();
    for index in 0..JOURNAL_CAPACITY {
        assert_eq!(
            journal.push(record(
                ActivationKey::A,
                PhysicalPhase::Repeat,
                index as u64
            )),
            Ok(())
        );
    }
    assert_eq!(journal.len(), JOURNAL_CAPACITY);
    let snapshot = journal;
    assert_eq!(
        journal.push(record(ActivationKey::B, PhysicalPhase::Down, 999)),
        Err(JournalError::Full)
    );
    assert_eq!(journal, snapshot);
}

#[test]
fn replay_preserves_native_fields_phase_timestamp_and_order() {
    let entries = [
        record(ActivationKey::X, PhysicalPhase::Down, 0),
        record(ActivationKey::X, PhysicalPhase::Repeat, 1),
        record(ActivationKey::P, PhysicalPhase::Down, 2),
        record(ActivationKey::P, PhysicalPhase::Up, 3),
    ];
    let mut journal = EventJournal::new();
    for entry in entries {
        journal.push(entry).unwrap();
    }
    let batch = journal.replay_batch().unwrap();
    assert_eq!(batch.entries(), entries);
    assert_eq!(batch.len(), entries.len());
}

#[test]
fn native_platform_flags_preserve_all_sixty_four_bits() {
    let mut entry = record(ActivationKey::V, PhysicalPhase::Down, 0);
    entry.native.platform_flags = 0xFEDC_BA98_7654_3210;
    let mut journal = EventJournal::new();
    journal.push(entry).unwrap();
    assert_eq!(
        journal.replay_batch().unwrap().entries()[0]
            .native
            .platform_flags,
        0xFEDC_BA98_7654_3210
    );
}

#[test]
fn commit_and_replay_are_mutually_exclusive() {
    let mut committed = EventJournal::new();
    committed
        .push(record(ActivationKey::A, PhysicalPhase::Down, 0))
        .unwrap();
    assert_eq!(committed.commit(), Ok(()));
    assert_eq!(committed.disposition(), JournalDisposition::Committed);
    assert_eq!(committed.commit(), Err(JournalError::Finalized));
    assert_eq!(committed.mark_replayed(), Err(JournalError::Finalized));
    assert_eq!(committed.replay_batch(), Err(JournalError::Finalized));

    let mut replayed = EventJournal::new();
    replayed
        .push(record(ActivationKey::A, PhysicalPhase::Down, 0))
        .unwrap();
    assert_eq!(replayed.mark_replayed(), Ok(()));
    assert_eq!(replayed.disposition(), JournalDisposition::Replayed);
    assert_eq!(replayed.commit(), Err(JournalError::Finalized));
}

#[test]
fn partial_replay_cleanup_releases_only_accepted_unbalanced_injected_downs() {
    let entries = [
        record(ActivationKey::X, PhysicalPhase::Down, 0),
        record(ActivationKey::X, PhysicalPhase::Repeat, 1),
        record(ActivationKey::P, PhysicalPhase::Down, 2),
        record(ActivationKey::X, PhysicalPhase::Up, 3),
        record(ActivationKey::P, PhysicalPhase::Up, 4),
    ];
    let mut journal = EventJournal::new();
    for entry in entries {
        journal.push(entry).unwrap();
    }
    let batch = journal.replay_batch().unwrap();

    assert!(batch.cleanup_for_accepted(0).unwrap().is_empty());
    let after_x = batch.cleanup_for_accepted(2).unwrap();
    assert_eq!(after_x.len(), 1);
    assert_eq!(
        after_x.entries()[0].key,
        KeyIdentity::Letter(ActivationKey::X)
    );
    assert_eq!(after_x.entries()[0].phase, PhysicalPhase::Up);

    let after_p = batch.cleanup_for_accepted(3).unwrap();
    assert_eq!(
        after_p
            .entries()
            .iter()
            .map(|entry| entry.key)
            .collect::<Vec<_>>(),
        vec![
            KeyIdentity::Letter(ActivationKey::P),
            KeyIdentity::Letter(ActivationKey::X),
        ]
    );
    assert_eq!(
        after_p
            .entries()
            .iter()
            .map(|entry| entry.phase)
            .collect::<Vec<_>>(),
        vec![PhysicalPhase::Up, PhysicalPhase::Up]
    );

    let after_x_up = batch.cleanup_for_accepted(4).unwrap();
    assert_eq!(after_x_up.len(), 1);
    assert_eq!(
        after_x_up.entries()[0].key,
        KeyIdentity::Letter(ActivationKey::P)
    );
    assert!(batch.cleanup_for_accepted(5).unwrap().is_empty());
    assert_eq!(
        batch.cleanup_for_accepted(6),
        Err(JournalError::InvalidAcceptedCount)
    );

    for accepted in 0..=batch.len() {
        let cleanup = batch.cleanup_for_accepted(accepted).unwrap();
        assert!(cleanup.len() <= accepted);
        assert!(cleanup.entries().iter().all(|entry| {
            matches!(entry.key, KeyIdentity::Letter(_)) && entry.phase == PhysicalPhase::Up
        }));
    }
}

#[test]
fn cleanup_never_contains_non_letter_or_synthesizes_a_modifier_release() {
    let mut journal = EventJournal::new();
    journal
        .push(ReplayRecord {
            key: KeyIdentity::Modifier(
                talking_quill_keyboard_core::transactional::ModifierSide::LeftAlt,
            ),
            native: NativeKey {
                virtual_key: 0xA4,
                ..NativeKey::default()
            },
            phase: PhysicalPhase::Down,
            observed_at_ms: 0,
        })
        .unwrap();
    let batch = journal.replay_batch().unwrap();
    assert!(batch.cleanup_for_accepted(1).unwrap().is_empty());
}
