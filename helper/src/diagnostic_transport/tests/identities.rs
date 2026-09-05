use super::*;

#[test]
fn entropy_failure_for_new_journal_retries_without_replay_or_acknowledgement() {
    let writer = SharedWriter::default();
    let store = Arc::new(MemoryStore::default());
    let entropy = Arc::new(InjectedEntropy::new(10_000));
    let transport =
        DiagnosticTransport::start_with_entropy(writer.clone(), store.clone(), entropy.clone());
    assert_eq!(
        transport.report_owner(
            diagnostic("service.poll"),
            "failed",
            OwnerProcessState::Exited,
        ),
        Err(DiagnosticDurabilityError::TimedOut)
    );
    assert!(records(&writer).is_empty());
    assert!(lock_option(&store.journal).is_none());
    assert_eq!(
        transport.acknowledge(&DiagnosticAck {
            journal_id: "0".repeat(64),
            journal_nonce: "f".repeat(64),
            dimensions: OwnerDiagnosticKey::from_diagnostic(
                diagnostic("service.poll"),
                "failed",
                OwnerProcessState::Exited,
            ),
            count: "1".into(),
        }),
        Err(DiagnosticDurabilityError::InvalidAcknowledgement)
    );

    entropy.recover();
    assert!(
        transport
            .report_owner(
                diagnostic("service.poll"),
                "failed",
                OwnerProcessState::Exited,
            )
            .is_ok()
    );
    wait_for_records(&writer, 1);
    let record = records(&writer).pop().unwrap();
    assert_eq!(record["durable"], true);
    let identities = [
        record["journalId"].as_str().unwrap(),
        record["journalNonce"].as_str().unwrap(),
        record["streamId"].as_str().unwrap(),
    ];
    assert!(identities.iter().all(|identity| valid_identity(identity)));
    assert_ne!(identities[0], identities[1]);
    assert_ne!(identities[0], identities[2]);
    assert_ne!(identities[1], identities[2]);
    assert!(transport.acknowledge(&ack_for(&record)).is_ok());
}

#[test]
fn entropy_failure_for_new_stream_blocks_existing_journal_replay_and_ack() {
    let store = Arc::new(MemoryStore::default());
    let mut seeded = JournalFile::new(&OsEntropy).unwrap();
    let dimensions = OwnerDiagnosticKey::from_diagnostic(
        diagnostic("health.get"),
        "failed",
        OwnerProcessState::Exited,
    );
    seeded.entries.push(JournalEntry {
        dimensions: dimensions.clone(),
        counter: JournalCounter {
            total: 1,
            ..JournalCounter::default()
        },
    });
    *lock_option(&store.journal) = Some(seeded.clone());
    let writer = SharedWriter::default();
    let entropy = Arc::new(InjectedEntropy::new(10_000));
    let transport = DiagnosticTransport::start_with_entropy(writer.clone(), store, entropy.clone());
    std::thread::sleep(DURABILITY_WAIT);
    assert!(records(&writer).is_empty());
    assert_eq!(
        transport.acknowledge(&DiagnosticAck {
            journal_id: seeded.journal_id.clone(),
            journal_nonce: seeded.journal_nonce.clone(),
            dimensions,
            count: "1".into(),
        }),
        Err(DiagnosticDurabilityError::WorkerUnavailable)
    );

    entropy.recover();
    wait_for_records(&writer, 1);
    let record = records(&writer).pop().unwrap();
    assert_eq!(record["journalId"], seeded.journal_id);
    assert_eq!(record["journalNonce"], seeded.journal_nonce);
    assert_ne!(record["streamId"], record["journalId"]);
    assert_ne!(record["streamId"], record["journalNonce"]);
    assert!(transport.acknowledge(&ack_for(&record)).is_ok());
}

#[test]
fn corrupt_recovery_waits_for_fresh_random_identity_set() {
    let writer = SharedWriter::default();
    let store = Arc::new(MemoryStore::default());
    store.corrupt.store(true, Ordering::Release);
    let entropy = Arc::new(InjectedEntropy::new(10_000));
    let transport =
        DiagnosticTransport::start_with_entropy(writer.clone(), store.clone(), entropy.clone());
    assert_eq!(
        transport.report_owner(
            diagnostic("service.poll"),
            "failed",
            OwnerProcessState::Exited,
        ),
        Err(DiagnosticDurabilityError::TimedOut)
    );
    assert!(records(&writer).is_empty());
    assert!(lock_option(&store.journal).is_none());
    assert_eq!(store.quarantines.load(Ordering::Acquire), 1);

    entropy.recover();
    assert!(
        transport
            .report_owner(
                diagnostic("service.poll"),
                "failed",
                OwnerProcessState::Exited,
            )
            .is_ok()
    );
    wait_for_records(&writer, 1);
    let record = records(&writer).pop().unwrap();
    assert_eq!(record["durable"], true);
    assert_ne!(record["journalId"], record["journalNonce"]);
    assert_ne!(record["journalId"], record["streamId"]);
    assert_ne!(record["journalNonce"], record["streamId"]);
}

#[test]
fn valid_identity_rejects_removed_deterministic_sentinels() {
    assert!(!valid_identity(&"0".repeat(64)));
    assert!(!valid_identity(&"f".repeat(64)));
    assert!(valid_identity(&format!("{}1", "0".repeat(63))));
}

#[test]
fn identity_is_256_bit_lower_hex_and_generation_survives_random_stream_collision() {
    let store = Arc::new(MemoryStore::default());
    let first = DiagnosticTransport::start(SharedWriter::default(), store.clone());
    first
        .report_owner(
            diagnostic("service.poll"),
            "failed",
            OwnerProcessState::Exited,
        )
        .unwrap();
    let first_state = lock_state(&first.shared);
    let journal_id = first_state.journal.journal_id.clone();
    let first_generation = first_state.journal.process_generation;
    assert!(valid_identity(&journal_id));
    assert!(valid_identity(&first_state.stream_id));
    drop(first_state);
    drop(first);
    let second = DiagnosticTransport::start(SharedWriter::default(), store);
    second
        .report_owner(
            diagnostic("service.poll"),
            "failed",
            OwnerProcessState::Exited,
        )
        .unwrap();
    let second_state = lock_state(&second.shared);
    assert_eq!(second_state.journal.journal_id, journal_id);
    assert_eq!(
        second_state.journal.process_generation,
        first_generation + 1
    );
}
