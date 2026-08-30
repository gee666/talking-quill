#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::mem::{size_of, zeroed};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::ptr::null_mut;

use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation_sys::base::{CFGetTypeID, CFRelease, CFTypeRef, kCFAllocatorDefault};
use core_foundation_sys::data::{
    CFDataCreate, CFDataGetBytePtr, CFDataGetLength, CFDataGetTypeID, CFDataRef,
};
use core_foundation_sys::dictionary::{
    CFDictionaryCreate, CFDictionaryGetValue, CFDictionaryRef, kCFTypeDictionaryKeyCallBacks,
    kCFTypeDictionaryValueCallBacks,
};
use core_foundation_sys::number::{
    CFNumberCreate, CFNumberGetValue, CFNumberRef, kCFNumberSInt64Type,
};
use core_foundation_sys::string::{
    CFStringCreateWithBytes, CFStringGetCString, CFStringRef, kCFStringEncodingUTF8,
};
use core_foundation_sys::url::{CFURLGetFileSystemRepresentation, CFURLRef};
use security_framework_sys::base::{SecCertificateRef, errSecSuccess};
use security_framework_sys::certificate::SecCertificateCopyData;
use security_framework_sys::code_signing::{
    SecCodeCheckValidity, SecCodeCopyGuestWithAttributes, SecCodeCopySelf, SecCodeRef,
    SecRequirementCreateWithString, SecRequirementRef, SecStaticCodeCheckValidity,
    SecStaticCodeRef, kSecCSNoNetworkAccess, kSecCSStrictValidate, kSecGuestAttributeAudit,
    kSecGuestAttributePid,
};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use talking_quill_owner_protocol::Bytes32;

use super::{
    AuditToken, CodeIdentity, IdentityError, LocalSigningIdentity, PeerCredentials,
    PeerEvidenceProvider, RequirementHash,
};

const PATH_BUFFER_BYTES: usize = 4096;
const MAX_EXECUTABLE_BYTES: i64 = 256 * 1024 * 1024;
const K_SEC_CS_SIGNING_INFORMATION: u32 = 1 << 1;
const K_SEC_CODE_SIGNATURE_ADHOC: i64 = 1 << 1;

unsafe extern "C" {
    static kSecCodeInfoFlags: CFStringRef;
    static kSecCodeInfoIdentifier: CFStringRef;
    static kSecCodeInfoUnique: CFStringRef;
    static kSecCodeInfoCertificates: CFStringRef;
    static kSecCodeInfoMainExecutable: CFStringRef;
    fn SecCodeCopyStaticCode(
        code: SecCodeRef,
        flags: u32,
        static_code: *mut SecStaticCodeRef,
    ) -> i32;
    fn SecCodeCopySigningInformation(
        code: SecCodeRef,
        flags: u32,
        information: *mut CFDictionaryRef,
    ) -> i32;
}

/// Kernel and Security.framework evidence retained for one accepted Unix socket.
pub struct NativePeerEvidence {
    credentials: PeerCredentials,
    pid: u32,
    token: AuditToken,
    expected_identifier: String,
    expected_path: PathBuf,
    expected_executable_sha256: Bytes32,
    expected_release_build_digest: Bytes32,
    expected_signing_identity: LocalSigningIdentity,
    connection_binding: Bytes32,
}

impl std::fmt::Debug for NativePeerEvidence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("NativePeerEvidence(<redacted>)")
    }
}

pub(crate) fn socket_kernel_peer_identity(
    stream: &std::os::unix::net::UnixStream,
) -> Result<(u32, AuditToken, u32), IdentityError> {
    let fd = stream.as_raw_fd();
    let mut uid = 0;
    let mut gid = 0;
    if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
        return Err(IdentityError::PeerCredentialsUnavailable);
    }
    let pid = socket_peer_pid(fd)?;
    let token = socket_peer_token(fd)?;
    if pid == 0 || token.pid() != pid || token.effective_uid() != uid {
        return Err(IdentityError::PeerTokenMismatch);
    }
    Ok((pid, token, uid))
}

impl NativePeerEvidence {
    #[allow(clippy::too_many_arguments)]
    pub fn acquire(
        stream: &std::os::unix::net::UnixStream,
        expected_identifier: String,
        expected_path: PathBuf,
        expected_executable_sha256: Bytes32,
        expected_release_build_digest: Bytes32,
        expected_signing_identity: LocalSigningIdentity,
    ) -> Result<Self, IdentityError> {
        let fd = stream.as_raw_fd();
        let mut uid = 0;
        let mut gid = 0;
        // SAFETY: uid/gid point to initialized storage and fd remains borrowed.
        if unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } != 0 {
            return Err(IdentityError::PeerCredentialsUnavailable);
        }
        let pid = socket_peer_pid(fd)?;
        let token = socket_peer_token(fd)?;
        if pid == 0 || token.pid() != pid {
            return Err(IdentityError::PeerTokenMismatch);
        }
        let connection_binding = super::audit_token_connection_binding(token, uid);
        Ok(Self {
            credentials: PeerCredentials { uid, gid },
            pid,
            token,
            expected_identifier,
            expected_path,
            expected_executable_sha256,
            expected_release_build_digest,
            expected_signing_identity,
            connection_binding,
        })
    }

    fn guest_code(&self) -> Result<OwnedCode, IdentityError> {
        guest_code_for_token(self.token)
    }

    fn bound_static_code(&self) -> Result<BoundStaticCode, IdentityError> {
        let dynamic = self.guest_code()?;
        let mut static_code: SecStaticCodeRef = null_mut();
        // SAFETY: dynamic is the audit-token-selected guest and output is writable.
        if unsafe { SecCodeCopyStaticCode(dynamic.0, kSecCSNoNetworkAccess, &mut static_code) }
            != errSecSuccess
            || static_code.is_null()
        {
            return Err(IdentityError::InvalidCode);
        }
        let static_code = OwnedStaticCode(static_code);
        let path = static_code_path(static_code.0)?;
        let file = open_executable_vnode(&path)?;
        let before = fstat(file.as_raw_fd())?;
        if (before.st_mode & libc::S_IFMT) != libc::S_IFREG
            || before.st_size <= 0
            || before.st_size > MAX_EXECUTABLE_BYTES
        {
            return Err(IdentityError::IdentityMismatch);
        }
        Ok(BoundStaticCode {
            dynamic,
            static_code,
            path,
            file,
            device: before.st_dev as u64,
            inode: before.st_ino,
        })
    }
}

impl PeerEvidenceProvider for NativePeerEvidence {
    fn peer_credentials(&self) -> Result<PeerCredentials, IdentityError> {
        Ok(self.credentials)
    }

    fn peer_pid(&self) -> Result<u32, IdentityError> {
        Ok(self.pid)
    }

    fn peer_audit_token(&self) -> Result<AuditToken, IdentityError> {
        Ok(self.token)
    }

    fn sec_code_identity(&self, token: AuditToken) -> Result<CodeIdentity, IdentityError> {
        if token != self.token {
            return Err(IdentityError::PeerTokenMismatch);
        }
        let mut bound = self.bound_static_code()?;
        // The audit-token-selected running guest is the primary authority.
        // Static-code and vnode validation below are additional disk binding.
        let dynamic_valid = unsafe {
            SecCodeCheckValidity(
                bound.dynamic.0,
                kSecCSStrictValidate | kSecCSNoNetworkAccess,
                null_mut(),
            )
        } == errSecSuccess;
        let static_valid = unsafe {
            SecStaticCodeCheckValidity(
                bound.static_code.0,
                kSecCSStrictValidate | kSecCSNoNetworkAccess,
                null_mut(),
            )
        } == errSecSuccess;
        let dynamic_signing = signing_identity(bound.dynamic.0)?;
        let static_signing = signing_identity(bound.static_code.0.cast())?;
        let signing_information_matches = dynamic_signing == static_signing;
        let actual_signing = dynamic_signing;
        let executable_sha256 = hash_retained_file(&mut bound.file)?;
        let after = fstat(bound.file.as_raw_fd())?;
        let vnode_stable = bound.device == after.st_dev as u64 && bound.inode == after.st_ino;
        let identity_matches = actual_signing.identity == self.expected_signing_identity;
        let identifier_matches = actual_signing.identifier == self.expected_identifier;
        let path_matches = self.expected_path == bound.path;
        Ok(CodeIdentity {
            signing_identifier: actual_signing.identifier,
            canonical_executable_path: bound.path,
            executable_sha256,
            code_directory_hash: actual_signing.code_directory_hash,
            release_build_digest: self.expected_release_build_digest,
            signing_identity: actual_signing.identity,
            statically_valid: dynamic_valid
                && static_valid
                && signing_information_matches
                && vnode_stable
                && executable_sha256 == self.expected_executable_sha256
                && identity_matches
                && identifier_matches
                && path_matches,
        })
    }

    fn evaluate_designated_requirement(
        &self,
        token: AuditToken,
        requirement: &str,
    ) -> Result<bool, IdentityError> {
        if token != self.token {
            return Err(IdentityError::PeerTokenMismatch);
        }
        let bound = self.bound_static_code()?;
        let text = cf_string(requirement)?;
        let mut compiled: SecRequirementRef = null_mut();
        // SAFETY: text is a valid UTF-8 CFString and output is writable.
        let create = unsafe { SecRequirementCreateWithString(text, 0, &mut compiled) };
        unsafe { CFRelease(text.cast()) };
        if create != errSecSuccess || compiled.is_null() {
            return Err(IdentityError::InvalidRequirement);
        }
        // The designated requirement must validate the exact running guest and
        // the additional static code bound to its retained executable vnode.
        let dynamic_status = unsafe {
            SecCodeCheckValidity(
                bound.dynamic.0,
                kSecCSStrictValidate | kSecCSNoNetworkAccess,
                compiled,
            )
        };
        let static_status = unsafe {
            SecStaticCodeCheckValidity(
                bound.static_code.0,
                kSecCSStrictValidate | kSecCSNoNetworkAccess,
                compiled,
            )
        };
        let dynamic_signing = signing_identity(bound.dynamic.0);
        let static_signing = signing_identity(bound.static_code.0.cast());
        unsafe { CFRelease(compiled.cast()) };
        let signing_information_matches = dynamic_signing? == static_signing?;
        let retained = fstat(bound.file.as_raw_fd())?;
        Ok(dynamic_status == errSecSuccess
            && static_status == errSecSuccess
            && signing_information_matches
            && retained.st_dev as u64 == bound.device
            && retained.st_ino == bound.inode)
    }

    fn connection_binding(&self) -> Result<Bytes32, IdentityError> {
        Ok(self.connection_binding)
    }
}

#[cfg(test)]
pub(crate) fn current_process_code_identity(
    release_build_digest: Bytes32,
) -> Result<CodeIdentity, IdentityError> {
    current_process_code_identity_and_vnode(release_build_digest).map(|(identity, _)| identity)
}

fn current_process_code_identity_and_vnode(
    release_build_digest: Bytes32,
) -> Result<(CodeIdentity, OwnedFd), IdentityError> {
    let mut code: SecCodeRef = null_mut();
    if unsafe { SecCodeCopySelf(kSecCSNoNetworkAccess, &mut code) } != errSecSuccess
        || code.is_null()
    {
        return Err(IdentityError::InvalidCode);
    }
    let dynamic = OwnedCode(code);
    let mut static_code: SecStaticCodeRef = null_mut();
    if unsafe { SecCodeCopyStaticCode(dynamic.0, kSecCSNoNetworkAccess, &mut static_code) }
        != errSecSuccess
        || static_code.is_null()
    {
        return Err(IdentityError::InvalidCode);
    }
    let static_code = OwnedStaticCode(static_code);
    if unsafe {
        SecCodeCheckValidity(
            dynamic.0,
            kSecCSStrictValidate | kSecCSNoNetworkAccess,
            null_mut(),
        )
    } != errSecSuccess
        || unsafe {
            SecStaticCodeCheckValidity(
                static_code.0,
                kSecCSStrictValidate | kSecCSNoNetworkAccess,
                null_mut(),
            )
        } != errSecSuccess
    {
        return Err(IdentityError::InvalidCode);
    }
    let dynamic_signing = signing_identity(dynamic.0)?;
    let static_signing = signing_identity(static_code.0.cast())?;
    if dynamic_signing != static_signing {
        return Err(IdentityError::InvalidCode);
    }
    let path = static_code_path(static_code.0)?;
    let mut file = open_executable_vnode(&path)?;
    let before = fstat(file.as_raw_fd())?;
    if (before.st_mode & libc::S_IFMT) != libc::S_IFREG
        || before.st_size <= 0
        || before.st_size > MAX_EXECUTABLE_BYTES
    {
        return Err(IdentityError::IdentityMismatch);
    }
    let executable_sha256 = hash_retained_file(&mut file)?;
    let after = fstat(file.as_raw_fd())?;
    if before.st_dev != after.st_dev || before.st_ino != after.st_ino {
        return Err(IdentityError::IdentityMismatch);
    }
    Ok((
        CodeIdentity {
            signing_identifier: dynamic_signing.identifier,
            canonical_executable_path: path,
            executable_sha256,
            code_directory_hash: dynamic_signing.code_directory_hash,
            release_build_digest,
            signing_identity: dynamic_signing.identity,
            statically_valid: true,
        },
        file,
    ))
}

pub(crate) fn validate_current_process_against_retaining_executable(
    expected: &super::RolePolicy,
    release_build_digest: Bytes32,
) -> Result<OwnedFd, IdentityError> {
    let (actual, executable) = current_process_code_identity_and_vnode(release_build_digest)?;
    validate_current_identity(&actual, expected)?;
    validate_current_requirement(&expected.designated_requirement)?;
    Ok(executable)
}

#[cfg(test)]
pub(crate) fn validate_current_process_against(
    expected: &super::RolePolicy,
    release_build_digest: Bytes32,
) -> Result<(), IdentityError> {
    let actual = current_process_code_identity(release_build_digest)?;
    validate_current_identity(&actual, expected)?;
    validate_current_requirement(&expected.designated_requirement)
}

fn validate_current_identity(
    actual: &CodeIdentity,
    expected: &super::RolePolicy,
) -> Result<(), IdentityError> {
    if actual.signing_identifier != expected.signing_identifier
        || actual.canonical_executable_path != expected.canonical_executable_path
        || actual.code_directory_hash != expected.code_directory_hash
        || actual.executable_sha256 != expected.executable_sha256
        || actual.signing_identity != expected.signing_identity
    {
        Err(IdentityError::IdentityMismatch)
    } else {
        Ok(())
    }
}

fn validate_current_requirement(requirement_text: &str) -> Result<(), IdentityError> {
    let text = cf_string(requirement_text)?;
    let mut requirement: SecRequirementRef = null_mut();
    let create = unsafe { SecRequirementCreateWithString(text, 0, &mut requirement) };
    unsafe { CFRelease(text.cast()) };
    if create != errSecSuccess || requirement.is_null() {
        return Err(IdentityError::InvalidRequirement);
    }
    let mut code: SecCodeRef = null_mut();
    let copy = unsafe { SecCodeCopySelf(kSecCSNoNetworkAccess, &mut code) };
    if copy != errSecSuccess || code.is_null() {
        unsafe { CFRelease(requirement.cast()) };
        return Err(IdentityError::InvalidCode);
    }
    let dynamic = OwnedCode(code);
    let status = unsafe {
        SecCodeCheckValidity(
            dynamic.0,
            kSecCSStrictValidate | kSecCSNoNetworkAccess,
            requirement,
        )
    };
    unsafe { CFRelease(requirement.cast()) };
    if status == errSecSuccess {
        Ok(())
    } else {
        Err(IdentityError::InvalidCode)
    }
}

pub(crate) fn validate_spawned_process_against(
    pid: u32,
    expected_audit_session: u32,
    expected: &super::RolePolicy,
) -> Result<(), IdentityError> {
    if pid == 0 || expected_audit_session == 0 {
        return Err(IdentityError::PeerTokenMismatch);
    }
    let dynamic = guest_code_for_pid(pid)?;
    let mut static_code: SecStaticCodeRef = null_mut();
    if unsafe { SecCodeCopyStaticCode(dynamic.0, kSecCSNoNetworkAccess, &mut static_code) }
        != errSecSuccess
        || static_code.is_null()
    {
        return Err(IdentityError::InvalidCode);
    }
    let static_code = OwnedStaticCode(static_code);
    let requirement = cf_string(&expected.designated_requirement)?;
    let mut compiled: SecRequirementRef = null_mut();
    let create = unsafe { SecRequirementCreateWithString(requirement, 0, &mut compiled) };
    unsafe { CFRelease(requirement.cast()) };
    if create != errSecSuccess || compiled.is_null() {
        return Err(IdentityError::InvalidRequirement);
    }
    let valid = unsafe {
        SecCodeCheckValidity(
            dynamic.0,
            kSecCSStrictValidate | kSecCSNoNetworkAccess,
            compiled,
        )
    } == errSecSuccess
        && unsafe {
            SecStaticCodeCheckValidity(
                static_code.0,
                kSecCSStrictValidate | kSecCSNoNetworkAccess,
                compiled,
            )
        } == errSecSuccess;
    unsafe { CFRelease(compiled.cast()) };
    let path = static_code_path(static_code.0)?;
    let mut file = open_executable_vnode(&path)?;
    let before = fstat(file.as_raw_fd())?;
    let hash = hash_retained_file(&mut file)?;
    let after = fstat(file.as_raw_fd())?;
    let dynamic_signing = signing_identity(dynamic.0)?;
    let static_signing = signing_identity(static_code.0.cast())?;
    let current_session = current_audit_token()?.audit_session_id();
    (valid
        && current_session == expected_audit_session
        && path == expected.canonical_executable_path
        && hash == expected.executable_sha256
        && before.st_dev == after.st_dev
        && before.st_ino == after.st_ino
        && dynamic_signing == static_signing
        && dynamic_signing.identifier == expected.signing_identifier
        && dynamic_signing.code_directory_hash == expected.code_directory_hash
        && dynamic_signing.identity == expected.signing_identity)
        .then_some(())
        .ok_or(IdentityError::IdentityMismatch)
}

pub(crate) fn validate_retained_spawned_process_against(
    expected: &super::RolePolicy,
) -> Result<(), IdentityError> {
    let mut code: SecCodeRef = null_mut();
    if unsafe { SecCodeCopySelf(kSecCSNoNetworkAccess, &mut code) } != errSecSuccess
        || code.is_null()
    {
        return Err(IdentityError::InvalidCode);
    }
    let dynamic = OwnedCode(code);
    let mut static_code: SecStaticCodeRef = null_mut();
    if unsafe { SecCodeCopyStaticCode(dynamic.0, kSecCSNoNetworkAccess, &mut static_code) }
        != errSecSuccess
        || static_code.is_null()
    {
        return Err(IdentityError::InvalidCode);
    }
    let static_code = OwnedStaticCode(static_code);
    if unsafe {
        SecCodeCheckValidity(
            dynamic.0,
            kSecCSStrictValidate | kSecCSNoNetworkAccess,
            null_mut(),
        )
    } != errSecSuccess
        || unsafe {
            SecStaticCodeCheckValidity(
                static_code.0,
                kSecCSStrictValidate | kSecCSNoNetworkAccess,
                null_mut(),
            )
        } != errSecSuccess
    {
        return Err(IdentityError::InvalidCode);
    }
    let dynamic_signing = signing_identity(dynamic.0)?;
    let static_signing = signing_identity(static_code.0.cast())?;
    if dynamic_signing != static_signing
        || dynamic_signing.identifier != expected.signing_identifier
        || dynamic_signing.identity != expected.signing_identity
    {
        return Err(IdentityError::IdentityMismatch);
    }
    validate_current_requirement(&expected.designated_requirement)
}

pub fn validate_current_process_code() -> Result<(), IdentityError> {
    let mut code: SecCodeRef = null_mut();
    if unsafe { SecCodeCopySelf(kSecCSNoNetworkAccess, &mut code) } != errSecSuccess
        || code.is_null()
    {
        return Err(IdentityError::InvalidCode);
    }
    let dynamic = OwnedCode(code);
    let mut static_code: SecStaticCodeRef = null_mut();
    if unsafe { SecCodeCopyStaticCode(dynamic.0, kSecCSNoNetworkAccess, &mut static_code) }
        != errSecSuccess
        || static_code.is_null()
    {
        return Err(IdentityError::InvalidCode);
    }
    let static_code = OwnedStaticCode(static_code);
    let status = unsafe {
        SecStaticCodeCheckValidity(
            static_code.0,
            kSecCSStrictValidate | kSecCSNoNetworkAccess,
            null_mut(),
        )
    };
    if status == errSecSuccess {
        Ok(())
    } else {
        Err(IdentityError::InvalidCode)
    }
}

pub fn current_audit_token() -> Result<AuditToken, IdentityError> {
    let mut descriptors = [0; 2];
    if unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_STREAM,
            0,
            descriptors.as_mut_ptr(),
        )
    } != 0
    {
        return Err(IdentityError::AuditTokenUnavailable);
    }
    let token = socket_peer_token(descriptors[0]);
    unsafe {
        libc::close(descriptors[0]);
        libc::close(descriptors[1]);
    }
    token
}

fn guest_code_for_pid(pid: u32) -> Result<OwnedCode, IdentityError> {
    let pid = i64::from(pid);
    let number = unsafe {
        CFNumberCreate(
            kCFAllocatorDefault,
            kCFNumberSInt64Type,
            (&raw const pid).cast(),
        )
    };
    if number.is_null() {
        return Err(IdentityError::InvalidCode);
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
    unsafe { CFRelease(number.cast()) };
    if attributes.is_null() {
        return Err(IdentityError::InvalidCode);
    }
    let mut code: SecCodeRef = null_mut();
    let status = unsafe {
        SecCodeCopyGuestWithAttributes(null_mut(), attributes, kSecCSNoNetworkAccess, &mut code)
    };
    unsafe { CFRelease(attributes.cast()) };
    if status != errSecSuccess || code.is_null() {
        Err(IdentityError::InvalidCode)
    } else {
        Ok(OwnedCode(code))
    }
}

fn guest_code_for_token(token: AuditToken) -> Result<OwnedCode, IdentityError> {
    let words = token.words();
    let data = unsafe {
        CFDataCreate(
            kCFAllocatorDefault,
            words.as_ptr().cast::<u8>(),
            isize::try_from(size_of::<[u32; 8]>())
                .map_err(|_| IdentityError::AuditTokenUnavailable)?,
        )
    };
    if data.is_null() {
        return Err(IdentityError::AuditTokenUnavailable);
    }
    let key = unsafe { kSecGuestAttributeAudit } as *const c_void;
    let value = data.cast::<c_void>();
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
    unsafe { CFRelease(data.cast()) };
    if attributes.is_null() {
        return Err(IdentityError::InvalidCode);
    }
    let mut code: SecCodeRef = null_mut();
    let status = unsafe {
        SecCodeCopyGuestWithAttributes(null_mut(), attributes, kSecCSNoNetworkAccess, &mut code)
    };
    unsafe { CFRelease(attributes.cast()) };
    if status != errSecSuccess || code.is_null() {
        Err(IdentityError::InvalidCode)
    } else {
        Ok(OwnedCode(code))
    }
}

fn signing_identity(code: SecCodeRef) -> Result<ActualSigningIdentity, IdentityError> {
    let mut information: CFDictionaryRef = null_mut();
    if unsafe {
        SecCodeCopySigningInformation(code, K_SEC_CS_SIGNING_INFORMATION, &mut information)
    } != errSecSuccess
        || information.is_null()
    {
        return Err(IdentityError::InvalidCode);
    }
    let information = OwnedCf(information.cast());
    let identifier = dictionary_string(information.0.cast(), unsafe { kSecCodeInfoIdentifier })?;
    let flags = dictionary_i64(information.0.cast(), unsafe { kSecCodeInfoFlags })?;
    // kSecCodeInfoUnique is the CodeDirectory unique hash (CDHash). Extract it
    // for every signing class so dynamic/static equality is not reduced to
    // certificate and identifier equality for locally signed code.
    let unique = dictionary_data(information.0.cast(), unsafe { kSecCodeInfoUnique })?;
    let code_directory_hash: [u8; 20] = unique
        .as_slice()
        .try_into()
        .map_err(|_| IdentityError::InvalidCode)?;
    let code_directory_hash = RequirementHash::new(code_directory_hash);
    let certificates = unsafe {
        CFDictionaryGetValue(
            information.0.cast(),
            kSecCodeInfoCertificates.cast::<c_void>(),
        )
    } as CFArrayRef;
    let identity = if flags & K_SEC_CODE_SIGNATURE_ADHOC != 0 {
        if !certificates.is_null() && unsafe { CFArrayGetCount(certificates) } != 0 {
            return Err(IdentityError::InvalidCode);
        }
        LocalSigningIdentity::AdHoc {
            code_directory_hash,
        }
    } else {
        if certificates.is_null() || unsafe { CFArrayGetCount(certificates) } != 1 {
            // Developer ID and ordinary certificate chains are intentionally excluded.
            return Err(IdentityError::InvalidCode);
        }
        let certificate = unsafe { CFArrayGetValueAtIndex(certificates, 0) } as SecCertificateRef;
        if certificate.is_null() {
            return Err(IdentityError::InvalidCode);
        }
        let der = certificate_der(certificate)?;
        let certificate_sha256 = Bytes32::new(Sha256::digest(&der).into());
        let requirement_certificate_hash =
            RequirementHash::new(<Sha1 as sha1::Digest>::digest(&der).into());
        LocalSigningIdentity::LocallyTrustedSelfSigned {
            certificate_sha256,
            requirement_certificate_hash,
        }
    };
    Ok(ActualSigningIdentity {
        identifier,
        code_directory_hash,
        identity,
    })
}

fn static_code_path(code: SecStaticCodeRef) -> Result<PathBuf, IdentityError> {
    let mut information: CFDictionaryRef = null_mut();
    if unsafe {
        SecCodeCopySigningInformation(
            code.cast::<security_framework_sys::code_signing::OpaqueSecCodeRef>(),
            K_SEC_CS_SIGNING_INFORMATION,
            &mut information,
        )
    } != errSecSuccess
        || information.is_null()
    {
        return Err(IdentityError::IdentityMismatch);
    }
    let information = OwnedCf(information.cast());
    let url = unsafe {
        CFDictionaryGetValue(
            information.0.cast(),
            kSecCodeInfoMainExecutable.cast::<c_void>(),
        )
    } as CFURLRef;
    if url.is_null() {
        return Err(IdentityError::IdentityMismatch);
    }
    let mut buffer = [0_u8; PATH_BUFFER_BYTES];
    if unsafe {
        CFURLGetFileSystemRepresentation(
            url,
            1,
            buffer.as_mut_ptr(),
            isize::try_from(buffer.len()).map_err(|_| IdentityError::IdentityMismatch)?,
        )
    } == 0
    {
        return Err(IdentityError::IdentityMismatch);
    }
    let length = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    if length == 0 {
        return Err(IdentityError::IdentityMismatch);
    }
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(
        &buffer[..length],
    )))
}

fn open_executable_vnode(path: &std::path::Path) -> Result<OwnedFd, IdentityError> {
    use std::path::Component;

    if !path.is_absolute()
        || path
            .as_os_str()
            .as_bytes()
            .split(|byte| *byte == b'/')
            .any(|component| matches!(component, b"." | b".."))
    {
        return Err(IdentityError::IdentityMismatch);
    }
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return Err(IdentityError::IdentityMismatch);
    }
    let parts: Vec<_> = components.collect();
    if parts.is_empty()
        || parts
            .iter()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(IdentityError::IdentityMismatch);
    }
    let root = std::ffi::CString::new("/").expect("fixed root");
    let root_fd = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if root_fd < 0 {
        return Err(IdentityError::IdentityMismatch);
    }
    let mut directory = unsafe { OwnedFd::from_raw_fd(root_fd) };
    for component in &parts[..parts.len() - 1] {
        let Component::Normal(name) = component else {
            return Err(IdentityError::IdentityMismatch);
        };
        let name =
            std::ffi::CString::new(name.as_bytes()).map_err(|_| IdentityError::IdentityMismatch)?;
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err(IdentityError::IdentityMismatch);
        }
        directory = unsafe { OwnedFd::from_raw_fd(fd) };
    }
    let Component::Normal(name) = parts[parts.len() - 1] else {
        return Err(IdentityError::IdentityMismatch);
    };
    let name =
        std::ffi::CString::new(name.as_bytes()).map_err(|_| IdentityError::IdentityMismatch)?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        Err(IdentityError::IdentityMismatch)
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

fn hash_retained_file(file: &mut OwnedFd) -> Result<Bytes32, IdentityError> {
    let duplicate = unsafe { libc::dup(file.as_raw_fd()) };
    if duplicate < 0 {
        return Err(IdentityError::IdentityMismatch);
    }
    let mut file = unsafe { File::from_raw_fd(duplicate) };
    file.seek(SeekFrom::Start(0))
        .map_err(|_| IdentityError::IdentityMismatch)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| IdentityError::IdentityMismatch)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(Bytes32::new(digest.finalize().into()))
}

fn certificate_der(certificate: SecCertificateRef) -> Result<Vec<u8>, IdentityError> {
    let data = unsafe { SecCertificateCopyData(certificate) };
    if data.is_null() {
        return Err(IdentityError::InvalidCode);
    }
    let data = OwnedCf(data.cast());
    let length = unsafe { CFDataGetLength(data.0.cast()) };
    if length <= 0 {
        return Err(IdentityError::InvalidCode);
    }
    Ok(unsafe {
        std::slice::from_raw_parts(
            CFDataGetBytePtr(data.0.cast()),
            usize::try_from(length).map_err(|_| IdentityError::InvalidCode)?,
        )
    }
    .to_vec())
}

fn dictionary_string(
    dictionary: CFDictionaryRef,
    key: CFStringRef,
) -> Result<String, IdentityError> {
    let value = unsafe { CFDictionaryGetValue(dictionary, key.cast()) } as CFStringRef;
    if value.is_null() {
        return Err(IdentityError::InvalidCode);
    }
    let mut buffer = [0_i8; 512];
    if unsafe {
        CFStringGetCString(
            value,
            buffer.as_mut_ptr(),
            buffer.len() as isize,
            kCFStringEncodingUTF8,
        )
    } == 0
    {
        return Err(IdentityError::InvalidCode);
    }
    Ok(unsafe { std::ffi::CStr::from_ptr(buffer.as_ptr()) }
        .to_str()
        .map_err(|_| IdentityError::InvalidCode)?
        .to_owned())
}

fn dictionary_i64(dictionary: CFDictionaryRef, key: CFStringRef) -> Result<i64, IdentityError> {
    let value = unsafe { CFDictionaryGetValue(dictionary, key.cast()) } as CFNumberRef;
    let mut result = 0_i64;
    if value.is_null()
        || !unsafe {
            CFNumberGetValue(value, kCFNumberSInt64Type, (&mut result as *mut i64).cast())
        }
    {
        Err(IdentityError::InvalidCode)
    } else {
        Ok(result)
    }
}

fn dictionary_data(
    dictionary: CFDictionaryRef,
    key: CFStringRef,
) -> Result<Vec<u8>, IdentityError> {
    let value = unsafe { CFDictionaryGetValue(dictionary, key.cast()) } as CFTypeRef;
    if value.is_null() || unsafe { CFGetTypeID(value) } != unsafe { CFDataGetTypeID() } {
        return Err(IdentityError::InvalidCode);
    }
    let data = value as CFDataRef;
    let length = unsafe { CFDataGetLength(data) };
    if length <= 0 {
        return Err(IdentityError::InvalidCode);
    }
    Ok(unsafe {
        std::slice::from_raw_parts(
            CFDataGetBytePtr(data),
            usize::try_from(length).map_err(|_| IdentityError::InvalidCode)?,
        )
    }
    .to_vec())
}

fn cf_string(value: &str) -> Result<CFStringRef, IdentityError> {
    let string = unsafe {
        CFStringCreateWithBytes(
            kCFAllocatorDefault,
            value.as_ptr(),
            isize::try_from(value.len()).map_err(|_| IdentityError::InvalidRequirement)?,
            kCFStringEncodingUTF8,
            0,
        )
    };
    if string.is_null() {
        Err(IdentityError::InvalidRequirement)
    } else {
        Ok(string)
    }
}

fn socket_peer_pid(fd: libc::c_int) -> Result<u32, IdentityError> {
    let mut pid: libc::pid_t = 0;
    let mut length = libc::socklen_t::try_from(size_of::<libc::pid_t>())
        .map_err(|_| IdentityError::PeerPidUnavailable)?;
    let status = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut length,
        )
    };
    if status != 0 || length as usize != size_of::<libc::pid_t>() || pid <= 0 {
        return Err(IdentityError::PeerPidUnavailable);
    }
    u32::try_from(pid).map_err(|_| IdentityError::PeerPidUnavailable)
}

fn socket_peer_token(fd: libc::c_int) -> Result<AuditToken, IdentityError> {
    let mut words = [0_u32; 8];
    let mut length = libc::socklen_t::try_from(size_of::<[u32; 8]>())
        .map_err(|_| IdentityError::AuditTokenUnavailable)?;
    let status = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERTOKEN,
            words.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if status != 0 || length as usize != size_of::<[u32; 8]>() {
        Err(IdentityError::AuditTokenUnavailable)
    } else {
        Ok(AuditToken::new(words))
    }
}

fn fstat(fd: libc::c_int) -> Result<libc::stat, IdentityError> {
    let mut stat: libc::stat = unsafe { zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } != 0 {
        Err(IdentityError::IdentityMismatch)
    } else {
        Ok(stat)
    }
}

#[derive(Eq, PartialEq)]
struct ActualSigningIdentity {
    identifier: String,
    code_directory_hash: RequirementHash,
    identity: LocalSigningIdentity,
}

struct BoundStaticCode {
    dynamic: OwnedCode,
    static_code: OwnedStaticCode,
    path: PathBuf,
    file: OwnedFd,
    device: u64,
    inode: u64,
}

struct OwnedCode(SecCodeRef);
struct OwnedStaticCode(SecStaticCodeRef);
struct OwnedCf(CFTypeRef);

impl Drop for OwnedCode {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0.cast()) };
    }
}

impl Drop for OwnedStaticCode {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0.cast()) };
    }
}

impl Drop for OwnedCf {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_owner_validation_is_exact_and_fails_on_hash_mismatch() {
        let release = Bytes32::new([6; 32]);
        let actual = current_process_code_identity(release).expect("current signed test code");
        let requirement = match actual.signing_identity {
            LocalSigningIdentity::AdHoc {
                code_directory_hash,
            } => super::super::ad_hoc_designated_requirement(
                &actual.signing_identifier,
                code_directory_hash,
            ),
            LocalSigningIdentity::LocallyTrustedSelfSigned {
                requirement_certificate_hash,
                ..
            } => super::super::self_signed_designated_requirement(
                &actual.signing_identifier,
                requirement_certificate_hash,
            ),
        };
        let mut expected = super::super::RolePolicy {
            signing_identifier: actual.signing_identifier,
            designated_requirement: requirement,
            canonical_executable_path: actual.canonical_executable_path,
            executable_sha256: actual.executable_sha256,
            code_directory_hash: actual.code_directory_hash,
            signing_identity: actual.signing_identity,
            capture_authorized: true,
        };
        validate_current_process_against(&expected, release).expect("exact owner identity");
        expected.executable_sha256 = Bytes32::new([99; 32]);
        assert!(matches!(
            validate_current_process_against(&expected, release),
            Err(IdentityError::IdentityMismatch)
        ));
    }

    #[test]
    fn self_signed_dynamic_static_equality_includes_code_directory_hash() {
        let identity = LocalSigningIdentity::LocallyTrustedSelfSigned {
            certificate_sha256: Bytes32::new([7; 32]),
            requirement_certificate_hash: RequirementHash::new([8; 20]),
        };
        let dynamic = ActualSigningIdentity {
            identifier: "com.talkingquill.gateway".into(),
            code_directory_hash: RequirementHash::new([9; 20]),
            identity,
        };
        let matching = ActualSigningIdentity {
            identifier: dynamic.identifier.clone(),
            code_directory_hash: dynamic.code_directory_hash,
            identity,
        };
        assert!(dynamic == matching);
        let changed_static_code = ActualSigningIdentity {
            identifier: dynamic.identifier.clone(),
            code_directory_hash: RequirementHash::new([10; 20]),
            identity,
        };
        assert!(dynamic != changed_static_code);
    }
}
