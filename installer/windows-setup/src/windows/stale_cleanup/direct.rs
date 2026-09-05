//! Direct elevated cleanup entry, rejection injection, and audit reporting.
use super::*;

#[cfg(feature = "stale-schema2-cleanup")]
pub(in super::super) fn force_stale_cleanup_rejection(stage: &str) -> Result<()> {
    if std::env::var("TQ_STALE_SCHEMA2_FORCE_REJECTION_STAGE").as_deref() == Ok(stage) {
        Err(fail(
            EXIT_REJECTED,
            format!("Forced stale schema-2 cleanup rejection at {stage}."),
        ))
    } else {
        Ok(())
    }
}

#[cfg(feature = "stale-schema2-cleanup")]
pub(in super::super) fn record_post_audit_cleanup_rejection(
    diagnostic: &mut StaleSchema2Diagnostic,
    audit: &mut StaleCleanupAudit,
    error: SetupError,
) -> Result<()> {
    let stage = "cleanup.rejected.after-audit";
    let evidence = serde_json::json!({ "error": error.message, "exitCode": EXIT_REJECTED });
    let diagnostic_result = diagnostic.record(stage, "rejected", evidence);
    let empty = retained_binding(&[], stage);
    let audit_result = audit.record(stage, &empty, &empty);
    if let Err(audit_error) = audit_result {
        return Err(fail(
            EXIT_REJECTED,
            format!(
                "{} Cleanup rejection audit failed: {}",
                error.message, audit_error.message
            ),
        ));
    }
    if let Err(diagnostic_error) = diagnostic_result {
        return Err(fail(
            EXIT_REJECTED,
            format!(
                "{} Cleanup rejection diagnostic failed: {}",
                error.message, diagnostic_error.message
            ),
        ));
    }
    Err(fail(EXIT_REJECTED, error.message))
}

#[cfg(feature = "stale-schema2-cleanup")]
pub(in super::super) fn run_direct_elevated_stale_schema2_cleanup() -> Result<()> {
    let mut diagnostic = StaleSchema2Diagnostic::open()?;
    let token = token_is_elevated().and_then(|elevated| {
        peer_identity::process_integrity(std::process::id()).map(|integrity| (elevated, integrity))
    });
    let (elevated, integrity_rid) = match token {
        Ok(value) if direct_cleanup_token_is_authorized(value.0, value.1) => {
            diagnostic.record(
                "token.identity",
                "passed",
                serde_json::json!({ "elevated": value.0, "integrityRid": value.1 }),
            )?;
            value
        }
        Ok(value) => {
            let error = "Direct stale cleanup requires a high elevated token.";
            diagnostic.record(
                "token.identity",
                "rejected",
                serde_json::json!({ "elevated": value.0, "integrityRid": value.1, "error": error }),
            )?;
            diagnostic.record(
                "cleanup.rejected.before-audit",
                "rejected",
                serde_json::json!({ "error": error }),
            )?;
            return Err(fail(EXIT_REJECTED, error));
        }
        Err(error) => {
            diagnostic.record(
                "cleanup.rejected.before-audit",
                "rejected",
                serde_json::json!({ "error": error.message }),
            )?;
            return Err(fail(EXIT_REJECTED, error.message));
        }
    };
    debug_assert!(direct_cleanup_token_is_authorized(elevated, integrity_rid));
    let image = open_authenticated_direct_cleanup_image();
    let (mut retained_image, expected_hash) = match image {
        Ok(value) => value,
        Err(error) => {
            diagnostic.record(
                "cleanup.rejected.before-audit",
                "rejected",
                serde_json::json!({ "error": error.message }),
            )?;
            return Err(fail(EXIT_REJECTED, error.message));
        }
    };
    // Retain a protected append handle so a later cleanup failure can be recorded durably.
    let audit = StaleCleanupAudit::open();
    let mut audit = match audit {
        Ok(audit) => audit,
        Err(error) => {
            diagnostic.record(
                "cleanup.rejected.before-audit",
                "rejected",
                serde_json::json!({ "error": error.message }),
            )?;
            return Err(fail(EXIT_REJECTED, error.message));
        }
    };
    let operation = (|| -> Result<()> {
        reclaim_exact_schema2_orphan_with_audit(true, false, &mut audit)?;
        retained_image
            .seek(SeekFrom::Start(0))
            .map_err(io_failure)?;
        if hash_reader(&mut retained_image)? != expected_hash {
            return Err(fail(
                EXIT_REJECTED,
                "Direct cleanup image changed during the operation.",
            ));
        }
        Ok(())
    })();
    if let Err(error) = operation {
        return record_post_audit_cleanup_rejection(&mut diagnostic, &mut audit, error);
    }
    Ok(())
}
