//! Scan codes, keyboard layouts, AltGr, and physical key queries.
use super::*;

pub(super) fn map_scan_code(scan_code: u32, extended: bool) -> PhysicalKey {
    if !extended
        && let Some(index) = LETTER_SCAN_CODES
            .iter()
            .position(|candidate| *candidate == scan_code)
    {
        return PhysicalKey::Letter(
            ActivationKey::from_index(index as u8).expect("scan table has exactly A-Z entries"),
        );
    }
    match scan_code {
        0x01 => PhysicalKey::Escape,
        // Preserve existing session behavior for both main and numpad Enter.
        0x1C => PhysicalKey::Enter,
        _ => PhysicalKey::Other,
    }
}

pub(super) fn physical_tracker_from_state(
    mut is_down: impl FnMut(PhysicalKey) -> bool,
) -> WindowsPhysicalTracker {
    let mut tracker = WindowsPhysicalTracker::default();
    for index in 0_u8..26 {
        let key = PhysicalKey::Letter(ActivationKey::from_index(index).expect("A-Z index"));
        if is_down(key) {
            tracker.observe(key, None, KeyPhase::Down);
        }
    }
    if is_down(PhysicalKey::Escape) {
        tracker.observe(PhysicalKey::Escape, None, KeyPhase::Down);
    }
    if is_down(PhysicalKey::Enter) {
        tracker.seed_enter_preheld();
    }
    tracker
}

pub(super) fn foreground_keyboard_layout() -> windows_sys::Win32::UI::Input::KeyboardAndMouse::HKL {
    // SAFETY: all calls take scalar values or a null optional process pointer.
    let foreground_thread = unsafe {
        let window = GetForegroundWindow();
        if window.is_null() {
            0
        } else {
            GetWindowThreadProcessId(window, null_mut())
        }
    };
    // SAFETY: zero requests the current thread layout.
    unsafe { GetKeyboardLayout(foreground_thread) }
}

pub(super) fn conservative_altgr(modifiers: &ModifierTracker, synthetic_ctrl: bool) -> bool {
    conservative_altgr_for_layout(modifiers, synthetic_ctrl, foreground_layout_uses_altgr())
}

pub(super) const fn conservative_altgr_for_layout(
    modifiers: &ModifierTracker,
    synthetic_ctrl: bool,
    layout_uses_altgr: bool,
) -> bool {
    modifiers.alt.right && (synthetic_ctrl || layout_uses_altgr)
}

pub(super) fn foreground_layout_uses_altgr() -> bool {
    const ALTGR_PROBES: [u16; 9] = [
        b'@' as u16,
        b'{' as u16,
        b'[' as u16,
        b']' as u16,
        b'}' as u16,
        b'\\' as u16,
        b'~' as u16,
        b'|' as u16,
        0x20AC, // Euro sign
    ];
    let layout = foreground_keyboard_layout();
    ALTGR_PROBES.iter().copied().any(|character| {
        // SAFETY: character and the current foreground HKL are scalar values.
        let mapped = unsafe { VkKeyScanExW(character, layout) };
        if mapped == -1 {
            return false;
        }
        let modifiers = (mapped as u16 >> 8) as u8;
        modifiers & 0b110 == 0b110
    })
}

pub(super) fn native_physical_key_is_down(key: PhysicalKey) -> bool {
    let virtual_key = match key {
        PhysicalKey::Letter(letter) => {
            let scan_code = LETTER_SCAN_CODES[usize::from(letter.index())];
            // Use the foreground layout to translate each physical scan
            // position into the virtual key queried by GetAsyncKeyState.
            // SAFETY: all calls take scalar values or a null optional pointer.
            let foreground_thread = unsafe {
                let window = GetForegroundWindow();
                if window.is_null() {
                    0
                } else {
                    GetWindowThreadProcessId(window, null_mut())
                }
            };
            // SAFETY: a zero thread ID requests the current thread's layout.
            let layout = unsafe { GetKeyboardLayout(foreground_thread) };
            // SAFETY: scan code, mapping mode, and layout are valid scalar inputs.
            let mapped = unsafe { MapVirtualKeyExW(scan_code, MAPVK_VSC_TO_VK_EX, layout) };
            let Ok(mapped) = u16::try_from(mapped & 0xFFFF) else {
                return false;
            };
            mapped
        }
        PhysicalKey::Escape => VK_ESCAPE,
        PhysicalKey::Enter => VK_RETURN,
        PhysicalKey::Other => return false,
    };
    virtual_key != 0 && key_is_down(virtual_key)
}

pub(super) fn key_is_down(key: u16) -> bool {
    // SAFETY: GetAsyncKeyState has no pointer preconditions.
    unsafe { GetAsyncKeyState(i32::from(key)) < 0 }
}
