//! Fixed capacities shared by native adapters and both reducers.

/// Unique physical A-Z generations that activation may own.
pub const ACTIVATION_KEY_CAPACITY: usize = 26;
/// Unique Escape/Enter generations that session capture may own.
pub const SESSION_KEY_CAPACITY: usize = 2;
/// Combined physical drain capacity across activation and session keys.
pub const COMBINED_PHYSICAL_DRAIN_CAPACITY: usize = ACTIVATION_KEY_CAPACITY + SESSION_KEY_CAPACITY;
/// At most one balancing cleanup release per unique activation letter.
pub const REPLAY_CLEANUP_EDGE_CAPACITY: usize = ACTIVATION_KEY_CAPACITY;
/// Fixed owner callback-admission/effect queue budget.
pub const OWNER_ADMITTED_EFFECT_CAPACITY: usize = 8;
