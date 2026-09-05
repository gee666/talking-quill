//! Windows audio-input invalidation monitor.
mod com;
mod monitor;
mod notification;
mod state;
mod topology;
mod worker;

pub(super) use worker::AudioDeviceMonitor;

#[cfg(test)]
mod tests {
    use super::{
        com::NotificationClientInterface, notification::*, state::*, topology::CaptureEndpoint,
        worker::finish_worker,
    };
    use crate::platform::{CallbackGate, TerminalSignal};
    use crossbeam_channel::bounded;
    use std::{
        ffi::c_void,
        ptr::{null, null_mut},
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicU32, Ordering},
        },
        thread,
        time::Duration,
    };
    use windows_sys::Win32::Media::Audio::{eCommunications, eMultimedia, eRender};
    use windows_sys::{
        Win32::{
            Foundation::{E_NOINTERFACE, PROPERTYKEY, S_OK},
            Media::Audio::{eCapture, eConsole},
        },
        core::GUID,
    };

    fn test_worker_state(initial_pending: u32) -> (Arc<AudioWorkerState>, Arc<CallbackGate>) {
        let gate = Arc::new(CallbackGate::new());
        gate.open();
        let (terminal_tx, _terminal_rx) = bounded(1);
        let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
        let state = Arc::new(AudioWorkerState::new(terminal));
        state.pending.store(initial_pending, Ordering::Release);
        (state, gate)
    }

    fn test_client() -> (*mut NotificationClient, Arc<AudioWorkerState>) {
        let (state, _gate) = test_worker_state(0);
        let client = Box::into_raw(Box::new(NotificationClient {
            vtable: &raw const NOTIFICATION_CLIENT_VTABLE,
            references: AtomicU32::new(1),
            state: Arc::clone(&state),
        }));
        (client, state)
    }

    #[test]
    fn default_filter_accepts_only_capture_console_changes() {
        assert!(is_relevant_default_change(eCapture, eConsole));
        for (flow, role) in [
            (eRender, eConsole),
            (eCapture, eMultimedia),
            (eCapture, eCommunications),
        ] {
            assert!(!is_relevant_default_change(flow, role));
        }
    }

    #[test]
    fn callbacks_ignore_render_roles_and_properties_but_mark_topology() {
        let (client, state) = test_client();
        let interface = client.cast::<NotificationClientInterface>();
        // SAFETY: the test owns a live callback object. Ignored callbacks never
        // post, and the topology callback observes an already-pending batch.
        unsafe {
            assert_eq!(
                on_default_device_changed(interface, eRender, eConsole, null()),
                S_OK
            );
            assert_eq!(
                on_default_device_changed(interface, eCapture, eMultimedia, null()),
                S_OK
            );
            assert_eq!(
                on_property_value_changed(interface, null(), PROPERTYKEY::default()),
                S_OK
            );
            assert_eq!(state.pending.load(Ordering::Acquire), 0);
            state.pending.store(CHANGE_DEFAULT, Ordering::Release);
            assert_eq!(on_device_added(interface, null()), S_OK);
            assert_eq!(
                state.pending.load(Ordering::Acquire),
                CHANGE_DEFAULT | CHANGE_TOPOLOGY
            );
            notification_release(client.cast::<c_void>());
        }
    }

    #[test]
    fn preinitialize_changes_remain_dirty_until_protocol_ready() {
        let pending = AtomicU32::new(INITIAL_CHANGES);
        let protocol_ready = AtomicBool::new(false);
        let wakes = AtomicU32::new(0);
        assert!(coalesce_change(&pending, CHANGE_DEFAULT, || {
            wakes.fetch_add(1, Ordering::Relaxed);
            true
        }));
        assert_eq!(wakes.load(Ordering::Relaxed), 0);
        assert_eq!(take_ready_changes(&pending, &protocol_ready), 0);
        assert_eq!(
            pending.load(Ordering::Acquire),
            INITIAL_CHANGES | CHANGE_DEFAULT
        );

        assert!(mark_protocol_ready(&protocol_ready, || {
            wakes.fetch_add(1, Ordering::Relaxed);
            true
        }));
        assert_eq!(wakes.load(Ordering::Relaxed), 1);
        assert_eq!(
            take_ready_changes(&pending, &protocol_ready),
            INITIAL_CHANGES | CHANGE_DEFAULT
        );
    }

    #[test]
    fn every_fresh_worker_forces_one_restart_resynchronization() {
        for _restart in 0..2 {
            let (state, _gate) = test_worker_state(INITIAL_CHANGES);
            state.protocol_ready.store(true, Ordering::Release);
            let changes = take_ready_changes(&state.pending, &state.protocol_ready);
            assert_ne!(changes & CHANGE_FORCE_SYNC, 0);
            assert_ne!(changes & CHANGE_TOPOLOGY, 0);
            assert_eq!(take_ready_changes(&state.pending, &state.protocol_ready), 0);
        }
    }

    #[test]
    fn coalescer_posts_once_until_the_worker_drains_then_rearms() {
        let pending = AtomicU32::new(0);
        let wakes = AtomicU32::new(0);
        for change in [CHANGE_DEFAULT, CHANGE_TOPOLOGY, CHANGE_DEFAULT] {
            assert!(coalesce_change(&pending, change, || {
                wakes.fetch_add(1, Ordering::Relaxed);
                true
            }));
        }
        assert_eq!(wakes.load(Ordering::Relaxed), 1);
        assert_eq!(
            pending.swap(0, Ordering::AcqRel),
            CHANGE_DEFAULT | CHANGE_TOPOLOGY
        );
        assert!(coalesce_change(&pending, CHANGE_TOPOLOGY, || {
            wakes.fetch_add(1, Ordering::Relaxed);
            true
        }));
        assert_eq!(wakes.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn failed_worker_wake_clears_pending_for_a_later_attempt() {
        let pending = AtomicU32::new(0);
        assert!(!coalesce_change(&pending, CHANGE_DEFAULT, || false));
        assert_eq!(pending.load(Ordering::Acquire), 0);
        assert!(coalesce_change(&pending, CHANGE_DEFAULT, || true));
        assert_eq!(pending.load(Ordering::Acquire), CHANGE_DEFAULT);
    }

    #[test]
    fn callback_lifecycle_suppresses_changes_after_deactivation() {
        let (state, _gate) = test_worker_state(CHANGE_DEFAULT);
        state.queue_change(CHANGE_TOPOLOGY);
        assert_eq!(
            state.pending.load(Ordering::Acquire),
            CHANGE_DEFAULT | CHANGE_TOPOLOGY
        );
        state.active.store(false, Ordering::Release);
        state.pending.store(0, Ordering::Release);
        state.queue_change(CHANGE_DEFAULT);
        assert_eq!(state.pending.load(Ordering::Acquire), 0);
    }

    #[test]
    fn notification_client_query_interface_and_references_are_com_correct() {
        let (client, _state) = test_client();
        assert_eq!(std::mem::offset_of!(NotificationClient, vtable), 0);
        let callback_interface = client.cast::<NotificationClientInterface>();
        let mut interface = null_mut();
        // SAFETY: the test owns a live callback object and invokes QueryInterface
        // through the same ABI/vtable path used by Core Audio.
        unsafe {
            assert_eq!(
                ((*(*callback_interface).vtable).base.query_interface)(
                    callback_interface.cast::<c_void>(),
                    &IID_IMM_NOTIFICATION_CLIENT,
                    &raw mut interface,
                ),
                S_OK
            );
            assert_eq!(interface, client.cast::<c_void>());
            assert_eq!((*client).references.load(Ordering::Acquire), 2);
            assert_eq!(notification_release(interface), 1);

            let unsupported = GUID::from_u128(0x11111111_2222_3333_4444_555555555555);
            interface = std::ptr::dangling_mut::<c_void>();
            assert_eq!(
                notification_query_interface(
                    client.cast::<c_void>(),
                    &unsupported,
                    &raw mut interface,
                ),
                E_NOINTERFACE
            );
            assert!(interface.is_null());
            assert_eq!(notification_release(client.cast::<c_void>()), 0);
        }
    }

    #[test]
    fn capture_topology_comparison_is_order_independent_and_state_sensitive() {
        let mut first = vec![
            CaptureEndpoint {
                id: vec![2],
                state: 1,
            },
            CaptureEndpoint {
                id: vec![1],
                state: 1,
            },
        ];
        first.sort_unstable();
        let same = vec![
            CaptureEndpoint {
                id: vec![1],
                state: 1,
            },
            CaptureEndpoint {
                id: vec![2],
                state: 1,
            },
        ];
        assert_eq!(first, same);
        let changed = vec![
            CaptureEndpoint {
                id: vec![1],
                state: 1,
            },
            CaptureEndpoint {
                id: vec![2],
                state: 8,
            },
        ];
        assert_ne!(first, changed);
    }

    #[test]
    fn worker_shutdown_wait_is_bounded_and_joins_only_after_completion() {
        let (completion_tx, completion_rx) = bounded(1);
        let blocked = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            let _ = completion_tx.send(Ok(()));
        });
        let started = std::time::Instant::now();
        assert!(finish_worker(blocked, &completion_rx, Duration::from_millis(1)).is_err());
        assert!(started.elapsed() < Duration::from_millis(25));

        let (completion_tx, completion_rx) = bounded(1);
        let (release_tx, release_rx) = bounded(1);
        let signalled_but_running = thread::spawn(move || {
            let _ = completion_tx.send(Ok(()));
            let _ = release_rx.recv();
        });
        let started = std::time::Instant::now();
        assert!(
            finish_worker(
                signalled_but_running,
                &completion_rx,
                Duration::from_millis(1)
            )
            .is_err()
        );
        assert!(started.elapsed() < Duration::from_millis(25));
        let _ = release_tx.send(());

        let (completion_tx, completion_rx) = bounded(1);
        let completed = thread::spawn(move || {
            let _ = completion_tx.send(Ok(()));
        });
        assert!(finish_worker(completed, &completion_rx, Duration::from_secs(1)).is_ok());
    }
}
