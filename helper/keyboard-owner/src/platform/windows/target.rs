use std::ptr::null_mut;

use windows_sys::Win32::UI::WindowsAndMessaging::{
    GUITHREADINFO, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, IsWindow,
};
use windows_sys::Win32::{
    Foundation::HWND,
    Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom},
};

use talking_quill_keyboard_core::{ActivationContext, ActivationGeneration, NativeTargetToken};

pub(super) const TARGET_REGISTRY_CAPACITY: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct FocusTargetEvidence {
    process_id: u32,
    foreground_window: isize,
    foreground_thread: u32,
    focused_control: isize,
}

/// Candidate-start evidence. Native focus and caret identities are mandatory
/// until a nonblocking UI Automation cache can distinguish virtual controls
/// that share one renderer HWND.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CandidateTargetEvidence {
    focus: FocusTargetEvidence,
    paste: Option<TargetEvidence>,
}

impl CandidateTargetEvidence {
    pub(super) const fn paste_evidence(self) -> Option<TargetEvidence> {
        self.paste
    }

    #[cfg(test)]
    pub(super) const fn test_only() -> Self {
        Self {
            focus: FocusTargetEvidence {
                process_id: 0,
                foreground_window: 0,
                foreground_thread: 0,
                focused_control: 0,
            },
            paste: None,
        }
    }

    #[cfg(test)]
    pub(super) const fn is_test_only(self) -> bool {
        self.focus.process_id == 0
            && self.focus.foreground_window == 0
            && self.focus.foreground_thread == 0
            && self.focus.focused_control == 0
            && self.paste.is_none()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TargetEvidence {
    focus: FocusTargetEvidence,
    caret_window: isize,
    caret_rect: [i32; 4],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TargetEntry {
    generation: ActivationGeneration,
    token: NativeTargetToken,
    evidence: TargetEvidence,
}

/// Fixed generation-to-target ring. Eviction, a stale generation, or a token
/// mismatch is deliberately indistinguishable from an unavailable target.
#[derive(Clone, Debug)]
pub(super) struct TargetRegistry {
    epoch: Option<u64>,
    entries: [Option<TargetEntry>; TARGET_REGISTRY_CAPACITY],
}

impl TargetRegistry {
    pub(super) fn new() -> Self {
        Self {
            epoch: random_epoch(),
            entries: [None; TARGET_REGISTRY_CAPACITY],
        }
    }

    /// Registers immutable candidate-start paste evidence at the accepted
    /// activation boundary. Missing caret evidence deliberately yields a
    /// targetless activation and therefore clipboard-only insertion.
    pub(super) fn capture_context(
        &mut self,
        generation: ActivationGeneration,
        evidence: Option<TargetEvidence>,
    ) -> ActivationContext {
        evidence.map_or_else(
            || ActivationContext::target_unavailable(generation),
            |evidence| self.record(generation, evidence),
        )
    }

    pub(super) fn take(&mut self, context: ActivationContext) -> Option<TargetEvidence> {
        let token = context.target_token()?;
        let slot = slot(context.activation_generation());
        let entry = self.entries[slot]?;
        if entry.generation != context.activation_generation() || entry.token != token {
            return None;
        }
        self.entries[slot] = None;
        Some(entry.evidence)
    }

    pub(super) fn remove(&mut self, context: ActivationContext) {
        let slot = slot(context.activation_generation());
        if self.entries[slot].is_some_and(|entry| {
            entry.generation == context.activation_generation()
                && Some(entry.token) == context.target_token()
        }) {
            self.entries[slot] = None;
        }
    }

    fn record(
        &mut self,
        generation: ActivationGeneration,
        evidence: TargetEvidence,
    ) -> ActivationContext {
        let Some(epoch) = self.epoch else {
            return ActivationContext::target_unavailable(generation);
        };
        let token = token_for(epoch, generation);
        self.entries[slot(generation)] = Some(TargetEntry {
            generation,
            token,
            evidence,
        });
        ActivationContext::target_unavailable(generation).with_target_token(token)
    }
}

impl Default for TargetRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[must_use]
pub(super) fn revalidate_candidate_target(evidence: CandidateTargetEvidence) -> bool {
    #[cfg(test)]
    if evidence.is_test_only() {
        return true;
    }
    revalidate_focus_target(evidence.focus) && evidence.paste.is_none_or(revalidate_target)
}

#[must_use]
pub(super) fn revalidate_target(evidence: TargetEvidence) -> bool {
    if !revalidate_focus_target(evidence.focus) {
        return false;
    }
    // SAFETY: the retained thread/handle evidence came from User32 and all
    // writable storage belongs to this call.
    unsafe {
        let caret = evidence.caret_window as HWND;
        let mut caret_process_id = 0;
        if caret.is_null()
            || IsWindow(caret) == 0
            || GetWindowThreadProcessId(caret, &raw mut caret_process_id) == 0
            || caret_process_id != evidence.focus.process_id
        {
            return false;
        }
        let mut gui = gui_thread_info();
        GetGUIThreadInfo(evidence.focus.foreground_thread, &raw mut gui) != 0
            && !gui.hwndCaret.is_null()
            && gui.hwndCaret as isize == evidence.caret_window
            && rect_evidence(gui.rcCaret) == evidence.caret_rect
    }
}

fn revalidate_focus_target(evidence: FocusTargetEvidence) -> bool {
    // SAFETY: User32 receives only scalar handles captured from User32 and
    // writable process-ID/GUITHREADINFO storage owned by this call.
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.is_null()
            || foreground as isize != evidence.foreground_window
            || IsWindow(foreground) == 0
        {
            return false;
        }
        let mut process_id = 0;
        let foreground_thread = GetWindowThreadProcessId(foreground, &raw mut process_id);
        if process_id != evidence.process_id || foreground_thread != evidence.foreground_thread {
            return false;
        }
        let focused = evidence.focused_control as HWND;
        if focused.is_null() || IsWindow(focused) == 0 {
            return false;
        }
        let mut focused_process_id = 0;
        if GetWindowThreadProcessId(focused, &raw mut focused_process_id) == 0
            || focused_process_id != evidence.process_id
        {
            return false;
        }
        if evidence.focused_control == evidence.foreground_window {
            return true;
        }
        let mut gui = gui_thread_info();
        if GetGUIThreadInfo(foreground_thread, &raw mut gui) == 0 || gui.hwndFocus.is_null() {
            return false;
        }
        focus_identity_matches(
            evidence,
            FocusTargetEvidence {
                process_id,
                foreground_window: foreground as isize,
                foreground_thread,
                focused_control: gui.hwndFocus as isize,
            },
        )
    }
}

pub(super) fn capture_candidate_target() -> Option<CandidateTargetEvidence> {
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.is_null() || IsWindow(foreground) == 0 {
            return None;
        }
        let mut process_id = 0;
        let foreground_thread = GetWindowThreadProcessId(foreground, &raw mut process_id);
        if foreground_thread == 0 || process_id == 0 {
            return None;
        }
        // Never call GetGUIThreadInfo for another process from the synchronous
        // WH_KEYBOARD_LL callback. The foreground HWND plus WinEvent epoch is
        // bounded activation evidence; paste authority remains unavailable
        // unless a separately captured caret identity exists.
        Some(CandidateTargetEvidence {
            focus: FocusTargetEvidence {
                process_id,
                foreground_window: foreground as isize,
                foreground_thread,
                focused_control: foreground as isize,
            },
            paste: None,
        })
    }
}

const fn focus_identity_matches(
    expected: FocusTargetEvidence,
    current: FocusTargetEvidence,
) -> bool {
    expected.process_id == current.process_id
        && expected.foreground_window == current.foreground_window
        && expected.foreground_thread == current.foreground_thread
        && expected.focused_control == current.focused_control
}

fn gui_thread_info() -> GUITHREADINFO {
    GUITHREADINFO {
        cbSize: u32::try_from(size_of::<GUITHREADINFO>()).expect("GUITHREADINFO size fits u32"),
        ..GUITHREADINFO::default()
    }
}

const fn rect_evidence(rect: windows_sys::Win32::Foundation::RECT) -> [i32; 4] {
    [rect.left, rect.top, rect.right, rect.bottom]
}

fn random_epoch() -> Option<u64> {
    let mut bytes = [0_u8; 8];
    // SAFETY: a null algorithm handle with BCRYPT_USE_SYSTEM_PREFERRED_RNG is
    // the documented system CSPRNG form; the output buffer is fully writable.
    let status = unsafe {
        BCryptGenRandom(
            null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    (status == 0).then(|| u64::from_le_bytes(bytes))
}

const fn slot(generation: ActivationGeneration) -> usize {
    generation.get() as usize % TARGET_REGISTRY_CAPACITY
}

fn token_for(epoch: u64, generation: ActivationGeneration) -> NativeTargetToken {
    const PREFIX: &[u8] = b"win-v8:";
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0_u8; 40];
    let mut index = 0;
    while index < PREFIX.len() {
        bytes[index] = PREFIX[index];
        index += 1;
    }
    let mut nibble = 0;
    while nibble < 16 {
        let shift = (15 - nibble) * 4;
        bytes[PREFIX.len() + nibble] = HEX[((epoch >> shift) & 0xF) as usize];
        nibble += 1;
    }
    bytes[PREFIX.len() + 16] = b':';
    nibble = 0;
    while nibble < 16 {
        let shift = (15 - nibble) * 4;
        bytes[PREFIX.len() + 17 + nibble] = HEX[((generation.get() >> shift) & 0xF) as usize];
        nibble += 1;
    }
    let value = std::str::from_utf8(&bytes).expect("token alphabet is ASCII");
    NativeTargetToken::new(value).expect("fixed Windows target token is bounded")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generation(value: u64) -> ActivationGeneration {
        ActivationGeneration::new(value).unwrap()
    }

    fn evidence(value: isize) -> TargetEvidence {
        TargetEvidence {
            focus: FocusTargetEvidence {
                process_id: value as u32,
                foreground_window: value,
                foreground_thread: value as u32,
                focused_control: value,
            },
            caret_window: value,
            caret_rect: [value as i32; 4],
        }
    }

    #[test]
    fn focus_identity_rejects_same_window_control_window_and_app_changes() {
        let base = evidence(7).focus;
        let mut changed = base;
        changed.focused_control += 1;
        assert!(!focus_identity_matches(base, changed));
        changed = base;
        changed.foreground_window += 1;
        assert!(!focus_identity_matches(base, changed));
        changed = base;
        changed.process_id += 1;
        assert!(!focus_identity_matches(base, changed));
        assert!(focus_identity_matches(base, base));
    }

    #[test]
    fn target_tokens_are_fixed_bounded_ascii_and_generation_distinct() {
        let first = token_for(0xA5, generation(1));
        let last = token_for(0xA5, ActivationGeneration::MAX);
        assert_eq!(first.as_str(), "win-v8:00000000000000a5:0000000000000001");
        assert_ne!(first, last);
        assert!(last.as_str().is_ascii());
        assert!(last.as_str().len() <= NativeTargetToken::MAX_BYTES);
    }

    #[test]
    fn registry_consumes_exact_generation_and_token_once() {
        let mut registry = TargetRegistry::new();
        let context = registry.record(generation(7), evidence(7));
        let wrong = ActivationContext::target_unavailable(generation(8))
            .with_target_token(context.target_token().unwrap());
        assert_eq!(registry.take(wrong), None);
        assert_eq!(registry.take(context), Some(evidence(7)));
        assert_eq!(registry.take(context), None);
    }

    #[test]
    fn bounded_ring_evicts_old_generations_to_safe_failure() {
        let mut registry = TargetRegistry::new();
        let old = registry.record(generation(1), evidence(1));
        for value in 2..=TARGET_REGISTRY_CAPACITY as u64 + 1 {
            registry.record(generation(value), evidence(value as isize));
        }
        assert_eq!(registry.take(old), None);
        let newest_generation = generation(TARGET_REGISTRY_CAPACITY as u64 + 1);
        let newest = ActivationContext::target_unavailable(newest_generation)
            .with_target_token(token_for(registry.epoch.unwrap(), newest_generation));
        assert_eq!(
            registry.take(newest),
            Some(evidence(TARGET_REGISTRY_CAPACITY as isize + 1))
        );
    }

    #[test]
    fn missing_registered_paste_evidence_is_explicitly_clipboard_only() {
        let mut registry = TargetRegistry::new();
        let context = registry.capture_context(generation(1), None);
        assert_eq!(context.target_token(), None);
        assert_eq!(registry.take(context), None);
    }

    #[test]
    fn process_epoch_makes_same_generation_tokens_distinct() {
        assert_ne!(token_for(1, generation(1)), token_for(2, generation(1)));
        assert!(random_epoch().is_some());
    }

    #[test]
    fn failed_delivery_removes_only_the_matching_entry() {
        let mut registry = TargetRegistry::new();
        let first = registry.record(generation(1), evidence(1));
        let second = registry.record(generation(2), evidence(2));
        registry.remove(first);
        assert_eq!(registry.take(first), None);
        assert_eq!(registry.take(second), Some(evidence(2)));
    }
}
