#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::ptr::null;

use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation_sys::base::{CFGetTypeID, CFRelease, CFTypeRef, kCFAllocatorDefault};
use core_foundation_sys::data::{
    CFDataCreate, CFDataGetBytePtr, CFDataGetLength, CFDataGetTypeID, CFDataRef,
};
use core_foundation_sys::dictionary::{
    CFDictionaryCreate, kCFTypeDictionaryKeyCallBacks, kCFTypeDictionaryValueCallBacks,
};
use core_foundation_sys::number::kCFBooleanTrue;
use core_foundation_sys::string::{CFStringCreateWithBytes, kCFStringEncodingUTF8};
use security_framework_sys::base::{errSecItemNotFound, errSecSuccess};
use security_framework_sys::item::{
    kSecAttrAccount, kSecAttrService, kSecClass, kSecClassGenericPassword, kSecMatchLimit,
    kSecMatchLimitAll, kSecReturnData, kSecUseAuthenticationUI, kSecValueData,
};
use security_framework_sys::keychain_item::{SecItemCopyMatching, SecItemDelete, SecItemUpdate};

const SERVICE: &str = "com.talkingquill.app.keyboard-owner";
const ACCOUNT: &str = "owner-ipc-v1";

pub fn read_handshake_secret() -> Result<[u8; 32], KeychainError> {
    let bytes = read_unique_item(ACCOUNT, 32)?;
    let secret: [u8; 32] = bytes.try_into().map_err(|_| KeychainError)?;
    if secret.iter().all(|byte| *byte == 0) {
        Err(KeychainError)
    } else {
        Ok(secret)
    }
}

pub(super) fn validate_unique_item(
    account: &str,
    expected_len: usize,
    valid: impl FnOnce(&[u8]) -> bool,
) -> Result<(), KeychainError> {
    let bytes = read_unique_item(account, expected_len)?;
    valid(&bytes).then_some(()).ok_or(KeychainError)
}

fn read_unique_item(account_name: &str, expected_len: usize) -> Result<Vec<u8>, KeychainError> {
    let service = cf_string(SERVICE)?;
    let account = cf_string(account_name)?;
    let keys = [
        unsafe { kSecClass } as *const c_void,
        unsafe { kSecAttrService } as *const c_void,
        unsafe { kSecAttrAccount } as *const c_void,
        unsafe { kSecUseAuthenticationUI } as *const c_void,
        unsafe { kSecReturnData } as *const c_void,
        unsafe { kSecMatchLimit } as *const c_void,
    ];
    let values = [
        unsafe { kSecClassGenericPassword }.cast::<c_void>(),
        service.cast::<c_void>(),
        account.cast::<c_void>(),
        unsafe { kSecUseAuthenticationUIFail }.cast::<c_void>(),
        unsafe { kCFBooleanTrue }.cast::<c_void>(),
        unsafe { kSecMatchLimitAll }.cast::<c_void>(),
    ];
    let dictionary = unsafe {
        CFDictionaryCreate(
            kCFAllocatorDefault,
            keys.as_ptr(),
            values.as_ptr(),
            keys.len() as isize,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    };
    if dictionary.is_null() {
        unsafe {
            CFRelease(service.cast());
            CFRelease(account.cast());
        }
        return Err(KeychainError);
    }
    let mut result: CFTypeRef = null();
    let status = unsafe { SecItemCopyMatching(dictionary, &raw mut result) };
    unsafe {
        CFRelease(dictionary.cast());
        CFRelease(service.cast());
        CFRelease(account.cast());
    }
    if status == errSecItemNotFound || status != errSecSuccess || result.is_null() {
        return Err(KeychainError);
    }
    let valid_array =
        unsafe { CFGetTypeID(result) == core_foundation_sys::array::CFArrayGetTypeID() };
    if !valid_array || unsafe { CFArrayGetCount(result as CFArrayRef) } != 1 {
        unsafe { CFRelease(result) };
        return Err(KeychainError);
    }
    let data = unsafe { CFArrayGetValueAtIndex(result as CFArrayRef, 0) } as CFDataRef;
    if data.is_null()
        || unsafe { CFGetTypeID(data.cast()) != CFDataGetTypeID() }
        || unsafe { CFDataGetLength(data) } != expected_len as isize
    {
        unsafe { CFRelease(result) };
        return Err(KeychainError);
    }
    let mut bytes = vec![0_u8; expected_len];
    unsafe {
        std::ptr::copy_nonoverlapping(CFDataGetBytePtr(data), bytes.as_mut_ptr(), bytes.len());
        CFRelease(result);
    }
    Ok(bytes)
}

pub(super) fn fixed_items_absent() -> Result<bool, KeychainError> {
    Ok(item_absent(ACCOUNT)? && item_absent("maintenance-latch-v1")?)
}

fn item_absent(account_name: &str) -> Result<bool, KeychainError> {
    let service = cf_string(SERVICE)?;
    let account = cf_string(account_name)?;
    let keys = [
        unsafe { kSecClass } as *const c_void,
        unsafe { kSecAttrService } as *const c_void,
        unsafe { kSecAttrAccount } as *const c_void,
        unsafe { kSecUseAuthenticationUI } as *const c_void,
        unsafe { kSecReturnData } as *const c_void,
        unsafe { kSecMatchLimit } as *const c_void,
    ];
    let values = [
        unsafe { kSecClassGenericPassword }.cast::<c_void>(),
        service.cast::<c_void>(),
        account.cast::<c_void>(),
        unsafe { kSecUseAuthenticationUIFail }.cast::<c_void>(),
        unsafe { kCFBooleanTrue }.cast::<c_void>(),
        unsafe { kSecMatchLimitAll }.cast::<c_void>(),
    ];
    let dictionary = unsafe {
        CFDictionaryCreate(
            kCFAllocatorDefault,
            keys.as_ptr(),
            values.as_ptr(),
            keys.len() as isize,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    };
    if dictionary.is_null() {
        return Err(KeychainError);
    }
    let mut result: CFTypeRef = null();
    let status = unsafe { SecItemCopyMatching(dictionary, &raw mut result) };
    unsafe {
        CFRelease(dictionary.cast());
        CFRelease(service.cast());
        CFRelease(account.cast());
    }
    if !result.is_null() {
        unsafe { CFRelease(result) };
    }
    if status == errSecItemNotFound {
        Ok(true)
    } else if status == errSecSuccess {
        Ok(false)
    } else {
        Err(KeychainError)
    }
}

pub(super) fn read_maintenance_record() -> Result<
    Option<talking_quill_owner_protocol::macos_maintenance::MacosMaintenanceRecord>,
    KeychainError,
> {
    let bytes = read_unique_item(
        "maintenance-latch-v1",
        talking_quill_owner_protocol::macos_maintenance::MACOS_MAINTENANCE_RECORD_BYTES,
    )?;
    talking_quill_owner_protocol::macos_maintenance::MacosMaintenanceRecord::decode(&bytes)
        .map_err(|_| KeychainError)
}

pub(super) fn write_maintenance_latch(
    value: &[u8; talking_quill_owner_protocol::macos_maintenance::MACOS_MAINTENANCE_RECORD_BYTES],
) -> Result<(), KeychainError> {
    validate_unique_item("maintenance-latch-v1", value.len(), |existing| {
        talking_quill_owner_protocol::macos_maintenance::MacosMaintenanceRecord::decode(existing)
            .is_ok()
    })?;
    let service = cf_string(SERVICE)?;
    let account = cf_string("maintenance-latch-v1")?;
    let data = unsafe { CFDataCreate(kCFAllocatorDefault, value.as_ptr(), value.len() as isize) };
    if data.is_null() {
        unsafe {
            CFRelease(service.cast());
            CFRelease(account.cast())
        };
        return Err(KeychainError);
    }
    let query_keys = [
        unsafe { kSecClass } as *const c_void,
        unsafe { kSecAttrService } as *const c_void,
        unsafe { kSecAttrAccount } as *const c_void,
        unsafe { kSecUseAuthenticationUI } as *const c_void,
    ];
    let query_values = [
        unsafe { kSecClassGenericPassword }.cast::<c_void>(),
        service.cast::<c_void>(),
        account.cast::<c_void>(),
        unsafe { kSecUseAuthenticationUIFail }.cast::<c_void>(),
    ];
    let update_keys = [unsafe { kSecValueData } as *const c_void];
    let update_values = [data.cast::<c_void>()];
    let query = unsafe {
        CFDictionaryCreate(
            kCFAllocatorDefault,
            query_keys.as_ptr(),
            query_values.as_ptr(),
            query_keys.len() as isize,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    };
    let updates = unsafe {
        CFDictionaryCreate(
            kCFAllocatorDefault,
            update_keys.as_ptr(),
            update_values.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    };
    let status = if query.is_null() || updates.is_null() {
        -1
    } else {
        unsafe { SecItemUpdate(query, updates) }
    };
    unsafe {
        if !query.is_null() {
            CFRelease(query.cast())
        };
        if !updates.is_null() {
            CFRelease(updates.cast())
        };
        CFRelease(data.cast());
        CFRelease(service.cast());
        CFRelease(account.cast());
    }
    (status == errSecSuccess).then_some(()).ok_or(KeychainError)
}

pub(super) fn delete_after_unregistration(
    _proof: super::service_management::UnregisteredLoginItem,
) -> Result<(), KeychainError> {
    for account in [ACCOUNT, "maintenance-latch-v1"] {
        if item_absent(account)? {
            continue;
        }
        match validate_unique_item(
            account,
            if account == ACCOUNT {
                32
            } else {
                talking_quill_owner_protocol::macos_maintenance::MACOS_MAINTENANCE_RECORD_BYTES
            },
            |_| true,
        ) {
            Ok(()) => {}
            Err(_) => return Err(KeychainError),
        }
        let service = cf_string(SERVICE)?;
        let account_value = cf_string(account)?;
        let keys = [
            unsafe { kSecClass } as *const c_void,
            unsafe { kSecAttrService } as *const c_void,
            unsafe { kSecAttrAccount } as *const c_void,
            unsafe { kSecUseAuthenticationUI } as *const c_void,
        ];
        let values = [
            unsafe { kSecClassGenericPassword }.cast::<c_void>(),
            service.cast::<c_void>(),
            account_value.cast::<c_void>(),
            unsafe { kSecUseAuthenticationUIFail }.cast::<c_void>(),
        ];
        let dictionary = unsafe {
            CFDictionaryCreate(
                kCFAllocatorDefault,
                keys.as_ptr(),
                values.as_ptr(),
                keys.len() as isize,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            )
        };
        let status = if dictionary.is_null() {
            -1
        } else {
            unsafe { SecItemDelete(dictionary) }
        };
        unsafe {
            if !dictionary.is_null() {
                CFRelease(dictionary.cast())
            };
            CFRelease(service.cast());
            CFRelease(account_value.cast());
        }
        if status != errSecSuccess {
            return Err(KeychainError);
        }
    }
    Ok(())
}

fn cf_string(value: &str) -> Result<core_foundation_sys::string::CFStringRef, KeychainError> {
    let string = unsafe {
        CFStringCreateWithBytes(
            kCFAllocatorDefault,
            value.as_ptr(),
            value.len() as isize,
            kCFStringEncodingUTF8,
            0,
        )
    };
    (!string.is_null()).then_some(string).ok_or(KeychainError)
}

unsafe extern "C" {
    static kSecUseAuthenticationUIFail: CFTypeRef;
}

#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("the installed owner Keychain credential is unavailable")]
pub struct KeychainError;
