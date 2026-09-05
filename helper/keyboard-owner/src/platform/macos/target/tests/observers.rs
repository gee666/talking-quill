use super::*;

#[test]
fn focus_and_programmatic_range_notifications_invalidate_exact_epochs() {
    let cache = TargetCache::without_worker();
    let now = Instant::now();
    cache.publish_for_test(evidence(1), now);
    let focused_control = cache
        .shared
        .slot
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .evidence
        .focused_control
        .retained_clone()
        .unwrap();
    let context = TargetObserverContext {
        shared: Arc::clone(&cache.shared),
        focused_control,
    };
    let epoch = cache.current_epoch();
    let range_epoch = cache.current_selected_range_epoch();
    unsafe {
        target_observer_callback(
            null_mut(),
            context.focused_control.as_type_ref().cast_mut(),
            null(),
            (&raw const context).cast_mut().cast(),
        );
    }
    assert_ne!(cache.current_epoch(), epoch);
    assert_ne!(cache.current_selected_range_epoch(), range_epoch);
    assert!(cache.reserve_activation_at(now).is_none());

    let focus_epoch = cache.current_epoch();
    unsafe {
        target_observer_callback(
            null_mut(),
            null_mut(),
            null(),
            (&raw const context).cast_mut().cast(),
        );
    }
    assert_ne!(cache.current_epoch(), focus_epoch);
}

#[test]
fn same_pid_control_switch_and_range_aba_require_a_fresh_observer_publication() {
    assert!(observer_identity_matches(41, 41, true));
    assert!(!observer_identity_matches(41, 41, false));

    let cache = TargetCache::without_worker();
    let now = Instant::now();
    let control_a = distinct_evidence(41, c"app", c"window", c"control-a");
    let old_context = TargetObserverContext {
        shared: Arc::clone(&cache.shared),
        focused_control: control_a.focused_control.retained_clone().unwrap(),
    };
    cache.publish_for_test(control_a, now);
    let old_publication = cache.reserve_activation_at(now).unwrap().publication_id;

    // Reinstall policy for a same-PID control switch poisons broad/range
    // epochs before any capture under control B can publish.
    cache.shared.invalidate_selected_range();
    let control_b = distinct_evidence(41, c"app", c"window", c"control-b");
    let new_context = TargetObserverContext {
        shared: Arc::clone(&cache.shared),
        focused_control: control_b.focused_control.retained_clone().unwrap(),
    };
    cache.publish_for_test(control_b, now);
    let switched_publication = cache.reserve_activation_at(now).unwrap().publication_id;
    assert_ne!(switched_publication, old_publication);

    // A callback already admitted by the retired A observer can only
    // invalidate B; it cannot authorize B under A's old epochs.
    unsafe {
        target_observer_callback(
            null_mut(),
            old_context.focused_control.as_type_ref().cast_mut(),
            null(),
            (&raw const old_context).cast_mut().cast(),
        );
    }
    assert!(cache.reserve_activation_at(now).is_none());

    // Programmatic B range move-and-return is still an ABA: both callbacks
    // advance the independent range epoch, so the switched publication
    // never becomes current again merely because the scalar range matches.
    for _ in 0..2 {
        unsafe {
            target_observer_callback(
                null_mut(),
                new_context.focused_control.as_type_ref().cast_mut(),
                null(),
                (&raw const new_context).cast_mut().cast(),
            );
        }
    }
    assert!(cache.reserve_activation_at(now).is_none());
    cache.publish_for_test(distinct_evidence(41, c"app", c"window", c"control-b"), now);
    assert_ne!(
        cache.reserve_activation_at(now).unwrap().publication_id,
        switched_publication
    );
}

#[test]
fn workspace_retirement_waits_for_admitted_callback_arc() {
    let shared = Arc::new(CacheShared::new());
    let object = unsafe {
        ffi::objc_msgSend(
            ffi::objc_getClass(c"NSObject".as_ptr()).cast(),
            ffi::sel_registerName(c"new".as_ptr()),
        )
    };
    assert!(!object.is_null());
    let observer = object as usize;
    workspace_callback_registry()
        .0
        .lock()
        .unwrap()
        .entries
        .insert(
            observer,
            WorkspaceCallbackEntry {
                shared: Arc::clone(&shared),
                active: 0,
                retiring: false,
            },
        );
    let invocation = WorkspaceInvocation::begin(observer as ffi::ObjcId).unwrap();
    let (started_tx, started_rx) = bounded(1);
    let (done_tx, done_rx) = bounded(1);
    let retire = thread::spawn(move || {
        started_tx.send(()).unwrap();
        retire_workspace_callback(observer);
        done_tx.send(()).unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert!(done_rx.recv_timeout(Duration::from_millis(10)).is_err());
    drop(invocation);
    done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    retire.join().unwrap();
    assert_eq!(Arc::strong_count(&shared), 1);
    unsafe {
        let _ = ffi::objc_msgSend(object, ffi::sel_registerName(c"release".as_ptr()));
    }
}
