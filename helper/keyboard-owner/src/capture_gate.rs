use talking_quill_keyboard_core::SessionCaptureMode;

use crate::build_mode::OWNER_BUILD_MODE;

/// Process-lifetime gate for every native keyboard-suppression facility.
///
/// The gate is deliberately one-way for a helper process: a closed gate can
/// never be reopened by protocol traffic. It filters both activation capture
/// and session Escape/Enter capture before either can reach a native owner.
/// Test-only feature builds may open it through compile-time seams that
/// production package inspection rejects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActivationCaptureGate {
    open: bool,
    runtime_rollback: bool,
    development_disabled: bool,
}

impl ActivationCaptureGate {
    /// Safe production default: no physical keyboard event may be suppressed.
    #[must_use]
    pub const fn closed() -> Self {
        Self::from_controls(true, false)
    }

    /// Explicit compile-time seam for protocol and native behavior harnesses.
    /// Optimized production builds cannot construct an open gate this way.
    #[cfg(any(test, talking_quill_unoptimized_test_support))]
    #[doc(hidden)]
    #[must_use]
    pub const fn open_for_test_harness() -> Self {
        Self::from_controls(false, false)
    }

    /// Explicit test seam for closed-gate observability combinations.
    #[cfg(talking_quill_unoptimized_test_support)]
    #[doc(hidden)]
    #[must_use]
    pub const fn closed_for_test_harness(
        runtime_rollback: bool,
        development_disabled: bool,
    ) -> Self {
        Self {
            open: false,
            runtime_rollback,
            development_disabled,
        }
    }

    /// Resolves the independent development and runtime rollback controls.
    /// Either control closes capture; neither control can override the other.
    const fn from_controls(transactional_development_build: bool, runtime_rollback: bool) -> Self {
        Self {
            open: !transactional_development_build && !runtime_rollback,
            runtime_rollback,
            development_disabled: transactional_development_build,
        }
    }

    /// Resolves process controls once, before protocol traffic can enable
    /// capture. Ordinary and packaged helpers remain safely disabled for I1.
    /// The runtime switch is an independent one-way rollback: it can close a
    /// default-enabled native-test build but can never open a production build.
    #[must_use]
    #[doc(hidden)]
    pub fn for_process() -> Self {
        let _ = std::hint::black_box(OWNER_BUILD_MODE.marker().as_bytes());
        let runtime_rollback = std::env::var_os("TALKING_QUILL_DISABLE_ACTIVATION_CAPTURE")
            .is_some_and(|value| value == "1");
        Self::from_controls(
            !OWNER_BUILD_MODE.capture_enabled_by_default(),
            runtime_rollback,
        )
    }

    #[must_use]
    pub const fn is_open(self) -> bool {
        self.open
    }

    #[must_use]
    pub const fn runtime_rollback_active(self) -> bool {
        self.runtime_rollback
    }

    #[must_use]
    pub const fn development_disabled(self) -> bool {
        self.development_disabled
    }

    /// Returns the only activation state that may cross into a native owner.
    #[must_use]
    pub const fn filter_enabled(self, requested: bool) -> bool {
        self.open && requested
    }

    /// Returns the only session capture mode that may cross into a native owner.
    #[must_use]
    pub const fn filter_session_mode(self, requested: SessionCaptureMode) -> SessionCaptureMode {
        if self.open {
            requested
        } else {
            SessionCaptureMode::Off
        }
    }
}

impl Default for ActivationCaptureGate {
    /// Generic construction never grants capture authority. Runtime code must
    /// resolve the immutable build mode and rollback exactly once through
    /// `for_process`.
    fn default() -> Self {
        Self::closed()
    }
}

#[cfg(test)]
mod tests {
    use super::ActivationCaptureGate;
    use crate::build_mode::OWNER_BUILD_MODE;
    use talking_quill_keyboard_core::SessionCaptureMode;

    #[test]
    fn either_control_closes_the_all_or_nothing_gate() {
        for (development, rollback, expected_open) in [
            (false, false, true),
            (false, true, false),
            (true, false, false),
            (true, true, false),
        ] {
            let gate = ActivationCaptureGate::from_controls(development, rollback);
            assert_eq!(gate.is_open(), expected_open);
            assert_eq!(gate.filter_enabled(true), expected_open);
            assert!(!gate.filter_enabled(false));
            assert_eq!(
                gate.filter_session_mode(SessionCaptureMode::Recording),
                if expected_open {
                    SessionCaptureMode::Recording
                } else {
                    SessionCaptureMode::Off
                }
            );
        }
    }

    #[test]
    fn safe_build_disable_never_falls_back_to_legacy_capture() {
        let gate = ActivationCaptureGate::closed();
        assert!(!gate.is_open());
        assert!(gate.development_disabled());
    }

    #[test]
    fn generic_default_never_grants_process_capture_authority() {
        assert!(!ActivationCaptureGate::default().is_open());
        assert!(ActivationCaptureGate::default().development_disabled());
        let rollback = std::env::var_os("TALKING_QUILL_DISABLE_ACTIVATION_CAPTURE")
            .is_some_and(|value| value == "1");
        assert_eq!(
            ActivationCaptureGate::for_process().is_open(),
            OWNER_BUILD_MODE.capture_enabled_by_default() && !rollback
        );
    }

    #[cfg(not(feature = "local-unsigned-owner"))]
    #[test]
    fn opening_requires_the_explicit_test_harness_seam() {
        assert!(ActivationCaptureGate::open_for_test_harness().is_open());
    }
}
