#![cfg(target_os = "macos")]

use std::ptr::null_mut;

use core_foundation_sys::base::{CFRelease, kCFAllocatorDefault};
use core_foundation_sys::data::{CFDataCreate, CFDataGetBytePtr, CFDataGetLength, CFDataRef};
use security_framework_sys::base::{SecCertificateRef, errSecSuccess};
use security_framework_sys::certificate::SecCertificateCopyData;
use security_framework_sys::cms::{
    CMSDecoderCopySignerCert, CMSDecoderCopySignerStatus, CMSDecoderCreate,
    CMSDecoderFinalizeMessage, CMSDecoderGetNumSigners, CMSDecoderRef,
    CMSDecoderSetDetachedContent, CMSDecoderUpdateMessage, CMSSignerStatus,
};
use sha2::{Digest, Sha256};
use talking_quill_owner_protocol::Bytes32;
use talking_quill_owner_protocol::release_policy::{PolicyBlob, PolicySignature};

use super::MacosEndpointError;

/// Verifies one detached release-policy CMS against a locally trusted signer
/// and an installation-pinned exact signer certificate hash.
pub fn verify_release_policy_cms(
    policy: &PolicyBlob,
    signature: &PolicySignature,
    expected_signer_sha256: Bytes32,
) -> Result<(), MacosEndpointError> {
    // SAFETY: CoreFoundation copies the fixed policy bytes.
    let content = unsafe {
        CFDataCreate(
            kCFAllocatorDefault,
            policy.as_bytes().as_ptr(),
            isize::try_from(policy.as_bytes().len())
                .map_err(|_| MacosEndpointError::Configuration)?,
        )
    };
    if content.is_null() {
        return Err(MacosEndpointError::Configuration);
    }
    let mut decoder: CMSDecoderRef = null_mut();
    // SAFETY: output points to writable null storage.
    let created = unsafe { CMSDecoderCreate(&mut decoder) };
    if created != errSecSuccess || decoder.is_null() {
        unsafe { CFRelease(content.cast()) };
        return Err(MacosEndpointError::Configuration);
    }
    // SAFETY: decoder/content/signature buffers remain valid for all calls.
    let status = unsafe {
        let detached = CMSDecoderSetDetachedContent(decoder, content);
        let updated = CMSDecoderUpdateMessage(
            decoder,
            signature.as_der().as_ptr().cast(),
            signature.as_der().len(),
        );
        let finalized = CMSDecoderFinalizeMessage(decoder);
        (detached, updated, finalized)
    };
    unsafe { CFRelease(content.cast()) };
    if status != (errSecSuccess, errSecSuccess, errSecSuccess) {
        unsafe { CFRelease(decoder.cast()) };
        return Err(MacosEndpointError::Configuration);
    }
    let mut signers = 0_usize;
    let mut signer_status = CMSSignerStatus::kCMSSignerUnsigned;
    let mut trust = null_mut();
    let mut verify_status = errSecSuccess;
    // SAFETY: all outputs point to writable initialized storage. A null policy
    // requests the CMS signer's ordinary system trust evaluation.
    let signer_result = unsafe {
        let count = CMSDecoderGetNumSigners(decoder, &mut signers);
        let result = CMSDecoderCopySignerStatus(
            decoder,
            0,
            null_mut(),
            1,
            &mut signer_status,
            &mut trust,
            &mut verify_status,
        );
        (count, result)
    };
    if !trust.is_null() {
        unsafe { CFRelease(trust.cast()) };
    }
    if signer_result != (errSecSuccess, errSecSuccess)
        || signers != 1
        || signer_status != CMSSignerStatus::kCMSSignerValid
        || verify_status != errSecSuccess
    {
        unsafe { CFRelease(decoder.cast()) };
        return Err(MacosEndpointError::Configuration);
    }
    let mut certificate: SecCertificateRef = null_mut();
    // SAFETY: decoder is finalized and certificate output is writable.
    if unsafe { CMSDecoderCopySignerCert(decoder, 0, &mut certificate) } != errSecSuccess
        || certificate.is_null()
    {
        unsafe { CFRelease(decoder.cast()) };
        return Err(MacosEndpointError::Configuration);
    }
    // SAFETY: certificate is retained; returned data follows the copy rule.
    let certificate_data: CFDataRef = unsafe { SecCertificateCopyData(certificate) };
    unsafe {
        CFRelease(certificate.cast());
        CFRelease(decoder.cast());
    }
    if certificate_data.is_null() {
        return Err(MacosEndpointError::Configuration);
    }
    let length = unsafe { CFDataGetLength(certificate_data) };
    if length <= 0 {
        unsafe { CFRelease(certificate_data.cast()) };
        return Err(MacosEndpointError::Configuration);
    }
    let bytes = unsafe {
        std::slice::from_raw_parts(
            CFDataGetBytePtr(certificate_data),
            usize::try_from(length).map_err(|_| MacosEndpointError::Configuration)?,
        )
    };
    let actual = Bytes32::new(Sha256::digest(bytes).into());
    unsafe { CFRelease(certificate_data.cast()) };
    if actual == expected_signer_sha256 {
        Ok(())
    } else {
        Err(MacosEndpointError::Configuration)
    }
}
