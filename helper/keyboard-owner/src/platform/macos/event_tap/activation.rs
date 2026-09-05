//! Activation context generation and target registry delivery.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ActiveActivationContext {
    pub(super) binding: ActivationBinding,
    pub(super) context: ActivationContext,
}

pub(super) struct ActivationDispatcher {
    pub(super) next_generation: Option<ActivationGeneration>,
    pub(super) active: Option<ActiveActivationContext>,
    pub(super) targets: TargetRegistry,
}

impl ActivationDispatcher {
    pub(super) fn initialize_process_epoch(&mut self) {
        let _ = self.targets.initialize_process_epoch();
    }

    pub(super) fn deliver(
        &mut self,
        outbound: &Sender<NativeEvent>,
        terminal: &TerminalSignal,
        notice: ActivationNotice,
        cached_target: Option<TargetHandle>,
    ) -> bool {
        match notice {
            ActivationNotice::Down { binding } => {
                if self.active.is_some() {
                    return false;
                }
                let Some(context) = self.take_context(cached_target) else {
                    return false;
                };
                if deliver_callback_event(
                    outbound,
                    terminal,
                    KeyboardEvent::Activation {
                        binding,
                        context,
                        phase: EventPhase::Down,
                    },
                ) {
                    self.active = Some(ActiveActivationContext { binding, context });
                    true
                } else {
                    self.targets.remove(context);
                    false
                }
            }
            ActivationNotice::Up { binding, .. } => {
                let Some(active) = self.active.take() else {
                    return false;
                };
                if active.binding != binding {
                    self.targets.remove(active.context);
                    return false;
                }
                deliver_callback_event(
                    outbound,
                    terminal,
                    KeyboardEvent::Activation {
                        binding,
                        context: active.context,
                        phase: EventPhase::Up,
                    },
                )
            }
            ActivationNotice::Complete { binding, held_ms } => {
                if self.active.is_some() {
                    return false;
                }
                let Some(context) = self.take_context(cached_target) else {
                    return false;
                };
                if deliver_callback_event(
                    outbound,
                    terminal,
                    KeyboardEvent::ActivationComplete {
                        binding,
                        context,
                        held_ms,
                    },
                ) {
                    true
                } else {
                    self.targets.remove(context);
                    false
                }
            }
        }
    }

    pub(super) fn take_context(
        &mut self,
        target: Option<TargetHandle>,
    ) -> Option<ActivationContext> {
        let generation = self.next_generation?;
        self.next_generation = if generation == ActivationGeneration::MAX {
            None
        } else {
            ActivationGeneration::new(generation.get() + 1)
        };
        Some(self.targets.bind_context(generation, target))
    }
}

impl Default for ActivationDispatcher {
    fn default() -> Self {
        Self {
            next_generation: Some(ActivationGeneration::FIRST),
            active: None,
            targets: TargetRegistry::new(),
        }
    }
}
