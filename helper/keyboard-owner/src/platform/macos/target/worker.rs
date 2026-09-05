//! AX run-loop scheduling, observer-confirmed capture, and publication.

use super::*;

pub(super) fn service_worker_notifications(seconds: f64) {
    // SAFETY: called only by the AX worker that owns its current run loop.
    unsafe {
        let _ = ffi::CFRunLoopRunInMode(ffi::kCFRunLoopDefaultMode, seconds, false);
    }
}

pub(super) fn capture_observer_confirmed(
    resources: &TargetCaptureResources,
    shared: &Arc<CacheShared>,
    run_loop: ffi::CFRunLoopRef,
    observer: &mut Option<WorkerObserver>,
) -> Option<ConfirmedTarget> {
    // Flush already-signalled observer/workspace sources before establishing
    // the exact epoch that brackets the complete AX double sample.
    service_worker_notifications(0.000_1);
    let epoch_before = shared.current_notification_epoch();
    let boundary_before = shared.current_boundary_epoch();
    let selected_range_before = shared.current_selected_range_epoch();
    let evidence = capture_target(resources)?;
    if observer
        .as_ref()
        .is_none_or(|current| !current.observes_control(&evidence))
    {
        // PID/application/window continuity is not focused-control identity.
        // Retire the old control observer first, poison both broad and range
        // epochs, and require a later double capture under the new observer.
        *observer = None;
        shared.invalidate_selected_range();
        *observer = WorkerObserver::install(run_loop, shared, resources, &evidence).ok();
        return None;
    }
    // Drain notifications that raced any AX query. Evidence carries the exact
    // validated epoch; no caller may reload a newer epoch and relabel it.
    service_worker_notifications(0.001);
    let epoch_after = shared.current_notification_epoch();
    let boundary_after = shared.current_boundary_epoch();
    let selected_range_after = shared.current_selected_range_epoch();
    (epoch_before == epoch_after
        && boundary_before == boundary_after
        && selected_range_before == selected_range_after)
        .then_some(ConfirmedTarget {
            evidence,
            notification_epoch: epoch_before,
            boundary_epoch: boundary_before,
            selected_range_epoch: selected_range_before,
        })
}

pub(super) fn publish_confirmed_target(
    shared: &Arc<CacheShared>,
    confirmed: ConfirmedTarget,
    last_target: &mut Option<TargetEvidence>,
) {
    if confirmed.notification_epoch != shared.current_notification_epoch()
        || confirmed.boundary_epoch != shared.current_boundary_epoch()
        || confirmed.selected_range_epoch != shared.current_selected_range_epoch()
    {
        return;
    }
    let changed = last_target
        .as_ref()
        .is_some_and(|previous| !same_target(previous, &confirmed.evidence));
    let Ok(retained_for_comparison) = confirmed.evidence.retained_clone() else {
        *last_target = None;
        shared.invalidate_notification();
        return;
    };
    if changed {
        // A tuple change without a delivered notification is itself focus
        // evidence. Advance the epoch and discard this pre-invalidation sample;
        // only a subsequent capture may publish under the new exact epoch.
        *last_target = None;
        shared.invalidate_notification();
        return;
    }
    let mut slot = shared
        .slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if confirmed.notification_epoch == shared.current_notification_epoch()
        && confirmed.boundary_epoch == shared.current_boundary_epoch()
        && confirmed.selected_range_epoch == shared.current_selected_range_epoch()
    {
        let publication_id = slot
            .as_ref()
            .filter(|cached| {
                cached.notification_epoch == confirmed.notification_epoch
                    && same_target(&cached.evidence, &confirmed.evidence)
            })
            .map_or_else(
                || {
                    shared
                        .published_id
                        .fetch_add(1, Ordering::AcqRel)
                        .wrapping_add(1)
                },
                |cached| cached.publication_id,
            );
        if publication_id == 0 {
            shared.invalidate_notification();
            return;
        }
        *slot = Some(CachedTarget {
            evidence: confirmed.evidence,
            captured_at: Instant::now(),
            notification_epoch: confirmed.notification_epoch,
            boundary_epoch: confirmed.boundary_epoch,
            selected_range_epoch: confirmed.selected_range_epoch,
            publication_id,
        });
        *last_target = Some(retained_for_comparison);
    }
}

pub(super) fn target_cache_worker(
    shared: Arc<CacheShared>,
    validation_pool: Arc<ValidationPool>,
    requests: Receiver<ValidationWork>,
    insertions: Receiver<InsertionWork>,
) {
    let Ok(_worker_pool) = AutoreleasePool::push() else {
        shared.invalidate_notification();
        validation_pool.stop();
        return;
    };
    target_cache_worker_inner(&shared, &validation_pool, &requests, &insertions);
    validation_pool.stop();
}

pub(super) fn target_cache_worker_inner(
    shared: &Arc<CacheShared>,
    validation_pool: &ValidationPool,
    requests: &Receiver<ValidationWork>,
    insertions: &Receiver<InsertionWork>,
) {
    let Ok(resources) = TargetCaptureResources::new() else {
        shared.invalidate_notification();
        return;
    };
    // SAFETY: this worker owns and services its current Core Foundation loop.
    let run_loop = unsafe { ffi::CFRunLoopGetCurrent() };
    let Ok(_workspace_observer) = WorkspaceObserver::install(shared) else {
        shared.invalidate_notification();
        return;
    };
    let mut observer: Option<WorkerObserver> = None;
    let mut last_target: Option<TargetEvidence> = None;
    while !shared.stopping.load(Ordering::Acquire) {
        let Ok(_iteration_pool) = AutoreleasePool::push() else {
            shared.invalidate_notification();
            break;
        };
        service_worker_notifications(TARGET_CACHE_REFRESH_INTERVAL.as_secs_f64());
        while let Ok(work) = insertions.try_recv() {
            process_insertion_work(&resources, shared, work);
        }
        while let Ok(request) = requests.try_recv() {
            if !validation_pool.is_pending(request) {
                continue;
            }
            service_worker_notifications(0.001);
            let confirmed = capture_observer_confirmed(&resources, shared, run_loop, &mut observer);
            let observed_epoch = confirmed.as_ref().map_or_else(
                || shared.current_notification_epoch(),
                |value| value.notification_epoch,
            );
            let observed_boundary_epoch = confirmed.as_ref().map_or_else(
                || shared.current_boundary_epoch(),
                |value| value.boundary_epoch,
            );
            let observed_selected_range_epoch = confirmed.as_ref().map_or_else(
                || shared.current_selected_range_epoch(),
                |value| value.selected_range_epoch,
            );
            let pre_boundary_confirmed = confirmed.as_ref().is_some_and(|confirmed| {
                if request.expected_publication_id == 0 {
                    return true;
                }
                shared
                    .slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_ref()
                    .is_some_and(|cached| {
                        cached.publication_id == request.expected_publication_id
                            && cached.notification_epoch == request.ticket.start_epoch
                            && same_target(&cached.evidence, &confirmed.evidence)
                    })
            });
            let handle = confirmed.as_ref().and_then(|confirmed| {
                (pre_boundary_confirmed
                    && request.expected_publication_id != 0
                    && request.ticket.start_epoch == confirmed.notification_epoch
                    && request.ticket.start_boundary_epoch == confirmed.boundary_epoch
                    && confirmed.notification_epoch == shared.current_notification_epoch()
                    && confirmed.boundary_epoch == shared.current_boundary_epoch()
                    && confirmed.selected_range_epoch == shared.current_selected_range_epoch())
                .then_some(TargetHandle {
                    publication_id: request.expected_publication_id,
                })
            });
            let _ = validation_pool.publish(
                request,
                ValidationResponse {
                    ticket: request.ticket,
                    handle,
                    observed_epoch,
                    observed_boundary_epoch,
                    observed_selected_range_epoch,
                },
            );
        }
        if let Some(confirmed) =
            capture_observer_confirmed(&resources, shared, run_loop, &mut observer)
        {
            publish_confirmed_target(shared, confirmed, &mut last_target);
        } else if last_target.take().is_some() {
            shared.invalidate_notification();
        }
    }
    drop(observer);
    // Release cached CF/AX evidence on the AX worker, never from the event tap
    // or owner teardown thread.
    *shared
        .slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}
