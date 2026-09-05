//! Event-tap timeout recovery policy, independent of OS resources.
use super::TerminalReason;

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct TapRecoveryPolicy {
    consecutive_timeouts: u8,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TapRecoveryEvent {
    Activity,
    TimeoutRecovered,
    TimeoutRecoveryFailed,
    DisabledByUserInput,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TapRecoveryDecision {
    Continue,
    Terminal(TerminalReason),
}

#[cfg(any(target_os = "macos", test))]
impl TapRecoveryPolicy {
    pub(crate) const fn from_consecutive_timeouts(value: u8) -> Self {
        Self {
            consecutive_timeouts: value,
        }
    }

    pub(crate) const fn consecutive_timeouts(self) -> u8 {
        self.consecutive_timeouts
    }

    pub(crate) const fn observe(self, event: TapRecoveryEvent) -> (Self, TapRecoveryDecision) {
        match event {
            TapRecoveryEvent::Activity => (
                Self::from_consecutive_timeouts(0),
                TapRecoveryDecision::Continue,
            ),
            TapRecoveryEvent::TimeoutRecovered if self.consecutive_timeouts == 0 => (
                Self::from_consecutive_timeouts(1),
                TapRecoveryDecision::Continue,
            ),
            TapRecoveryEvent::TimeoutRecovered => (
                self,
                TapRecoveryDecision::Terminal(TerminalReason::EventTapRepeatedTimeout),
            ),
            TapRecoveryEvent::TimeoutRecoveryFailed => (
                self,
                TapRecoveryDecision::Terminal(TerminalReason::EventTapTimeoutRecoveryFailed),
            ),
            TapRecoveryEvent::DisabledByUserInput => (
                self,
                TapRecoveryDecision::Terminal(TerminalReason::EventTapDisabledByUserInput),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tap_recovery_policy_covers_timeout_and_user_disable_paths() {
        let initial = TapRecoveryPolicy::default();
        let (after_first, decision) = initial.observe(TapRecoveryEvent::TimeoutRecovered);
        assert_eq!(decision, TapRecoveryDecision::Continue);
        assert_eq!(after_first.consecutive_timeouts(), 1);

        assert_eq!(
            initial.observe(TapRecoveryEvent::TimeoutRecoveryFailed).1,
            TapRecoveryDecision::Terminal(TerminalReason::EventTapTimeoutRecoveryFailed)
        );
        assert_eq!(
            after_first.observe(TapRecoveryEvent::TimeoutRecovered).1,
            TapRecoveryDecision::Terminal(TerminalReason::EventTapRepeatedTimeout)
        );
        assert_eq!(
            initial.observe(TapRecoveryEvent::DisabledByUserInput).1,
            TapRecoveryDecision::Terminal(TerminalReason::EventTapDisabledByUserInput)
        );

        let (reset, decision) = after_first.observe(TapRecoveryEvent::Activity);
        assert_eq!(decision, TapRecoveryDecision::Continue);
        assert_eq!(reset, TapRecoveryPolicy::default());
        assert_eq!(
            reset.observe(TapRecoveryEvent::TimeoutRecovered).1,
            TapRecoveryDecision::Continue
        );
    }
}
