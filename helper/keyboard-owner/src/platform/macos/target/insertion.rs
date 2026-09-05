//! Target proofs and ordered range/text mutation on the AX worker.

use super::*;

pub(super) fn selected_range_for_control(
    resources: &TargetCaptureResources,
    control: &OwnedCf,
) -> Option<ffi::CFRange> {
    let value = ax_copy_attribute(
        control.as_type_ref().cast_mut(),
        resources.selected_text_range.as_type_ref(),
    )
    .ok()?;
    let value_type = unsafe { ffi::AXValueGetType(value.as_type_ref()) };
    let mut range = ffi::CFRange::default();
    let extracted = unsafe {
        ffi::AXValueGetValue(
            value.as_type_ref(),
            ffi::K_AX_VALUE_CFRANGE_TYPE,
            (&raw mut range).cast(),
        )
    } != 0;
    selected_text_range_is_valid(value_type, extracted, range).then_some(range)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum InsertionFailure {
    TargetInvalid,
    Paste(PasteFailure),
}

pub(super) const fn insertion_identity_failure(
    stopping: bool,
    identity_matches: bool,
) -> Option<InsertionFailure> {
    if stopping {
        Some(InsertionFailure::Paste(PasteFailure::Unavailable))
    } else if !identity_matches {
        Some(InsertionFailure::TargetInvalid)
    } else {
        None
    }
}

pub(super) fn insertion_target_failure(
    resources: &TargetCaptureResources,
    shared: &Arc<CacheShared>,
    work: &InsertionWork,
    cached: &CachedTarget,
) -> Option<InsertionFailure> {
    // Flush workspace/focus/range notifications around the AX range query. The
    // permission and real Secure Input preflights are deliberately last, so the
    // returned proof is immediately adjacent to claim or mutation.
    service_worker_notifications(0.000_1);
    if let Some(failure) = insertion_identity_failure(
        shared.stopping.load(Ordering::Acquire),
        shared.current_notification_epoch() == work.notification_epoch
            && shared.current_boundary_epoch() == work.boundary_epoch
            && shared.current_selected_range_epoch() == work.selected_range_epoch
            && shared.published_id.load(Ordering::Acquire) == work.publication_id
            && cached.publication_id == work.publication_id
            && cached.notification_epoch == work.notification_epoch
            && cached.boundary_epoch == work.boundary_epoch
            && cached.selected_range_epoch == work.selected_range_epoch,
    ) {
        return Some(failure);
    }
    let selected_range_matches =
        selected_range_for_control(resources, &cached.evidence.focused_control)
            == Some(cached.evidence.selected_text_range);
    if let Some(failure) = insertion_identity_failure(
        shared.stopping.load(Ordering::Acquire),
        selected_range_matches,
    ) {
        return Some(failure);
    }
    service_worker_notifications(0.000_1);
    if let Some(failure) = insertion_identity_failure(
        shared.stopping.load(Ordering::Acquire),
        shared.current_notification_epoch() == work.notification_epoch
            && shared.current_boundary_epoch() == work.boundary_epoch
            && shared.current_selected_range_epoch() == work.selected_range_epoch
            && shared.published_id.load(Ordering::Acquire) == work.publication_id,
    ) {
        return Some(failure);
    }
    if !permissions_allow_native_input(permission_snapshot()) {
        return Some(InsertionFailure::Paste(PasteFailure::PermissionDenied));
    }
    if secure_input_active() {
        return Some(InsertionFailure::Paste(PasteFailure::SecureInput));
    }
    if !insertion_completion_budget_available(work.deadline, Instant::now()) {
        return Some(InsertionFailure::Paste(PasteFailure::Unavailable));
    }
    if let Some(failure) = insertion_identity_failure(
        shared.stopping.load(Ordering::Acquire),
        shared.current_notification_epoch() == work.notification_epoch
            && shared.current_boundary_epoch() == work.boundary_epoch
            && shared.current_selected_range_epoch() == work.selected_range_epoch
            && shared.published_id.load(Ordering::Acquire) == work.publication_id,
    ) {
        return Some(failure);
    }
    None
}

pub(super) fn process_insertion_work(
    resources: &TargetCaptureResources,
    shared: &Arc<CacheShared>,
    work: InsertionWork,
) {
    #[cfg(feature = "transactional-shortcuts-dev")]
    let secure_input_scope = TestSecureInputScope::enable_for_preclaim_seam();
    #[cfg(feature = "transactional-shortcuts-dev")]
    if secure_input_scope.failed_to_enable() {
        fail_pending_insertion(&work.state, PasteFailure::Unavailable);
        return;
    }

    // Cheap real safety preflights precede AX messaging as well as being
    // repeated in the immediately-adjacent claim proof below.
    if !permissions_allow_native_input(permission_snapshot()) {
        fail_pending_insertion(&work.state, PasteFailure::PermissionDenied);
        return;
    }
    if secure_input_active() {
        fail_pending_insertion(&work.state, PasteFailure::SecureInput);
        return;
    }

    let slot = shared
        .slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(cached) = slot.as_ref() else {
        fail_insertion(&work.state, InsertionFailure::TargetInvalid);
        return;
    };
    if let Some(reason) = insertion_target_failure(resources, shared, &work, cached) {
        fail_insertion(&work.state, reason);
        return;
    }
    let Some(conversion_deadline) = work
        .deadline
        .checked_sub(AX_MESSAGING_TIMEOUT + INSERTION_RESULT_MARGIN)
    else {
        fail_pending_insertion(&work.state, PasteFailure::Unavailable);
        return;
    };
    let Some(preclaim_clipboard) = clipboard_plain_text(conversion_deadline) else {
        fail_pending_insertion(&work.state, PasteFailure::Unavailable);
        return;
    };
    if !clipboard_sample_is_authorized(
        preclaim_clipboard.hash,
        preclaim_clipboard.change_count,
        work.expected_clipboard_sha256,
        current_clipboard_change_count().unwrap_or(isize::MIN),
    ) {
        fail_pending_insertion(&work.state, PasteFailure::Unavailable);
        return;
    }
    if let Some(reason) = insertion_target_failure(resources, shared, &work, cached) {
        fail_insertion(&work.state, reason);
        return;
    }
    if !clipboard_change_count_is_current(preclaim_clipboard.change_count) {
        fail_pending_insertion(&work.state, PasteFailure::Unavailable);
        return;
    }
    if work
        .state
        .compare_exchange(
            INSERTION_PENDING,
            INSERTION_CLAIMED,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_err()
    {
        return;
    }

    #[cfg(feature = "transactional-shortcuts-dev")]
    pause_after_insertion_claim(work.deadline);

    if insertion_target_failure(resources, shared, &work, cached).is_some()
        || !postclaim_clipboard_revision_is_authorized(
            preclaim_clipboard.change_count,
            current_clipboard_change_count().unwrap_or(isize::MIN),
        )
    {
        mark_claimed_insertion_ambiguous(&work.state);
        return;
    }

    // Restore the exact retained typed AXValue, not a reconstructed scalar.
    // Any timeout/unknown acceptance after claim is terminal ambiguity.
    // SAFETY: the cache lock keeps the control and original typed range retained
    // throughout this synchronous AX call on the worker that captured them.
    let range_error = unsafe {
        ffi::AXUIElementSetAttributeValue(
            cached.evidence.focused_control.as_type_ref().cast_mut(),
            resources.selected_text_range.as_type_ref(),
            cached.evidence.selected_text_range_value.as_type_ref(),
        )
    };
    if !settle_claimed_ax_error(&work.state, range_error) {
        return;
    }

    #[cfg(feature = "transactional-shortcuts-dev")]
    pause_after_range_set(work.deadline);

    if insertion_target_failure(resources, shared, &work, cached).is_some()
        || !postclaim_clipboard_revision_is_authorized(
            preclaim_clipboard.change_count,
            current_clipboard_change_count().unwrap_or(isize::MIN),
        )
        || !clipboard_change_count_is_current(preclaim_clipboard.change_count)
    {
        mark_claimed_insertion_ambiguous(&work.state);
        return;
    }

    // SAFETY: the retained control/range and preclaim-bounded immutable
    // NSString remain live on this worker. Postclaim performs only scalar
    // changeCount and target checks—never a second conversion, hash, or large
    // allocation—immediately before selected-text mutation.
    let text_error = unsafe {
        ffi::AXUIElementSetAttributeValue(
            cached.evidence.focused_control.as_type_ref().cast_mut(),
            resources.selected_text.as_type_ref(),
            preclaim_clipboard.text.as_type_ref(),
        )
    };
    if settle_claimed_ax_error(&work.state, text_error) {
        complete_claimed_insertion(&work.state, true);
    }
}
