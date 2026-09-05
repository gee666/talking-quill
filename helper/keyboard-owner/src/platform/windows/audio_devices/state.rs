//! Shared callback admission, change coalescing, and worker wakeups.
use crate::platform::{TerminalReason, TerminalSignal};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};
use windows_sys::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_APP, WM_QUIT};

pub(super) const WM_AUDIO_INPUT_DEVICES_CHANGED: u32 = WM_APP + 0x46;
pub(super) const WM_AUDIO_PROTOCOL_READY: u32 = WM_APP + 0x47;
pub(super) const CHANGE_DEFAULT: u32 = 1 << 0;
pub(super) const CHANGE_TOPOLOGY: u32 = 1 << 1;
pub(super) const CHANGE_FORCE_SYNC: u32 = 1 << 2;
pub(super) const INITIAL_CHANGES: u32 = CHANGE_TOPOLOGY | CHANGE_FORCE_SYNC;
pub(super) struct AudioWorkerState {
    pub(super) active: AtomicBool,
    pub(super) stopping: AtomicBool,
    pub(super) protocol_ready: AtomicBool,
    pub(super) pending: AtomicU32,
    pub(super) thread_id: AtomicU32,
    pub(super) terminal: Arc<TerminalSignal>,
}

impl AudioWorkerState {
    pub(super) fn new(terminal: Arc<TerminalSignal>) -> Self {
        Self {
            active: AtomicBool::new(true),
            stopping: AtomicBool::new(false),
            protocol_ready: AtomicBool::new(false),
            // Every fresh helper performs one post-initialize resnapshot and
            // invalidation. This covers changes while the previous helper was down.
            pending: AtomicU32::new(INITIAL_CHANGES),
            thread_id: AtomicU32::new(0),
            terminal,
        }
    }

    pub(super) fn queue_change(&self, change: u32) {
        if !self.active.load(Ordering::Acquire) {
            return;
        }
        let queued = coalesce_change(&self.pending, change, || {
            post_audio_message(
                self.thread_id.load(Ordering::Acquire),
                WM_AUDIO_INPUT_DEVICES_CHANGED,
            )
        });
        if !queued && !self.stopping.load(Ordering::Acquire) {
            self.terminal
                .trigger(TerminalReason::AudioDeviceMonitorUnavailable);
        }
    }

    pub(super) fn protocol_initialized(&self) {
        if !self.active.load(Ordering::Acquire) {
            return;
        }
        let signaled = mark_protocol_ready(&self.protocol_ready, || {
            post_audio_message(
                self.thread_id.load(Ordering::Acquire),
                WM_AUDIO_PROTOCOL_READY,
            )
        });
        if !signaled {
            self.terminal
                .trigger(TerminalReason::AudioDeviceMonitorUnavailable);
        }
    }

    pub(super) fn begin_shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
        self.active.store(false, Ordering::Release);
        let _ = post_audio_message(self.thread_id.load(Ordering::Acquire), WM_QUIT);
    }
}

pub(super) fn coalesce_change(
    pending: &AtomicU32,
    change: u32,
    wake_worker: impl FnOnce() -> bool,
) -> bool {
    if pending.fetch_or(change, Ordering::AcqRel) != 0 {
        return true;
    }
    if wake_worker() {
        true
    } else {
        pending.store(0, Ordering::Release);
        false
    }
}

pub(super) fn mark_protocol_ready(
    protocol_ready: &AtomicBool,
    wake_worker: impl FnOnce() -> bool,
) -> bool {
    protocol_ready.store(true, Ordering::Release);
    wake_worker()
}

pub(super) fn take_ready_changes(pending: &AtomicU32, protocol_ready: &AtomicBool) -> u32 {
    if protocol_ready.load(Ordering::Acquire) {
        pending.swap(0, Ordering::AcqRel)
    } else {
        0
    }
}

fn post_audio_message(thread_id: u32, message: u32) -> bool {
    if thread_id == 0 {
        return false;
    }
    // SAFETY: all audio-worker messages are pointer-free and its queue is
    // created before the thread ID is published.
    unsafe { PostThreadMessageW(thread_id, message, 0, 0) != 0 }
}
