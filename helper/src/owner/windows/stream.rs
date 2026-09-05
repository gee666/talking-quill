//! Stream handle registration, cancellation, and I/O ownership.

use std::io::{Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;

use super::super::client::{CaptureRevocation, ConnectError, OwnerShutdownControl};

#[derive(Debug)]
struct ActiveWindowsStream {
    generation: u64,
    reader: usize,
    writer: usize,
    cancelled: Arc<AtomicBool>,
}

#[derive(Debug, Default)]
pub(super) struct WindowsStreamRegistry {
    active: Mutex<Option<ActiveWindowsStream>>,
    issuing_thread: Mutex<Option<(u64, OwnedHandle)>>,
    issuing_thread_stopped: Condvar,
    next_generation: AtomicU64,
    pub(super) shutdown_requested: AtomicBool,
}

struct IssuingIoGuard {
    registry: Arc<WindowsStreamRegistry>,
    generation: u64,
}

impl Drop for IssuingIoGuard {
    fn drop(&mut self) {
        if let Ok(mut issuing) = self.registry.issuing_thread.lock()
            && issuing
                .as_ref()
                .is_some_and(|(generation, _)| *generation == self.generation)
        {
            issuing.take();
            self.registry.issuing_thread_stopped.notify_all();
        }
    }
}

#[derive(Debug)]
pub(super) struct WindowsShutdownControl {
    pub(super) registry: Arc<WindowsStreamRegistry>,
}

impl OwnerShutdownControl for WindowsShutdownControl {
    fn revoke_capture_and_cancel_io(&self) -> CaptureRevocation {
        self.registry
            .shutdown_requested
            .store(true, Ordering::Release);
        let Ok(mut active) = self.registry.active.lock() else {
            return CaptureRevocation::Failed;
        };
        let Some(active_stream) = active.as_ref() else {
            // No lease can be acquired after shutdown_requested becomes true.
            return CaptureRevocation::Confirmed;
        };
        let generation = active_stream.generation;
        active_stream.cancelled.store(true, Ordering::Release);
        unsafe {
            // CancelIoEx handles overlapped work. CancelSynchronousIo below
            // targets the exact thread that issued a blocking cloned-handle write.
            windows_sys::Win32::System::IO::CancelIoEx(
                active_stream.reader as windows_sys::Win32::Foundation::HANDLE,
                std::ptr::null(),
            );
            windows_sys::Win32::System::IO::CancelIoEx(
                active_stream.writer as windows_sys::Win32::Foundation::HANDLE,
                std::ptr::null(),
            );
        }
        let Ok(mut issuing) = self.registry.issuing_thread.lock() else {
            return CaptureRevocation::Failed;
        };
        if let Some((issuing_generation, thread)) = issuing.as_ref()
            && *issuing_generation == generation
        {
            unsafe {
                windows_sys::Win32::System::IO::CancelSynchronousIo(thread.as_raw_handle());
            }
            let waited = self.registry.issuing_thread_stopped.wait_timeout_while(
                issuing,
                Duration::from_millis(500),
                |value| {
                    value
                        .as_ref()
                        .is_some_and(|(issuing_generation, _)| *issuing_generation == generation)
                },
            );
            let Ok((updated, timeout)) = waited else {
                return CaptureRevocation::Failed;
            };
            issuing = updated;
            if timeout.timed_out()
                && issuing
                    .as_ref()
                    .is_some_and(|(issuing_generation, _)| *issuing_generation == generation)
            {
                return CaptureRevocation::Failed;
            }
        }
        drop(issuing);
        active.take();
        CaptureRevocation::Confirmed
    }
}

pub(crate) struct LocalChildStream {
    reader: std::fs::File,
    writer: std::fs::File,
    registry: Arc<WindowsStreamRegistry>,
    generation: u64,
    cancelled: Arc<AtomicBool>,
    pub(super) verified_peers: Option<(
        talking_quill_windows_owner_ipc::peer::VerifiedPeer,
        talking_quill_windows_owner_ipc::peer::VerifiedPeer,
    )>,
}

impl LocalChildStream {
    pub(super) fn new(
        reader: std::fs::File,
        writer: std::fs::File,
        registry: Arc<WindowsStreamRegistry>,
    ) -> Result<Self, ConnectError> {
        if registry.shutdown_requested.load(Ordering::Acquire) {
            return Err(ConnectError::Unavailable);
        }
        let generation = registry.next_generation.fetch_add(1, Ordering::Relaxed);
        let cancelled = Arc::new(AtomicBool::new(false));
        let active = ActiveWindowsStream {
            generation,
            reader: reader.as_raw_handle() as usize,
            writer: writer.as_raw_handle() as usize,
            cancelled: Arc::clone(&cancelled),
        };
        let mut slot = registry
            .active
            .lock()
            .map_err(|_| ConnectError::Unavailable)?;
        if registry.shutdown_requested.load(Ordering::Acquire) {
            return Err(ConnectError::Unavailable);
        }
        if slot.is_some() {
            return Err(ConnectError::Busy);
        }
        *slot = Some(active);
        drop(slot);
        Ok(Self {
            reader,
            writer,
            registry,
            generation,
            cancelled,
            verified_peers: None,
        })
    }

    fn check_cancelled(&self) -> std::io::Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(std::io::ErrorKind::BrokenPipe.into())
        } else {
            Ok(())
        }
    }

    fn begin_issuing_io(&self) -> std::io::Result<IssuingIoGuard> {
        self.check_cancelled()?;
        let process = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcess() };
        let thread = unsafe { windows_sys::Win32::System::Threading::GetCurrentThread() };
        let mut duplicate = std::ptr::null_mut();
        let duplicated = unsafe {
            windows_sys::Win32::Foundation::DuplicateHandle(
                process,
                thread,
                process,
                &mut duplicate,
                0,
                0,
                windows_sys::Win32::Foundation::DUPLICATE_SAME_ACCESS,
            )
        };
        if duplicated == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(duplicate) };
        let mut issuing = self
            .registry
            .issuing_thread
            .lock()
            .map_err(|_| std::io::Error::other("private pipe I/O registry poisoned"))?;
        self.check_cancelled()?;
        if issuing.is_some() {
            return Err(std::io::Error::other("concurrent private pipe I/O"));
        }
        *issuing = Some((self.generation, handle));
        Ok(IssuingIoGuard {
            registry: Arc::clone(&self.registry),
            generation: self.generation,
        })
    }
}

impl Drop for LocalChildStream {
    fn drop(&mut self) {
        if let Ok(mut active) = self.registry.active.lock()
            && active
                .as_ref()
                .is_some_and(|value| value.generation == self.generation)
        {
            active.take();
        }
    }
}

impl Read for LocalChildStream {
    fn read(&mut self, value: &mut [u8]) -> std::io::Result<usize> {
        self.check_cancelled()?;
        let mut available = 0_u32;
        if unsafe {
            windows_sys::Win32::System::Pipes::PeekNamedPipe(
                self.reader.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        if available == 0 {
            return Err(std::io::ErrorKind::WouldBlock.into());
        }
        let length = value.len().min(available as usize);
        self.reader.read(&mut value[..length])
    }
}
impl Write for LocalChildStream {
    fn write(&mut self, value: &[u8]) -> std::io::Result<usize> {
        let guard = self.begin_issuing_io()?;
        let result = self.writer.write(value);
        drop(guard);
        result
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let guard = self.begin_issuing_io()?;
        let result = self.writer.flush();
        drop(guard);
        result
    }
}
