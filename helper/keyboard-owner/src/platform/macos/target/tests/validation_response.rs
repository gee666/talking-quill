use super::*;

#[test]
fn simultaneous_activation_and_paste_replies_cannot_cross_consume() {
    let activation_ticket = ValidationTicket {
        request_id: 41,
        start_epoch: 7,
        start_boundary_epoch: 9,
    };
    let paste_ticket = ValidationTicket {
        request_id: 42,
        start_epoch: 7,
        start_boundary_epoch: 9,
    };
    let pool = Arc::new(ValidationPool::new());
    let (mut activation_request, activation_work) = pool.acquire(activation_ticket).unwrap();
    let (mut paste_request, paste_work) = pool.acquire(paste_ticket).unwrap();

    // Worker completion order is intentionally reversed. Each request has
    // a fixed private slot, so polling activation cannot remove paste's
    // response or vice versa.
    assert!(pool.publish(
        paste_work,
        ValidationResponse {
            ticket: paste_ticket,
            handle: Some(handle(2)),
            observed_epoch: 7,
            observed_boundary_epoch: 9,
            observed_selected_range_epoch: 1,
        }
    ));
    assert!(activation_request.try_response().is_none());
    assert_eq!(paste_request.try_response().unwrap().ticket, paste_ticket);

    assert!(pool.publish(
        activation_work,
        ValidationResponse {
            ticket: activation_ticket,
            handle: Some(handle(1)),
            observed_epoch: 7,
            observed_boundary_epoch: 9,
            observed_selected_range_epoch: 1,
        }
    ));
    assert_eq!(
        activation_request.try_response().unwrap().ticket,
        activation_ticket
    );
    assert!(paste_request.try_response().is_none());
}

#[test]
fn cancelled_validation_drops_its_bounded_reply_without_affecting_others() {
    let ticket = ValidationTicket {
        request_id: 43,
        start_epoch: 7,
        start_boundary_epoch: 9,
    };
    let pool = Arc::new(ValidationPool::new());
    let (request, work) = pool.acquire(ticket).unwrap();
    drop(request);
    assert!(!pool.publish(
        work,
        ValidationResponse {
            ticket,
            handle: None,
            observed_epoch: 7,
            observed_boundary_epoch: 9,
            observed_selected_range_epoch: 1,
        }
    ));
}

#[test]
fn validation_request_epoch_rejects_notification_races() {
    let cache = TargetCache::without_worker();
    let ticket = ValidationTicket {
        request_id: 7,
        start_epoch: cache.current_epoch(),
        start_boundary_epoch: cache.current_boundary_epoch(),
    };
    let accepted = ValidationResponse {
        ticket,
        handle: Some(handle(1)),
        observed_epoch: ticket.start_epoch,
        observed_boundary_epoch: ticket.start_boundary_epoch,
        observed_selected_range_epoch: 1,
    };
    assert_eq!(
        accepted
            .into_current_handle(
                ticket,
                cache.current_epoch(),
                cache.current_boundary_epoch(),
                cache.current_selected_range_epoch(),
            )
            .unwrap()
            .0,
        handle(1)
    );

    let raced = ValidationResponse {
        ticket,
        handle: Some(handle(2)),
        observed_epoch: ticket.start_epoch,
        observed_boundary_epoch: ticket.start_boundary_epoch,
        observed_selected_range_epoch: 1,
    };
    cache.shared.invalidate_notification();
    assert!(
        raced
            .into_current_handle(
                ticket,
                cache.current_epoch(),
                cache.current_boundary_epoch(),
                cache.current_selected_range_epoch(),
            )
            .is_none()
    );

    let boundary_ticket = ValidationTicket {
        request_id: 8,
        start_epoch: cache.current_epoch(),
        start_boundary_epoch: cache.current_boundary_epoch(),
    };
    let boundary_raced = ValidationResponse {
        ticket: boundary_ticket,
        handle: Some(handle(3)),
        observed_epoch: boundary_ticket.start_epoch,
        observed_boundary_epoch: boundary_ticket.start_boundary_epoch,
        observed_selected_range_epoch: 1,
    };
    cache.invalidate_boundary();
    assert!(
        boundary_raced
            .into_current_handle(
                boundary_ticket,
                cache.current_epoch(),
                cache.current_boundary_epoch(),
                cache.current_selected_range_epoch(),
            )
            .is_none()
    );
}
