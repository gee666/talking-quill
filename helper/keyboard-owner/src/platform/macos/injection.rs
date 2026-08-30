#![cfg_attr(all(test, feature = "local-unsigned-owner"), allow(dead_code))]

use std::ptr::{null, null_mut};

use super::ffi;
use crate::platform::PlatformError;
use talking_quill_keyboard_core::transactional::{
    CleanupBatch, InputSource, JOURNAL_CAPACITY, PhysicalPhase, ReplayBatch, ReplayRecord,
};

const BARRIER_EVENT_COUNT: usize = 2;
const PASTE_BARRIER_EVENT_COUNT: usize = 2;
const INJECTION_MARKER_COUNT: usize = 1;
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) const TEST_PHYSICAL_MARKER: i64 = 0x5451_5048_5953_4943;
#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) const TEST_PERMISSION_LOSS_MARKER: i64 = TEST_PHYSICAL_MARKER + 1;
#[cfg(feature = "transactional-shortcuts-dev")]
const MACOS_TEST_SEAM_BUILD_MARKER: &[u8] = b"TALKING_QUILL_MACOS_NATIVE_TEST_SEAMS=ENABLED";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct InjectionIdentity {
    pub(super) source_pid: i64,
    process_nonce: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct OperationToken {
    generation: u64,
    marker: i64,
}

impl OperationToken {
    #[cfg(feature = "transactional-shortcuts-dev")]
    pub(super) const fn generation(self) -> u64 {
        self.generation
    }

    #[cfg(test)]
    pub(super) const fn marker(self) -> i64 {
        self.marker
    }

    #[cfg(test)]
    pub(super) const fn for_test(generation: u64) -> Self {
        Self {
            generation,
            marker: 0x5a5a_0000_0000_0000_u64.wrapping_add(generation) as i64,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Submission {
    pub(super) count: usize,
    pub(super) token: Option<OperationToken>,
}

impl InjectionIdentity {
    pub(super) fn new() -> Result<Self, PlatformError> {
        #[cfg(feature = "transactional-shortcuts-dev")]
        let _ = std::hint::black_box(MACOS_TEST_SEAM_BUILD_MARKER);
        let mut markers = [0_i64; INJECTION_MARKER_COUNT];
        // SAFETY: null selects kSecRandomDefault and the fixed marker array is
        // writable for the exact byte count. This is owner startup, pre-tap.
        let status = unsafe {
            ffi::SecRandomCopyBytes(
                null_mut(),
                std::mem::size_of_val(&markers),
                markers.as_mut_ptr().cast(),
            )
        };
        if status != 0 || markers.contains(&0) {
            return Err(PlatformError::NativeFailure);
        }
        // SAFETY: getpid has no preconditions and cannot block or allocate.
        let source_pid = i64::from(unsafe { ffi::getpid() });
        if source_pid <= 0 {
            return Err(PlatformError::NativeFailure);
        }
        Ok(Self {
            source_pid,
            process_nonce: markers[0],
        })
    }

    #[cfg(test)]
    pub(super) const fn for_test(source_pid: i64) -> Self {
        Self {
            source_pid,
            process_nonce: 0x5a5a_0000_0000_0000,
        }
    }
}
const REPLAY_POOL_START: usize = 0;
const BARRIER_POOL_START: usize = REPLAY_POOL_START + JOURNAL_CAPACITY;
const PASTE_BARRIER_POOL_START: usize = BARRIER_POOL_START + BARRIER_EVENT_COUNT;
pub(super) const DEFERRED_EDGE_CAPACITY: usize = 64;
pub(super) const DEFERRED_POOL_BANKS: usize = 2;
const DEFERRED_POOL_START: usize = PASTE_BARRIER_POOL_START + PASTE_BARRIER_EVENT_COUNT;
pub(super) const NATIVE_EVENT_POOL_CAPACITY: usize = JOURNAL_CAPACITY
    + BARRIER_EVENT_COUNT
    + PASTE_BARRIER_EVENT_COUNT
    + DEFERRED_EDGE_CAPACITY * DEFERRED_POOL_BANKS;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EventDescriptor {
    event_type: u32,
    key_code: u16,
    key_down: bool,
    repeat: bool,
    flags: u64,
    marker: i64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct DeferredEvent {
    pub(super) event_type: u32,
    pub(super) key_code: u16,
    pub(super) repeat: bool,
    pub(super) is_down: bool,
    pub(super) flags: u64,
    pub(super) keyboard_type: i64,
    pub(super) original_timestamp: ffi::CGEventTimestamp,
    pub(super) source: InputSource,
    pub(super) source_pid: i64,
    pub(super) original_marker: i64,
    pub(super) generation: u32,
    pub(super) uncertain_owned: bool,
    pub(super) foreground_balance: bool,
    pub(super) hidden_generation: bool,
    pub(super) location: ffi::CGPoint,
    pub(super) mouse_number: i64,
    pub(super) mouse_click_state: i64,
    pub(super) mouse_pressure: f64,
    pub(super) mouse_button: i64,
    pub(super) mouse_delta_x: i64,
    pub(super) mouse_delta_y: i64,
    pub(super) mouse_instant_mouser: i64,
    pub(super) mouse_subtype: i64,
}

impl DeferredEvent {
    pub(super) const EMPTY: Self = Self {
        event_type: 0,
        key_code: 0,
        repeat: false,
        is_down: false,
        flags: 0,
        keyboard_type: 0,
        original_timestamp: 0,
        source: InputSource::External,
        source_pid: 0,
        original_marker: 0,
        generation: 0,
        uncertain_owned: false,
        foreground_balance: false,
        hidden_generation: false,
        location: ffi::CGPoint { x: 0.0, y: 0.0 },
        mouse_number: 0,
        mouse_click_state: 0,
        mouse_pressure: 0.0,
        mouse_button: 0,
        mouse_delta_x: 0,
        mouse_delta_y: 0,
        mouse_instant_mouser: 0,
        mouse_subtype: 0,
    };

    pub(super) const fn is_mouse(self) -> bool {
        matches!(
            self.event_type,
            ffi::K_CG_EVENT_LEFT_MOUSE_DOWN
                | ffi::K_CG_EVENT_LEFT_MOUSE_UP
                | ffi::K_CG_EVENT_RIGHT_MOUSE_DOWN
                | ffi::K_CG_EVENT_RIGHT_MOUSE_UP
                | ffi::K_CG_EVENT_OTHER_MOUSE_DOWN
                | ffi::K_CG_EVENT_OTHER_MOUSE_UP
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EventMutation {
    event_type: u32,
    key_code: i64,
    repeat: i64,
    flags: u64,
    marker: i64,
}

fn event_mutation(descriptor: EventDescriptor) -> EventMutation {
    EventMutation {
        event_type: descriptor.event_type,
        key_code: i64::from(descriptor.key_code),
        repeat: i64::from(descriptor.repeat),
        flags: descriptor.flags,
        marker: descriptor.marker,
    }
}

/// Owner-thread CGEvent storage allocated completely before the tap is enabled.
/// Replay/cleanup, gap barrier, paste, and both deferred rollover banks use
/// disjoint ranges, so an observed batch is never mutated by another effect.
pub(super) struct NativeEventPool {
    events: [ffi::CGEventRef; NATIVE_EVENT_POOL_CAPACITY],
    identity: InjectionIdentity,
    timebase: ffi::MachTimebaseInfo,
    last_post_timestamp: ffi::CGEventTimestamp,
    next_operation_generation: u64,
}

// SAFETY: the pool is created, used, and released only by the hook owner. This
// marker permits the initially-empty CallbackContext field to cross into that
// owner thread before construction.
unsafe impl Send for NativeEventPool {}

impl NativeEventPool {
    pub(super) fn new(identity: InjectionIdentity) -> Result<Self, PlatformError> {
        let timebase = event_timestamp_timebase()?;
        let events = initialize_event_refs(
            || {
                // SAFETY: null source requests the default source. A generic
                // event can be configured from the startup pool as keyboard,
                // flags-changed, or mouse without callback-time creation.
                unsafe { ffi::CGEventCreate(null()) }
            },
            |event| {
                // SAFETY: partial initialization owns every non-null Create result.
                unsafe { ffi::CFRelease(event.cast_const()) };
            },
        )
        .map_err(|_| PlatformError::NativeFailure)?;
        Ok(Self {
            events,
            identity,
            timebase,
            last_post_timestamp: 0,
            next_operation_generation: 1,
        })
    }

    fn next_operation_token(&mut self) -> Option<OperationToken> {
        loop {
            let generation = self.next_operation_generation;
            self.next_operation_generation = generation.checked_add(1)?;
            let marker = self.identity.process_nonce ^ generation as i64;
            if marker != 0 {
                return Some(OperationToken { generation, marker });
            }
        }
    }

    fn configure(&mut self, index: usize, descriptor: EventDescriptor) {
        let event = self.events[index];
        let mutation = event_mutation(descriptor);
        // SAFETY: every pool reference is a retained mutable CGEvent, and all
        // descriptor fields are valid keyboard event fields. CGEventPost copies
        // the event into the system event stream (callers may release an event
        // immediately after posting), so these retained objects may be reused
        // after the complete batch has been submitted.
        unsafe {
            ffi::CGEventSetType(event, mutation.event_type);
            ffi::CGEventSetIntegerValueField(
                event,
                ffi::K_CG_KEYBOARD_EVENT_KEYCODE,
                mutation.key_code,
            );
            ffi::CGEventSetIntegerValueField(
                event,
                ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT,
                mutation.repeat,
            );
            ffi::CGEventSetFlags(event, mutation.flags);
            ffi::CGEventSetIntegerValueField(
                event,
                ffi::K_CG_EVENT_SOURCE_USER_DATA,
                mutation.marker,
            );
        }
    }

    fn configure_deferred(&mut self, bank: usize, offset: usize, edge: DeferredEvent, marker: i64) {
        let event = self.events[DEFERRED_POOL_START + bank * DEFERRED_EDGE_CAPACITY + offset];
        unsafe {
            ffi::CGEventSetType(event, edge.event_type);
            ffi::CGEventSetFlags(event, edge.flags);
            ffi::CGEventSetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA, marker);
            ffi::CGEventSetIntegerValueField(
                event,
                ffi::K_CG_KEYBOARD_EVENT_KEYCODE,
                i64::from(edge.key_code),
            );
            ffi::CGEventSetIntegerValueField(
                event,
                ffi::K_CG_KEYBOARD_EVENT_AUTOREPEAT,
                i64::from(edge.repeat),
            );
            ffi::CGEventSetIntegerValueField(
                event,
                ffi::K_CG_KEYBOARD_EVENT_KEYBOARD_TYPE,
                edge.keyboard_type,
            );
            if edge.is_mouse() {
                ffi::CGEventSetLocation(event, edge.location);
                for (field, value) in [
                    (ffi::K_CG_MOUSE_EVENT_NUMBER, edge.mouse_number),
                    (ffi::K_CG_MOUSE_EVENT_CLICK_STATE, edge.mouse_click_state),
                    (ffi::K_CG_MOUSE_EVENT_BUTTON_NUMBER, edge.mouse_button),
                    (ffi::K_CG_MOUSE_EVENT_DELTA_X, edge.mouse_delta_x),
                    (ffi::K_CG_MOUSE_EVENT_DELTA_Y, edge.mouse_delta_y),
                    (
                        ffi::K_CG_MOUSE_EVENT_INSTANT_MOUSER,
                        edge.mouse_instant_mouser,
                    ),
                    (ffi::K_CG_MOUSE_EVENT_SUBTYPE, edge.mouse_subtype),
                ] {
                    ffi::CGEventSetIntegerValueField(event, field, value);
                }
                ffi::CGEventSetDoubleValueField(
                    event,
                    ffi::K_CG_MOUSE_EVENT_PRESSURE,
                    edge.mouse_pressure,
                );
            }
        }
    }

    fn post_range(&mut self, start: usize, len: usize) {
        self.post_range_to(start, len, None);
    }

    fn post_range_at_proxy(&mut self, start: usize, len: usize, proxy: ffi::CGEventTapProxy) {
        self.post_range_to(start, len, Some(proxy));
    }

    fn post_range_to(&mut self, start: usize, len: usize, proxy: Option<ffi::CGEventTapProxy>) {
        let numer = self.timebase.numer;
        let denom = self.timebase.denom;
        post_events_with_fresh_timestamps(
            &self.events[start..start + len],
            &mut self.last_post_timestamp,
            || {
                // SAFETY: mach_absolute_time has no pointer, allocation, or
                // blocking preconditions and is monotonic since startup.
                let ticks = unsafe { ffi::mach_absolute_time() };
                mach_ticks_to_event_timestamp(ticks, numer, denom).unwrap_or(u64::MAX)
            },
            |event, timestamp| {
                // SAFETY: the selected disjoint range was fully configured and
                // remains retained by this owner pool. CoreGraphics requires
                // CGEventTimestamp as elapsed nanoseconds since startup.
                unsafe { ffi::CGEventSetTimestamp(event, timestamp) };
            },
            |event| {
                // SAFETY: timestamp refresh is the immediately preceding
                // operation for this retained, fully configured event. A live
                // proxy inserts replay after this tap in exact call order;
                // owner controls without a callback proxy use the HID stream.
                unsafe {
                    if let Some(proxy) = proxy {
                        ffi::CGEventTapPostEvent(proxy, event);
                    } else {
                        ffi::CGEventPost(ffi::K_CG_HID_EVENT_TAP, event);
                    }
                };
            },
        );
    }
}

impl Drop for NativeEventPool {
    fn drop(&mut self) {
        for event in self.events {
            // SAFETY: successful startup created every entry exactly once. Pool
            // teardown runs on the owner after tap drain and releases each once.
            unsafe { ffi::CFRelease(event.cast_const()) };
        }
    }
}

fn event_timestamp_timebase() -> Result<ffi::MachTimebaseInfo, PlatformError> {
    let mut timebase = ffi::MachTimebaseInfo::default();
    // SAFETY: timebase points to valid writable storage. The timebase is read
    // once during owner startup, before the event tap can invoke callbacks.
    let status = unsafe { ffi::mach_timebase_info(&raw mut timebase) };
    if status != 0 || timebase.numer == 0 || timebase.denom == 0 {
        return Err(PlatformError::NativeFailure);
    }
    Ok(timebase)
}

fn mach_ticks_to_event_timestamp(
    ticks: u64,
    numer: u32,
    denom: u32,
) -> Option<ffi::CGEventTimestamp> {
    if numer == 0 || denom == 0 {
        return None;
    }
    let nanoseconds = u128::from(ticks) * u128::from(numer) / u128::from(denom);
    u64::try_from(nanoseconds).ok()
}

fn post_events_with_fresh_timestamps<T: Copy>(
    events: &[T],
    last_post_timestamp: &mut ffi::CGEventTimestamp,
    mut now: impl FnMut() -> ffi::CGEventTimestamp,
    mut set_timestamp: impl FnMut(T, ffi::CGEventTimestamp),
    mut post: impl FnMut(T),
) {
    for event in events.iter().copied() {
        let timestamp = now().max(*last_post_timestamp);
        *last_post_timestamp = timestamp;
        set_timestamp(event, timestamp);
        post(event);
    }
}

fn initialize_event_refs(
    mut create: impl FnMut() -> ffi::CGEventRef,
    mut release: impl FnMut(ffi::CGEventRef),
) -> Result<[ffi::CGEventRef; NATIVE_EVENT_POOL_CAPACITY], usize> {
    let mut events = [null_mut(); NATIVE_EVENT_POOL_CAPACITY];
    for index in 0..events.len() {
        let event = create();
        if event.is_null() {
            for initialized in events[..index].iter().copied() {
                release(initialized);
            }
            return Err(index);
        }
        events[index] = event;
    }
    Ok(events)
}

#[must_use]
pub(super) const fn unmarked_source(identity: InjectionIdentity, source_pid: i64) -> InputSource {
    if source_pid == identity.source_pid || source_pid > 0 {
        InputSource::External
    } else {
        InputSource::Physical
    }
}

pub(super) const fn token_matches(
    identity: InjectionIdentity,
    expected: Option<OperationToken>,
    marker: i64,
    source_pid: i64,
) -> bool {
    source_pid == identity.source_pid && matches!(expected, Some(token) if token.marker == marker)
}

pub(super) const fn replay_shape_is_valid(event_type: u32, key_code: i64, repeat: bool) -> bool {
    key_code >= 0
        && key_code <= 127
        && matches!(
            event_type,
            ffi::K_CG_EVENT_KEY_DOWN | ffi::K_CG_EVENT_KEY_UP | ffi::K_CG_EVENT_FLAGS_CHANGED
        )
        && (!repeat || event_type == ffi::K_CG_EVENT_KEY_DOWN)
}

#[derive(Clone, Copy)]
pub(super) struct PreparedDeferredEvents {
    start: usize,
    count: usize,
    token: OperationToken,
}

impl PreparedDeferredEvents {
    pub(super) const fn token(self) -> OperationToken {
        self.token
    }

    pub(super) fn post(self, pool: &mut NativeEventPool) {
        pool.post_range(self.start, self.count);
    }
}

pub(super) fn prepare_deferred_events(
    pool: &mut NativeEventPool,
    bank: usize,
    edges: &[DeferredEvent],
) -> Option<PreparedDeferredEvents> {
    if bank >= DEFERRED_POOL_BANKS
        || edges.is_empty()
        || edges.len() > DEFERRED_EDGE_CAPACITY
        || !posting_is_available()
    {
        return None;
    }
    let token = pool.next_operation_token()?;
    for (offset, edge) in edges.iter().copied().enumerate() {
        pool.configure_deferred(bank, offset, edge, token.marker);
    }
    Some(PreparedDeferredEvents {
        start: DEFERRED_POOL_START + bank * DEFERRED_EDGE_CAPACITY,
        count: edges.len(),
        token,
    })
}

#[derive(Clone, Copy)]
pub(super) struct PreparedGapBarrier {
    token: OperationToken,
}

impl PreparedGapBarrier {
    pub(super) const fn token(self) -> OperationToken {
        self.token
    }

    pub(super) fn post(self, pool: &mut NativeEventPool) {
        pool.post_range(BARRIER_POOL_START, BARRIER_EVENT_COUNT);
    }
}

pub(super) fn prepare_gap_barrier(pool: &mut NativeEventPool) -> Option<PreparedGapBarrier> {
    if !posting_is_available() {
        return None;
    }
    let token = pool.next_operation_token()?;
    for (offset, descriptor) in gap_barrier_descriptors(token).into_iter().enumerate() {
        pool.configure(BARRIER_POOL_START + offset, descriptor);
    }
    Some(PreparedGapBarrier { token })
}

#[cfg(test)]
pub(super) fn post_gap_barrier(pool: &mut NativeEventPool) -> Option<OperationToken> {
    let prepared = prepare_gap_barrier(pool)?;
    prepared.post(pool);
    Some(prepared.token())
}

#[derive(Clone, Copy)]
pub(super) struct PreparedPasteBarrier {
    token: OperationToken,
}

impl PreparedPasteBarrier {
    pub(super) const fn token(self) -> OperationToken {
        self.token
    }

    pub(super) fn post(self, pool: &mut NativeEventPool) {
        pool.post_range(PASTE_BARRIER_POOL_START, PASTE_BARRIER_EVENT_COUNT);
    }

    #[cfg(feature = "transactional-shortcuts-dev")]
    pub(super) fn post_down(self, pool: &mut NativeEventPool) {
        pool.post_range(PASTE_BARRIER_POOL_START, 1);
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) fn post_prepared_paste_barrier_up(pool: &mut NativeEventPool) {
    pool.post_range(PASTE_BARRIER_POOL_START + 1, 1);
}

pub(super) fn prepare_paste_barrier(pool: &mut NativeEventPool) -> Option<PreparedPasteBarrier> {
    if !posting_is_available() {
        return None;
    }
    let token = pool.next_operation_token()?;
    for (offset, descriptor) in paste_barrier_descriptors(token).into_iter().enumerate() {
        pool.configure(PASTE_BARRIER_POOL_START + offset, descriptor);
    }
    Some(PreparedPasteBarrier { token })
}

#[cfg(test)]
pub(super) fn post_paste_barrier(pool: &mut NativeEventPool) -> Option<OperationToken> {
    let prepared = prepare_paste_barrier(pool)?;
    prepared.post(pool);
    Some(prepared.token())
}

const fn gap_barrier_descriptors(token: OperationToken) -> [EventDescriptor; BARRIER_EVENT_COUNT] {
    [
        EventDescriptor {
            event_type: ffi::K_CG_EVENT_KEY_DOWN,
            key_code: 127,
            key_down: true,
            repeat: false,
            flags: 0,
            marker: token.marker,
        },
        EventDescriptor {
            event_type: ffi::K_CG_EVENT_KEY_UP,
            key_code: 127,
            key_down: false,
            repeat: false,
            flags: 0,
            marker: token.marker,
        },
    ]
}

const fn paste_barrier_descriptors(
    token: OperationToken,
) -> [EventDescriptor; PASTE_BARRIER_EVENT_COUNT] {
    [
        EventDescriptor {
            event_type: ffi::K_CG_EVENT_KEY_DOWN,
            key_code: 127,
            key_down: true,
            repeat: false,
            flags: 0,
            marker: token.marker,
        },
        EventDescriptor {
            event_type: ffi::K_CG_EVENT_KEY_UP,
            key_code: 127,
            key_down: false,
            repeat: false,
            flags: 0,
            marker: token.marker,
        },
    ]
}

#[derive(Clone, Copy)]
pub(super) struct PreparedReplay {
    count: usize,
    token: OperationToken,
}

impl PreparedReplay {
    pub(super) const fn submission(self) -> Submission {
        Submission {
            count: self.count,
            token: Some(self.token),
        }
    }

    pub(super) fn post(self, pool: &mut NativeEventPool, proxy: Option<ffi::CGEventTapProxy>) {
        if let Some(proxy) = proxy {
            pool.post_range_at_proxy(REPLAY_POOL_START, self.count, proxy);
        } else {
            pool.post_range(REPLAY_POOL_START, self.count);
        }
    }
}

pub(super) fn prepare_replay(
    pool: &mut NativeEventPool,
    batch: ReplayBatch,
) -> Option<PreparedReplay> {
    prepare_records(pool, batch.entries())
}

pub(super) fn prepare_cleanup(
    pool: &mut NativeEventPool,
    batch: CleanupBatch,
) -> Option<PreparedReplay> {
    prepare_records(pool, batch.entries())
}

fn prepare_records(pool: &mut NativeEventPool, records: &[ReplayRecord]) -> Option<PreparedReplay> {
    if records.is_empty() || records.len() > JOURNAL_CAPACITY || !posting_is_available() {
        return None;
    }
    let token = pool.next_operation_token()?;
    for (offset, record) in records.iter().copied().enumerate() {
        pool.configure(
            REPLAY_POOL_START + offset,
            replay_descriptor(record, token.marker),
        );
    }
    Some(PreparedReplay {
        count: records.len(),
        token,
    })
}

fn replay_descriptor(record: ReplayRecord, marker: i64) -> EventDescriptor {
    EventDescriptor {
        event_type: if matches!(
            record.key,
            talking_quill_keyboard_core::transactional::KeyIdentity::Modifier(_)
        ) {
            ffi::K_CG_EVENT_FLAGS_CHANGED
        } else if record.phase == PhysicalPhase::Up {
            ffi::K_CG_EVENT_KEY_UP
        } else {
            ffi::K_CG_EVENT_KEY_DOWN
        },
        key_code: record.native.virtual_key,
        key_down: record.phase != PhysicalPhase::Up,
        repeat: record.phase == PhysicalPhase::Repeat,
        flags: record.native.platform_flags,
        marker,
    }
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) const fn is_test_physical_marker(marker: i64) -> bool {
    matches!(marker, TEST_PHYSICAL_MARKER | TEST_PERMISSION_LOSS_MARKER)
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) fn post_test_physical_key(key_code: u16, key_down: bool, flags: u64) -> bool {
    // This source exists only in explicitly feature-built trusted harnesses. It
    // traverses CoreGraphics and the production tap callback, but callback-side
    // classification treats its dedicated marker as physical input.
    let event = unsafe { ffi::CGEventCreateKeyboardEvent(null(), key_code, key_down) };
    if event.is_null() {
        return false;
    }
    unsafe {
        if matches!(key_code, 54 | 55 | 56 | 58 | 59 | 60 | 61 | 62) {
            ffi::CGEventSetType(event, ffi::K_CG_EVENT_FLAGS_CHANGED);
        }
        ffi::CGEventSetFlags(event, flags);
        ffi::CGEventSetIntegerValueField(
            event,
            ffi::K_CG_EVENT_SOURCE_USER_DATA,
            TEST_PHYSICAL_MARKER,
        );
        ffi::CGEventPost(ffi::K_CG_HID_EVENT_TAP, event);
        ffi::CFRelease(event.cast_const());
    }
    true
}

#[cfg(feature = "transactional-shortcuts-dev")]
fn post_test_control_marker(marker: i64) -> bool {
    let event = unsafe { ffi::CGEventCreateKeyboardEvent(null(), 127, true) };
    if event.is_null() {
        return false;
    }
    unsafe {
        ffi::CGEventSetIntegerValueField(event, ffi::K_CG_EVENT_SOURCE_USER_DATA, marker);
        ffi::CGEventPost(ffi::K_CG_HID_EVENT_TAP, event);
        ffi::CFRelease(event.cast_const());
    }
    true
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) fn post_test_permission_loss() -> bool {
    post_test_control_marker(TEST_PERMISSION_LOSS_MARKER)
}

#[cfg(feature = "transactional-shortcuts-dev")]
pub(super) fn post_test_physical_mouse_down() -> bool {
    // Snapshot the real cursor rather than injecting at (0,0), which can hit a
    // menu/display boundary instead of the controlled focused fixture.
    let cursor_event = unsafe { ffi::CGEventCreate(null()) };
    if cursor_event.is_null() {
        return false;
    }
    let cursor = unsafe { ffi::CGEventGetLocation(cursor_event) };
    let location = if cursor.x == 0.0 && cursor.y == 0.0 {
        // Trusted fixture pins its TextEdit window over this fallback point.
        ffi::CGPoint { x: 300.0, y: 250.0 }
    } else {
        cursor
    };
    unsafe { ffi::CFRelease(cursor_event.cast_const()) };
    let down = unsafe {
        ffi::CGEventCreateMouseEvent(null(), ffi::K_CG_EVENT_LEFT_MOUSE_DOWN, location, 0)
    };
    let up =
        unsafe { ffi::CGEventCreateMouseEvent(null(), ffi::K_CG_EVENT_LEFT_MOUSE_UP, location, 0) };
    if down.is_null() || up.is_null() {
        unsafe {
            if !down.is_null() {
                ffi::CFRelease(down.cast_const());
            }
            if !up.is_null() {
                ffi::CFRelease(up.cast_const());
            }
        }
        return false;
    }
    unsafe {
        for event in [down, up] {
            ffi::CGEventSetIntegerValueField(
                event,
                ffi::K_CG_EVENT_SOURCE_USER_DATA,
                TEST_PHYSICAL_MARKER,
            );
        }
        // Both events are prebuilt before either post, so every successful
        // seam invocation is balanced even when the tap cancels/reposts down.
        ffi::CGEventPost(ffi::K_CG_HID_EVENT_TAP, down);
        ffi::CGEventPost(ffi::K_CG_HID_EVENT_TAP, up);
        ffi::CFRelease(up.cast_const());
        ffi::CFRelease(down.cast_const());
    }
    true
}

fn posting_is_available() -> bool {
    // SAFETY: permission and Secure Event Input probes take no pointers and do
    // not prompt. Replay must not post while Secure Event Input is active.
    unsafe { ffi::CGPreflightPostEventAccess() && ffi::IsSecureEventInputEnabled() == 0 }
}

#[cfg(all(test, feature = "transactional-shortcuts-dev"))]
mod tests {
    use std::{cell::RefCell, ffi::c_void};

    use super::*;
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

    #[test]
    fn callback_effect_entrypoints_require_the_prebuilt_native_pool() {
        let _: fn(&mut NativeEventPool, ReplayBatch) -> Option<PreparedReplay> = prepare_replay;
        let _: fn(&mut NativeEventPool, usize, &[DeferredEvent]) -> Option<PreparedDeferredEvents> =
            prepare_deferred_events;
        let _: fn(&mut NativeEventPool, CleanupBatch) -> Option<PreparedReplay> = prepare_cleanup;
        let _: fn(&mut NativeEventPool) -> Option<OperationToken> = post_gap_barrier;
        let _: fn(&mut NativeEventPool) -> Option<OperationToken> = post_paste_barrier;
        assert_eq!(
            NATIVE_EVENT_POOL_CAPACITY,
            JOURNAL_CAPACITY
                + BARRIER_EVENT_COUNT
                + PASTE_BARRIER_EVENT_COUNT
                + DEFERRED_EDGE_CAPACITY * DEFERRED_POOL_BANKS
        );
    }

    #[test]
    fn native_pool_ranges_are_fixed_disjoint_and_cover_maximum_batch_plus_pairs() {
        assert_eq!(REPLAY_POOL_START, 0);
        assert_eq!(BARRIER_POOL_START, JOURNAL_CAPACITY);
        assert_eq!(
            PASTE_BARRIER_POOL_START,
            JOURNAL_CAPACITY + BARRIER_EVENT_COUNT
        );
        assert_eq!(
            DEFERRED_POOL_START,
            JOURNAL_CAPACITY + BARRIER_EVENT_COUNT + PASTE_BARRIER_EVENT_COUNT
        );
        let deferred_tail_pool_start = DEFERRED_POOL_START + DEFERRED_EDGE_CAPACITY;
        assert_eq!(
            deferred_tail_pool_start + DEFERRED_EDGE_CAPACITY,
            NATIVE_EVENT_POOL_CAPACITY
        );
        assert_eq!(
            NATIVE_EVENT_POOL_CAPACITY,
            JOURNAL_CAPACITY
                + BARRIER_EVENT_COUNT
                + PASTE_BARRIER_EVENT_COUNT
                + DEFERRED_EDGE_CAPACITY * DEFERRED_POOL_BANKS
        );
        const {
            assert!(
                DEFERRED_POOL_START + DEFERRED_EDGE_CAPACITY * DEFERRED_POOL_BANKS
                    == NATIVE_EVENT_POOL_CAPACITY
            )
        };
    }

    #[test]
    fn mach_ticks_are_converted_to_cgevent_nanoseconds_without_losing_the_abi_width() {
        assert_eq!(mach_ticks_to_event_timestamp(5, 3, 2), Some(7));
        assert_eq!(
            mach_ticks_to_event_timestamp(u64::MAX, 1, 1),
            Some(u64::MAX)
        );
        assert_eq!(mach_ticks_to_event_timestamp(1, 0, 1), None);
        assert_eq!(mach_ticks_to_event_timestamp(1, 1, 0), None);
        assert_eq!(mach_ticks_to_event_timestamp(u64::MAX, u32::MAX, 1), None);
    }

    #[test]
    fn every_reused_batch_and_pair_post_refreshes_timestamp_immediately_and_in_order() {
        let replay = [
            REPLAY_POOL_START,
            REPLAY_POOL_START + 1,
            REPLAY_POOL_START + 2,
        ];
        let cleanup_reusing_replay_slots = [REPLAY_POOL_START, REPLAY_POOL_START + 1];
        let barrier = [BARRIER_POOL_START, BARRIER_POOL_START + 1];
        let raw_times = RefCell::new([100_u64, 99, 101, 98, 102, 102, 101].into_iter());
        let operations = RefCell::new(Vec::new());
        let mut last_post_timestamp = 0;

        for events in [
            replay.as_slice(),
            cleanup_reusing_replay_slots.as_slice(),
            barrier.as_slice(),
        ] {
            post_events_with_fresh_timestamps(
                events,
                &mut last_post_timestamp,
                || raw_times.borrow_mut().next().expect("one time per post"),
                |event, timestamp| {
                    operations
                        .borrow_mut()
                        .push(PostOperation::SetTimestamp(event, timestamp));
                },
                |event| operations.borrow_mut().push(PostOperation::Post(event)),
            );
        }

        assert_eq!(
            operations.into_inner(),
            vec![
                PostOperation::SetTimestamp(REPLAY_POOL_START, 100),
                PostOperation::Post(REPLAY_POOL_START),
                PostOperation::SetTimestamp(REPLAY_POOL_START + 1, 100),
                PostOperation::Post(REPLAY_POOL_START + 1),
                PostOperation::SetTimestamp(REPLAY_POOL_START + 2, 101),
                PostOperation::Post(REPLAY_POOL_START + 2),
                PostOperation::SetTimestamp(REPLAY_POOL_START, 101),
                PostOperation::Post(REPLAY_POOL_START),
                PostOperation::SetTimestamp(REPLAY_POOL_START + 1, 102),
                PostOperation::Post(REPLAY_POOL_START + 1),
                PostOperation::SetTimestamp(BARRIER_POOL_START, 102),
                PostOperation::Post(BARRIER_POOL_START),
                PostOperation::SetTimestamp(BARRIER_POOL_START + 1, 102),
                PostOperation::Post(BARRIER_POOL_START + 1),
            ]
        );
        assert_eq!(last_post_timestamp, 102);
        assert!(raw_times.borrow_mut().next().is_none());
    }

    #[test]
    fn cgevent_timestamp_ffi_round_trips_exact_uint64_value() {
        // SAFETY: the event is created and released exactly once in this test;
        // setting/getting its scalar timestamp does not post or require input
        // monitoring/accessibility permission.
        let event = unsafe { ffi::CGEventCreateKeyboardEvent(null(), 127, false) };
        assert!(!event.is_null());
        let expected = 0x0123_4567_89AB_CDEF;
        let actual = unsafe {
            ffi::CGEventSetTimestamp(event, expected);
            let actual = ffi::CGEventGetTimestamp(event);
            ffi::CFRelease(event.cast_const());
            actual
        };
        assert_eq!(actual, expected);
    }

    #[test]
    fn full_pool_initialization_creates_exactly_the_fixed_startup_capacity() {
        let next = RefCell::new(0_usize);
        let events = initialize_event_refs(
            || {
                let mut next = next.borrow_mut();
                *next += 1;
                *next as *mut c_void
            },
            |_| panic!("successful initialization must not release early"),
        )
        .unwrap();
        assert_eq!(*next.borrow(), NATIVE_EVENT_POOL_CAPACITY);
        assert!(events.iter().all(|event| !event.is_null()));
    }

    #[test]
    fn partial_pool_initialization_releases_every_created_prefix_entry() {
        let next = RefCell::new(0_usize);
        let released = RefCell::new(Vec::new());
        let result = initialize_event_refs(
            || {
                let mut next = next.borrow_mut();
                let index = *next;
                *next += 1;
                if index == 5 {
                    null_mut()
                } else {
                    (index + 1) as *mut c_void
                }
            },
            |event| released.borrow_mut().push(event as usize),
        );
        assert_eq!(result.unwrap_err(), 5);
        assert_eq!(&*released.borrow(), &[1, 2, 3, 4, 5]);
    }

    #[test]
    fn operation_tokens_require_own_pid_and_reject_delayed_aba_sequences() {
        let identity = identity();
        let old = OperationToken::for_test(1);
        let current = OperationToken::for_test(2);
        assert_ne!(old, current);
        assert!(token_matches(
            identity,
            Some(current),
            current.marker,
            identity.source_pid
        ));
        assert!(!token_matches(
            identity,
            Some(current),
            old.marker,
            identity.source_pid
        ));
        assert!(!token_matches(identity, Some(current), current.marker, 99));
        assert!(!token_matches(
            identity,
            None,
            current.marker,
            identity.source_pid
        ));
        assert_eq!(unmarked_source(identity, 42), InputSource::External);
        assert_eq!(unmarked_source(identity, 0), InputSource::Physical);
    }

    #[test]
    fn replay_shape_rejects_forged_type_keycode_and_repeat_state() {
        assert!(replay_shape_is_valid(ffi::K_CG_EVENT_KEY_DOWN, 9, false));
        assert!(replay_shape_is_valid(ffi::K_CG_EVENT_KEY_DOWN, 9, true));
        assert!(replay_shape_is_valid(ffi::K_CG_EVENT_KEY_UP, 9, false));
        assert!(!replay_shape_is_valid(ffi::K_CG_EVENT_KEY_UP, 9, true));
        assert!(!replay_shape_is_valid(99, 9, false));
        assert!(!replay_shape_is_valid(ffi::K_CG_EVENT_KEY_DOWN, -1, false));
        assert!(!replay_shape_is_valid(ffi::K_CG_EVENT_KEY_DOWN, 128, false));
    }

    #[test]
    fn replay_descriptor_preserves_key_phase_repeat_flags_and_marker() {
        let marker = OperationToken::for_test(7).marker;
        let down = replay_descriptor(record(PhysicalPhase::Down, 0x12_3400), marker);
        assert_eq!(down.key_code, 9);
        assert!(down.key_down);
        assert!(!down.repeat);
        assert_eq!(down.flags, 0x12_3400);
        assert_eq!(down.marker, marker);
        assert_eq!(
            event_mutation(down),
            EventMutation {
                event_type: ffi::K_CG_EVENT_KEY_DOWN,
                key_code: 9,
                repeat: 0,
                flags: 0x12_3400,
                marker,
            }
        );

        let repeat = replay_descriptor(record(PhysicalPhase::Repeat, 7), marker);
        assert_eq!(repeat.key_code, 9);
        assert!(repeat.key_down);
        assert!(repeat.repeat);
        assert_eq!(repeat.flags, 7);
        assert_eq!(repeat.marker, marker);
        assert_eq!(event_mutation(repeat).event_type, ffi::K_CG_EVENT_KEY_DOWN);
        assert_eq!(event_mutation(repeat).repeat, 1);

        let up = replay_descriptor(record(PhysicalPhase::Up, 9), marker);
        assert_eq!(up.key_code, 9);
        assert!(!up.key_down);
        assert!(!up.repeat);
        assert_eq!(up.flags, 9);
        assert_eq!(up.marker, marker);
        assert_eq!(event_mutation(up).event_type, ffi::K_CG_EVENT_KEY_UP);
        assert_eq!(event_mutation(up).repeat, 0);

        let modifier = ReplayRecord {
            key: KeyIdentity::Modifier(
                talking_quill_keyboard_core::transactional::ModifierSide::LeftAlt,
            ),
            native: NativeKey {
                virtual_key: 58,
                platform_flags: ffi::K_CG_EVENT_FLAG_MASK_ALTERNATE,
                ..NativeKey::default()
            },
            phase: PhysicalPhase::Up,
            observed_at_ms: 18,
        };
        assert_eq!(
            replay_descriptor(modifier, marker).event_type,
            ffi::K_CG_EVENT_FLAGS_CHANGED
        );
    }

    #[test]
    fn gap_barrier_is_an_ordered_operation_tagged_dummy_pair() {
        let token = OperationToken::for_test(8);
        let pair = gap_barrier_descriptors(token);
        assert!(pair[0].key_down);
        assert!(!pair[1].key_down);
        for event in pair {
            assert_eq!(event.key_code, 127);
            assert_eq!(event.marker, token.marker);
            assert_eq!(event.flags, 0);
            assert!(!event.repeat);
        }
    }
}
