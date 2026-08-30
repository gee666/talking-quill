#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::ptr::null;

use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation_sys::base::{CFGetTypeID, CFRelease, CFTypeRef, kCFAllocatorDefault};
use core_foundation_sys::data::{
    CFDataCreate, CFDataGetBytePtr, CFDataGetLength, CFDataGetTypeID, CFDataRef,
};
use core_foundation_sys::dictionary::{
    CFDictionaryCreate, CFDictionaryRef, kCFTypeDictionaryKeyCallBacks,
    kCFTypeDictionaryValueCallBacks,
};
use core_foundation_sys::number::kCFBooleanTrue;
use core_foundation_sys::string::{CFStringCreateWithBytes, CFStringRef, kCFStringEncodingUTF8};
use security_framework_sys::base::{errSecItemNotFound, errSecSuccess};
use security_framework_sys::item::{
    kSecAttrAccessGroup, kSecAttrAccount, kSecAttrService, kSecClass, kSecClassGenericPassword,
    kSecMatchLimit, kSecMatchLimitAll, kSecReturnData, kSecUseAuthenticationUI, kSecValueData,
};
use security_framework_sys::keychain_item::{SecItemCopyMatching, SecItemDelete, SecItemUpdate};

use super::{
    HANDSHAKE_SECRET_ACCOUNT, KEYCHAIN_SECRET_BYTES, KEYCHAIN_SERVICE, KeychainError, KeychainItem,
    KeychainStore, MAINTENANCE_LATCH_ACCOUNT,
};

/// Fixed-query Security.framework Keychain adapter. It never creates an item
/// and every operation explicitly selects fail authentication UI.
#[derive(Debug, Default)]
pub struct NativeKeychainStore;

/// Removes only the two fixed owner items after ServiceManagement has
/// authoritatively unregistered the fixed LoginItem in this process.
pub fn delete_after_unregistration(
    _proof: super::service_management::UnregisteredLoginItem,
) -> Result<(), KeychainError> {
    for account in [HANDSHAKE_SECRET_ACCOUNT, MAINTENANCE_LATCH_ACCOUNT] {
        match ensure_unique_item(account) {
            Ok(()) => {}
            Err(KeychainError::Missing) => continue,
            Err(error) => return Err(error),
        }
        let query = Query::new(account, false)?;
        let status = unsafe { SecItemDelete(query.dictionary) };
        if status != errSecSuccess && status != errSecItemNotFound {
            return Err(KeychainError::Unavailable);
        }
    }
    Ok(())
}

pub fn read_maintenance_record_without_ui() -> Result<
    Option<talking_quill_owner_protocol::macos_maintenance::MacosMaintenanceRecord>,
    KeychainError,
> {
    let bytes = read_unique_bytes(
        MAINTENANCE_LATCH_ACCOUNT,
        talking_quill_owner_protocol::macos_maintenance::MACOS_MAINTENANCE_RECORD_BYTES,
    )?;
    talking_quill_owner_protocol::macos_maintenance::MacosMaintenanceRecord::decode(&bytes)
        .map_err(|_| KeychainError::InvalidItem)
}

pub fn clear_maintenance_record_without_ui() -> Result<(), KeychainError> {
    let mut store = NativeKeychainStore;
    store.write_maintenance_latch_without_ui(
        &[0_u8; talking_quill_owner_protocol::macos_maintenance::MACOS_MAINTENANCE_RECORD_BYTES],
    )
}

fn read_unique_bytes(account: &str, expected_len: usize) -> Result<Vec<u8>, KeychainError> {
    let query = Query::new(account, true)?;
    let mut result: CFTypeRef = null();
    let status = unsafe { SecItemCopyMatching(query.dictionary, &mut result) };
    if status == errSecItemNotFound {
        return Err(KeychainError::Missing);
    }
    if status != errSecSuccess || result.is_null() {
        return Err(KeychainError::Unavailable);
    }
    if unsafe { CFGetTypeID(result) } != unsafe { core_foundation_sys::array::CFArrayGetTypeID() }
        || unsafe { CFArrayGetCount(result as CFArrayRef) } != 1
    {
        unsafe { CFRelease(result) };
        return Err(KeychainError::InvalidItem);
    }
    let data = unsafe { CFArrayGetValueAtIndex(result as CFArrayRef, 0) } as CFDataRef;
    if data.is_null()
        || unsafe { CFGetTypeID(data.cast()) } != unsafe { CFDataGetTypeID() }
        || unsafe { CFDataGetLength(data) } != expected_len as isize
    {
        unsafe { CFRelease(result) };
        return Err(KeychainError::InvalidItem);
    }
    let mut bytes = vec![0_u8; expected_len];
    unsafe {
        std::ptr::copy_nonoverlapping(CFDataGetBytePtr(data), bytes.as_mut_ptr(), bytes.len());
        CFRelease(result);
    }
    Ok(bytes)
}

impl KeychainStore for NativeKeychainStore {
    fn read_owner_handshake_secret_without_ui(&self) -> Result<KeychainItem, KeychainError> {
        let bytes = read_unique_bytes(HANDSHAKE_SECRET_ACCOUNT, KEYCHAIN_SECRET_BYTES)?;
        KeychainItem::from_bytes(bytes.try_into().map_err(|_| KeychainError::InvalidItem)?)
    }

    fn write_maintenance_latch_without_ui(&mut self, value: &[u8]) -> Result<(), KeychainError> {
        ensure_unique_item(MAINTENANCE_LATCH_ACCOUNT)?;
        let query = Query::new(MAINTENANCE_LATCH_ACCOUNT, false)?;
        // SAFETY: CoreFoundation copies the bounded latch bytes.
        let data = unsafe {
            CFDataCreate(
                kCFAllocatorDefault,
                value.as_ptr(),
                isize::try_from(value.len()).map_err(|_| KeychainError::Unavailable)?,
            )
        };
        if data.is_null() {
            return Err(KeychainError::Unavailable);
        }
        let keys = [unsafe { kSecValueData } as *const c_void];
        let values = [data.cast::<c_void>()];
        let updates = dictionary(&keys, &values)?;
        // SAFETY: both dictionaries and their retained values remain live.
        let status = unsafe { SecItemUpdate(query.dictionary, updates) };
        // SAFETY: create-rule objects are owned here.
        unsafe {
            CFRelease(updates.cast());
            CFRelease(data.cast());
        }
        if status == errSecSuccess {
            Ok(())
        } else if status == errSecItemNotFound {
            Err(KeychainError::Missing)
        } else {
            Err(KeychainError::Unavailable)
        }
    }
}

pub(super) fn fixed_items_absent_without_ui() -> Result<bool, KeychainError> {
    let statuses = fixed_item_query_statuses_without_ui()?;
    if statuses
        .into_iter()
        .all(|status| status == errSecItemNotFound)
    {
        Ok(true)
    } else if statuses.into_iter().any(|status| status == errSecSuccess) {
        Ok(false)
    } else {
        Err(KeychainError::Unavailable)
    }
}

/// Raw no-UI terminal evidence for a separately signed, ACL-enrolled native
/// lifecycle fixture. Callers must require `errSecItemNotFound` exactly; ACL
/// denial and every other OSStatus remain distinguishable failures.
pub fn fixed_item_query_statuses_without_ui() -> Result<[i32; 2], KeychainError> {
    Ok([
        item_query_status(HANDSHAKE_SECRET_ACCOUNT)?,
        item_query_status(MAINTENANCE_LATCH_ACCOUNT)?,
    ])
}

fn item_query_status(account: &str) -> Result<i32, KeychainError> {
    let query = Query::new(account, true)?;
    let mut result: CFTypeRef = null();
    let status = unsafe { SecItemCopyMatching(query.dictionary, &mut result) };
    if !result.is_null() {
        unsafe { CFRelease(result) };
    }
    Ok(status)
}

fn ensure_unique_item(account: &str) -> Result<(), KeychainError> {
    let query = Query::new(account, true)?;
    let mut result: CFTypeRef = null();
    let status = unsafe { SecItemCopyMatching(query.dictionary, &mut result) };
    if status == errSecItemNotFound {
        return Err(KeychainError::Missing);
    }
    if status != errSecSuccess || result.is_null() {
        return Err(KeychainError::Unavailable);
    }
    let unique = unsafe { CFGetTypeID(result) }
        == unsafe { core_foundation_sys::array::CFArrayGetTypeID() }
        && unsafe { CFArrayGetCount(result as CFArrayRef) } == 1;
    unsafe { CFRelease(result) };
    if unique {
        Ok(())
    } else {
        Err(KeychainError::InvalidItem)
    }
}

struct Query {
    dictionary: CFDictionaryRef,
    _service: CFStringRef,
    _account: CFStringRef,
    _access_group: Option<CFStringRef>,
}

unsafe extern "C" {
    static kSecUseAuthenticationUIFail: CFTypeRef;
}

fn optional_access_group(value: Option<&str>) -> Option<&str> {
    value.filter(|group| !group.is_empty())
}

impl Query {
    fn new(account: &str, return_data: bool) -> Result<Self, KeychainError> {
        let access_group_name =
            optional_access_group(option_env!("TALKING_QUILL_MACOS_KEYCHAIN_ACCESS_GROUP"));
        let service = cf_string(KEYCHAIN_SERVICE)?;
        let account = cf_string(account)?;
        let access_group = access_group_name.map(cf_string).transpose()?;
        let mut keys = vec![
            unsafe { kSecClass } as *const c_void,
            unsafe { kSecAttrService } as *const c_void,
            unsafe { kSecAttrAccount } as *const c_void,
            unsafe { kSecUseAuthenticationUI } as *const c_void,
        ];
        let mut values = vec![
            unsafe { kSecClassGenericPassword }.cast::<c_void>(),
            service.cast::<c_void>(),
            account.cast::<c_void>(),
            unsafe { kSecUseAuthenticationUIFail }.cast::<c_void>(),
        ];
        if let Some(access_group) = access_group {
            keys.push(unsafe { kSecAttrAccessGroup } as *const c_void);
            values.push(access_group.cast::<c_void>());
        }
        if return_data {
            keys.push(unsafe { kSecReturnData } as *const c_void);
            values.push(unsafe { kCFBooleanTrue }.cast::<c_void>());
            keys.push(unsafe { kSecMatchLimit } as *const c_void);
            values.push(unsafe { kSecMatchLimitAll }.cast::<c_void>());
        }
        let dictionary = dictionary(&keys, &values)?;
        Ok(Self {
            dictionary,
            _service: service,
            _account: account,
            _access_group: access_group,
        })
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        // SAFETY: all values follow create rules; the dictionary retained
        // them independently.
        unsafe {
            CFRelease(self.dictionary.cast());
            CFRelease(self._service.cast());
            CFRelease(self._account.cast());
            if let Some(access_group) = self._access_group {
                CFRelease(access_group.cast());
            }
        }
    }
}

fn dictionary(
    keys: &[*const c_void],
    values: &[*const c_void],
) -> Result<CFDictionaryRef, KeychainError> {
    // SAFETY: arrays have equal lengths and callbacks retain CF values.
    let dictionary = unsafe {
        CFDictionaryCreate(
            kCFAllocatorDefault,
            keys.as_ptr(),
            values.as_ptr(),
            isize::try_from(keys.len()).map_err(|_| KeychainError::Unavailable)?,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    };
    if dictionary.is_null() {
        Err(KeychainError::Unavailable)
    } else {
        Ok(dictionary)
    }
}

fn cf_string(value: &str) -> Result<CFStringRef, KeychainError> {
    // SAFETY: value is valid UTF-8 and CoreFoundation copies it.
    let string = unsafe {
        CFStringCreateWithBytes(
            kCFAllocatorDefault,
            value.as_ptr(),
            isize::try_from(value.len()).map_err(|_| KeychainError::Unavailable)?,
            kCFStringEncodingUTF8,
            0,
        )
    };
    if string.is_null() {
        Err(KeychainError::Unavailable)
    } else {
        Ok(string)
    }
}

#[cfg(test)]
mod tests {
    use super::optional_access_group;

    #[test]
    fn absent_or_empty_access_group_omits_only_that_selector() {
        assert_eq!(optional_access_group(None), None);
        assert_eq!(optional_access_group(Some("")), None);
        assert_eq!(
            optional_access_group(Some("local.group")),
            Some("local.group")
        );
    }
}
