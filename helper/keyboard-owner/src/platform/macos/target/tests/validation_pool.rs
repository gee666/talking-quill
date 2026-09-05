use super::*;

#[test]
fn validation_pool_has_fixed_capacity_and_stopped_pool_fails_closed() {
    let pool = Arc::new(ValidationPool::new());
    let mut requests = Vec::new();
    for request_id in 1..=VALIDATION_QUEUE_CAPACITY as u64 {
        let ticket = ValidationTicket {
            request_id,
            start_epoch: 7,
            start_boundary_epoch: 9,
        };
        let (request, _work) = pool.acquire(ticket).expect("fixed slot available");
        requests.push(request);
    }
    assert!(
        pool.acquire(ValidationTicket {
            request_id: 99,
            start_epoch: 7,
            start_boundary_epoch: 9,
        })
        .is_none()
    );
    drop(requests.pop());
    assert!(
        pool.acquire(ValidationTicket {
            request_id: 100,
            start_epoch: 7,
            start_boundary_epoch: 9,
        })
        .is_some()
    );
    pool.stop();
    assert!(
        pool.acquire(ValidationTicket {
            request_id: 101,
            start_epoch: 7,
            start_boundary_epoch: 9,
        })
        .is_none()
    );
}

#[test]
fn worker_shutdown_rejects_pending_publication_and_new_acquisition() {
    let pool = Arc::new(ValidationPool::new());
    let ticket = ValidationTicket {
        request_id: 102,
        start_epoch: 7,
        start_boundary_epoch: 9,
    };
    let (mut request, work) = pool.acquire(ticket).unwrap();
    pool.stop();
    assert!(!pool.publish(
        work,
        ValidationResponse {
            ticket,
            handle: Some(handle(1)),
            observed_epoch: 7,
            observed_boundary_epoch: 9,
            observed_selected_range_epoch: 1,
        }
    ));
    assert!(request.try_response().is_none());
    assert!(
        pool.acquire(ValidationTicket {
            request_id: 103,
            start_epoch: 7,
            start_boundary_epoch: 9,
        })
        .is_none()
    );
}

#[test]
fn full_work_queue_recycles_the_reserved_slot_without_waiting() {
    let shared = Arc::new(CacheShared::new());
    let validation_pool = Arc::new(ValidationPool::new());
    let (requests, queued) = bounded(1);
    let (insertions, _insertion_receiver) = bounded(1);
    let cache = TargetCache {
        shared,
        validation_pool: Arc::clone(&validation_pool),
        requests,
        insertions,
        next_request_id: AtomicU64::new(1),
        worker: None,
        worker_completion: bounded(1).1,
        _test_request_receiver: None,
    };
    let first = cache.request_validation().expect("first work item fits");
    drop(first);
    assert!(cache.request_validation().is_none());
    let _stale = queued.try_recv().unwrap();
    assert!(cache.request_validation().is_some());
}

#[test]
fn stale_aba_work_cannot_publish_into_a_reused_slot() {
    let pool = Arc::new(ValidationPool::new());
    let first_ticket = ValidationTicket {
        request_id: 31,
        start_epoch: 7,
        start_boundary_epoch: 9,
    };
    let (first_request, stale_work) = pool.acquire(first_ticket).unwrap();
    let first_slot = first_request.slot_index;
    let first_generation = first_request.slot_generation;
    drop(first_request);

    let second_ticket = ValidationTicket {
        request_id: 32,
        start_epoch: 7,
        start_boundary_epoch: 9,
    };
    let (mut second_request, second_work) = pool.acquire(second_ticket).unwrap();
    assert_eq!(second_request.slot_index, first_slot);
    assert_ne!(second_request.slot_generation, first_generation);
    assert!(!pool.publish(
        stale_work,
        ValidationResponse {
            ticket: first_ticket,
            handle: Some(handle(1)),
            observed_epoch: 7,
            observed_boundary_epoch: 9,
            observed_selected_range_epoch: 1,
        }
    ));
    assert!(second_request.try_response().is_none());
    assert!(pool.publish(
        second_work,
        ValidationResponse {
            ticket: second_ticket,
            handle: Some(handle(2)),
            observed_epoch: 7,
            observed_boundary_epoch: 9,
            observed_selected_range_epoch: 1,
        }
    ));
    assert_eq!(second_request.try_response().unwrap().ticket, second_ticket);
}

#[test]
fn ready_response_cancellation_reuses_slot_without_exposing_stale_value() {
    let pool = Arc::new(ValidationPool::new());
    let first_ticket = ValidationTicket {
        request_id: 33,
        start_epoch: 7,
        start_boundary_epoch: 9,
    };
    let (first_request, first_work) = pool.acquire(first_ticket).unwrap();
    assert!(pool.publish(
        first_work,
        ValidationResponse {
            ticket: first_ticket,
            handle: Some(handle(1)),
            observed_epoch: 7,
            observed_boundary_epoch: 9,
            observed_selected_range_epoch: 1,
        }
    ));
    drop(first_request);

    let second_ticket = ValidationTicket {
        request_id: 34,
        start_epoch: 7,
        start_boundary_epoch: 9,
    };
    let (mut second_request, second_work) = pool.acquire(second_ticket).unwrap();
    assert!(second_request.try_response().is_none());
    assert!(pool.publish(
        second_work,
        ValidationResponse {
            ticket: second_ticket,
            handle: Some(handle(2)),
            observed_epoch: 7,
            observed_boundary_epoch: 9,
            observed_selected_range_epoch: 1,
        }
    ));
    assert_eq!(second_request.try_response().unwrap().ticket, second_ticket);
}
