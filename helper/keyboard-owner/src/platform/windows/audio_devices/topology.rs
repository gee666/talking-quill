//! Capture-only snapshots with owned endpoint IDs and deterministic ordering.
use super::com::{Device, DeviceCollection, DeviceEnumerator, release_unknown};
use crate::platform::PlatformError;
use std::{ffi::c_void, ptr::null_mut};
use windows_sys::Win32::{
    Media::Audio::{DEVICE_STATEMASK_ALL, eCapture},
    System::Com::CoTaskMemFree,
};
const MAX_ENDPOINT_ID_CODE_UNITS: usize = 32 * 1024;

#[derive(Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct CaptureEndpoint {
    pub(super) id: Vec<u16>,
    pub(super) state: u32,
}

/// # Safety
/// A non-null enumerator must remain live and callable in the current COM
/// apartment for the entire snapshot. The caller retains its reference.
pub(super) unsafe fn capture_topology(
    enumerator: *mut DeviceEnumerator,
) -> Result<Vec<CaptureEndpoint>, PlatformError> {
    if enumerator.is_null() {
        return Err(PlatformError::NativeFailure);
    }
    let mut collection = null_mut::<DeviceCollection>();
    // SAFETY: the live enumerator writes one capture-only collection pointer.
    let enumerated = unsafe {
        ((*(*enumerator).vtable).enum_audio_endpoints)(
            enumerator,
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
    // SAFETY: successful enumeration returned a live collection retained below.
    let result = unsafe { capture_topology_from_collection(collection) };
    // SAFETY: release the collection returned by EnumAudioEndpoints once.
    unsafe { release_unknown(collection.cast::<c_void>()) };
    result
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
    // SAFETY: successful GetId returned a NUL-terminated COM task string.
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
        // SAFETY: GetId guarantees readability through the terminating NUL.
        // The cap bounds scanning work, but cannot validate a malformed pointer.
        if unsafe { *value.add(length) } == 0 {
            // SAFETY: the preceding scan established this initialized range.
            return Ok(unsafe { std::slice::from_raw_parts(value, length) }.to_vec());
        }
    }
    Err(PlatformError::NativeFailure)
}
