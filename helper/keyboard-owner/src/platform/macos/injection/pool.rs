//! Startup-owned event storage, timestamp refresh, posting, and release.

use super::*;

/// Owner-thread CGEvent storage allocated completely before the tap is enabled.
/// Replay/cleanup, gap barrier, paste, and both deferred rollover banks use
/// disjoint ranges, so an observed batch is never mutated by another effect.
pub(in crate::platform::macos) struct NativeEventPool {
    pub(super) events: [ffi::CGEventRef; NATIVE_EVENT_POOL_CAPACITY],
    pub(super) identity: InjectionIdentity,
    pub(super) timebase: ffi::MachTimebaseInfo,
    pub(super) last_post_timestamp: ffi::CGEventTimestamp,
    pub(super) next_operation_generation: u64,
}

// SAFETY: the pool is created, used, and released only by the hook owner. This
// marker permits the initially-empty CallbackContext field to cross into that
// owner thread before construction.
unsafe impl Send for NativeEventPool {}

impl NativeEventPool {
    pub(in crate::platform::macos) fn new(
        identity: InjectionIdentity,
    ) -> Result<Self, PlatformError> {
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

    pub(super) fn next_operation_token(&mut self) -> Option<OperationToken> {
        loop {
            let generation = self.next_operation_generation;
            self.next_operation_generation = generation.checked_add(1)?;
            let marker = self.identity.process_nonce ^ generation as i64;
            if marker != 0 {
                return Some(OperationToken { generation, marker });
            }
        }
    }

    pub(super) fn configure(&mut self, index: usize, descriptor: EventDescriptor) {
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

    pub(super) fn configure_deferred(
        &mut self,
        bank: usize,
        offset: usize,
        edge: DeferredEvent,
        marker: i64,
    ) {
        let event = self.events[DEFERRED_POOL_START + bank * DEFERRED_EDGE_CAPACITY + offset];
        // SAFETY: deferred preparation bounds the bank and offset before calling
        // here. The owner retains every event; snapshot fields are copied scalars
        // and no callback or other thread mutates this pool concurrently.
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

    pub(super) fn post_range(&mut self, start: usize, len: usize) {
        self.post_range_to(start, len, None);
    }

    pub(super) fn post_range_at_proxy(
        &mut self,
        start: usize,
        len: usize,
        proxy: ffi::CGEventTapProxy,
    ) {
        self.post_range_to(start, len, Some(proxy));
    }

    pub(super) fn post_range_to(
        &mut self,
        start: usize,
        len: usize,
        proxy: Option<ffi::CGEventTapProxy>,
    ) {
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

pub(super) fn event_timestamp_timebase() -> Result<ffi::MachTimebaseInfo, PlatformError> {
    let mut timebase = ffi::MachTimebaseInfo::default();
    // SAFETY: timebase points to valid writable storage. The timebase is read
    // once during owner startup, before the event tap can invoke callbacks.
    let status = unsafe { ffi::mach_timebase_info(&raw mut timebase) };
    if status != 0 || timebase.numer == 0 || timebase.denom == 0 {
        return Err(PlatformError::NativeFailure);
    }
    Ok(timebase)
}

pub(super) fn mach_ticks_to_event_timestamp(
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

pub(super) fn post_events_with_fresh_timestamps<T: Copy>(
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

pub(super) fn initialize_event_refs(
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
