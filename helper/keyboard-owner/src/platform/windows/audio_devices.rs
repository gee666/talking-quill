use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    ptr::{null, null_mut},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering, fence},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crossbeam_channel::{Receiver, Sender, bounded};
use windows_sys::{
    Win32::{
        Foundation::{E_NOINTERFACE, E_POINTER, PROPERTYKEY, S_OK},
        Media::Audio::{
            DEVICE_STATEMASK_ALL, EDataFlow, ERole, MMDeviceEnumerator, eCapture, eConsole,
        },
        System::{
            Com::{
                CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
                CoUninitialize,
            },
            Threading::GetCurrentThreadId,
        },
        UI::WindowsAndMessaging::{
            GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_APP, WM_QUIT,
        },
    },
    core::{GUID, HRESULT, IID_IUnknown, PCWSTR, PWSTR},
};

use crate::{
    platform::NativeEvent,
    platform::{CallbackGate, PlatformError, TerminalReason, TerminalSignal},
};

const WM_AUDIO_INPUT_DEVICES_CHANGED: u32 = WM_APP + 0x46;
const WM_AUDIO_PROTOCOL_READY: u32 = WM_APP + 0x47;
const AUDIO_STARTUP_TIMEOUT: Duration = Duration::from_secs(2);
const AUDIO_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);
const IID_IMM_NOTIFICATION_CLIENT: GUID = GUID::from_u128(0x7991eec9_7e89_4d85_8390_6c703cec60c0);
const IID_IMM_DEVICE_ENUMERATOR: GUID = GUID::from_u128(0xa95664d2_9614_4f35_a746_de8db63617e6);
const CHANGE_DEFAULT: u32 = 1 << 0;
const CHANGE_TOPOLOGY: u32 = 1 << 1;
const CHANGE_FORCE_SYNC: u32 = 1 << 2;
const INITIAL_CHANGES: u32 = CHANGE_TOPOLOGY | CHANGE_FORCE_SYNC;
const MAX_ENDPOINT_ID_CODE_UNITS: usize = 32 * 1024;

#[repr(C)]
struct UnknownVTable {
    query_interface: unsafe extern "system" fn(
        this: *mut c_void,
        iid: *const GUID,
        interface: *mut *mut c_void,
    ) -> HRESULT,
    add_ref: unsafe extern "system" fn(this: *mut c_void) -> u32,
    release: unsafe extern "system" fn(this: *mut c_void) -> u32,
}

#[repr(C)]
struct Unknown {
    vtable: *const UnknownVTable,
}

#[repr(C)]
struct DeviceEnumeratorVTable {
    base: UnknownVTable,
    enum_audio_endpoints: unsafe extern "system" fn(
        this: *mut DeviceEnumerator,
        data_flow: EDataFlow,
        state_mask: u32,
        devices: *mut *mut DeviceCollection,
    ) -> HRESULT,
    get_default_audio_endpoint: unsafe extern "system" fn(
        this: *mut DeviceEnumerator,
        data_flow: EDataFlow,
        role: ERole,
        endpoint: *mut *mut Device,
    ) -> HRESULT,
    get_device: unsafe extern "system" fn(
        this: *mut DeviceEnumerator,
        id: PCWSTR,
        device: *mut *mut Device,
    ) -> HRESULT,
    register_endpoint_notification_callback: unsafe extern "system" fn(
        this: *mut DeviceEnumerator,
        client: *mut NotificationClientInterface,
    ) -> HRESULT,
    unregister_endpoint_notification_callback: unsafe extern "system" fn(
        this: *mut DeviceEnumerator,
        client: *mut NotificationClientInterface,
    ) -> HRESULT,
}

#[repr(C)]
struct DeviceEnumerator {
    vtable: *const DeviceEnumeratorVTable,
}

#[repr(C)]
struct DeviceCollectionVTable {
    base: UnknownVTable,
    get_count: unsafe extern "system" fn(this: *mut DeviceCollection, count: *mut u32) -> HRESULT,
    item: unsafe extern "system" fn(
        this: *mut DeviceCollection,
        index: u32,
        device: *mut *mut Device,
    ) -> HRESULT,
}

#[repr(C)]
struct DeviceCollection {
    vtable: *const DeviceCollectionVTable,
}

#[repr(C)]
struct DeviceVTable {
    base: UnknownVTable,
    activate: unsafe extern "system" fn(
        this: *mut Device,
        iid: *const GUID,
        class_context: u32,
        activation_params: *mut c_void,
        interface: *mut *mut c_void,
    ) -> HRESULT,
    open_property_store: unsafe extern "system" fn(
        this: *mut Device,
        storage_access: u32,
        properties: *mut *mut c_void,
    ) -> HRESULT,
    get_id: unsafe extern "system" fn(this: *mut Device, id: *mut PWSTR) -> HRESULT,
    get_state: unsafe extern "system" fn(this: *mut Device, state: *mut u32) -> HRESULT,
}

#[repr(C)]
struct Device {
    vtable: *const DeviceVTable,
}

#[repr(C)]
struct NotificationClientVTable {
    base: UnknownVTable,
    on_device_state_changed: unsafe extern "system" fn(
        this: *mut NotificationClientInterface,
        device_id: PCWSTR,
        new_state: u32,
    ) -> HRESULT,
    on_device_added: unsafe extern "system" fn(
        this: *mut NotificationClientInterface,
        device_id: PCWSTR,
    ) -> HRESULT,
    on_device_removed: unsafe extern "system" fn(
        this: *mut NotificationClientInterface,
        device_id: PCWSTR,
    ) -> HRESULT,
    on_default_device_changed: unsafe extern "system" fn(
        this: *mut NotificationClientInterface,
        data_flow: EDataFlow,
        role: ERole,
        default_device_id: PCWSTR,
    ) -> HRESULT,
    on_property_value_changed: unsafe extern "system" fn(
        this: *mut NotificationClientInterface,
        device_id: PCWSTR,
        key: PROPERTYKEY,
    ) -> HRESULT,
}

#[repr(C)]
struct NotificationClientInterface {
    vtable: *const NotificationClientVTable,
}

#[repr(C)]
struct NotificationClient {
    vtable: *const NotificationClientVTable,
    references: AtomicU32,
    state: Arc<AudioWorkerState>,
}

static NOTIFICATION_CLIENT_VTABLE: NotificationClientVTable = NotificationClientVTable {
    base: UnknownVTable {
        query_interface: notification_query_interface,
        add_ref: notification_add_ref,
        release: notification_release,
    },
    on_device_state_changed,
    on_device_added,
    on_device_removed,
    on_default_device_changed,
    on_property_value_changed,
};

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
struct CaptureEndpoint {
    id: Vec<u16>,
    state: u32,
}

struct AudioWorkerState {
    active: AtomicBool,
    stopping: AtomicBool,
    protocol_ready: AtomicBool,
    pending: AtomicU32,
    thread_id: AtomicU32,
    terminal: Arc<TerminalSignal>,
}

impl AudioWorkerState {
    fn new(terminal: Arc<TerminalSignal>) -> Self {
        Self {
            active: AtomicBool::new(true),
            stopping: AtomicBool::new(false),
            protocol_ready: AtomicBool::new(false),
            // Every fresh helper performs one post-initialize resnapshot and
            // invalidation. This covers changes while the previous helper was down.
            pending: AtomicU32::new(INITIAL_CHANGES),
            thread_id: AtomicU32::new(0),
            terminal,
        }
    }

    fn queue_change(&self, change: u32) {
        if !self.active.load(Ordering::Acquire) {
            return;
        }
        let queued = coalesce_change(&self.pending, change, || {
            post_audio_message(
                self.thread_id.load(Ordering::Acquire),
                WM_AUDIO_INPUT_DEVICES_CHANGED,
            )
        });
        if !queued && !self.stopping.load(Ordering::Acquire) {
            self.terminal
                .trigger(TerminalReason::AudioDeviceMonitorUnavailable);
        }
    }

    fn protocol_initialized(&self) {
        if !self.active.load(Ordering::Acquire) {
            return;
        }
        let signaled = mark_protocol_ready(&self.protocol_ready, || {
            post_audio_message(
                self.thread_id.load(Ordering::Acquire),
                WM_AUDIO_PROTOCOL_READY,
            )
        });
        if !signaled {
            self.terminal
                .trigger(TerminalReason::AudioDeviceMonitorUnavailable);
        }
    }

    fn begin_shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
        self.active.store(false, Ordering::Release);
        let _ = post_audio_message(self.thread_id.load(Ordering::Acquire), WM_QUIT);
    }
}

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

struct CoreAudioMonitor {
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
    fn start(
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

    fn drain_pending(&mut self) {
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
            match self.capture_topology() {
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

    fn shutdown(&mut self) -> Result<(), PlatformError> {
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

    fn capture_topology(&self) -> Result<Vec<CaptureEndpoint>, PlatformError> {
        if self.enumerator.is_null() {
            return Err(PlatformError::NativeFailure);
        }
        let mut collection = null_mut::<DeviceCollection>();
        // SAFETY: the live enumerator writes one capture-only collection pointer.
        let enumerated = unsafe {
            ((*(*self.enumerator).vtable).enum_audio_endpoints)(
                self.enumerator,
                eCapture,
                DEVICE_STATEMASK_ALL,
                &raw mut collection,
            )
        };
        if enumerated < 0 || collection.is_null() {
            if !collection.is_null() {
                // SAFETY: defensively release any failure-path collection.
                unsafe { release_unknown(collection.cast::<c_void>()) };
            }
            return Err(PlatformError::NativeFailure);
        }
        let result = unsafe { capture_topology_from_collection(collection) };
        // SAFETY: release the collection returned by EnumAudioEndpoints once.
        unsafe { release_unknown(collection.cast::<c_void>()) };
        result
    }
}

impl Drop for CoreAudioMonitor {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

struct WorkerCompletion {
    sender: Sender<Result<(), PlatformError>>,
    result: Option<Result<(), PlatformError>>,
}

impl WorkerCompletion {
    const fn new(sender: Sender<Result<(), PlatformError>>) -> Self {
        Self {
            sender,
            result: None,
        }
    }

    fn finish(&mut self, result: Result<(), PlatformError>) {
        self.result = Some(result);
    }
}

impl Drop for WorkerCompletion {
    fn drop(&mut self) {
        let result = self
            .result
            .take()
            .unwrap_or(Err(PlatformError::ThreadStopped));
        let _ = self.sender.try_send(result);
    }
}

pub(super) struct AudioDeviceMonitor {
    state: Arc<AudioWorkerState>,
    completion: Receiver<Result<(), PlatformError>>,
    thread: Option<JoinHandle<()>>,
}

impl AudioDeviceMonitor {
    pub(super) fn start(
        outbound: Sender<NativeEvent>,
        gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
    ) -> Result<Self, PlatformError> {
        let state = Arc::new(AudioWorkerState::new(terminal));
        let (ready_tx, ready_rx) = bounded(1);
        let (completion_tx, completion) = bounded(1);
        let worker_state = Arc::clone(&state);
        let thread = thread::Builder::new()
            .name("talking-quill-helper-win-audio".into())
            .spawn(move || {
                audio_worker(outbound, gate, worker_state, ready_tx, completion_tx);
            })
            .map_err(|_| PlatformError::ThreadStopped)?;

        match ready_rx.recv_timeout(AUDIO_STARTUP_TIMEOUT) {
            Ok(Ok(())) => Ok(Self {
                state,
                completion,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                state.begin_shutdown();
                let _ = finish_worker(thread, &completion, AUDIO_SHUTDOWN_TIMEOUT);
                Err(error)
            }
            Err(_) => {
                state.begin_shutdown();
                let _ = finish_worker(thread, &completion, AUDIO_SHUTDOWN_TIMEOUT);
                Err(PlatformError::ThreadStopped)
            }
        }
    }

    pub(super) fn protocol_initialized(&self) {
        self.state.protocol_initialized();
    }

    pub(super) fn begin_shutdown(&self) {
        self.state.begin_shutdown();
    }

    pub(super) fn shutdown_with_timeout(&mut self, timeout: Duration) -> Result<(), PlatformError> {
        self.begin_shutdown();
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        finish_worker(thread, &self.completion, timeout)
    }

    pub(super) fn shutdown(&mut self) -> Result<(), PlatformError> {
        self.shutdown_with_timeout(AUDIO_SHUTDOWN_TIMEOUT)
    }
}

impl Drop for AudioDeviceMonitor {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn audio_worker(
    outbound: Sender<NativeEvent>,
    gate: Arc<CallbackGate>,
    state: Arc<AudioWorkerState>,
    ready: Sender<Result<(), PlatformError>>,
    completion: Sender<Result<(), PlatformError>>,
) {
    // Declared first so completion is reported only after all COM resources
    // have been released, or unwinding has abandoned this worker.
    let mut completion = WorkerCompletion::new(completion);
    let mut message = MSG::default();
    // SAFETY: this no-remove peek creates the dedicated worker queue before its
    // thread ID is published to callbacks or the coordinator.
    unsafe { PeekMessageW(&raw mut message, null_mut(), 0, 0, PM_NOREMOVE) };
    // SAFETY: reads this dedicated worker's native thread identifier.
    state
        .thread_id
        .store(unsafe { GetCurrentThreadId() }, Ordering::Release);
    if !state.active.load(Ordering::Acquire) {
        let _ = ready.try_send(Err(PlatformError::ThreadStopped));
        completion.finish(Ok(()));
        return;
    }

    let mut monitor = match CoreAudioMonitor::start(
        outbound,
        gate,
        Arc::clone(&state.terminal),
        Arc::clone(&state),
    ) {
        Ok(monitor) => monitor,
        Err(error) => {
            let _ = ready.try_send(Err(error));
            completion.finish(Ok(()));
            return;
        }
    };
    if !state.active.load(Ordering::Acquire) {
        let _ = ready.try_send(Err(PlatformError::ThreadStopped));
        completion.finish(monitor.shutdown());
        return;
    }
    if ready.try_send(Ok(())).is_err() {
        state.begin_shutdown();
        completion.finish(monitor.shutdown());
        return;
    }

    loop {
        // SAFETY: `message` is writable storage and this worker owns the queue.
        let result = unsafe { GetMessageW(&raw mut message, null_mut(), 0, 0) };
        if result <= 0 {
            if result < 0 || !state.stopping.load(Ordering::Acquire) {
                state
                    .terminal
                    .trigger(TerminalReason::AudioDeviceMonitorUnavailable);
            }
            break;
        }
        if message.message == WM_AUDIO_INPUT_DEVICES_CHANGED
            || message.message == WM_AUDIO_PROTOCOL_READY
        {
            monitor.drain_pending();
        }
    }

    state.active.store(false, Ordering::Release);
    completion.finish(monitor.shutdown());
}

fn finish_worker(
    thread: JoinHandle<()>,
    completion: &Receiver<Result<(), PlatformError>>,
    timeout: Duration,
) -> Result<(), PlatformError> {
    let result = match completion.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => {
            // Never join a worker which may be blocked in an audio driver or
            // COM call. Process exit owns its remaining native resources.
            drop(thread);
            return Err(PlatformError::ThreadStopped);
        }
    };
    if thread.join().is_err() {
        Err(PlatformError::ThreadStopped)
    } else {
        result
    }
}

unsafe fn capture_topology_from_collection(
    collection: *mut DeviceCollection,
) -> Result<Vec<CaptureEndpoint>, PlatformError> {
    let mut count = 0;
    // SAFETY: `collection` is live and `count` is writable.
    if unsafe { ((*(*collection).vtable).get_count)(collection, &raw mut count) } < 0 {
        return Err(PlatformError::NativeFailure);
    }
    let mut topology = Vec::with_capacity(count as usize);
    for index in 0..count {
        let mut device = null_mut::<Device>();
        // SAFETY: the index is below the reported count and `device` is writable.
        let item = unsafe { ((*(*collection).vtable).item)(collection, index, &raw mut device) };
        if item < 0 || device.is_null() {
            if !device.is_null() {
                // SAFETY: defensively release any failure-path item reference.
                unsafe { release_unknown(device.cast::<c_void>()) };
            }
            return Err(PlatformError::NativeFailure);
        }
        // SAFETY: `device` remains live until the release below.
        let endpoint = unsafe { capture_endpoint(device) };
        // SAFETY: release the collection's item reference exactly once.
        unsafe { release_unknown(device.cast::<c_void>()) };
        topology.push(endpoint?);
    }
    topology.sort_unstable();
    Ok(topology)
}

unsafe fn capture_endpoint(device: *mut Device) -> Result<CaptureEndpoint, PlatformError> {
    let mut id = null_mut::<u16>();
    let mut state = 0;
    // SAFETY: `device` is live and `id` is writable.
    let id_result = unsafe { ((*(*device).vtable).get_id)(device, &raw mut id) };
    if id_result < 0 || id.is_null() {
        if !id.is_null() {
            // SAFETY: defensively free any failure-path task string.
            unsafe { CoTaskMemFree(id.cast::<c_void>()) };
        }
        return Err(PlatformError::NativeFailure);
    }
    // SAFETY: `device` is live and `state` is writable.
    let state_result = unsafe { ((*(*device).vtable).get_state)(device, &raw mut state) };
    let copied_id = unsafe { copy_task_string(id) };
    // SAFETY: IMMDevice::GetId allocates this string with the COM task allocator.
    unsafe { CoTaskMemFree(id.cast::<c_void>()) };
    if state_result < 0 {
        return Err(PlatformError::NativeFailure);
    }
    Ok(CaptureEndpoint {
        id: copied_id?,
        state,
    })
}

unsafe fn copy_task_string(value: *const u16) -> Result<Vec<u16>, PlatformError> {
    for length in 0..MAX_ENDPOINT_ID_CODE_UNITS {
        // SAFETY: the system-owned string is readable through its terminating
        // NUL; the finite cap prevents an unbounded scan on invalid input.
        if unsafe { *value.add(length) } == 0 {
            // SAFETY: the preceding scan established this initialized range.
            return Ok(unsafe { std::slice::from_raw_parts(value, length) }.to_vec());
        }
    }
    Err(PlatformError::NativeFailure)
}

fn coalesce_change(pending: &AtomicU32, change: u32, wake_worker: impl FnOnce() -> bool) -> bool {
    if pending.fetch_or(change, Ordering::AcqRel) != 0 {
        return true;
    }
    if wake_worker() {
        true
    } else {
        pending.store(0, Ordering::Release);
        false
    }
}

fn mark_protocol_ready(protocol_ready: &AtomicBool, wake_worker: impl FnOnce() -> bool) -> bool {
    protocol_ready.store(true, Ordering::Release);
    wake_worker()
}

fn take_ready_changes(pending: &AtomicU32, protocol_ready: &AtomicBool) -> u32 {
    if protocol_ready.load(Ordering::Acquire) {
        pending.swap(0, Ordering::AcqRel)
    } else {
        0
    }
}

fn post_audio_message(thread_id: u32, message: u32) -> bool {
    if thread_id == 0 {
        return false;
    }
    // SAFETY: all audio-worker messages are pointer-free and its queue is
    // created before the thread ID is published.
    unsafe { PostThreadMessageW(thread_id, message, 0, 0) != 0 }
}

fn is_relevant_default_change(data_flow: EDataFlow, role: ERole) -> bool {
    data_flow == eCapture && role == eConsole
}

fn with_notification_client(
    this: *mut NotificationClientInterface,
    callback: impl FnOnce(&NotificationClient),
) -> HRESULT {
    if this.is_null() {
        return E_POINTER;
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: Core Audio calls through our vtable with the original boxed object.
        callback(unsafe { &*this.cast::<NotificationClient>() });
    }));
    if result.is_err() {
        // SAFETY: the callback object remains registered for this invocation.
        unsafe {
            let client = &*this.cast::<NotificationClient>();
            client
                .state
                .terminal
                .trigger(TerminalReason::CallbackPanicked);
        }
    }
    S_OK
}

unsafe extern "system" fn notification_query_interface(
    this: *mut c_void,
    iid: *const GUID,
    interface: *mut *mut c_void,
) -> HRESULT {
    if iid.is_null() || interface.is_null() || this.is_null() {
        return E_POINTER;
    }
    // SAFETY: `interface` is a caller-provided writable out pointer.
    unsafe { *interface = null_mut() };
    // SAFETY: `iid` is non-null for this call.
    let requested = unsafe { &*iid };
    if guid_eq(requested, &IID_IUnknown) || guid_eq(requested, &IID_IMM_NOTIFICATION_CLIENT) {
        // SAFETY: `this` is this object's identity pointer and `interface` is writable.
        unsafe {
            notification_add_ref(this);
            *interface = this;
        }
        S_OK
    } else {
        E_NOINTERFACE
    }
}

unsafe extern "system" fn notification_add_ref(this: *mut c_void) -> u32 {
    if this.is_null() {
        return 0;
    }
    // SAFETY: every interface pointer is the original NotificationClient address.
    unsafe {
        (*this.cast::<NotificationClient>())
            .references
            .fetch_add(1, Ordering::Relaxed)
            + 1
    }
}

unsafe extern "system" fn notification_release(this: *mut c_void) -> u32 {
    if this.is_null() {
        return 0;
    }
    // SAFETY: every interface pointer is the original NotificationClient address.
    let client = unsafe { &*this.cast::<NotificationClient>() };
    let previous = client.references.fetch_sub(1, Ordering::Release);
    debug_assert!(previous > 0);
    if previous == 1 {
        fence(Ordering::Acquire);
        // SAFETY: this was the final COM reference to the Box allocation.
        unsafe { drop(Box::from_raw(this.cast::<NotificationClient>())) };
        0
    } else {
        previous - 1
    }
}

unsafe extern "system" fn on_device_state_changed(
    this: *mut NotificationClientInterface,
    _device_id: PCWSTR,
    _new_state: u32,
) -> HRESULT {
    with_notification_client(this, |client| {
        client.state.queue_change(CHANGE_TOPOLOGY);
    })
}

unsafe extern "system" fn on_device_added(
    this: *mut NotificationClientInterface,
    _device_id: PCWSTR,
) -> HRESULT {
    with_notification_client(this, |client| {
        client.state.queue_change(CHANGE_TOPOLOGY);
    })
}

unsafe extern "system" fn on_device_removed(
    this: *mut NotificationClientInterface,
    _device_id: PCWSTR,
) -> HRESULT {
    with_notification_client(this, |client| {
        client.state.queue_change(CHANGE_TOPOLOGY);
    })
}

unsafe extern "system" fn on_default_device_changed(
    this: *mut NotificationClientInterface,
    data_flow: EDataFlow,
    role: ERole,
    _default_device_id: PCWSTR,
) -> HRESULT {
    with_notification_client(this, |client| {
        if is_relevant_default_change(data_flow, role) {
            client.state.queue_change(CHANGE_DEFAULT);
        }
    })
}

unsafe extern "system" fn on_property_value_changed(
    this: *mut NotificationClientInterface,
    _device_id: PCWSTR,
    _key: PROPERTYKEY,
) -> HRESULT {
    with_notification_client(this, |_| {})
}

unsafe fn release_unknown(interface: *mut c_void) -> u32 {
    // SAFETY: all COM interfaces begin with an IUnknown-compatible vtable.
    let unknown = interface.cast::<Unknown>();
    unsafe { ((*(*unknown).vtable).release)(interface) }
}

const fn guid_eq(left: &GUID, right: &GUID) -> bool {
    left.data1 == right.data1
        && left.data2 == right.data2
        && left.data3 == right.data3
        && left.data4[0] == right.data4[0]
        && left.data4[1] == right.data4[1]
        && left.data4[2] == right.data4[2]
        && left.data4[3] == right.data4[3]
        && left.data4[4] == right.data4[4]
        && left.data4[5] == right.data4[5]
        && left.data4[6] == right.data4[6]
        && left.data4[7] == right.data4[7]
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Media::Audio::{eCommunications, eMultimedia, eRender};

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
        let completed = thread::spawn(move || {
            let _ = completion_tx.send(Ok(()));
        });
        assert!(finish_worker(completed, &completion_rx, Duration::from_secs(1)).is_ok());
    }
}
