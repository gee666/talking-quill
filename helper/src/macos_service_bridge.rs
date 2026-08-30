#![cfg(target_os = "macos")]

use std::ffi::{c_char, c_void};
use std::ptr::{null, null_mut};

use core_foundation_sys::base::{CFRelease, CFTypeRef};
use core_foundation_sys::string::{CFStringCreateWithBytes, CFStringRef, kCFStringEncodingUTF8};

const OWNER_LOGIN_ITEM_ID: &str = "com.talkingquill.app.keyboard-owner";
const K_SEC_CS_STRICT_VALIDATE: u32 = 1 << 1;
const K_SEC_CS_NO_NETWORK_ACCESS: u32 = 1 << 27;

pub fn validate_current_process(requirement: &str) -> Result<(), BridgeError> {
    let text = unsafe {
        CFStringCreateWithBytes(
            null(),
            requirement.as_ptr(),
            requirement.len() as isize,
            kCFStringEncodingUTF8,
            0,
        )
    };
    if text.is_null() {
        return Err(BridgeError);
    }
    let mut requirement_ref: CFTypeRef = null();
    let mut code: CFTypeRef = null();
    let statuses = unsafe {
        (
            SecRequirementCreateWithString(text, 0, &raw mut requirement_ref),
            SecCodeCopySelf(K_SEC_CS_NO_NETWORK_ACCESS, &raw mut code),
        )
    };
    unsafe { CFRelease(text.cast()) };
    let valid = statuses == (0, 0)
        && !requirement_ref.is_null()
        && !code.is_null()
        && unsafe {
            SecCodeCheckValidity(
                code,
                K_SEC_CS_STRICT_VALIDATE | K_SEC_CS_NO_NETWORK_ACCESS,
                requirement_ref,
            )
        } == 0;
    if !requirement_ref.is_null() {
        unsafe { CFRelease(requirement_ref) }
    }
    if !code.is_null() {
        unsafe { CFRelease(code) }
    }
    valid.then_some(()).ok_or(BridgeError)
}

/// Authorizes only the kernel parent that launched this exact bridge. Every
/// trust anchor is loaded independently from the CMS-authenticated installed
/// policy; the parent supplies no path, hash, CDHash, or requirement.
pub fn validate_parent_process(parent_pid: u32) -> Result<(), BridgeError> {
    if parent_pid == 0 || parent_pid != unsafe { libc::getppid() } as u32 {
        return Err(BridgeError);
    }
    crate::owner::macos::validate_bridge_parent(parent_pid)
        .then_some(())
        .ok_or(BridgeError)
}

/// Fixed native operation used only by the executable packaged in the outer
/// application's Contents/MacOS directory. Resource helpers and the nested
/// LoginItem invoke that bridge; they never call SMAppService themselves.
pub fn run(operation: &str) -> Result<usize, BridgeError> {
    let service = fixed_service()?;
    match operation {
        "status" => Ok(unsafe { send_usize(service, selector(b"status\0")?) }),
        "register" => {
            change(service, b"registerAndReturnError:\0")?;
            Ok(unsafe { send_usize(service, selector(b"status\0")?) })
        }
        "unregister" => {
            change(service, b"unregisterAndReturnError:\0")?;
            Ok(unsafe { send_usize(service, selector(b"status\0")?) })
        }
        "acl-denial" => acl_denial_probe(),
        _ => Err(BridgeError),
    }
}

fn acl_denial_probe() -> Result<usize, BridgeError> {
    let mut length = 0_u32;
    let mut data: *mut c_void = null_mut();
    let status = unsafe {
        SecKeychainFindGenericPassword(
            null_mut(),
            35,
            b"com.talkingquill.app.keyboard-owner".as_ptr().cast(),
            12,
            b"owner-ipc-v1".as_ptr().cast(),
            &raw mut length,
            &raw mut data,
            null_mut(),
        )
    };
    if !data.is_null() {
        unsafe { SecKeychainItemFreeContent(null(), data) };
    }
    // Any successful read by this non-enrolled outer bridge is a release blocker.
    (status != 0).then_some(77).ok_or(BridgeError)
}

fn change(service: *mut c_void, name: &'static [u8]) -> Result<(), BridgeError> {
    let mut error: *mut c_void = null_mut();
    unsafe { send_bool_error(service, selector(name)?, &raw mut error) }
        .then_some(())
        .ok_or(BridgeError)
}

fn fixed_service() -> Result<*mut c_void, BridgeError> {
    let class = unsafe { objc_getClass(c"SMAppService".as_ptr()) };
    if class.is_null() {
        return Err(BridgeError);
    }
    let identifier = unsafe {
        CFStringCreateWithBytes(
            null(),
            OWNER_LOGIN_ITEM_ID.as_ptr(),
            OWNER_LOGIN_ITEM_ID.len() as isize,
            kCFStringEncodingUTF8,
            0,
        )
    };
    if identifier.is_null() {
        return Err(BridgeError);
    }
    let service = unsafe {
        send_id_id(
            class,
            selector(b"loginItemServiceWithIdentifier:\0")?,
            identifier.cast_mut().cast(),
        )
    };
    unsafe { CFRelease(identifier.cast::<c_void>() as CFTypeRef) };
    (!service.is_null()).then_some(service).ok_or(BridgeError)
}

fn selector(name: &'static [u8]) -> Result<*mut c_void, BridgeError> {
    let value = unsafe { sel_registerName(name.as_ptr().cast()) };
    (!value.is_null()).then_some(value).ok_or(BridgeError)
}
unsafe fn send_id_id(
    receiver: *mut c_void,
    selector: *mut c_void,
    value: *mut c_void,
) -> *mut c_void {
    let function: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { function(receiver, selector, value) }
}
unsafe fn send_usize(receiver: *mut c_void, selector: *mut c_void) -> usize {
    let function: unsafe extern "C" fn(*mut c_void, *mut c_void) -> usize =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { function(receiver, selector) }
}
unsafe fn send_bool_error(
    receiver: *mut c_void,
    selector: *mut c_void,
    error: *mut *mut c_void,
) -> bool {
    let function: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> bool =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { function(receiver, selector, error) }
}

#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("outer application SMAppService bridge failed")]
pub struct BridgeError;

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> *mut c_void;
    fn sel_registerName(name: *const c_char) -> *mut c_void;
    fn objc_msgSend();
    fn SecKeychainFindGenericPassword(
        keychain: *mut c_void,
        service_length: u32,
        service: *const c_char,
        account_length: u32,
        account: *const c_char,
        password_length: *mut u32,
        password_data: *mut *mut c_void,
        item: *mut CFTypeRef,
    ) -> i32;
    fn SecKeychainItemFreeContent(attributes: *const c_void, data: *mut c_void) -> i32;
    fn SecRequirementCreateWithString(
        text: CFStringRef,
        flags: u32,
        requirement: *mut CFTypeRef,
    ) -> i32;
    fn SecCodeCopySelf(flags: u32, code: *mut CFTypeRef) -> i32;
    fn SecCodeCheckValidity(code: CFTypeRef, flags: u32, requirement: CFTypeRef) -> i32;
}
