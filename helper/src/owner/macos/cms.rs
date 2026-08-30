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

pub fn verify(policy: &PolicyBlob, signature: &PolicySignature) -> Result<(), CmsError> {
    let expected = compiled_signer()?;
    let content = unsafe {
        CFDataCreate(
            kCFAllocatorDefault,
            policy.as_bytes().as_ptr(),
            policy.as_bytes().len() as isize,
        )
    };
    if content.is_null() {
        return Err(CmsError);
    }
    let mut decoder: CMSDecoderRef = null_mut();
    if unsafe { CMSDecoderCreate(&raw mut decoder) } != errSecSuccess || decoder.is_null() {
        unsafe { CFRelease(content.cast()) };
        return Err(CmsError);
    }
    let statuses = unsafe {
        (
            CMSDecoderSetDetachedContent(decoder, content),
            CMSDecoderUpdateMessage(
                decoder,
                signature.as_der().as_ptr().cast(),
                signature.as_der().len(),
            ),
            CMSDecoderFinalizeMessage(decoder),
        )
    };
    unsafe { CFRelease(content.cast()) };
    if statuses != (errSecSuccess, errSecSuccess, errSecSuccess) {
        unsafe { CFRelease(decoder.cast()) };
        return Err(CmsError);
    }
    let mut count = 0_usize;
    let mut status = CMSSignerStatus::kCMSSignerUnsigned;
    let mut trust = null_mut();
    let mut verify_status = errSecSuccess;
    let checked = unsafe {
        (
            CMSDecoderGetNumSigners(decoder, &raw mut count),
            CMSDecoderCopySignerStatus(
                decoder,
                0,
                null_mut(),
                1,
                &raw mut status,
                &raw mut trust,
                &raw mut verify_status,
            ),
        )
    };
    if !trust.is_null() {
        unsafe { CFRelease(trust.cast()) };
    }
    if checked != (errSecSuccess, errSecSuccess)
        || count != 1
        || status != CMSSignerStatus::kCMSSignerValid
        || verify_status != errSecSuccess
    {
        unsafe { CFRelease(decoder.cast()) };
        return Err(CmsError);
    }
    let mut certificate: SecCertificateRef = null_mut();
    if unsafe { CMSDecoderCopySignerCert(decoder, 0, &raw mut certificate) } != errSecSuccess
        || certificate.is_null()
    {
        unsafe { CFRelease(decoder.cast()) };
        return Err(CmsError);
    }
    let data: CFDataRef = unsafe { SecCertificateCopyData(certificate) };
    unsafe {
        CFRelease(certificate.cast());
        CFRelease(decoder.cast())
    };
    if data.is_null() {
        return Err(CmsError);
    }
    let length = unsafe { CFDataGetLength(data) };
    if length <= 0 {
        unsafe { CFRelease(data.cast()) };
        return Err(CmsError);
    }
    let bytes = unsafe { std::slice::from_raw_parts(CFDataGetBytePtr(data), length as usize) };
    let actual = Bytes32::new(Sha256::digest(bytes).into());
    unsafe { CFRelease(data.cast()) };
    (actual == expected).then_some(()).ok_or(CmsError)
}

fn compiled_signer() -> Result<Bytes32, CmsError> {
    let value = option_env!("TALKING_QUILL_MACOS_POLICY_SIGNER_SHA256").ok_or(CmsError)?;
    if value.len() != 64 {
        return Err(CmsError);
    }
    let mut bytes = [0_u8; 32];
    for (target, pair) in bytes.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        *target = u8::from_str_radix(std::str::from_utf8(pair).map_err(|_| CmsError)?, 16)
            .map_err(|_| CmsError)?;
    }
    Ok(Bytes32::new(bytes))
}

#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("candidate policy CMS verification failed")]
pub struct CmsError;
