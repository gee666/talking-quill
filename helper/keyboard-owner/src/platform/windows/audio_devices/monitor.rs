//! Worker-thread COM registration, invalidation delivery, and ordered teardown.
use super::{
    com::{
        DeviceEnumerator, IID_IMM_DEVICE_ENUMERATOR, NotificationClientInterface, release_unknown,
    },
    notification::{NOTIFICATION_CLIENT_VTABLE, NotificationClient},
    state::{
        AudioWorkerState, CHANGE_DEFAULT, CHANGE_FORCE_SYNC, CHANGE_TOPOLOGY, take_ready_changes,
    },
    topology::{CaptureEndpoint, capture_topology},
};
use crate::platform::{CallbackGate, NativeEvent, PlatformError, TerminalReason, TerminalSignal};
use crossbeam_channel::Sender;
use std::{
    ffi::c_void,
    ptr::{null, null_mut},
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
};
use windows_sys::Win32::{
    Foundation::S_OK,
    Media::Audio::MMDeviceEnumerator,
    System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    },
};

struct ComApartment;

impl ComApartment {
    fn initialize() -> Result<Self, PlatformError> {
        // SAFETY: the dedicated worker initializes and uninitializes this
        // apartment on the same thread and passes no reserved pointer.
        let result = unsafe { CoInitializeEx(null(), COINIT_MULTITHREADED as u32) };
        if result < 0 {
            Err(PlatformError::NativeFailure)
        } else {
            Ok(Self)
        }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: every successful CoInitializeEx above owns exactly one
        // matching worker-thread uninitialization.
        unsafe { CoUninitialize() };
    }
}

pub(super) struct CoreAudioMonitor {
    outbound: Sender<NativeEvent>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
    state: Arc<AudioWorkerState>,
    enumerator: *mut DeviceEnumerator,
    client: *mut NotificationClient,
    topology: Option<Vec<CaptureEndpoint>>,
    registered: bool,
    apartment: Option<ComApartment>,
}

impl CoreAudioMonitor {
    pub(super) fn start(
        outbound: Sender<NativeEvent>,
        gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
        state: Arc<AudioWorkerState>,
    ) -> Result<Self, PlatformError> {
        let apartment = ComApartment::initialize()?;
        let mut enumerator = null_mut::<DeviceEnumerator>();
        // SAFETY: COM is initialized on this worker, both IIDs are fixed system
        // identifiers, aggregation is disabled, and `enumerator` is writable.
        let created = unsafe {
            CoCreateInstance(
                &MMDeviceEnumerator,
                null_mut(),
                CLSCTX_ALL,
                &IID_IMM_DEVICE_ENUMERATOR,
                (&raw mut enumerator).cast::<*mut c_void>(),
            )
        };
        if created < 0 || enumerator.is_null() {
            if !enumerator.is_null() {
                // SAFETY: defensively release any interface returned alongside
                // a failing HRESULT before the apartment is uninitialized.
                unsafe { release_unknown(enumerator.cast::<c_void>()) };
            }
            return Err(PlatformError::NativeFailure);
        }

        let client = Box::into_raw(Box::new(NotificationClient {
            vtable: &raw const NOTIFICATION_CLIENT_VTABLE,
            references: AtomicU32::new(1),
            state: Arc::clone(&state),
        }));
        let mut monitor = Self {
            outbound,
            gate,
            terminal,
            state,
            enumerator,
            client,
            topology: None,
            registered: false,
            apartment: Some(apartment),
        };
        // SAFETY: both interfaces are live and retained by `monitor`; the
        // callback's initial application reference remains held until shutdown.
        let registered = unsafe {
            ((*(*monitor.enumerator).vtable).register_endpoint_notification_callback)(
                monitor.enumerator,
                monitor.client.cast::<NotificationClientInterface>(),
            )
        };
        if registered < 0 {
            return Err(PlatformError::NativeFailure);
        }
        monitor.registered = true;
        Ok(monitor)
    }

    pub(super) fn drain_pending(&mut self) {
        if !self.state.active.load(Ordering::Acquire)
            || self.terminal.is_triggered()
            || !self.gate.is_open()
        {
            return;
        }
        let changes = take_ready_changes(&self.state.pending, &self.state.protocol_ready);
        if changes == 0
            || !self.state.active.load(Ordering::Acquire)
            || self.terminal.is_triggered()
        {
            return;
        }

        let topology_changed = if changes & CHANGE_TOPOLOGY != 0 {
            // SAFETY: this worker retains the enumerator until shutdown.
            match unsafe { capture_topology(self.enumerator) } {
                Ok(topology) => {
                    let changed = self
                        .topology
                        .as_ref()
                        .is_none_or(|previous| previous != &topology);
                    self.topology = Some(topology);
                    changed
                }
                Err(_) => {
                    self.terminal
                        .trigger(TerminalReason::AudioDeviceMonitorUnavailable);
                    return;
                }
            }
        } else {
            false
        };
        let should_notify = changes & (CHANGE_DEFAULT | CHANGE_FORCE_SYNC) != 0 || topology_changed;
        if !should_notify
            || !self.state.active.load(Ordering::Acquire)
            || self.terminal.is_triggered()
        {
            return;
        }
        let Some(_delivery) = self.gate.try_acquire_delivery() else {
            return;
        };
        if self
            .outbound
            .try_send(NativeEvent::AudioInputDevicesChanged)
            .is_err()
        {
            self.terminal
                .trigger(TerminalReason::OutboundQueueUnavailable);
        }
    }

    pub(super) fn shutdown(&mut self) -> Result<(), PlatformError> {
        if self.client.is_null() {
            return Ok(());
        }
        // Callback admission closes synchronously from the coordinator before
        // this worker can enter a potentially slow Core Audio unregister call.
        self.state.active.store(false, Ordering::Release);
        self.state.pending.store(0, Ordering::Release);
        let unregister_result = if self.registered && !self.enumerator.is_null() {
            // SAFETY: registration used this exact enumerator/client pair.
            unsafe {
                ((*(*self.enumerator).vtable).unregister_endpoint_notification_callback)(
                    self.enumerator,
                    self.client.cast::<NotificationClientInterface>(),
                )
            }
        } else {
            S_OK
        };
        self.registered = false;

        // SAFETY: release the monitor's initial callback reference after
        // unregister, then release the enumerator before COM uninitializes.
        unsafe { release_unknown(self.client.cast::<c_void>()) };
        self.client = null_mut();
        if !self.enumerator.is_null() {
            // SAFETY: `enumerator` is the one successful CoCreateInstance result.
            unsafe { release_unknown(self.enumerator.cast::<c_void>()) };
            self.enumerator = null_mut();
        }
        drop(self.apartment.take());
        if unregister_result < 0 {
            Err(PlatformError::NativeFailure)
        } else {
            Ok(())
        }
    }
}

impl Drop for CoreAudioMonitor {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
