//! Dedicated message-pump startup and bounded shutdown coordination.
use super::{
    monitor::CoreAudioMonitor,
    state::{AudioWorkerState, WM_AUDIO_INPUT_DEVICES_CHANGED, WM_AUDIO_PROTOCOL_READY},
};
use crate::platform::{CallbackGate, NativeEvent, PlatformError, TerminalReason, TerminalSignal};
use crossbeam_channel::{Receiver, Sender, bounded};
use std::{
    ptr::null_mut,
    sync::{Arc, atomic::Ordering},
    thread::{self, JoinHandle},
    time::Duration,
};
use windows_sys::Win32::{
    System::Threading::GetCurrentThreadId,
    UI::WindowsAndMessaging::{GetMessageW, MSG, PM_NOREMOVE, PeekMessageW},
};
const AUDIO_STARTUP_TIMEOUT: Duration = Duration::from_secs(2);
const AUDIO_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(500);

struct WorkerCompletion {
    sender: Sender<Result<(), PlatformError>>,
    result: Option<Result<(), PlatformError>>,
}

impl WorkerCompletion {
    const fn new(sender: Sender<Result<(), PlatformError>>) -> Self {
        Self {
            sender,
            result: None,
        }
    }

    fn finish(&mut self, result: Result<(), PlatformError>) {
        self.result = Some(result);
    }
}

impl Drop for WorkerCompletion {
    fn drop(&mut self) {
        let result = self
            .result
            .take()
            .unwrap_or(Err(PlatformError::ThreadStopped));
        let _ = self.sender.try_send(result);
    }
}

pub(in super::super) struct AudioDeviceMonitor {
    state: Arc<AudioWorkerState>,
    completion: Receiver<Result<(), PlatformError>>,
    thread: Option<JoinHandle<()>>,
}

impl AudioDeviceMonitor {
    pub(in super::super) fn start(
        outbound: Sender<NativeEvent>,
        gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
    ) -> Result<Self, PlatformError> {
        let state = Arc::new(AudioWorkerState::new(terminal));
        let (ready_tx, ready_rx) = bounded(1);
        let (completion_tx, completion) = bounded(1);
        let worker_state = Arc::clone(&state);
        let thread = thread::Builder::new()
            .name("talking-quill-helper-win-audio".into())
            .spawn(move || {
                audio_worker(outbound, gate, worker_state, ready_tx, completion_tx);
            })
            .map_err(|_| PlatformError::ThreadStopped)?;

        match ready_rx.recv_timeout(AUDIO_STARTUP_TIMEOUT) {
            Ok(Ok(())) => Ok(Self {
                state,
                completion,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                state.begin_shutdown();
                let _ = finish_worker(thread, &completion, AUDIO_SHUTDOWN_TIMEOUT);
                Err(error)
            }
            Err(_) => {
                state.begin_shutdown();
                let _ = finish_worker(thread, &completion, AUDIO_SHUTDOWN_TIMEOUT);
                Err(PlatformError::ThreadStopped)
            }
        }
    }

    pub(in super::super) fn protocol_initialized(&self) {
        self.state.protocol_initialized();
    }

    pub(in super::super) fn begin_shutdown(&self) {
        self.state.begin_shutdown();
    }

    pub(in super::super) fn shutdown_with_timeout(
        &mut self,
        timeout: Duration,
    ) -> Result<(), PlatformError> {
        self.begin_shutdown();
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        finish_worker(thread, &self.completion, timeout)
    }

    pub(in super::super) fn shutdown(&mut self) -> Result<(), PlatformError> {
        self.shutdown_with_timeout(AUDIO_SHUTDOWN_TIMEOUT)
    }
}

impl Drop for AudioDeviceMonitor {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn audio_worker(
    outbound: Sender<NativeEvent>,
    gate: Arc<CallbackGate>,
    state: Arc<AudioWorkerState>,
    ready: Sender<Result<(), PlatformError>>,
    completion: Sender<Result<(), PlatformError>>,
) {
    // Declared first so completion is reported only after all COM resources
    // have been released, or unwinding has abandoned this worker.
    let mut completion = WorkerCompletion::new(completion);
    let mut message = MSG::default();
    // SAFETY: this no-remove peek creates the dedicated worker queue before its
    // thread ID is published to callbacks or the coordinator.
    unsafe { PeekMessageW(&raw mut message, null_mut(), 0, 0, PM_NOREMOVE) };
    // SAFETY: reads this dedicated worker's native thread identifier.
    state
        .thread_id
        .store(unsafe { GetCurrentThreadId() }, Ordering::Release);
    if !state.active.load(Ordering::Acquire) {
        let _ = ready.try_send(Err(PlatformError::ThreadStopped));
        completion.finish(Ok(()));
        return;
    }

    let mut monitor = match CoreAudioMonitor::start(
        outbound,
        gate,
        Arc::clone(&state.terminal),
        Arc::clone(&state),
    ) {
        Ok(monitor) => monitor,
        Err(error) => {
            let _ = ready.try_send(Err(error));
            completion.finish(Ok(()));
            return;
        }
    };
    if !state.active.load(Ordering::Acquire) {
        let _ = ready.try_send(Err(PlatformError::ThreadStopped));
        completion.finish(monitor.shutdown());
        return;
    }
    if ready.try_send(Ok(())).is_err() {
        state.begin_shutdown();
        completion.finish(monitor.shutdown());
        return;
    }

    loop {
        // SAFETY: `message` is writable storage and this worker owns the queue.
        let result = unsafe { GetMessageW(&raw mut message, null_mut(), 0, 0) };
        if result <= 0 {
            if result < 0 || !state.stopping.load(Ordering::Acquire) {
                state
                    .terminal
                    .trigger(TerminalReason::AudioDeviceMonitorUnavailable);
            }
            break;
        }
        if message.message == WM_AUDIO_INPUT_DEVICES_CHANGED
            || message.message == WM_AUDIO_PROTOCOL_READY
        {
            monitor.drain_pending();
        }
    }

    state.active.store(false, Ordering::Release);
    completion.finish(monitor.shutdown());
}

pub(super) fn finish_worker(
    thread: JoinHandle<()>,
    completion: &Receiver<Result<(), PlatformError>>,
    timeout: Duration,
) -> Result<(), PlatformError> {
    let deadline = std::time::Instant::now() + timeout;
    let result = match completion
        .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
    {
        Ok(result) => result,
        Err(_) => {
            // Never join a worker which may be blocked in an audio driver or
            // COM call. Process exit owns its remaining native resources.
            drop(thread);
            return Err(PlatformError::ThreadStopped);
        }
    };
    while !thread.is_finished() && std::time::Instant::now() < deadline {
        thread::yield_now();
    }
    if !thread.is_finished() {
        drop(thread);
        return Err(PlatformError::ThreadStopped);
    }
    if thread.join().is_err() {
        Err(PlatformError::ThreadStopped)
    } else {
        result
    }
}
