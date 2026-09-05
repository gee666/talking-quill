//! Sample foreground control identity away from the synchronous keyboard hook.

use super::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

static SNAPSHOT: Mutex<Option<(TargetEvidence, Instant)>> = Mutex::new(None);
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const MAX_SNAPSHOT_AGE: Duration = Duration::from_millis(250);

pub(in super::super) struct TargetMonitor(Arc<AtomicBool>);

impl TargetMonitor {
    pub(in super::super) fn start() -> std::io::Result<Self> {
        let stopped = Arc::new(AtomicBool::new(false));
        let signal = Arc::clone(&stopped);
        std::thread::Builder::new()
            .name("tq-focus-monitor".into())
            .spawn(move || {
                while !signal.load(Ordering::Acquire) {
                    let observed = capture_target();
                    if signal.load(Ordering::Acquire) {
                        break;
                    }
                    if let Ok(mut snapshot) = SNAPSHOT.lock() {
                        let previous = snapshot
                            .filter(|(_, captured_at)| captured_at.elapsed() <= MAX_SNAPSHOT_AGE)
                            .map(|(target, _)| target);
                        *snapshot = retain_hidden_caret(observed, previous)
                            .map(|target| (target, Instant::now()));
                    }
                    std::thread::sleep(POLL_INTERVAL);
                }
            })?;
        Ok(Self(stopped))
    }
}

/// A focused editor can stop exposing its caret while idle. Retain its last
/// observed insertion point only across uninterrupted samples of the same
/// foreground window and focused control. A moved caret replaces the saved
/// point, and a failed query or focus change discards it.
fn retain_hidden_caret(
    current: Option<TargetEvidence>,
    previous: Option<TargetEvidence>,
) -> Option<TargetEvidence> {
    current.map(|mut current| {
        if current.caret_window == 0
            && let Some(previous) = previous
            && focus_identity_matches(current.focus, previous.focus)
        {
            current.caret_window = previous.caret_window;
            current.caret_rect = previous.caret_rect;
        }
        current
    })
}

impl Drop for TargetMonitor {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
        // Never join a thread that may be waiting inside a foreign GUI query.
        // It owns no hooks or input authority and exits after that query returns.
        if let Ok(mut snapshot) = SNAPSHOT.lock() {
            *snapshot = None;
        }
    }
}

pub(super) fn cached_paste_evidence(focus: FocusTargetEvidence) -> Option<TargetEvidence> {
    let snapshot = SNAPSHOT.try_lock().ok()?;
    let (target, captured_at) = (*snapshot)?;
    (captured_at.elapsed() <= MAX_SNAPSHOT_AGE
        && target.focus.process_id == focus.process_id
        && target.focus.foreground_window == focus.foreground_window
        && target.focus.foreground_thread == focus.foreground_thread)
        .then_some(target)
}

fn capture_target() -> Option<TargetEvidence> {
    // SAFETY: User32 provides the handles and writes into our initialized buffers.
    // This worker owns no low-level hook, so it cannot block its callback thread.
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.is_null() {
            return None;
        }
        let mut process_id = 0;
        let thread = GetWindowThreadProcessId(foreground, &mut process_id);
        let mut gui = gui_thread_info();
        if thread == 0
            || GetGUIThreadInfo(thread, &mut gui) == 0
            || gui.hwndFocus.is_null()
            || has_transient_input_mode(gui.flags)
        {
            return None;
        }
        let mut focus_process = 0;
        if GetWindowThreadProcessId(gui.hwndFocus, &mut focus_process) == 0
            || focus_process != process_id
            || GetForegroundWindow() != foreground
        {
            return None;
        }
        Some(TargetEvidence {
            focus: FocusTargetEvidence {
                process_id,
                foreground_window: foreground as isize,
                foreground_thread: thread,
                focused_control: gui.hwndFocus as isize,
            },
            caret_window: gui.hwndCaret as isize,
            caret_rect: rect_evidence(gui.rcCaret),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> TargetEvidence {
        TargetEvidence {
            focus: FocusTargetEvidence {
                process_id: 1,
                foreground_window: 2,
                foreground_thread: 3,
                focused_control: 4,
            },
            caret_window: 4,
            caret_rect: [10, 20, 11, 40],
        }
    }

    #[test]
    fn hidden_caret_retains_the_same_focused_editors_last_position() {
        let previous = target();
        let hidden = TargetEvidence {
            caret_window: 0,
            caret_rect: [0; 4],
            ..previous
        };
        assert_eq!(
            retain_hidden_caret(Some(hidden), Some(previous)),
            Some(previous)
        );
        assert_eq!(retain_hidden_caret(Some(hidden), None), Some(hidden));
    }

    #[test]
    fn changed_focus_or_failed_query_cannot_reuse_a_caret() {
        let previous = target();
        for field in 0..4 {
            let mut current = TargetEvidence {
                caret_window: 0,
                ..previous
            };
            match field {
                0 => current.focus.process_id += 1,
                1 => current.focus.foreground_window += 1,
                2 => current.focus.foreground_thread += 1,
                _ => current.focus.focused_control += 1,
            }
            assert_eq!(
                retain_hidden_caret(Some(current), Some(previous)),
                Some(current)
            );
        }
        assert_eq!(retain_hidden_caret(None, Some(previous)), None);
    }

    #[test]
    fn moving_the_caret_replaces_the_retained_insertion_point() {
        let original = target();
        let moved = TargetEvidence {
            caret_rect: [50, 20, 51, 40],
            ..original
        };
        let observed = retain_hidden_caret(Some(moved), Some(original));
        assert_eq!(observed, Some(moved));
        let hidden = TargetEvidence {
            caret_window: 0,
            caret_rect: [0; 4],
            ..moved
        };
        assert_eq!(retain_hidden_caret(Some(hidden), observed), Some(moved));
        assert_ne!(retain_hidden_caret(Some(hidden), observed), Some(original));
    }

    #[test]
    fn menu_and_window_move_modes_cannot_supply_retained_caret_evidence() {
        for flag in [
            GUI_INMENUMODE,
            GUI_POPUPMENUMODE,
            GUI_SYSTEMMENUMODE,
            GUI_INMOVESIZE,
        ] {
            assert!(has_transient_input_mode(flag));
        }
        assert!(!has_transient_input_mode(0));
        assert!(!has_transient_input_mode(
            windows_sys::Win32::UI::WindowsAndMessaging::GUI_CARETBLINKING,
        ));
    }
}
