#![cfg(target_os = "macos")]

use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::null_mut;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use core_foundation_sys::base::{CFRelease, CFTypeRef};
use core_foundation_sys::dictionary::{CFDictionaryGetValue, CFDictionaryRef};
use core_foundation_sys::number::{CFNumberGetValue, CFNumberRef, kCFNumberSInt64Type};

use crate::runtime::{OsRuntimeSignals, RuntimeError, RuntimeSignal, RuntimeSignalSource};

use super::native_identity::current_audit_token;

const SESSION_POLL_INTERVAL: Duration = Duration::from_millis(250);

unsafe extern "C" {
    fn SCDynamicStoreCopyConsoleUser(
        store: *mut c_void,
        uid: *mut libc::uid_t,
        gid: *mut libc::gid_t,
    ) -> CFTypeRef;
    fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
    static kCGSessionAuditIDKey: CFTypeRef;
    static kCGSessionOnConsoleKey: CFTypeRef;
}

/// Process signals plus authoritative polling of the current console user and
/// this process's audit session. SIGHUP alone is never classified as session
/// end; it remains only a shutdown hint in `OsRuntimeSignals`.
pub struct MacosRuntimeSignals {
    process: OsRuntimeSignals,
    expected_audit_session: u32,
    expected_console_uid: u32,
    next_session_poll: Instant,
    outer_bundle_path: PathBuf,
    removal_runtime_path: PathBuf,
    outer_bundle_missing: Arc<AtomicBool>,
}

#[derive(Clone, Debug)]
pub struct MacosLifecycleHandle {
    outer_bundle_missing: Arc<AtomicBool>,
}

impl MacosLifecycleHandle {
    #[must_use]
    pub fn outer_bundle_missing(&self) -> bool {
        self.outer_bundle_missing.load(Ordering::Acquire)
    }
}

impl std::fmt::Debug for MacosRuntimeSignals {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MacosRuntimeSignals(<redacted>)")
    }
}

impl MacosRuntimeSignals {
    pub fn install(
        expected_audit_session: u32,
        expected_console_uid: u32,
        outer_bundle_path: PathBuf,
        removal_runtime_path: PathBuf,
    ) -> Result<(Self, MacosLifecycleHandle), RuntimeError> {
        if expected_audit_session == 0
            || current_audit_token().map(|token| token.audit_session_id())
                != Ok(expected_audit_session)
            || current_console_uid() != Some(expected_console_uid)
            || current_gui_session() != Some((expected_audit_session, true))
        {
            return Err(RuntimeError::SignalInstall);
        }
        if !outer_bundle_path.is_absolute()
            || !removal_runtime_path.is_absolute()
            || outer_bundle_path
                .extension()
                .and_then(|value| value.to_str())
                != Some("app")
        {
            return Err(RuntimeError::SignalInstall);
        }
        let outer_bundle_missing = Arc::new(AtomicBool::new(false));
        Ok((
            Self {
                process: OsRuntimeSignals::install()?,
                expected_audit_session,
                expected_console_uid,
                next_session_poll: Instant::now(),
                outer_bundle_path,
                removal_runtime_path,
                outer_bundle_missing: Arc::clone(&outer_bundle_missing),
            },
            MacosLifecycleHandle {
                outer_bundle_missing,
            },
        ))
    }

    fn session_signal(&mut self) -> Option<RuntimeSignal> {
        let now = Instant::now();
        if now < self.next_session_poll {
            return None;
        }
        self.next_session_poll = now + SESSION_POLL_INTERVAL;
        if !self.outer_bundle_path.is_dir() {
            if super::prepare_removed_install(&self.removal_runtime_path).is_err() {
                return Some(RuntimeSignal::RemovalPoisonFailed);
            }
            self.outer_bundle_missing.store(true, Ordering::Release);
            return Some(RuntimeSignal::SessionEnded);
        }
        (current_audit_token().map(|token| token.audit_session_id())
            != Ok(self.expected_audit_session)
            || current_console_uid() != Some(self.expected_console_uid)
            || current_gui_session() != Some((self.expected_audit_session, true)))
        .then_some(RuntimeSignal::SessionEnded)
    }
}

impl RuntimeSignalSource for MacosRuntimeSignals {
    fn poll_signal(&mut self) -> Option<RuntimeSignal> {
        if let Some(signal) = self.process.poll_signal() {
            return Some(signal);
        }
        self.session_signal()
    }
}

fn current_gui_session() -> Option<(u32, bool)> {
    let dictionary = unsafe { CGSessionCopyCurrentDictionary() };
    if dictionary.is_null() {
        return None;
    }
    let audit_value =
        unsafe { CFDictionaryGetValue(dictionary, kCGSessionAuditIDKey.cast()) } as CFNumberRef;
    let console_value =
        unsafe { CFDictionaryGetValue(dictionary, kCGSessionOnConsoleKey.cast()) } as CFTypeRef;
    let mut audit_id = 0_i64;
    let valid = !audit_value.is_null()
        && unsafe {
            CFNumberGetValue(audit_value, kCFNumberSInt64Type, (&raw mut audit_id).cast())
        }
        && (0..=u32::MAX as i64).contains(&audit_id)
        && !console_value.is_null();
    let on_console = console_value == unsafe { core_foundation_sys::number::kCFBooleanTrue }.cast();
    unsafe { CFRelease(dictionary.cast()) };
    valid.then_some((audit_id as u32, on_console))
}

fn current_console_uid() -> Option<u32> {
    let mut uid = 0;
    let mut gid = 0;
    // SAFETY: null store selects the default dynamic store; uid/gid are live.
    let user = unsafe { SCDynamicStoreCopyConsoleUser(null_mut(), &mut uid, &mut gid) };
    if user.is_null() {
        return None;
    }
    unsafe { CFRelease(user) };
    Some(uid)
}
