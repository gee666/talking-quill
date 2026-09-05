//! Owner wake retries, startup arbitration, and native hook resource lifetimes.
use super::*;

pub(super) fn retry_owner_wake(
    mut post: impl FnMut() -> bool,
    mut backoff: impl FnMut(Duration),
) -> bool {
    if post() {
        return true;
    }
    for delay in OWNER_WAKE_RETRY_DELAYS {
        backoff(delay);
        if post() {
            return true;
        }
    }
    false
}

pub(super) fn post_owner_message_once(thread_id: u32, message: u32) -> bool {
    // SAFETY: owner messages are pointer-free and target the thread whose queue
    // is created before startup readiness is reported.
    unsafe { PostThreadMessageW(thread_id, message, 0, 0) != 0 }
}

pub(super) fn post_owner_message(thread_id: u32, message: u32) -> bool {
    retry_owner_wake(
        || post_owner_message_once(thread_id, message),
        thread::sleep,
    )
}

pub(super) fn owner_completed(receiver: &Receiver<()>, timeout: Duration) -> bool {
    receiver.recv_timeout(timeout).is_ok()
}

pub(super) fn join_completed_owner(
    thread: JoinHandle<()>,
    completion: &Receiver<()>,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    if !owner_completed(completion, timeout) {
        return false;
    }
    while !thread.is_finished() && Instant::now() < deadline {
        thread::yield_now();
    }
    thread.is_finished() && thread.join().is_ok()
}

pub(super) struct OwnerHookInstallation {
    pub(super) hook: windows_sys::Win32::UI::WindowsAndMessaging::HHOOK,
    pub(super) win_events: [HWINEVENTHOOK; 3],
    pub(super) timer: usize,
}

pub(super) struct HookThreadDesktop {
    pub(super) input: windows_sys::Win32::System::StationsAndDesktops::HDESK,
    pub(super) previous: windows_sys::Win32::System::StationsAndDesktops::HDESK,
}

impl Drop for HookThreadDesktop {
    fn drop(&mut self) {
        // SAFETY: both handles were acquired on this owner thread. The input
        // handle is owned; the previous assignment is borrowed and never closed.
        // OwnerHookInstallation drops before this guard restores the assignment.
        unsafe {
            let _ = SetThreadDesktop(self.previous);
            let _ = CloseDesktop(self.input);
        }
    }
}

impl Drop for OwnerHookInstallation {
    fn drop(&mut self) {
        if self.timer != 0 {
            // SAFETY: timer belongs to the current owner thread.
            unsafe { KillTimer(null_mut(), self.timer) };
        }
        for hook in self.win_events {
            if !hook.is_null() {
                // SAFETY: every handle was installed by this owner thread.
                unsafe { UnhookWinEvent(hook) };
            }
        }
        // SAFETY: this hook was installed on the current owner thread. The boxed
        // callback context outlives this guard, including same-thread unhooking.
        unsafe { UnhookWindowsHookEx(self.hook) };
        CALLBACK_CONTEXT.store(null_mut(), Ordering::Release);
    }
}

impl OwnerHookInstallation {
    pub(super) fn refresh_keyboard_hook(&mut self) -> bool {
        // Windows silently removes timed-out low-level hooks. Reinstall on the
        // owning message thread, preserving the reducer and its key state.
        // Install first so a failed refresh leaves the previous hook intact.
        // SAFETY: the callback uses the system ABI and its context stays live
        // through installation and removal on this same message-loop thread.
        let replacement = unsafe {
            SetWindowsHookExW(
                WH_KEYBOARD_LL,
                Some(keyboard_hook),
                low_level_hook_module(),
                0,
            )
        };
        if replacement.is_null() {
            return false;
        }
        let previous = std::mem::replace(&mut self.hook, replacement);
        // SAFETY: no message pump runs between installation and same-thread
        // removal. A stale handle from Windows' automatic removal is harmless.
        unsafe { UnhookWindowsHookEx(previous) };
        true
    }
}

pub(super) unsafe extern "system" fn target_change_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    _window: windows_sys::Win32::Foundation::HWND,
    object_id: i32,
    _child_id: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    // Candidate activation evidence intentionally models the foreground HWND,
    // not a virtual control inside it. Redundant focus/location notifications
    // from that same foreground must not cancel Alt+X between down and up.
    // Foreground transitions still provide the A->B->A epoch fence; concrete
    // HWND/process/thread identity is revalidated at every effect boundary.
    if event == EVENT_SYSTEM_FOREGROUND {
        let _ = TARGET_CHANGE_EPOCH.fetch_update(Ordering::AcqRel, Ordering::Acquire, |epoch| {
            Some(epoch.saturating_add(1))
        });
    }
    let _ = object_id;
}

pub(super) struct OwnerCompletion(pub(super) Sender<()>);

impl Drop for OwnerCompletion {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(super) enum StartupState {
    Pending,
    Running,
    Cancelled,
}

pub(super) fn claim_startup(state: &AtomicU8) -> bool {
    state
        .compare_exchange(
            StartupState::Pending as u8,
            StartupState::Running as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
}

pub(super) fn cancel_startup(state: &AtomicU8) -> StartupState {
    match state.compare_exchange(
        StartupState::Pending as u8,
        StartupState::Cancelled as u8,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => StartupState::Cancelled,
        Err(value) if value == StartupState::Running as u8 => StartupState::Running,
        Err(_) => StartupState::Cancelled,
    }
}
