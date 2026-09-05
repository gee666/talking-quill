//! Platform-neutral keyboard models and bounded state machines.
#[cfg(all(feature = "native-test-input", not(debug_assertions)))]
compile_error!("native-test-input cannot grant physical authority in an optimized core build");

mod activation;
/// Shared fixed capacities derived from the v1 keyboard grammar.
pub mod bounds;
mod input;
mod keys;
mod profile;
mod reducer;
mod shortcut;
pub mod transactional;

pub use activation::{
    ActivationContext, ActivationGeneration, NativeTargetToken, NativeTargetTokenError,
};
pub use bounds::{
    ACTIVATION_KEY_CAPACITY, COMBINED_PHYSICAL_DRAIN_CAPACITY, OWNER_ADMITTED_EFFECT_CAPACITY,
    REPLAY_CLEANUP_EDGE_CAPACITY, SESSION_KEY_CAPACITY,
};
pub use input::{
    EventPhase, KeyInput, KeyPhase, KeyboardEvent, PhysicalKey, PhysicalKeyTracker,
    SessionCaptureMode, SessionKey,
};
pub use keys::{ActivationKey, ModifierMask, ShortcutModifiers};
pub use profile::ProfileId;
pub use reducer::{DecisionPlan, KeyboardReducer};
pub use shortcut::{ActivationBinding, ActivationBindings, Shortcut, ShortcutValidationError};
