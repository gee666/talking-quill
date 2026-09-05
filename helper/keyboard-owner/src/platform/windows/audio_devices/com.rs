//! Core Audio interface layouts and the shared IUnknown release operation.
//!
//! Keep unused vtable slots: their order is part of the Windows COM ABI.
use std::ffi::c_void;
use windows_sys::{
    Win32::{
        Foundation::PROPERTYKEY,
        Media::Audio::{EDataFlow, ERole},
    },
    core::{GUID, HRESULT, PCWSTR, PWSTR},
};
pub(super) const IID_IMM_DEVICE_ENUMERATOR: GUID =
    GUID::from_u128(0xa95664d2_9614_4f35_a746_de8db63617e6);

#[repr(C)]
pub(super) struct UnknownVTable {
    pub(super) query_interface: unsafe extern "system" fn(
        this: *mut c_void,
        iid: *const GUID,
        interface: *mut *mut c_void,
    ) -> HRESULT,
    pub(super) add_ref: unsafe extern "system" fn(this: *mut c_void) -> u32,
    pub(super) release: unsafe extern "system" fn(this: *mut c_void) -> u32,
}

#[repr(C)]
struct Unknown {
    vtable: *const UnknownVTable,
}

#[repr(C)]
pub(super) struct DeviceEnumeratorVTable {
    pub(super) base: UnknownVTable,
    pub(super) enum_audio_endpoints: unsafe extern "system" fn(
        this: *mut DeviceEnumerator,
        data_flow: EDataFlow,
        state_mask: u32,
        devices: *mut *mut DeviceCollection,
    ) -> HRESULT,
    pub(super) get_default_audio_endpoint: unsafe extern "system" fn(
        this: *mut DeviceEnumerator,
        data_flow: EDataFlow,
        role: ERole,
        endpoint: *mut *mut Device,
    ) -> HRESULT,
    pub(super) get_device: unsafe extern "system" fn(
        this: *mut DeviceEnumerator,
        id: PCWSTR,
        device: *mut *mut Device,
    ) -> HRESULT,
    pub(super) register_endpoint_notification_callback: unsafe extern "system" fn(
        this: *mut DeviceEnumerator,
        client: *mut NotificationClientInterface,
    ) -> HRESULT,
    pub(super) unregister_endpoint_notification_callback: unsafe extern "system" fn(
        this: *mut DeviceEnumerator,
        client: *mut NotificationClientInterface,
    ) -> HRESULT,
}

#[repr(C)]
pub(super) struct DeviceEnumerator {
    pub(super) vtable: *const DeviceEnumeratorVTable,
}

#[repr(C)]
pub(super) struct DeviceCollectionVTable {
    pub(super) base: UnknownVTable,
    pub(super) get_count:
        unsafe extern "system" fn(this: *mut DeviceCollection, count: *mut u32) -> HRESULT,
    pub(super) item: unsafe extern "system" fn(
        this: *mut DeviceCollection,
        index: u32,
        device: *mut *mut Device,
    ) -> HRESULT,
}

#[repr(C)]
pub(super) struct DeviceCollection {
    pub(super) vtable: *const DeviceCollectionVTable,
}

#[repr(C)]
pub(super) struct DeviceVTable {
    pub(super) base: UnknownVTable,
    pub(super) activate: unsafe extern "system" fn(
        this: *mut Device,
        iid: *const GUID,
        class_context: u32,
        activation_params: *mut c_void,
        interface: *mut *mut c_void,
    ) -> HRESULT,
    pub(super) open_property_store: unsafe extern "system" fn(
        this: *mut Device,
        storage_access: u32,
        properties: *mut *mut c_void,
    ) -> HRESULT,
    pub(super) get_id: unsafe extern "system" fn(this: *mut Device, id: *mut PWSTR) -> HRESULT,
    pub(super) get_state: unsafe extern "system" fn(this: *mut Device, state: *mut u32) -> HRESULT,
}

#[repr(C)]
pub(super) struct Device {
    pub(super) vtable: *const DeviceVTable,
}

#[repr(C)]
pub(super) struct NotificationClientVTable {
    pub(super) base: UnknownVTable,
    pub(super) on_device_state_changed: unsafe extern "system" fn(
        this: *mut NotificationClientInterface,
        device_id: PCWSTR,
        new_state: u32,
    ) -> HRESULT,
    pub(super) on_device_added: unsafe extern "system" fn(
        this: *mut NotificationClientInterface,
        device_id: PCWSTR,
    ) -> HRESULT,
    pub(super) on_device_removed: unsafe extern "system" fn(
        this: *mut NotificationClientInterface,
        device_id: PCWSTR,
    ) -> HRESULT,
    pub(super) on_default_device_changed: unsafe extern "system" fn(
        this: *mut NotificationClientInterface,
        data_flow: EDataFlow,
        role: ERole,
        default_device_id: PCWSTR,
    ) -> HRESULT,
    pub(super) on_property_value_changed: unsafe extern "system" fn(
        this: *mut NotificationClientInterface,
        device_id: PCWSTR,
        key: PROPERTYKEY,
    ) -> HRESULT,
}

#[repr(C)]
pub(super) struct NotificationClientInterface {
    pub(super) vtable: *const NotificationClientVTable,
}

/// # Safety
/// `interface` must be non-null and live with an IUnknown-compatible vtable.
/// The caller must own a reference it can release in the current COM apartment.
pub(super) unsafe fn release_unknown(interface: *mut c_void) -> u32 {
    // SAFETY: all COM interfaces begin with an IUnknown-compatible vtable.
    let unknown = interface.cast::<Unknown>();
    unsafe { ((*(*unknown).vtable).release)(interface) }
}
