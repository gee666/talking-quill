use std::{
    ffi::{CStr, c_char, c_int, c_void},
    ptr::{null, null_mut},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicU64, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
};

use crossbeam_channel::{Sender, bounded};

use super::{
    CallbackGate, FrontApp, HookStatus, PasteFailure, PasteResult, PermissionState, Permissions,
    Platform, PlatformError, TapRecoveryDecision, TapRecoveryEvent, TapRecoveryPolicy,
    TerminalReason, TerminalSignal, WindowBounds, deliver_callback_event_with_session_arm,
    secure_input_paste_result,
};
use crate::{
    keyboard::{
        ActivationBindings, ActivationKey, KeyInput, KeyPhase, KeyboardReducer, PhysicalKey,
    },
    protocol::Outbound,
};

type CFIndex = isize;
type CFTypeId = usize;
type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFRunLoopRef = *mut c_void;
type CFRunLoopSourceRef = *mut c_void;
type CFMachPortRef = *mut c_void;
type CGEventRef = *mut c_void;
type CGEventTapProxy = *mut c_void;
type AXUIElementRef = *mut c_void;

const K_CG_SESSION_EVENT_TAP: u32 = 1;
const K_CG_HID_EVENT_TAP: u32 = 0;
const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 0;
const K_CG_EVENT_TAP_OPTION_DEFAULT: u32 = 0;
const K_CG_EVENT_KEY_DOWN: u32 = 10;
const K_CG_EVENT_KEY_UP: u32 = 11;
const K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;
const K_CG_KEYBOARD_EVENT_AUTOREPEAT: u32 = 8;
const K_CG_KEYBOARD_EVENT_KEYCODE: u32 = 9;
const K_CG_EVENT_SOURCE_USER_DATA: u32 = 42;
const K_CG_EVENT_FLAG_MASK_SHIFT: u64 = 0x0002_0000;
const K_CG_EVENT_FLAG_MASK_CONTROL: u64 = 0x0004_0000;
const K_CG_EVENT_FLAG_MASK_ALTERNATE: u64 = 0x0008_0000;
const K_CG_EVENT_FLAG_MASK_COMMAND: u64 = 0x0010_0000;
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const INJECTED_MARKER: i64 = 0x4D45_4348_4F50_5354;
const MAX_AX_STRING_BYTES: usize = 4096;
const AX_FOCUSED_APPLICATION: &CStr = c"AXFocusedApplication";
const AX_FOCUSED_WINDOW: &CStr = c"AXFocusedWindow";
const AX_TITLE: &CStr = c"AXTitle";
const AX_POSITION: &CStr = c"AXPosition";
const AX_SIZE: &CStr = c"AXSize";
const K_AX_VALUE_CGPOINT_TYPE: u32 = 1;
const K_AX_VALUE_CGSIZE_TYPE: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGSize {
    width: f64,
    height: f64,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopCommonModes: CFStringRef;
    fn CFRelease(value: CFTypeRef);
    fn CFGetTypeID(value: CFTypeRef) -> CFTypeId;
    fn CFMachPortCreateRunLoopSource(
        allocator: CFTypeRef,
        port: CFMachPortRef,
        order: CFIndex,
    ) -> CFRunLoopSourceRef;
    fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    fn CFRunLoopAddSource(run_loop: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFRunLoopRemoveSource(run_loop: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
    fn CFMachPortInvalidate(port: CFMachPortRef);
    fn CFRunLoopRun();
    fn CFRunLoopStop(run_loop: CFRunLoopRef);
    fn CFStringCreateWithCString(
        allocator: CFTypeRef,
        string: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    fn CFStringGetTypeID() -> CFTypeId;
    fn CFStringGetLength(string: CFStringRef) -> CFIndex;
    fn CFStringGetMaximumSizeForEncoding(length: CFIndex, encoding: u32) -> CFIndex;
    fn CFStringGetCString(
        string: CFStringRef,
        buffer: *mut c_char,
        buffer_size: CFIndex,
        encoding: u32,
    ) -> u8;
}

#[link(name = "proc")]
unsafe extern "C" {
    fn proc_name(pid: c_int, buffer: *mut c_void, buffer_size: u32) -> c_int;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapCreate(
        tap: u32,
        place: u32,
        options: u32,
        events_of_interest: u64,
        callback: Option<
            unsafe extern "C" fn(
                proxy: CGEventTapProxy,
                event_type: u32,
                event: CGEventRef,
                user_info: *mut c_void,
            ) -> CGEventRef,
        >,
        user_info: *mut c_void,
    ) -> CFMachPortRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    fn CGEventTapIsEnabled(tap: CFMachPortRef) -> bool;
    fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    fn CGEventGetFlags(event: CGEventRef) -> u64;
    fn CGEventCreateKeyboardEvent(
        source: CFTypeRef,
        virtual_key: u16,
        key_down: bool,
    ) -> CGEventRef;
    fn CGEventSetFlags(event: CGEventRef, flags: u64);
    fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
    fn CGEventPost(tap: u32, event: CGEventRef);
    fn CGPreflightListenEventAccess() -> bool;
    fn CGPreflightPostEventAccess() -> bool;
}

// IsSecureEventInputEnabled is declared by HIToolbox/Events.h. Carbon is the
// public umbrella framework that links the HIToolbox subframework symbol.
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn IsSecureEventInputEnabled() -> u8;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> c_int;
    fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut c_int) -> c_int;
    fn AXValueGetType(value: CFTypeRef) -> u32;
    fn AXValueGetValue(value: CFTypeRef, value_type: u32, output: *mut c_void) -> u8;
}

/// Owns one Core Foundation object returned by a Create/Copy function.
struct OwnedCf(CFTypeRef);

impl OwnedCf {
    fn from_created(value: CFTypeRef) -> Result<Self, PlatformError> {
        if value.is_null() {
            Err(PlatformError::NativeFailure)
        } else {
            Ok(Self(value))
        }
    }

    const fn as_type_ref(&self) -> CFTypeRef {
        self.0
    }

    fn as_ax_element(&self) -> AXUIElementRef {
        self.0.cast_mut()
    }

    fn as_string_ref(&self) -> CFStringRef {
        self.0
    }
}

impl Drop for OwnedCf {
    fn drop(&mut self) {
        // SAFETY: this wrapper is created only from Create/Copy-rule values and
        // releases its non-null reference exactly once.
        unsafe { CFRelease(self.0) };
    }
}

struct AxAttributes {
    focused_application: OwnedCf,
    focused_window: OwnedCf,
    title: OwnedCf,
    position: OwnedCf,
    size: OwnedCf,
}

impl AxAttributes {
    fn new() -> Result<Self, PlatformError> {
        Ok(Self {
            focused_application: create_cf_string(AX_FOCUSED_APPLICATION)?,
            focused_window: create_cf_string(AX_FOCUSED_WINDOW)?,
            title: create_cf_string(AX_TITLE)?,
            position: create_cf_string(AX_POSITION)?,
            size: create_cf_string(AX_SIZE)?,
        })
    }
}

struct SharedState {
    activation_config: AtomicU64,
    session_capture: AtomicBool,
    hook_status: AtomicU8,
    run_loop: AtomicUsize,
    event_tap: AtomicPtr<c_void>,
    tap_recovery: AtomicU8,
    stopping: AtomicBool,
}

impl SharedState {
    fn new() -> Self {
        Self {
            activation_config: AtomicU64::new(activation_config_value(
                false,
                ActivationBindings::default(),
            )),
            session_capture: AtomicBool::new(false),
            hook_status: AtomicU8::new(hook_status_to_u8(HookStatus::Unavailable)),
            run_loop: AtomicUsize::new(0),
            event_tap: AtomicPtr::new(null_mut()),
            tap_recovery: AtomicU8::new(0),
            stopping: AtomicBool::new(false),
        }
    }
}

struct CallbackContext {
    state: Arc<SharedState>,
    reducer: Mutex<KeyboardReducer>,
    outbound: Sender<Outbound>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
}

pub struct NativePlatform {
    state: Arc<SharedState>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
    thread: Option<JoinHandle<()>>,
}

impl Platform for NativePlatform {
    fn start(
        outbound: Sender<Outbound>,
        gate: Arc<CallbackGate>,
        terminal: Arc<TerminalSignal>,
    ) -> Result<Self, PlatformError> {
        let state = Arc::new(SharedState::new());
        let context = CallbackContext {
            state: Arc::clone(&state),
            reducer: Mutex::new(KeyboardReducer::default()),
            outbound,
            gate: Arc::clone(&gate),
            terminal: Arc::clone(&terminal),
        };
        let (ready_tx, ready_rx) = bounded(1);
        let thread = thread::Builder::new()
            .name("talking-quill-helper-macos-hook".into())
            .spawn(move || hook_thread(context, ready_tx))
            .map_err(|_| PlatformError::ThreadStopped)?;
        if ready_rx.recv().is_err() {
            let _ = thread.join();
            return Err(PlatformError::ThreadStopped);
        }
        Ok(Self {
            state,
            gate,
            terminal,
            thread: Some(thread),
        })
    }

    fn hook_status(&self) -> HookStatus {
        if self.terminal.is_triggered() {
            HookStatus::Unavailable
        } else {
            hook_status_from_u8(self.state.hook_status.load(Ordering::Acquire))
        }
    }

    fn configure_activation(
        &self,
        enabled: bool,
        bindings: ActivationBindings,
    ) -> Result<(), PlatformError> {
        self.state.activation_config.store(
            activation_config_value(enabled, bindings),
            Ordering::Release,
        );
        Ok(())
    }

    fn set_session_capture(&self, active: bool) -> Result<(), PlatformError> {
        self.state.session_capture.store(active, Ordering::Release);
        Ok(())
    }

    fn inject_paste(&self) -> PasteResult {
        inject_paste()
    }

    fn front_app(&self) -> Result<FrontApp, PlatformError> {
        front_app()
    }

    fn permissions(&self) -> Permissions {
        permission_snapshot()
    }

    fn shutdown(&mut self) {
        self.gate.close();
        self.state.stopping.store(true, Ordering::Release);
        self.state.session_capture.store(false, Ordering::Release);
        let run_loop = self.state.run_loop.load(Ordering::Acquire) as CFRunLoopRef;
        if !run_loop.is_null() {
            // SAFETY: CFRunLoopStop is thread-safe and the pointer remains owned
            // by the hook thread until that thread has joined.
            unsafe { CFRunLoopStop(run_loop) };
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.state
            .hook_status
            .store(hook_status_to_u8(HookStatus::Stopped), Ordering::Release);
    }
}

impl Drop for NativePlatform {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn hook_thread(context: CallbackContext, ready: Sender<()>) {
    let mut context = Box::new(context);
    let context_ptr = (&raw mut *context).cast::<c_void>();
    let mask = (1_u64 << K_CG_EVENT_KEY_DOWN) | (1_u64 << K_CG_EVENT_KEY_UP);
    // SAFETY: callback context remains boxed until the event tap and run-loop
    // source are disabled and released below.
    let tap = unsafe {
        CGEventTapCreate(
            K_CG_SESSION_EVENT_TAP,
            K_CG_HEAD_INSERT_EVENT_TAP,
            K_CG_EVENT_TAP_OPTION_DEFAULT,
            mask,
            Some(event_tap_callback),
            context_ptr,
        )
    };
    if tap.is_null() {
        let permissions = permission_snapshot();
        let status = if permissions.input_monitoring == PermissionState::Denied
            || permissions.accessibility == PermissionState::Denied
        {
            HookStatus::PermissionRequired
        } else {
            HookStatus::Unavailable
        };
        context
            .state
            .hook_status
            .store(hook_status_to_u8(status), Ordering::Release);
        let _ = ready.send(());
        return;
    }
    context.state.event_tap.store(tap, Ordering::Release);

    // SAFETY: `tap` is a valid CFMachPort returned above.
    let source = unsafe { CFMachPortCreateRunLoopSource(null(), tap, 0) };
    if source.is_null() {
        // SAFETY: `tap` is an owned CFMachPort. Invalidating before release
        // guarantees it cannot schedule callbacks with the boxed context.
        unsafe {
            CGEventTapEnable(tap, false);
            CFMachPortInvalidate(tap);
            CFRelease(tap.cast_const());
        }
        context.state.event_tap.store(null_mut(), Ordering::Release);
        let _ = ready.send(());
        return;
    }

    // SAFETY: called on the run-loop owner thread.
    let run_loop = unsafe { CFRunLoopGetCurrent() };
    context
        .state
        .run_loop
        .store(run_loop as usize, Ordering::Release);
    context
        .state
        .hook_status
        .store(hook_status_to_u8(HookStatus::Ready), Ordering::Release);
    // SAFETY: all CF objects are valid and remain alive through CFRunLoopRun.
    unsafe {
        CFRunLoopAddSource(run_loop, source, kCFRunLoopCommonModes);
        CGEventTapEnable(tap, true);
    }
    if ready.send(()).is_err() {
        context.gate.close();
    } else {
        // SAFETY: runs until shutdown calls CFRunLoopStop.
        unsafe { CFRunLoopRun() };
    }

    context.gate.close();
    context.state.run_loop.store(0, Ordering::Release);
    context.state.event_tap.store(null_mut(), Ordering::Release);
    context
        .state
        .hook_status
        .store(hook_status_to_u8(HookStatus::Stopped), Ordering::Release);
    if !context.state.stopping.load(Ordering::Acquire) {
        context.terminal.trigger(TerminalReason::HookStopped);
    }
    // SAFETY: source and tap are owned by this function. Disabling,
    // unregistering, and invalidating them prevents callbacks before the boxed
    // callback context is dropped.
    unsafe {
        CGEventTapEnable(tap, false);
        CFRunLoopRemoveSource(run_loop, source, kCFRunLoopCommonModes);
        CFMachPortInvalidate(tap);
        CFRelease(source.cast_const());
        CFRelease(tap.cast_const());
    }
}

fn apply_tap_recovery(context: &CallbackContext, event: TapRecoveryEvent) {
    let policy = TapRecoveryPolicy::from_consecutive_timeouts(
        context.state.tap_recovery.load(Ordering::Acquire),
    );
    let (next, decision) = policy.observe(event);
    context
        .state
        .tap_recovery
        .store(next.consecutive_timeouts(), Ordering::Release);
    if let TapRecoveryDecision::Terminal(reason) = decision {
        context.state.hook_status.store(
            hook_status_to_u8(HookStatus::Unavailable),
            Ordering::Release,
        );
        context.terminal.trigger(reason);
    }
}

unsafe extern "C" fn event_tap_callback(
    _proxy: CGEventTapProxy,
    event_type: u32,
    event: CGEventRef,
    user_info: *mut c_void,
) -> CGEventRef {
    let handled = std::panic::catch_unwind(|| {
        if user_info.is_null() {
            return false;
        }
        // SAFETY: user_info points to the boxed context retained for the event
        // tap's full lifetime.
        let context = unsafe { &*user_info.cast::<CallbackContext>() };
        if event_type == K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT {
            apply_tap_recovery(context, TapRecoveryEvent::DisabledByUserInput);
            return false;
        }
        if event_type == K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT {
            if !context.gate.is_open() {
                return false;
            }
            let tap = context.state.event_tap.load(Ordering::Acquire);
            let recovered = if tap.is_null() {
                false
            } else {
                // Exactly one nonblocking re-enable attempt is allowed. Core
                // Graphics confirmation, not the void enable call, determines
                // whether recovery succeeded.
                // SAFETY: tap is owned by the active hook thread.
                unsafe {
                    CGEventTapEnable(tap, true);
                    CGEventTapIsEnabled(tap)
                }
            };
            apply_tap_recovery(
                context,
                if recovered {
                    TapRecoveryEvent::TimeoutRecovered
                } else {
                    TapRecoveryEvent::TimeoutRecoveryFailed
                },
            );
            return false;
        }
        if event.is_null() || !context.gate.is_open() {
            return false;
        }
        let phase = match event_type {
            K_CG_EVENT_KEY_DOWN => KeyPhase::Down,
            K_CG_EVENT_KEY_UP => KeyPhase::Up,
            _ => return false,
        };
        apply_tap_recovery(context, TapRecoveryEvent::Activity);
        // SAFETY: Core Graphics guarantees the event for this callback.
        let key_code = unsafe { CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_KEYCODE) };
        let repeat =
            unsafe { CGEventGetIntegerValueField(event, K_CG_KEYBOARD_EVENT_AUTOREPEAT) != 0 };
        let marker = unsafe { CGEventGetIntegerValueField(event, K_CG_EVENT_SOURCE_USER_DATA) };
        let flags = unsafe { CGEventGetFlags(event) };
        // Option/Shift are allowed activation context; Control/Command disallow
        // activation. Modifier flag-change events are not in the tap mask and
        // remain untouched; only the configured letter sequence may be swallowed.
        let input = KeyInput {
            key: map_key_code(key_code),
            phase,
            alt: flags & K_CG_EVENT_FLAG_MASK_ALTERNATE != 0,
            shift: flags & K_CG_EVENT_FLAG_MASK_SHIFT != 0,
            disallowed_modifiers: activation_disallowed_modifiers(flags),
            repeat,
            injected: marker == INJECTED_MARKER,
        };
        let (activation_enabled, configured) =
            activation_config_from_value(context.state.activation_config.load(Ordering::Acquire));
        let capture = context.state.session_capture.load(Ordering::Acquire);
        let mut reducer = match context.reducer.try_lock() {
            Ok(reducer) => reducer,
            Err(_) => {
                context.state.hook_status.store(
                    hook_status_to_u8(HookStatus::Unavailable),
                    Ordering::Release,
                );
                context.terminal.trigger(TerminalReason::ReducerPoisoned);
                return false;
            }
        };
        let plan = reducer.plan_bindings(input, configured, activation_enabled, capture);
        let delivered = plan.event().is_none()
            || deliver_callback_event_with_session_arm(
                &context.outbound,
                &context.terminal,
                &context.state.session_capture,
                plan.event().expect("event checked above"),
            );
        if !delivered {
            context.state.hook_status.store(
                hook_status_to_u8(HookStatus::Unavailable),
                Ordering::Release,
            );
        }
        reducer.apply(plan, delivered)
    });

    match handled {
        Ok(true) => null_mut(),
        Ok(false) => event,
        Err(_) => {
            if !user_info.is_null() {
                // SAFETY: the context remains alive until the event tap is
                // disabled and invalidated on its owner thread.
                let context = unsafe { &*user_info.cast::<CallbackContext>() };
                context.state.hook_status.store(
                    hook_status_to_u8(HookStatus::Unavailable),
                    Ordering::Release,
                );
                context.terminal.trigger(TerminalReason::CallbackPanicked);
            }
            event
        }
    }
}

fn inject_paste() -> PasteResult {
    let permissions = permission_snapshot();
    if permissions.event_post != PermissionState::Granted {
        return PasteResult {
            submitted: false,
            reason: Some(PasteFailure::PermissionDenied),
        };
    }

    // SAFETY: this HIToolbox probe has no pointer arguments or ownership.
    if let Some(result) = secure_input_paste_result(unsafe { IsSecureEventInputEnabled() != 0 }) {
        return result;
    }

    // Physical V key on the macOS virtual-key map.
    // SAFETY: null event source requests the system source and key code 9 is V.
    let down = unsafe { CGEventCreateKeyboardEvent(null(), 9, true) };
    let up = unsafe { CGEventCreateKeyboardEvent(null(), 9, false) };
    if down.is_null() || up.is_null() {
        if !down.is_null() {
            // SAFETY: down is owned by this function.
            unsafe { CFRelease(down.cast_const()) };
        }
        if !up.is_null() {
            // SAFETY: up is owned by this function.
            unsafe { CFRelease(up.cast_const()) };
        }
        return PasteResult {
            submitted: false,
            reason: Some(PasteFailure::Unavailable),
        };
    }

    // SAFETY: both event references are valid and owned until released below.
    unsafe {
        CGEventSetFlags(down, K_CG_EVENT_FLAG_MASK_COMMAND);
        CGEventSetFlags(up, K_CG_EVENT_FLAG_MASK_COMMAND);
        CGEventSetIntegerValueField(down, K_CG_EVENT_SOURCE_USER_DATA, INJECTED_MARKER);
        CGEventSetIntegerValueField(up, K_CG_EVENT_SOURCE_USER_DATA, INJECTED_MARKER);
        CGEventPost(K_CG_HID_EVENT_TAP, down);
        CGEventPost(K_CG_HID_EVENT_TAP, up);
        CFRelease(down.cast_const());
        CFRelease(up.cast_const());
    }
    PasteResult {
        submitted: true,
        reason: None,
    }
}

fn front_app() -> Result<FrontApp, PlatformError> {
    if permission_snapshot().accessibility != PermissionState::Granted {
        return Err(PlatformError::PermissionDenied);
    }
    let attributes = AxAttributes::new()?;
    // SAFETY: AXUIElementCreateSystemWide follows the Core Foundation Create
    // rule and returns an owned reference when non-null.
    let system = OwnedCf::from_created(unsafe { AXUIElementCreateSystemWide() }.cast_const())?;
    let application = ax_copy_attribute(
        system.as_ax_element(),
        attributes.focused_application.as_string_ref(),
    )?;
    let app_element = application.as_ax_element();

    let application_name =
        ax_string_attribute(app_element, attributes.title.as_string_ref()).unwrap_or_default();
    let mut pid = 0;
    // SAFETY: pid is writable and app_element is a valid retained AX object.
    if unsafe { AXUIElementGetPid(app_element, &raw mut pid) } != 0 || pid <= 0 {
        return Err(PlatformError::NativeFailure);
    }

    let process_name = process_name(pid).unwrap_or(application_name);
    if process_name.is_empty() {
        return Err(PlatformError::NativeFailure);
    }

    let focused_window =
        ax_copy_attribute(app_element, attributes.focused_window.as_string_ref()).ok();
    let window_title = focused_window
        .as_ref()
        .and_then(|window| {
            ax_string_attribute(window.as_ax_element(), attributes.title.as_string_ref()).ok()
        })
        .unwrap_or_default();
    let window_bounds = focused_window.as_ref().and_then(|window| {
        ax_window_bounds(
            window.as_ax_element(),
            attributes.position.as_string_ref(),
            attributes.size.as_string_ref(),
        )
    });

    Ok(FrontApp {
        process_name,
        window_title,
        window_bounds,
    })
}

fn ax_window_bounds(
    window: AXUIElementRef,
    position_attribute: CFStringRef,
    size_attribute: CFStringRef,
) -> Option<WindowBounds> {
    let position_value = ax_copy_attribute(window, position_attribute).ok()?;
    let size_value = ax_copy_attribute(window, size_attribute).ok()?;
    // SAFETY: both retained values came from the documented AXPosition/AXSize attributes.
    if unsafe { AXValueGetType(position_value.as_type_ref()) } != K_AX_VALUE_CGPOINT_TYPE
        || unsafe { AXValueGetType(size_value.as_type_ref()) } != K_AX_VALUE_CGSIZE_TYPE
    {
        return None;
    }
    let mut position = CGPoint::default();
    let mut size = CGSize::default();
    // SAFETY: output pointers match the requested AXValue types and remain writable for the calls.
    if unsafe {
        AXValueGetValue(
            position_value.as_type_ref(),
            K_AX_VALUE_CGPOINT_TYPE,
            (&raw mut position).cast(),
        )
    } == 0
        || unsafe {
            AXValueGetValue(
                size_value.as_type_ref(),
                K_AX_VALUE_CGSIZE_TYPE,
                (&raw mut size).cast(),
            )
        } == 0
        || !position.x.is_finite()
        || !position.y.is_finite()
        || !size.width.is_finite()
        || !size.height.is_finite()
        || size.width < 1.0
        || size.height < 1.0
    {
        return None;
    }
    Some(WindowBounds {
        x: position
            .x
            .round()
            .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32,
        y: position
            .y
            .round()
            .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32,
        width: size.width.round().clamp(1.0, f64::from(u32::MAX)) as u32,
        height: size.height.round().clamp(1.0, f64::from(u32::MAX)) as u32,
    })
}

fn process_name(pid: c_int) -> Option<String> {
    let mut bytes = vec![0_u8; 1024];
    // SAFETY: the buffer is writable for the exact supplied size and pid was
    // returned by Accessibility for the focused application.
    let copied = unsafe {
        proc_name(
            pid,
            bytes.as_mut_ptr().cast(),
            u32::try_from(bytes.len()).expect("fixed process-name buffer fits u32"),
        )
    };
    let copied = usize::try_from(copied).ok()?;
    if copied == 0 || copied > bytes.len() {
        return None;
    }
    String::from_utf8(bytes[..copied].to_vec()).ok()
}

fn create_cf_string(value: &CStr) -> Result<OwnedCf, PlatformError> {
    // SAFETY: value is NUL-terminated for the full call and UTF-8 is the
    // declared encoding. The returned Create-rule object is owned.
    let string =
        unsafe { CFStringCreateWithCString(null(), value.as_ptr(), K_CF_STRING_ENCODING_UTF8) };
    OwnedCf::from_created(string)
}

fn ax_copy_attribute(
    element: AXUIElementRef,
    attribute: CFStringRef,
) -> Result<OwnedCf, PlatformError> {
    let mut value: CFTypeRef = null();
    // SAFETY: element and attribute are valid retained references and value is
    // writable. CopyAttributeValue returns a retained value on success.
    let error = unsafe { AXUIElementCopyAttributeValue(element, attribute, &raw mut value) };
    if error != 0 {
        return Err(PlatformError::NativeFailure);
    }
    OwnedCf::from_created(value)
}

fn ax_string_attribute(
    element: AXUIElementRef,
    attribute: CFStringRef,
) -> Result<String, PlatformError> {
    let value = ax_copy_attribute(element, attribute)?;
    cf_string_to_string(value.as_type_ref())
}

fn cf_string_to_string(value: CFTypeRef) -> Result<String, PlatformError> {
    // SAFETY: both functions only inspect a non-null Core Foundation object.
    if unsafe { CFGetTypeID(value) } != unsafe { CFStringGetTypeID() } {
        return Err(PlatformError::NativeFailure);
    }
    let string = value.cast::<c_void>();
    // SAFETY: the type-ID check above proves this object is a CFString.
    let length = unsafe { CFStringGetLength(string) };
    let maximum = unsafe { CFStringGetMaximumSizeForEncoding(length, K_CF_STRING_ENCODING_UTF8) };
    let capacity = usize::try_from(maximum)
        .unwrap_or(MAX_AX_STRING_BYTES - 1)
        .saturating_add(1)
        .min(MAX_AX_STRING_BYTES);
    let mut bytes = vec![0_u8; capacity.max(1)];
    // SAFETY: bytes is writable for capacity and string is a CFString.
    let converted = unsafe {
        CFStringGetCString(
            string,
            bytes.as_mut_ptr().cast(),
            CFIndex::try_from(bytes.len()).map_err(|_| PlatformError::NativeFailure)?,
            K_CF_STRING_ENCODING_UTF8,
        )
    };
    if converted == 0 {
        return Err(PlatformError::NativeFailure);
    }
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8(bytes[..end].to_vec()).map_err(|_| PlatformError::NativeFailure)
}

fn permission_snapshot() -> Permissions {
    // SAFETY: all three functions are side-effect-free permission preflight
    // queries and explicitly do not trigger system prompts.
    unsafe {
        Permissions {
            accessibility: permission_state(AXIsProcessTrusted() != 0),
            input_monitoring: permission_state(CGPreflightListenEventAccess()),
            event_post: permission_state(CGPreflightPostEventAccess()),
        }
    }
}

const fn permission_state(granted: bool) -> PermissionState {
    if granted {
        PermissionState::Granted
    } else {
        PermissionState::Denied
    }
}

const ACTIVATION_ENABLED_MASK: u64 = 1_u64 << 63;
const fn activation_config_value(enabled: bool, bindings: ActivationBindings) -> u64 {
    bindings.bits() | if enabled { ACTIVATION_ENABLED_MASK } else { 0 }
}

const fn activation_config_from_value(value: u64) -> (bool, ActivationBindings) {
    (
        value & ACTIVATION_ENABLED_MASK != 0,
        ActivationBindings::from_bits(value),
    )
}

const fn activation_disallowed_modifiers(flags: u64) -> bool {
    flags & (K_CG_EVENT_FLAG_MASK_CONTROL | K_CG_EVENT_FLAG_MASK_COMMAND) != 0
}

fn map_key_code(code: i64) -> PhysicalKey {
    let letter = match code {
        0 => Some(ActivationKey::A),
        11 => Some(ActivationKey::B),
        8 => Some(ActivationKey::C),
        2 => Some(ActivationKey::D),
        14 => Some(ActivationKey::E),
        3 => Some(ActivationKey::F),
        5 => Some(ActivationKey::G),
        4 => Some(ActivationKey::H),
        34 => Some(ActivationKey::I),
        38 => Some(ActivationKey::J),
        40 => Some(ActivationKey::K),
        37 => Some(ActivationKey::L),
        46 => Some(ActivationKey::M),
        45 => Some(ActivationKey::N),
        31 => Some(ActivationKey::O),
        35 => Some(ActivationKey::P),
        12 => Some(ActivationKey::Q),
        15 => Some(ActivationKey::R),
        1 => Some(ActivationKey::S),
        17 => Some(ActivationKey::T),
        32 => Some(ActivationKey::U),
        9 => Some(ActivationKey::V),
        13 => Some(ActivationKey::W),
        7 => Some(ActivationKey::X),
        16 => Some(ActivationKey::Y),
        6 => Some(ActivationKey::Z),
        _ => None,
    };
    if let Some(letter) = letter {
        PhysicalKey::Letter(letter)
    } else if code == 53 {
        PhysicalKey::Escape
    } else if code == 36 || code == 76 {
        PhysicalKey::Enter
    } else {
        PhysicalKey::Other
    }
}

const fn hook_status_to_u8(status: HookStatus) -> u8 {
    match status {
        HookStatus::Ready => 0,
        HookStatus::PermissionRequired => 1,
        HookStatus::Unavailable => 2,
        HookStatus::Stopped => 3,
    }
}

const fn hook_status_from_u8(value: u8) -> HookStatus {
    match value {
        0 => HookStatus::Ready,
        1 => HookStatus::PermissionRequired,
        3 => HookStatus::Stopped,
        _ => HookStatus::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessibility_attribute_names_match_the_header_macros() {
        assert_eq!(AX_FOCUSED_APPLICATION.to_bytes(), b"AXFocusedApplication");
        assert_eq!(AX_FOCUSED_WINDOW.to_bytes(), b"AXFocusedWindow");
        assert_eq!(AX_TITLE.to_bytes(), b"AXTitle");
    }

    #[test]
    fn activation_config_is_atomic_and_defaults_disabled() {
        let state = SharedState::new();
        assert_eq!(
            activation_config_from_value(state.activation_config.load(Ordering::Acquire)),
            (false, ActivationBindings::default())
        );
        let bindings =
            ActivationBindings::from_exact(&[(ActivationKey::B, false), (ActivationKey::Q, true)])
                .unwrap();
        state
            .activation_config
            .store(activation_config_value(true, bindings), Ordering::Release);
        assert_eq!(
            activation_config_from_value(state.activation_config.load(Ordering::Acquire)),
            (true, bindings)
        );
    }

    #[test]
    fn control_and_command_flags_exhaustively_disallow_activation() {
        for mask in 0_u8..16 {
            let mut flags = 0;
            if mask & 0b0001 != 0 {
                flags |= K_CG_EVENT_FLAG_MASK_SHIFT;
            }
            if mask & 0b0010 != 0 {
                flags |= K_CG_EVENT_FLAG_MASK_ALTERNATE;
            }
            if mask & 0b0100 != 0 {
                flags |= K_CG_EVENT_FLAG_MASK_CONTROL;
            }
            if mask & 0b1000 != 0 {
                flags |= K_CG_EVENT_FLAG_MASK_COMMAND;
            }
            assert_eq!(
                activation_disallowed_modifiers(flags),
                mask & 0b1100 != 0,
                "mask {mask:04b}"
            );
        }
    }

    #[test]
    fn mac_key_codes_map_to_supported_physical_keys() {
        assert_eq!(map_key_code(0), PhysicalKey::Letter(ActivationKey::A));
        assert_eq!(map_key_code(6), PhysicalKey::Letter(ActivationKey::Z));
        assert_eq!(map_key_code(53), PhysicalKey::Escape);
        assert_eq!(map_key_code(36), PhysicalKey::Enter);
        assert_eq!(map_key_code(76), PhysicalKey::Enter);
        assert_eq!(map_key_code(999), PhysicalKey::Other);
    }

    #[test]
    fn permission_booleans_have_explicit_wire_states() {
        assert_eq!(permission_state(true), PermissionState::Granted);
        assert_eq!(permission_state(false), PermissionState::Denied);
    }
}
