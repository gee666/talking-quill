//! Publish current-edge ownership before advancing native authority.

use super::*;

pub(super) fn set_current_edge_disposition(
    context: &CallbackContext,
    keyboard: &mut CallbackKeyboard,
    disposition: CurrentEdgeDisposition,
) {
    keyboard.current_edge_disposition = disposition;
    context
        .current_edge_disposition
        .store(disposition as u8, Ordering::Release);
}

pub(super) fn set_atomic_current_edge_disposition(
    context: &CallbackContext,
    disposition: CurrentEdgeDisposition,
) {
    context
        .current_edge_disposition
        .store(disposition as u8, Ordering::Release);
}

pub(super) fn atomic_current_edge_disposition(context: &CallbackContext) -> CurrentEdgeDisposition {
    CurrentEdgeDisposition::from_u8(context.current_edge_disposition.load(Ordering::Acquire))
}
pub(super) struct CallbackProxyGuard<'a>(pub(super) &'a AtomicPtr<c_void>);

impl Drop for CallbackProxyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(null_mut(), Ordering::Release);
    }
}

pub(super) fn recovery_drain_disposition(
    context: &CallbackContext,
    disposition: CurrentEdgeDisposition,
) -> CurrentEdgeDisposition {
    set_atomic_current_edge_disposition(context, disposition);
    disposition
}

pub(super) fn recovery_test_physical_source(context: &CallbackContext, marker: i64) -> bool {
    #[cfg(test)]
    if marker == TEST_RECOVERY_PHYSICAL_MARKER {
        return true;
    }
    #[cfg(feature = "transactional-shortcuts-dev")]
    if context.test_physical_seam_enabled && injection::is_test_physical_marker(marker) {
        return true;
    }
    let _ = (context.test_physical_seam_enabled, marker);
    false
}
