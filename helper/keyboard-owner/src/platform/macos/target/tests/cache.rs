use super::*;

#[test]
fn cache_reservation_is_nondestructive_epoch_index_only_and_fresh() {
    let cache = TargetCache::without_worker();
    let now = Instant::now();
    cache.publish_for_test(evidence(1), now);
    let first = cache.reserve_activation_at(now).unwrap();
    let second = cache.reserve_activation_at(now).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.publication_id, 1);

    cache.publish_for_test(
        evidence(2),
        now - TARGET_CACHE_MAX_AGE - Duration::from_millis(1),
    );
    assert!(cache.reserve_activation_at(now).is_none());

    cache.publish_for_test(evidence(3), now);
    cache.invalidate_boundary();
    assert!(cache.reserve_activation_at(now).is_none());
}

#[test]
fn notification_epoch_change_rejects_old_tuple_and_accepts_only_new_publication() {
    let cache = TargetCache::without_worker();
    let now = Instant::now();
    cache.publish_for_test(evidence(1), now);
    cache.shared.invalidate_notification();
    assert!(cache.reserve_activation_at(now).is_none());
    cache.publish_for_test(evidence(2), now);
    assert_eq!(cache.reserve_activation_at(now).unwrap().publication_id, 2);
}

#[test]
fn candidate_reservation_tracks_target_and_range_but_not_keyboard_boundaries() {
    let cache = TargetCache::without_worker();
    cache.shared.notification_epoch.store(7, Ordering::Release);
    cache.shared.boundary_epoch.store(11, Ordering::Release);
    cache.publish_for_test(evidence(3), Instant::now());
    let reservation = ActivationReservation {
        notification_epoch: 7,
        boundary_epoch: 11,
        selected_range_epoch: 1,
        publication_id: 1,
    };
    assert!(cache.reservation_is_current(&reservation));
    cache.invalidate_boundary();
    assert!(cache.reservation_is_current(&reservation));
    cache.shared.invalidate_selected_range();
    assert!(!cache.reservation_is_current(&reservation));
}

#[test]
fn activation_before_after_proof_rejects_every_epoch_or_worker_race() {
    let reservation = ActivationReservation {
        notification_epoch: 7,
        boundary_epoch: 11,
        selected_range_epoch: 13,
        publication_id: 3,
    };
    assert!(reservation.confirms_after_boundary(7, 12, 7, 12, true));
    assert!(!reservation.confirms_after_boundary(8, 12, 8, 12, true));
    assert!(!reservation.confirms_after_boundary(7, 11, 7, 11, true));
    assert!(!reservation.confirms_after_boundary(7, 12, 8, 12, true));
    assert!(!reservation.confirms_after_boundary(7, 12, 7, 12, false));
}

#[test]
fn unchanged_target_refresh_preserves_scalar_publication_handle() {
    let cache = TargetCache::without_worker();
    let now = Instant::now();
    cache.publish_for_test(evidence(1), now);
    let first = cache.reserve_activation_at(now).unwrap().publication_id;
    let mut last = Some(evidence(1));
    publish_confirmed_target(
        &cache.shared,
        ConfirmedTarget {
            evidence: evidence(1),
            notification_epoch: cache.current_epoch(),
            boundary_epoch: cache.current_boundary_epoch(),
            selected_range_epoch: cache.current_selected_range_epoch(),
        },
        &mut last,
    );
    assert_eq!(
        cache
            .reserve_activation_at(Instant::now())
            .unwrap()
            .publication_id,
        first
    );
}

#[test]
fn publication_never_relabels_evidence_with_a_newer_epoch() {
    let cache = TargetCache::without_worker();
    let validated_epoch = cache.current_epoch();
    let confirmed = ConfirmedTarget {
        evidence: evidence(1),
        notification_epoch: validated_epoch,
        boundary_epoch: cache.shared.current_boundary_epoch(),
        selected_range_epoch: cache.shared.current_selected_range_epoch(),
    };
    cache.shared.invalidate_notification();
    let mut last = None;
    publish_confirmed_target(&cache.shared, confirmed, &mut last);
    assert!(cache.shared.slot.lock().unwrap().is_none());
    assert!(last.is_none());
}

#[test]
fn selected_range_epoch_invalidates_final_retained_handle_check() {
    let cache = TargetCache::without_worker();
    let handle = handle(9);
    cache.install_current_handle_for_test(handle, 11, 13);
    assert!(cache.handle_is_current(handle, 11, 13, 1));
    cache.shared.invalidate_selected_range();
    assert!(!cache.handle_is_current(handle, 11, 13, 1));
}

#[test]
fn worker_notification_epoch_after_sample_invalidates_immediate_post_proof() {
    let cache = TargetCache::without_worker();
    let validated_epoch = cache.current_epoch();
    assert_eq!(cache.current_epoch(), validated_epoch);
    cache.shared.invalidate_notification();
    assert_ne!(cache.current_epoch(), validated_epoch);
}

#[test]
fn cache_read_never_waits_for_worker_slot_lock() {
    let cache = TargetCache::without_worker();
    let _worker_guard = cache.shared.slot.lock().unwrap();
    assert!(cache.reserve_activation_at(Instant::now()).is_none());
}
