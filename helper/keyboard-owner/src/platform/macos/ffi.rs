#![cfg_attr(all(test, feature = "local-unsigned-owner"), allow(dead_code))]

use std::ffi::{c_char, c_int, c_void};

pub(super) type CFIndex = isize;
pub(super) type CFTypeId = usize;
pub(super) type CFTypeRef = *const c_void;
pub(super) type CFStringRef = *const c_void;
pub(super) type CFRunLoopRef = *mut c_void;
pub(super) type CFRunLoopSourceRef = *mut c_void;
pub(super) type CFRunLoopTimerRef = *mut c_void;
pub(super) type CFMachPortRef = *mut c_void;
pub(super) type CGEventRef = *mut c_void;
pub(super) type CGEventTapProxy = *mut c_void;
pub(super) type AXUIElementRef = *mut c_void;
pub(super) type AXObserverRef = *mut c_void;
pub(super) type ObjcId = *mut c_void;
pub(super) type ObjcClass = *mut c_void;
pub(super) type ObjcSel = *mut c_void;
pub(super) type CGEventSourceStateId = i32;
// CoreGraphics defines CGEventTimestamp as uint64_t and specifies elapsed
// nanoseconds since startup. mach_absolute_time supplies the monotonic source;
// callers must apply mach_timebase_info before passing a value to CGEventSetTimestamp.
pub(super) type CGEventTimestamp = u64;

pub(super) const K_CG_EVENT_SOURCE_STATE_HID_SYSTEM: CGEventSourceStateId = 1;
pub(super) const K_CG_SESSION_EVENT_TAP: u32 = 1;
pub(super) const K_CG_HID_EVENT_TAP: u32 = 0;
pub(super) const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 0;
pub(super) const K_CG_EVENT_TAP_OPTION_DEFAULT: u32 = 0;
pub(super) const K_CG_EVENT_TAP_OPTION_LISTEN_ONLY: u32 = 1;
pub(super) const K_CG_EVENT_LEFT_MOUSE_DOWN: u32 = 1;
pub(super) const K_CG_EVENT_LEFT_MOUSE_UP: u32 = 2;
pub(super) const K_CG_EVENT_RIGHT_MOUSE_DOWN: u32 = 3;
pub(super) const K_CG_EVENT_RIGHT_MOUSE_UP: u32 = 4;
pub(super) const K_CG_EVENT_KEY_DOWN: u32 = 10;
pub(super) const K_CG_EVENT_KEY_UP: u32 = 11;
pub(super) const K_CG_EVENT_FLAGS_CHANGED: u32 = 12;
pub(super) const K_CG_EVENT_OTHER_MOUSE_DOWN: u32 = 25;
pub(super) const K_CG_EVENT_OTHER_MOUSE_UP: u32 = 26;
pub(super) const K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
pub(super) const K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;
pub(super) const K_CG_MOUSE_EVENT_NUMBER: u32 = 0;
pub(super) const K_CG_MOUSE_EVENT_CLICK_STATE: u32 = 1;
pub(super) const K_CG_MOUSE_EVENT_PRESSURE: u32 = 2;
pub(super) const K_CG_MOUSE_EVENT_BUTTON_NUMBER: u32 = 3;
pub(super) const K_CG_MOUSE_EVENT_DELTA_X: u32 = 4;
pub(super) const K_CG_MOUSE_EVENT_DELTA_Y: u32 = 5;
pub(super) const K_CG_MOUSE_EVENT_INSTANT_MOUSER: u32 = 6;
pub(super) const K_CG_MOUSE_EVENT_SUBTYPE: u32 = 7;
pub(super) const K_CG_KEYBOARD_EVENT_AUTOREPEAT: u32 = 8;
pub(super) const K_CG_KEYBOARD_EVENT_KEYCODE: u32 = 9;
pub(super) const K_CG_KEYBOARD_EVENT_KEYBOARD_TYPE: u32 = 10;
pub(super) const K_CG_EVENT_SOURCE_UNIX_PROCESS_ID: u32 = 41;
pub(super) const K_CG_EVENT_SOURCE_USER_DATA: u32 = 42;
pub(super) const K_CG_EVENT_FLAG_MASK_SHIFT: u64 = 0x0002_0000;
pub(super) const K_CG_EVENT_FLAG_MASK_CONTROL: u64 = 0x0004_0000;
pub(super) const K_CG_EVENT_FLAG_MASK_ALTERNATE: u64 = 0x0008_0000;
pub(super) const K_CG_EVENT_FLAG_MASK_COMMAND: u64 = 0x0010_0000;
pub(super) const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
pub(super) const K_AX_VALUE_CGPOINT_TYPE: u32 = 1;
pub(super) const K_AX_VALUE_CGSIZE_TYPE: u32 = 2;
pub(super) const K_AX_VALUE_CFRANGE_TYPE: u32 = 4;
pub(super) const K_AX_ERROR_SUCCESS: c_int = 0;
pub(super) const K_AX_ERROR_ILLEGAL_ARGUMENT: c_int = -25_201;
pub(super) const K_AX_ERROR_INVALID_UI_ELEMENT: c_int = -25_202;
pub(super) const K_AX_ERROR_CANNOT_COMPLETE: c_int = -25_204;
pub(super) const K_AX_ERROR_ATTRIBUTE_UNSUPPORTED: c_int = -25_205;
pub(super) const K_AX_ERROR_API_DISABLED: c_int = -25_211;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct CGPoint {
    pub(super) x: f64,
    pub(super) y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct CGSize {
    pub(super) width: f64,
    pub(super) height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct CFRange {
    pub(super) location: CFIndex,
    pub(super) length: CFIndex,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct MachTimebaseInfo {
    pub(super) numer: u32,
    pub(super) denom: u32,
}

#[repr(C)]
pub(super) struct CFRunLoopSourceContext {
    pub(super) version: CFIndex,
    pub(super) info: *mut c_void,
    pub(super) retain: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
    pub(super) release: Option<unsafe extern "C" fn(*const c_void)>,
    pub(super) copy_description: Option<unsafe extern "C" fn(*const c_void) -> CFStringRef>,
    pub(super) equal: Option<unsafe extern "C" fn(*const c_void, *const c_void) -> u8>,
    pub(super) hash: Option<unsafe extern "C" fn(*const c_void) -> usize>,
    pub(super) schedule: Option<unsafe extern "C" fn(*mut c_void, CFRunLoopRef, CFStringRef)>,
    pub(super) cancel: Option<unsafe extern "C" fn(*mut c_void, CFRunLoopRef, CFStringRef)>,
    pub(super) perform: Option<unsafe extern "C" fn(*mut c_void)>,
}

#[repr(C)]
pub(super) struct CFRunLoopTimerContext {
    pub(super) version: CFIndex,
    pub(super) info: *mut c_void,
    pub(super) retain: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
    pub(super) release: Option<unsafe extern "C" fn(*const c_void)>,
    pub(super) copy_description: Option<unsafe extern "C" fn(*const c_void) -> CFStringRef>,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub(super) static kCFRunLoopCommonModes: CFStringRef;
    pub(super) static kCFRunLoopDefaultMode: CFStringRef;
    pub(super) fn CFRetain(value: CFTypeRef) -> CFTypeRef;
    pub(super) fn CFRelease(value: CFTypeRef);
    pub(super) fn CFEqual(left: CFTypeRef, right: CFTypeRef) -> u8;
    pub(super) fn CFGetTypeID(value: CFTypeRef) -> CFTypeId;
    pub(super) fn CFMachPortCreateRunLoopSource(
        allocator: CFTypeRef,
        port: CFMachPortRef,
        order: CFIndex,
    ) -> CFRunLoopSourceRef;
    pub(super) fn CFRunLoopSourceCreate(
        allocator: CFTypeRef,
        order: CFIndex,
        context: *mut CFRunLoopSourceContext,
    ) -> CFRunLoopSourceRef;
    pub(super) fn CFRunLoopSourceSignal(source: CFRunLoopSourceRef);
    pub(super) fn CFRunLoopSourceInvalidate(source: CFRunLoopSourceRef);
    pub(super) fn CFRunLoopTimerCreate(
        allocator: CFTypeRef,
        fire_date: f64,
        interval: f64,
        flags: u64,
        order: CFIndex,
        callback: Option<unsafe extern "C" fn(CFRunLoopTimerRef, *mut c_void)>,
        context: *mut CFRunLoopTimerContext,
    ) -> CFRunLoopTimerRef;
    pub(super) fn CFRunLoopTimerSetNextFireDate(timer: CFRunLoopTimerRef, fire_date: f64);
    pub(super) fn CFRunLoopTimerInvalidate(timer: CFRunLoopTimerRef);
    pub(super) fn CFAbsoluteTimeGetCurrent() -> f64;
    pub(super) fn CFRunLoopGetCurrent() -> CFRunLoopRef;
    pub(super) fn CFRunLoopAddSource(
        run_loop: CFRunLoopRef,
        source: CFRunLoopSourceRef,
        mode: CFStringRef,
    );
    pub(super) fn CFRunLoopRemoveSource(
        run_loop: CFRunLoopRef,
        source: CFRunLoopSourceRef,
        mode: CFStringRef,
    );
    pub(super) fn CFRunLoopAddTimer(
        run_loop: CFRunLoopRef,
        timer: CFRunLoopTimerRef,
        mode: CFStringRef,
    );
    pub(super) fn CFRunLoopRemoveTimer(
        run_loop: CFRunLoopRef,
        timer: CFRunLoopTimerRef,
        mode: CFStringRef,
    );
    pub(super) fn CFMachPortInvalidate(port: CFMachPortRef);
    pub(super) fn CFRunLoopRun();
    pub(super) fn CFRunLoopRunInMode(
        mode: CFStringRef,
        seconds: f64,
        return_after_source_handled: bool,
    ) -> i32;
    pub(super) fn CFRunLoopStop(run_loop: CFRunLoopRef);
    pub(super) fn CFRunLoopWakeUp(run_loop: CFRunLoopRef);
    pub(super) fn CFStringCreateWithCString(
        allocator: CFTypeRef,
        string: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    pub(super) fn CFStringGetTypeID() -> CFTypeId;
    pub(super) fn CFStringGetLength(string: CFStringRef) -> CFIndex;
    pub(super) fn CFStringGetMaximumSizeForEncoding(length: CFIndex, encoding: u32) -> CFIndex;
    pub(super) fn CFStringGetBytes(
        string: CFStringRef,
        range: CFRange,
        encoding: u32,
        loss_byte: u8,
        external_representation: u8,
        buffer: *mut u8,
        maximum_buffer_length: CFIndex,
        used_buffer_length: *mut CFIndex,
    ) -> CFIndex;
}

#[link(name = "objc")]
unsafe extern "C" {
    pub(super) fn objc_getClass(name: *const c_char) -> ObjcClass;
    pub(super) fn objc_allocateClassPair(
        superclass: ObjcClass,
        name: *const c_char,
        extra_bytes: usize,
    ) -> ObjcClass;
    pub(super) fn objc_registerClassPair(class: ObjcClass);
    pub(super) fn sel_registerName(name: *const c_char) -> ObjcSel;
    pub(super) fn class_addMethod(
        class: ObjcClass,
        name: ObjcSel,
        implementation: *const c_void,
        types: *const c_char,
    ) -> bool;
    pub(super) fn objc_msgSend(receiver: ObjcId, operation: ObjcSel, ...) -> ObjcId;
    #[allow(clashing_extern_declarations)]
    #[link_name = "objc_msgSend"]
    pub(super) fn objc_msgSend_isize(receiver: ObjcId, operation: ObjcSel, ...) -> isize;
    pub(super) fn objc_autoreleasePoolPush() -> *mut c_void;
    pub(super) fn objc_autoreleasePoolPop(pool: *mut c_void);
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    pub(super) static NSWorkspaceDidActivateApplicationNotification: ObjcId;
}

#[link(name = "proc")]
unsafe extern "C" {
    pub(super) fn proc_name(pid: c_int, buffer: *mut c_void, buffer_size: u32) -> c_int;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub(super) fn CGEventTapCreate(
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
    pub(super) fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    pub(super) fn CGEventTapPostEvent(proxy: CGEventTapProxy, event: CGEventRef);
    pub(super) fn CGEventTapIsEnabled(tap: CFMachPortRef) -> bool;
    pub(super) fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    pub(super) fn CGEventGetDoubleValueField(event: CGEventRef, field: u32) -> f64;
    pub(super) fn CGEventGetFlags(event: CGEventRef) -> u64;
    pub(super) fn CGEventGetTimestamp(event: CGEventRef) -> CGEventTimestamp;
    pub(super) fn CGEventSourceKeyState(state_id: CGEventSourceStateId, virtual_key: u16) -> bool;
    pub(super) fn CGEventSourceButtonState(state_id: CGEventSourceStateId, button: u32) -> bool;
    pub(super) fn CGEventCreate(source: CFTypeRef) -> CGEventRef;
    pub(super) fn CGEventGetLocation(event: CGEventRef) -> CGPoint;
    #[cfg(any(feature = "transactional-shortcuts-dev", test))]
    pub(super) fn CGEventCreateKeyboardEvent(
        source: CFTypeRef,
        virtual_key: u16,
        key_down: bool,
    ) -> CGEventRef;
    #[cfg(any(feature = "transactional-shortcuts-dev", test))]
    pub(super) fn CGEventCreateMouseEvent(
        source: CFTypeRef,
        mouse_type: u32,
        mouse_cursor_position: CGPoint,
        mouse_button: u32,
    ) -> CGEventRef;
    pub(super) fn CGEventSetType(event: CGEventRef, event_type: u32);
    pub(super) fn CGEventSetTimestamp(event: CGEventRef, timestamp: CGEventTimestamp);
    pub(super) fn CGEventSetLocation(event: CGEventRef, location: CGPoint);
    pub(super) fn CGEventSetFlags(event: CGEventRef, flags: u64);
    pub(super) fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
    pub(super) fn CGEventSetDoubleValueField(event: CGEventRef, field: u32, value: f64);
    pub(super) fn CGEventPost(tap: u32, event: CGEventRef);
    pub(super) fn CGPreflightListenEventAccess() -> bool;
    pub(super) fn CGPreflightPostEventAccess() -> bool;
}

#[link(name = "System")]
unsafe extern "C" {
    pub(super) fn getpid() -> c_int;
    pub(super) fn mach_absolute_time() -> u64;
    pub(super) fn mach_timebase_info(info: *mut MachTimebaseInfo) -> c_int;
}

// IsSecureEventInputEnabled is declared by HIToolbox/Events.h. Carbon is the
// public umbrella framework that links the HIToolbox subframework symbol.
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    pub(super) fn IsSecureEventInputEnabled() -> u8;
    #[cfg(feature = "transactional-shortcuts-dev")]
    pub(super) fn EnableSecureEventInput() -> c_int;
    #[cfg(feature = "transactional-shortcuts-dev")]
    pub(super) fn DisableSecureEventInput() -> c_int;
}

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    pub(super) fn SecRandomCopyBytes(
        random: *mut c_void,
        count: usize,
        bytes: *mut c_void,
    ) -> c_int;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    pub(super) fn AXIsProcessTrusted() -> u8;
    pub(super) fn AXObserverCreate(
        application: c_int,
        callback: Option<
            unsafe extern "C" fn(
                observer: AXObserverRef,
                element: AXUIElementRef,
                notification: CFStringRef,
                refcon: *mut c_void,
            ),
        >,
        observer: *mut AXObserverRef,
    ) -> c_int;
    pub(super) fn AXObserverAddNotification(
        observer: AXObserverRef,
        element: AXUIElementRef,
        notification: CFStringRef,
        refcon: *mut c_void,
    ) -> c_int;
    pub(super) fn AXObserverGetRunLoopSource(observer: AXObserverRef) -> CFRunLoopSourceRef;
    pub(super) fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    pub(super) fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> c_int;
    pub(super) fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut c_int) -> c_int;
    pub(super) fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> c_int;
    pub(super) fn AXUIElementSetMessagingTimeout(
        element: AXUIElementRef,
        timeout_in_seconds: f32,
    ) -> c_int;
    #[cfg(any(test, feature = "transactional-shortcuts-dev"))]
    pub(super) fn AXValueCreate(value_type: u32, value: *const c_void) -> CFTypeRef;
    pub(super) fn AXValueGetType(value: CFTypeRef) -> u32;
    pub(super) fn AXValueGetValue(value: CFTypeRef, value_type: u32, output: *mut c_void) -> u8;
}
