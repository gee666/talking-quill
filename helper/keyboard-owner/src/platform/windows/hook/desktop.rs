//! Observe desktop changes and reconcile physical keyboard state.
use super::*;

pub(super) fn desktop_identity(
    desktop: windows_sys::Win32::System::StationsAndDesktops::HDESK,
) -> Option<DesktopIdentity> {
    if desktop.is_null() {
        return None;
    }
    let mut identity = DesktopIdentity {
        name: [0; 64],
        len: 0,
    };
    let mut needed = 0_u32;
    let bytes = u32::try_from(identity.name.len() * size_of::<u16>()).ok()?;
    // SAFETY: desktop is a retained handle and the bounded UTF-16 output is
    // owner-local writable storage.
    let read = unsafe {
        GetUserObjectInformationW(
            desktop,
            UOI_NAME,
            identity.name.as_mut_ptr().cast(),
            bytes,
            &raw mut needed,
        ) != 0
    };
    if !read || needed < 2 || needed > bytes {
        return None;
    }
    identity.len = u8::try_from(needed / 2 - 1).ok()?;
    Some(identity)
}

pub(super) fn current_input_desktop() -> Option<DesktopIdentity> {
    // SAFETY: opens a non-inheritable read-only handle to the current input
    // desktop. The handle is closed after extracting its stable object name.
    unsafe {
        let desktop = OpenInputDesktop(0, 0, DESKTOP_READOBJECTS);
        if desktop.is_null() {
            return None;
        }
        let identity = desktop_identity(desktop);
        CloseDesktop(desktop);
        identity
    }
}

pub(super) fn reconcile_native_state(context: &CallbackContext) {
    let input_desktop = current_input_desktop();
    let physical = physical_tracker_from_state(native_physical_key_is_down);
    let modifiers = ModifierTracker::from_state(key_is_down);
    let logical_v_down = key_is_down(VK_V);
    reconcile_sampled_state(context, input_desktop, physical, modifiers, logical_v_down);
}

pub(super) fn reconcile_sampled_state(
    context: &CallbackContext,
    input_desktop: Option<DesktopIdentity>,
    mut physical: WindowsPhysicalTracker,
    modifiers: ModifierTracker,
    logical_v_down: bool,
) {
    let Some(mut keyboard) = lock_keyboard_recovering(context) else {
        return;
    };
    if !recover_transaction_authority(context, &mut keyboard) {
        return;
    }
    if keyboard.input_desktop != input_desktop {
        keyboard.input_desktop = input_desktop;
        let _ = begin_transaction_control(
            context,
            &mut keyboard,
            Control::CloseAdmission(CancelReason::SecureDesktop),
        );
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
        return;
    }
    // Suppressed downs do not reach Windows' asynchronous key state. Its zero
    // bit is not a physical release and must not cancel/replay a held prefix.
    // Only the ordered hook up can release a key we own. Polling still repairs
    // unsuppressed keys and modifiers after missed native transitions.
    let retained_letters =
        keyboard.transactional.owned_letters() & keyboard.physical.held_letter_bits();
    for index in 0_u8..26 {
        if retained_letters & (1_u32 << index) != 0 {
            physical.observe(
                PhysicalKey::Letter(ActivationKey::from_index(index).expect("A-Z index")),
                None,
                KeyPhase::Down,
            );
        }
    }
    // Native recovery cannot distinguish the synthetic Ctrl emitted by AltGr
    // from an intentionally held Ctrl. Fail closed while both left Ctrl and
    // right Alt are down, even when foreground layout probing is unavailable.
    keyboard.altgr_synthetic_ctrl = modifiers.ctrl.left && modifiers.alt.right;
    let altgr_active = conservative_altgr(&modifiers, keyboard.altgr_synthetic_ctrl);
    let snapshot = PhysicalSnapshot::new(
        physical.held_letter_bits(),
        modifiers.transactional_sides(),
        altgr_active,
    );
    if keyboard.external_held_letters != 0
        || keyboard
            .external_reconcile_after
            .is_some_and(|deadline| Instant::now() < deadline)
        || keyboard.transactional.pending_injected_cleanup().is_some()
        || keyboard.transactional.pending_menu_cleanup().is_some()
        || !keyboard.pending_paste_cleanup.is_empty()
    {
        return;
    }
    if keyboard.transactional.physical_letters() == snapshot.held_letters
        && keyboard.transactional.physical_modifiers() == snapshot.modifiers
        && keyboard.transactional.physical_modifiers() == keyboard.modifiers.transactional_sides()
        && keyboard.altgr_active == snapshot.alt_gr_active
        && keyboard.logical_v_down == logical_v_down
    {
        return;
    }
    keyboard.physical = physical;
    keyboard.modifiers = modifiers;
    keyboard.logical_v_down = logical_v_down;
    keyboard.altgr_active = altgr_active;
    let outcome = begin_transaction_control(context, &mut keyboard, Control::Reconcile(snapshot));
    if outcome.is_some_and(|outcome| outcome.shutdown == ShutdownState::Terminal)
        && !context.terminal.is_triggered()
    {
        context
            .terminal
            .trigger(TerminalReason::InputInjectionUnavailable);
    }
}

pub(super) const fn low_level_hook_module() -> windows_sys::Win32::Foundation::HMODULE {
    // WH_KEYBOARD_LL runs in the installing process and this callback is linked
    // into the owner executable rather than an injectable DLL. Passing the EXE
    // image as hMod can produce a non-null hook that never receives global
    // callbacks; NULL is the documented in-process low-level-hook identity.
    null_mut()
}
