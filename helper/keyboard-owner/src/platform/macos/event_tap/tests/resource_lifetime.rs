//! Resource lifetime contracts.

use super::*;

#[test]
fn owner_resource_release_order_keeps_refcon_and_pool_until_native_invalidation() {
    assert_eq!(
        OWNER_RELEASE_ORDER,
        [
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
        ]
    );
}

#[test]
fn owner_callback_panic_is_contained_and_recovery_commits_before_return() {
    let (context, _outbound, _commands) = test_context();
    // SAFETY: the local context remains live on this thread for the callback.
    unsafe {
        run_owner_callback(
            (&context as *const CallbackContext).cast_mut().cast(),
            |_| panic!("induced owner callback panic"),
        );
    }
    assert_eq!(
        context.terminal.reason(),
        Some(TerminalReason::CallbackPanicked)
    );
    assert!(!context.state.recovery_pending.load(Ordering::Acquire));
    assert!(!context.keyboard.is_poisoned());
    assert!(!pending_native_work(&context));
}
