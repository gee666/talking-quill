//! IMMNotificationClient identity, reference counting, and pointer-free callbacks.
use super::{
    com::{NotificationClientInterface, NotificationClientVTable, UnknownVTable},
    state::{AudioWorkerState, CHANGE_DEFAULT, CHANGE_TOPOLOGY},
};
use crate::platform::TerminalReason;
use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    ptr::null_mut,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering, fence},
    },
};
use windows_sys::{
    Win32::{
        Foundation::{E_NOINTERFACE, E_POINTER, PROPERTYKEY, S_OK},
        Media::Audio::{EDataFlow, ERole, eCapture, eConsole},
    },
    core::{GUID, HRESULT, IID_IUnknown, PCWSTR},
};
pub(super) const IID_IMM_NOTIFICATION_CLIENT: GUID =
    GUID::from_u128(0x7991eec9_7e89_4d85_8390_6c703cec60c0);

#[repr(C)]
pub(super) struct NotificationClient {
    pub(super) vtable: *const NotificationClientVTable,
    pub(super) references: AtomicU32,
    pub(super) state: Arc<AudioWorkerState>,
}

pub(super) static NOTIFICATION_CLIENT_VTABLE: NotificationClientVTable = NotificationClientVTable {
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

pub(super) fn is_relevant_default_change(data_flow: EDataFlow, role: ERole) -> bool {
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

pub(super) unsafe extern "system" fn notification_query_interface(
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

pub(super) unsafe extern "system" fn notification_release(this: *mut c_void) -> u32 {
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

pub(super) unsafe extern "system" fn on_device_added(
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

pub(super) unsafe extern "system" fn on_default_device_changed(
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

pub(super) unsafe extern "system" fn on_property_value_changed(
    this: *mut NotificationClientInterface,
    _device_id: PCWSTR,
    _key: PROPERTYKEY,
) -> HRESULT {
    with_notification_client(this, |_| {})
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
