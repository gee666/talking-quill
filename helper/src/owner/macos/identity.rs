#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use core_foundation_sys::base::{CFRelease, CFTypeRef, OSStatus, kCFAllocatorDefault};
use core_foundation_sys::data::{CFDataGetBytePtr, CFDataGetLength, CFDataRef};
use core_foundation_sys::dictionary::{
    CFDictionaryCreate, CFDictionaryGetValue, CFDictionaryRef, kCFTypeDictionaryKeyCallBacks,
    kCFTypeDictionaryValueCallBacks,
};
use core_foundation_sys::number::{CFNumberCreate, kCFNumberSInt64Type};
use core_foundation_sys::string::{CFStringCreateWithBytes, kCFStringEncodingUTF8};
use core_foundation_sys::url::{CFURLGetFileSystemRepresentation, CFURLRef};
use security_framework_sys::base::errSecSuccess;
use security_framework_sys::code_signing::{
    SecCodeCheckValidity, SecCodeCopyGuestWithAttributes, SecCodeCopyPath, SecCodeRef,
    SecStaticCodeRef, kSecCSNoNetworkAccess, kSecCSStrictValidate, kSecGuestAttributePid,
};
use sha2::{Digest, Sha256};
use talking_quill_owner_protocol::Bytes32;

/// Uses Security.framework's static-code authority, rather than trusting policy
/// text or a filesystem hash, to bind an installed role to its requirement.
pub fn validate_native_sec_code_identity(
    path: &Path,
    requirement: &str,
) -> Result<(), IdentityError> {
    let path_bytes = path.as_os_str().as_encoded_bytes();
    let url = unsafe {
        CFURLCreateFromFileSystemRepresentation(
            kCFAllocatorDefault,
            path_bytes.as_ptr(),
            path_bytes.len() as isize,
            0,
        )
    };
    let requirement_text = unsafe {
        CFStringCreateWithBytes(
            kCFAllocatorDefault,
            requirement.as_ptr(),
            requirement.len() as isize,
            kCFStringEncodingUTF8,
            0,
        )
    };
    if url.is_null() || requirement_text.is_null() {
        release(url.cast());
        release(requirement_text.cast());
        return Err(IdentityError);
    }
    let mut code: CFTypeRef = null();
    let mut requirement_ref: CFTypeRef = null();
    let statuses = unsafe {
        (
            SecStaticCodeCreateWithPath(url, 0, &raw mut code),
            SecRequirementCreateWithString(requirement_text, 0, &raw mut requirement_ref),
        )
    };
    let valid = statuses == (errSecSuccess, errSecSuccess)
        && !code.is_null()
        && !requirement_ref.is_null()
        && unsafe { SecStaticCodeCheckValidity(code, 0, requirement_ref) } == errSecSuccess;
    release(code);
    release(requirement_ref);
    release(url.cast());
    release(requirement_text.cast());
    valid.then_some(()).ok_or(IdentityError)
}

pub fn validate_spawned_sec_code_identity(
    pid: u32,
    expected_path: &Path,
    expected_hash: Bytes32,
    expected_code_directory_hash: [u8; 20],
    requirement: &str,
) -> Result<(), IdentityError> {
    let pid_value = i64::from(pid);
    let number = unsafe {
        CFNumberCreate(
            kCFAllocatorDefault,
            kCFNumberSInt64Type,
            (&raw const pid_value).cast(),
        )
    };
    if number.is_null() {
        return Err(IdentityError);
    }
    let key = unsafe { kSecGuestAttributePid } as *const c_void;
    let value = number.cast::<c_void>();
    let attributes = unsafe {
        CFDictionaryCreate(
            kCFAllocatorDefault,
            &key,
            &value,
            1,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    };
    release(number.cast());
    if attributes.is_null() {
        return Err(IdentityError);
    }
    let mut dynamic: SecCodeRef = null_mut();
    let guest_status = unsafe {
        SecCodeCopyGuestWithAttributes(null_mut(), attributes, kSecCSNoNetworkAccess, &mut dynamic)
    };
    release(attributes.cast());
    if guest_status != errSecSuccess || dynamic.is_null() {
        return Err(IdentityError);
    }
    let mut static_code: SecStaticCodeRef = null_mut();
    let mut requirement_ref: CFTypeRef = null();
    let requirement_text = unsafe {
        CFStringCreateWithBytes(
            kCFAllocatorDefault,
            requirement.as_ptr(),
            requirement.len() as isize,
            kCFStringEncodingUTF8,
            0,
        )
    };
    if requirement_text.is_null() {
        release(dynamic.cast());
        return Err(IdentityError);
    }
    let statuses = unsafe {
        (
            SecCodeCopyStaticCode(dynamic, kSecCSNoNetworkAccess, &mut static_code),
            SecRequirementCreateWithString(requirement_text, 0, &mut requirement_ref),
        )
    };
    if statuses != (errSecSuccess, errSecSuccess)
        || static_code.is_null()
        || requirement_ref.is_null()
        || unsafe {
            SecCodeCheckValidity(
                dynamic,
                kSecCSStrictValidate | kSecCSNoNetworkAccess,
                requirement_ref.cast_mut().cast(),
            )
        } != errSecSuccess
        || unsafe {
            SecStaticCodeCheckValidity(
                static_code.cast(),
                kSecCSStrictValidate | kSecCSNoNetworkAccess,
                requirement_ref.cast(),
            )
        } != errSecSuccess
    {
        release(dynamic.cast());
        release(static_code.cast());
        release(requirement_ref.cast());
        release(requirement_text.cast());
        return Err(IdentityError);
    }
    let dynamic_cdhash = code_directory_hash(dynamic)?;
    let static_cdhash = code_directory_hash(static_code.cast())?;
    if dynamic_cdhash != expected_code_directory_hash || static_cdhash != dynamic_cdhash {
        release(dynamic.cast());
        release(static_code.cast());
        release(requirement_ref.cast());
        release(requirement_text.cast());
        return Err(IdentityError);
    }
    let mut url: CFURLRef = null_mut();
    let copied = unsafe { SecCodeCopyPath(static_code, kSecCSNoNetworkAccess, &mut url) };
    let actual_path = if copied == errSecSuccess && !url.is_null() {
        path_from_url(url)
    } else {
        Err(IdentityError)
    };
    release(url.cast());
    release(dynamic.cast());
    release(static_code.cast());
    release(requirement_ref.cast());
    release(requirement_text.cast());
    let actual_path = actual_path?;
    if actual_path != expected_path || hash_no_follow(&actual_path)? != expected_hash {
        return Err(IdentityError);
    }
    Ok(())
}

fn code_directory_hash(code: SecCodeRef) -> Result<[u8; 20], IdentityError> {
    let mut information: CFDictionaryRef = null_mut();
    if unsafe { SecCodeCopySigningInformation(code, 1 << 1, &mut information) } != errSecSuccess
        || information.is_null()
    {
        return Err(IdentityError);
    }
    let value = unsafe { CFDictionaryGetValue(information, kSecCodeInfoUnique.cast::<c_void>()) }
        as CFDataRef;
    if value.is_null() || unsafe { CFDataGetLength(value) } != 20 {
        release(information.cast());
        return Err(IdentityError);
    }
    let mut hash = [0_u8; 20];
    unsafe {
        std::ptr::copy_nonoverlapping(CFDataGetBytePtr(value), hash.as_mut_ptr(), hash.len())
    };
    release(information.cast());
    Ok(hash)
}

fn path_from_url(url: CFURLRef) -> Result<PathBuf, IdentityError> {
    let mut bytes = [0_u8; 4096];
    if unsafe { CFURLGetFileSystemRepresentation(url, 1, bytes.as_mut_ptr(), bytes.len() as isize) }
        == 0
    {
        return Err(IdentityError);
    }
    let length = bytes
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(IdentityError)?;
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&bytes[..length])))
}

fn hash_no_follow(path: &Path) -> Result<Bytes32, IdentityError> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| IdentityError)?;
    let metadata = file.metadata().map_err(|_| IdentityError)?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() == 0 {
        return Err(IdentityError);
    }
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|_| IdentityError)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(Bytes32::new(digest.finalize().into()))
}

fn release(value: CFTypeRef) {
    if !value.is_null() {
        unsafe { CFRelease(value) };
    }
}

#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("native macOS code identity validation failed")]
pub struct IdentityError;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecCodeInfoUnique: core_foundation_sys::string::CFStringRef;
    fn SecCodeCopySigningInformation(
        code: SecCodeRef,
        flags: u32,
        information: *mut CFDictionaryRef,
    ) -> OSStatus;
    fn SecStaticCodeCreateWithPath(path: CFTypeRef, flags: u32, code: *mut CFTypeRef) -> OSStatus;
    fn SecRequirementCreateWithString(
        text: core_foundation_sys::string::CFStringRef,
        flags: u32,
        requirement: *mut CFTypeRef,
    ) -> OSStatus;
    fn SecStaticCodeCheckValidity(code: CFTypeRef, flags: u32, requirement: CFTypeRef) -> OSStatus;
    fn SecCodeCopyStaticCode(
        code: SecCodeRef,
        flags: u32,
        static_code: *mut SecStaticCodeRef,
    ) -> OSStatus;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFURLCreateFromFileSystemRepresentation(
        allocator: CFTypeRef,
        bytes: *const u8,
        length: isize,
        is_directory: u8,
    ) -> CFTypeRef;
}
