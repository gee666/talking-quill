use super::*;

#[test]
fn blocked_writer_replays_ten_thousand_after_durable_journal_commit_and_ack() {
    let writer = SharedWriter::default();
    writer.blocked.store(true, Ordering::Release);
    let store = Arc::new(MemoryStore::default());
    let transport = DiagnosticTransport::start(writer.clone(), store.clone());
    for _ in 0..10_000 {
        transport
            .report_owner(
                diagnostic("lease.renew"),
                "not_attempted",
                OwnerProcessState::Running,
            )
            .unwrap();
    }
    let journal = lock_option(&store.journal).clone().unwrap();
    assert_eq!(journal.entries[0].counter.total, 10_000);
    writer.blocked.store(false, Ordering::Release);
    wait_for_records(&writer, 1);
    let latest = records(&writer).pop().unwrap();
    assert_eq!(latest["count"], "10000");
    assert!(transport.acknowledge(&ack_for(&latest)).is_ok());
    assert!(transport.flush(Duration::from_secs(2)));
}

#[test]
fn ack_loss_replays_and_restart_keeps_the_same_journal_high_water() {
    let writer = SharedWriter::default();
    let store = Arc::new(MemoryStore::default());
    let first = DiagnosticTransport::start(writer.clone(), store.clone());
    for _ in 0..40 {
        first
            .report_owner(
                diagnostic("service.poll"),
                "failed",
                OwnerProcessState::Exited,
            )
            .unwrap();
    }
    wait_for_records(&writer, 2);
    let replay = records(&writer).pop().unwrap();
    assert_eq!(replay["count"], "40");
    drop(first);
    let second_writer = SharedWriter::default();
    let second = DiagnosticTransport::start(second_writer.clone(), store);
    wait_for_records(&second_writer, 1);
    let restarted = records(&second_writer).pop().unwrap();
    assert_eq!(restarted["journalId"], replay["journalId"]);
    assert_ne!(restarted["streamId"], replay["streamId"]);
    assert!(second.acknowledge(&ack_for(&restarted)).is_ok());
    assert!(second.flush(Duration::from_secs(2)));
}

#[test]
fn report_failure_returns_error_emits_no_false_durable_record_and_recovers_later() {
    let writer = SharedWriter::default();
    let store = Arc::new(MemoryStore::default());
    store.failures_remaining.store(10_000, Ordering::Release);
    let transport = DiagnosticTransport::start(writer.clone(), store.clone());
    let started = Instant::now();
    assert_eq!(
        transport.report_owner(
            diagnostic("service.poll"),
            "failed",
            OwnerProcessState::Exited,
        ),
        Err(DiagnosticDurabilityError::TimedOut)
    );
    assert!(started.elapsed() < Duration::from_millis(500));
    assert!(records(&writer).is_empty());
    store.failures_remaining.store(0, Ordering::Release);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let durable = records(&writer)
            .into_iter()
            .find(|record| record["durable"] == true);
        if let Some(record) = durable {
            assert!(transport.acknowledge(&ack_for(&record)).is_ok());
            break;
        }
        assert!(Instant::now() < deadline, "durable replay did not recover");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn disk_full_poison_and_writer_spawn_failure_are_durable_and_recoverable() {
    let writer = SharedWriter::default();
    let store = Arc::new(MemoryStore::default());
    store.failures_remaining.store(10_000, Ordering::Release);
    let failed_writer = DiagnosticTransport::start_with_spawn(writer.clone(), store.clone(), false);
    failed_writer.poison_state();
    let started = Instant::now();
    assert_eq!(
        failed_writer.report_owner(
            diagnostic("health.get"),
            "failed",
            OwnerProcessState::Exited,
        ),
        Err(DiagnosticDurabilityError::TimedOut)
    );
    assert!(started.elapsed() < Duration::from_millis(500));
    assert!(records(&writer).is_empty());
    store.failures_remaining.store(0, Ordering::Release);
    let recovery_deadline = Instant::now() + Duration::from_secs(2);
    while lock_option(&store.journal).is_none() && Instant::now() < recovery_deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(lock_option(&store.journal).is_some());
    let successor_writer = SharedWriter::default();
    let successor = DiagnosticTransport::start(successor_writer.clone(), store.clone());
    wait_for_records(&successor_writer, 1);
    let record = records(&successor_writer).pop().unwrap();
    assert_eq!(record["count"], "1");
    assert!(
        record["writerStartFailures"]
            .as_str()
            .unwrap()
            .parse::<u128>()
            .unwrap()
            >= 1
    );
    assert!(
        record["synchronizationRecoveries"]
            .as_str()
            .unwrap()
            .parse::<u128>()
            .unwrap()
            >= 1
    );
    assert!(
        record["durabilityFailures"]
            .as_str()
            .unwrap()
            .parse::<u128>()
            .unwrap()
            >= 1
    );
    assert!(successor.acknowledge(&ack_for(&record)).is_ok());
}

#[test]
fn acknowledgement_persistence_has_a_bounded_wait_and_replays_after_failure() {
    let writer = SharedWriter::default();
    let store = Arc::new(MemoryStore::default());
    let transport = DiagnosticTransport::start(writer.clone(), store.clone());
    transport
        .report_owner(
            diagnostic("lease.renew"),
            "failed",
            OwnerProcessState::Exited,
        )
        .unwrap();
    wait_for_records(&writer, 1);
    let record = records(&writer).pop().unwrap();
    store.failures_remaining.store(10_000, Ordering::Release);
    let started = Instant::now();
    assert_eq!(
        transport.acknowledge(&ack_for(&record)),
        Err(DiagnosticDurabilityError::TimedOut)
    );
    assert!(started.elapsed() < Duration::from_millis(500));
    store.failures_remaining.store(0, Ordering::Release);
    assert!(transport.acknowledge(&ack_for(&record)).is_ok());
}

#[test]
fn corrupt_and_initially_denied_journals_recover_in_process() {
    let corrupt_store = Arc::new(MemoryStore::default());
    corrupt_store.corrupt.store(true, Ordering::Release);
    let corrupt = DiagnosticTransport::start(SharedWriter::default(), corrupt_store.clone());
    assert!(
        corrupt
            .report_owner(
                diagnostic("service.poll"),
                "failed",
                OwnerProcessState::Exited,
            )
            .is_ok()
    );
    assert_eq!(corrupt_store.quarantines.load(Ordering::Acquire), 1);

    let denied_store = Arc::new(MemoryStore::default());
    denied_store
        .load_failures_remaining
        .store(2, Ordering::Release);
    let denied = DiagnosticTransport::start(SharedWriter::default(), denied_store);
    assert!(
        denied
            .report_owner(
                diagnostic("health.get"),
                "failed",
                OwnerProcessState::Exited,
            )
            .is_ok()
    );
}
