use super::*;

#[test]
fn truncated_file_is_quarantined_and_replaced_without_exposing_content_in_its_name() {
    let directory = PathBuf::from("tmp/tests");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!(
        "owner-truncated-{}.json",
        random_identity().unwrap()
    ));
    std::fs::write(&path, br#"{\"private":"truncated""#).unwrap();
    let transport =
        DiagnosticTransport::start(io::sink(), Arc::new(FileJournalStore::new(path.clone())));
    assert!(
        transport
            .report_owner(
                diagnostic("service.poll"),
                "failed",
                OwnerProcessState::Exited,
            )
            .is_ok()
    );
    let prefix = path.file_stem().unwrap().to_string_lossy().into_owned();
    let quarantines = std::fs::read_dir(&directory)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(&format!("{prefix}.corrupt-")))
        .collect::<Vec<_>>();
    assert_eq!(quarantines.len(), 1);
    assert!(!quarantines[0].contains("private"));
}

#[test]
fn exclusive_store_rejects_overlap_and_allows_takeover_after_release() {
    let directory = PathBuf::from("tmp/tests");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("owner-overlap-{}.json", random_identity().unwrap()));
    let first = FileJournalStore::new(path.clone());
    let second = FileJournalStore::new(path.clone());
    assert!(first.load().unwrap().is_none());
    assert!(second.load().is_err());
    drop(first);
    assert!(second.load().unwrap().is_none());
    drop(second);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("journal.lock"));
}

#[test]
fn file_journal_atomically_round_trips_in_project_tmp() {
    let directory = PathBuf::from("tmp/tests");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!(
        "owner-diagnostic-{}.json",
        random_identity().unwrap()
    ));
    let store = FileJournalStore::new(path.clone());
    let mut journal = JournalFile::new(&OsEntropy).unwrap();
    journal.process_generation = 1;
    store.store(&journal).unwrap();
    let loaded = store.load().unwrap().unwrap();
    assert_eq!(loaded.journal_id, journal.journal_id);
    drop(store);
    std::fs::remove_file(&path).unwrap();
    let _ = std::fs::remove_file(path.with_extension("journal.lock"));
}
