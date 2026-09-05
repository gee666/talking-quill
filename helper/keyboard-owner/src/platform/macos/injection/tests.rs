use super::*;

use std::{cell::RefCell, ffi::c_void};
use talking_quill_keyboard_core::{
    ActivationKey,
    transactional::{KeyIdentity, NativeKey},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PostOperation {
    SetTimestamp(usize, ffi::CGEventTimestamp),
    Post(usize),
}

const fn identity() -> InjectionIdentity {
    InjectionIdentity::for_test(42)
}

fn record(phase: PhysicalPhase, flags: u64) -> ReplayRecord {
    ReplayRecord {
        key: KeyIdentity::Letter(ActivationKey::V),
        native: NativeKey {
            virtual_key: 9,
            scan_code: 0,
            extended: false,
            platform_flags: flags,
        },
        phase,
        observed_at_ms: 17,
    }
}

mod pool;

mod identity;

mod replay;
