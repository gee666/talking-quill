#![cfg(target_os = "macos")]

use std::ffi::{CString, c_char, c_void};
use std::ptr::{null, null_mut};

use core_foundation_sys::array::{CFArrayCreate, CFArrayRef, kCFTypeArrayCallBacks};
use core_foundation_sys::base::{CFRelease, CFTypeRef, OSStatus, kCFAllocatorDefault};
use core_foundation_sys::string::{CFStringCreateWithBytes, kCFStringEncodingUTF8};
use security_framework_sys::base::{errSecDuplicateItem, errSecSuccess};

use super::config::InstalledConfig;

const SERVICE: &[u8] = b"com.talkingquill.app.keyboard-owner";
const SECRET_ACCOUNT: &[u8] = b"owner-ipc-v1";
const LATCH_ACCOUNT: &[u8] = b"maintenance-latch-v1";

pub fn provision_if_missing(config: &InstalledConfig) -> Result<(), ProvisionError> {
    let trusted = trusted_applications(config)?;
    let access = access(trusted)?;
    unsafe {
        CFRelease(trusted.cast());
    }
    let mut secret = [0_u8; 32];
    if unsafe { SecRandomCopyBytes(kSecRandomDefault, secret.len(), secret.as_mut_ptr()) } != 0
        || secret.iter().all(|byte| *byte == 0)
    {
        unsafe { CFRelease(access) };
        return Err(ProvisionError);
    }
    add_item(SECRET_ACCOUNT, &secret, access)?;
    let empty_latch =
        [0_u8; talking_quill_owner_protocol::macos_maintenance::MACOS_MAINTENANCE_RECORD_BYTES];
    add_item(LATCH_ACCOUNT, &empty_latch, access)?;
    unsafe { CFRelease(access) };
    Ok(())
}

fn trusted_applications(config: &InstalledConfig) -> Result<CFArrayRef, ProvisionError> {
    let mut values: Vec<CFTypeRef> = Vec::new();
    for path in [
        &config.gateway.canonical_executable_path,
        &config.owner.canonical_executable_path,
    ] {
        let encoded =
            CString::new(path.to_string_lossy().as_bytes()).map_err(|_| ProvisionError)?;
        let mut application: CFTypeRef = null();
        if unsafe { SecTrustedApplicationCreateFromPath(encoded.as_ptr(), &raw mut application) }
            != errSecSuccess
            || application.is_null()
        {
            for value in values {
                unsafe { CFRelease(value) };
            }
            return Err(ProvisionError);
        }
        values.push(application);
    }
    let array = unsafe {
        CFArrayCreate(
            kCFAllocatorDefault,
            values.as_ptr().cast(),
            values.len() as isize,
            &kCFTypeArrayCallBacks,
        )
    };
    for value in values {
        unsafe { CFRelease(value) };
    }
    (!array.is_null()).then_some(array).ok_or(ProvisionError)
}

fn access(trusted: CFArrayRef) -> Result<CFTypeRef, ProvisionError> {
    let label = unsafe {
        CFStringCreateWithBytes(
            kCFAllocatorDefault,
            b"Talking Quill Keyboard Owner".as_ptr(),
            28,
            kCFStringEncodingUTF8,
            0,
        )
    };
    if label.is_null() {
        return Err(ProvisionError);
    }
    let mut access: CFTypeRef = null();
    let status = unsafe { SecAccessCreate(label, trusted, &raw mut access) };
    unsafe { CFRelease(label.cast()) };
    if status == errSecSuccess && !access.is_null() {
        Ok(access)
    } else {
        Err(ProvisionError)
    }
}

fn add_item(account: &[u8], value: &[u8], access: CFTypeRef) -> Result<(), ProvisionError> {
    let mut item: CFTypeRef = null();
    let status = unsafe {
        SecKeychainAddGenericPassword(
            null_mut(),
            SERVICE.len() as u32,
            SERVICE.as_ptr().cast(),
            account.len() as u32,
            account.as_ptr().cast(),
            value.len() as u32,
            value.as_ptr().cast(),
            &raw mut item,
        )
    };
    if status == errSecDuplicateItem {
        let account_text = std::str::from_utf8(account).map_err(|_| ProvisionError)?;
        super::keychain::validate_unique_item(account_text, value.len(), |existing| {
            existing == value
                || (account == LATCH_ACCOUNT
                    && existing.len()
                        == talking_quill_owner_protocol::macos_maintenance::MACOS_MAINTENANCE_RECORD_BYTES
                    && talking_quill_owner_protocol::macos_maintenance::MacosMaintenanceRecord::decode(existing).is_ok())
                || (account == SECRET_ACCOUNT
                    && existing.len() == 32
                    && !existing.iter().all(|byte| *byte == 0))
        })
        .map_err(|_| ProvisionError)?;
        let mut password_length = 0_u32;
        let mut password_data: *mut c_void = null_mut();
        if unsafe {
            SecKeychainFindGenericPassword(
                null_mut(),
                SERVICE.len() as u32,
                SERVICE.as_ptr().cast(),
                account.len() as u32,
                account.as_ptr().cast(),
                &raw mut password_length,
                &raw mut password_data,
                &raw mut item,
            )
        } != errSecSuccess
            || item.is_null()
            || password_length as usize != value.len()
        {
            if !password_data.is_null() {
                unsafe { SecKeychainItemFreeContent(null(), password_data) };
            }
            return Err(ProvisionError);
        }
        if !password_data.is_null() {
            unsafe { SecKeychainItemFreeContent(null(), password_data) };
        }
    } else if status != errSecSuccess || item.is_null() {
        return Err(ProvisionError);
    }
    let set = unsafe { SecKeychainItemSetAccess(item, access) };
    if set != errSecSuccess && status == errSecSuccess {
        let _ = unsafe { SecKeychainItemDelete(item) };
    }
    unsafe { CFRelease(item) };
    (set == errSecSuccess).then_some(()).ok_or(ProvisionError)
}

#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("the owner Keychain ACL could not be provisioned")]
pub struct ProvisionError;

unsafe extern "C" {
    static kSecRandomDefault: *const c_void;
    fn SecRandomCopyBytes(random: *const c_void, count: usize, bytes: *mut u8) -> OSStatus;
    fn SecTrustedApplicationCreateFromPath(
        path: *const c_char,
        application: *mut CFTypeRef,
    ) -> OSStatus;
    fn SecAccessCreate(
        descriptor: core_foundation_sys::string::CFStringRef,
        trusted: CFArrayRef,
        access: *mut CFTypeRef,
    ) -> OSStatus;
    fn SecKeychainAddGenericPassword(
        keychain: *mut c_void,
        service_len: u32,
        service: *const c_char,
        account_len: u32,
        account: *const c_char,
        password_len: u32,
        password: *const c_void,
        item: *mut CFTypeRef,
    ) -> OSStatus;
    fn SecKeychainFindGenericPassword(
        keychain: *mut c_void,
        service_len: u32,
        service: *const c_char,
        account_len: u32,
        account: *const c_char,
        password_len: *mut u32,
        password_data: *mut *mut c_void,
        item: *mut CFTypeRef,
    ) -> OSStatus;
    fn SecKeychainItemFreeContent(attribute_list: *const c_void, data: *mut c_void) -> OSStatus;
    fn SecKeychainItemSetAccess(item: CFTypeRef, access: CFTypeRef) -> OSStatus;
    fn SecKeychainItemDelete(item: CFTypeRef) -> OSStatus;
}
