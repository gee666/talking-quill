//! Installer transaction encoding, publication, and residue removal.
use super::*;

pub(in super::super) fn transaction_action(value: &Transaction) -> Result<Action> {
    match value.action.as_str() {
        "install" => Ok(Action::Install),
        "update" => Ok(Action::Update),
        "repair" => Ok(Action::Repair),
        "uninstall" => Ok(Action::Uninstall),
        _ => Err(fail(
            EXIT_REJECTED,
            "Installer transaction action is invalid.",
        )),
    }
}

pub(in super::super) fn write_transaction(
    paths: &Paths,
    phase: &str,
    action: Action,
    had_predecessor: bool,
) -> Result<()> {
    let temporary = paths
        .transaction
        .with_extension(format!("tmp-{}", std::process::id()));
    let action = match action {
        Action::Install => "install",
        Action::Update => "update",
        Action::Repair => "repair",
        Action::Uninstall => "uninstall",
        #[cfg(feature = "stale-schema2-cleanup")]
        Action::CleanStaleSchema2 => {
            return Err(fail(EXIT_REJECTED, "Cleanup cannot create a transaction."));
        }
    };
    let bytes = serde_json::to_vec(&Transaction {
        schema_version: TRANSACTION_SCHEMA,
        phase: phase.into(),
        action: action.into(),
        had_predecessor,
    })
    .map_err(|_| fail(EXIT_FAILURE, "Cannot encode installer transaction."))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(io_failure)?;
    output
        .write_all(&bytes)
        .and_then(|_| output.sync_all())
        .map_err(io_failure)?;
    durable_replace(&temporary, &paths.transaction)
}

pub(in super::super) fn cleanup_transaction_residue(root: &Path) -> Result<()> {
    for entry in fs::read_dir(root).map_err(io_failure)? {
        let entry = entry.map_err(io_failure)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(pid) = name.strip_prefix(".Talking Quill.native-transaction-v2.tmp-") else {
            continue;
        };
        if pid.is_empty() || !pid.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let file = open_plain_handle(&entry.path(), false, true)?;
        delete_retained(&file)?;
    }
    Ok(())
}

pub(in super::super) fn remove_transaction(paths: &Paths) -> Result<()> {
    if paths.transaction.exists() {
        let file = open_plain_handle(&paths.transaction, false, true)?;
        delete_retained(&file)?;
    }
    if paths.maintenance_generation_record.exists() {
        let file = open_plain_handle(&paths.maintenance_generation_record, false, true)?;
        delete_retained(&file)?;
    }
    Ok(())
}
