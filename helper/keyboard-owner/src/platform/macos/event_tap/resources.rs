//! Run-loop resource guard and ordered native teardown.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OwnerReleaseStep {
    StopAx,
    DisableTap,
    RemoveTapSource,
    RemoveCommandSource,
    RemoveTimer,
    InvalidateCommandSource,
    InvalidateTimer,
    InvalidateTap,
    DropTargetCache,
    ReleaseTimer,
    ReleaseCommandSource,
    ReleaseTapSource,
    ReleaseTap,
    DropNativePool,
}

pub(super) const OWNER_RELEASE_ORDER: [OwnerReleaseStep; 14] = [
    OwnerReleaseStep::StopAx,
    OwnerReleaseStep::DisableTap,
    OwnerReleaseStep::RemoveTapSource,
    OwnerReleaseStep::RemoveCommandSource,
    OwnerReleaseStep::RemoveTimer,
    OwnerReleaseStep::InvalidateCommandSource,
    OwnerReleaseStep::InvalidateTimer,
    OwnerReleaseStep::InvalidateTap,
    OwnerReleaseStep::DropTargetCache,
    OwnerReleaseStep::ReleaseTimer,
    OwnerReleaseStep::ReleaseCommandSource,
    OwnerReleaseStep::ReleaseTapSource,
    OwnerReleaseStep::ReleaseTap,
    OwnerReleaseStep::DropNativePool,
];

pub(super) struct OwnerNativeResourceGuard {
    pub(super) context: *mut CallbackContext,
    pub(super) run_loop: ffi::CFRunLoopRef,
    pub(super) tap: ffi::CFMachPortRef,
    pub(super) tap_source: ffi::CFRunLoopSourceRef,
    pub(super) command_source: ffi::CFRunLoopSourceRef,
    pub(super) maintenance_timer: ffi::CFRunLoopTimerRef,
}

impl Drop for OwnerNativeResourceGuard {
    fn drop(&mut self) {
        // SAFETY: this guard is created and dropped on the run-loop owner. Its
        // context outlives the guard, and the tested order drives the actual
        // operations, keeping callback refcon/pool alive through invalidation.
        let context = unsafe { &mut *self.context };
        context.gate.close();
        context.state.event_tap.store(null_mut(), Ordering::Release);
        context
            .state
            .maintenance_timer
            .store(null_mut(), Ordering::Release);
        for step in OWNER_RELEASE_ORDER {
            match step {
                OwnerReleaseStep::StopAx => {
                    if let Some(cache) = context.target_cache.as_ref() {
                        cache.request_stop();
                    }
                }
                OwnerReleaseStep::DisableTap => unsafe {
                    ffi::CGEventTapEnable(self.tap, false);
                },
                OwnerReleaseStep::RemoveTapSource => unsafe {
                    ffi::CFRunLoopRemoveSource(
                        self.run_loop,
                        self.tap_source,
                        ffi::kCFRunLoopCommonModes,
                    );
                },
                OwnerReleaseStep::RemoveCommandSource => unsafe {
                    ffi::CFRunLoopRemoveSource(
                        self.run_loop,
                        self.command_source,
                        ffi::kCFRunLoopCommonModes,
                    );
                },
                OwnerReleaseStep::RemoveTimer => unsafe {
                    ffi::CFRunLoopRemoveTimer(
                        self.run_loop,
                        self.maintenance_timer,
                        ffi::kCFRunLoopCommonModes,
                    );
                },
                OwnerReleaseStep::InvalidateCommandSource => unsafe {
                    ffi::CFRunLoopSourceInvalidate(self.command_source);
                },
                OwnerReleaseStep::InvalidateTimer => unsafe {
                    ffi::CFRunLoopTimerInvalidate(self.maintenance_timer);
                },
                OwnerReleaseStep::InvalidateTap => unsafe {
                    ffi::CFMachPortInvalidate(self.tap);
                },
                OwnerReleaseStep::DropTargetCache => drop(context.target_cache.take()),
                OwnerReleaseStep::ReleaseTimer => unsafe {
                    ffi::CFRelease(self.maintenance_timer.cast_const());
                },
                OwnerReleaseStep::ReleaseCommandSource => unsafe {
                    ffi::CFRelease(self.command_source.cast_const());
                },
                OwnerReleaseStep::ReleaseTapSource => unsafe {
                    ffi::CFRelease(self.tap_source.cast_const());
                },
                OwnerReleaseStep::ReleaseTap => unsafe {
                    ffi::CFRelease(self.tap.cast_const());
                },
                OwnerReleaseStep::DropNativePool => {
                    let native_events = context
                        .native_events
                        .get_mut()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    drop(native_events.take());
                }
            }
        }
        context
            .state
            .hook_status
            .store(hook_status_to_u8(HookStatus::Stopped), Ordering::Release);
    }
}
pub(super) fn cleanup_tap_without_sources(context: &CallbackContext, tap: ffi::CFMachPortRef) {
    context.state.event_tap.store(null_mut(), Ordering::Release);
    context.state.hook_status.store(
        hook_status_to_u8(HookStatus::Unavailable),
        Ordering::Release,
    );
    // SAFETY: tap is owned, has no live run-loop source, and is released once.
    unsafe {
        ffi::CGEventTapEnable(tap, false);
        ffi::CFMachPortInvalidate(tap);
        ffi::CFRelease(tap.cast_const());
    }
}
