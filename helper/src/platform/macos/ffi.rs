use std::ffi::{c_char, c_int, c_void};

pub(super) type CFIndex = isize;
pub(super) type CFTypeId = usize;
pub(super) type CFTypeRef = *const c_void;
pub(super) type CFStringRef = *const c_void;
pub(super) type CFRunLoopRef = *mut c_void;
pub(super) type CFRunLoopSourceRef = *mut c_void;
pub(super) type CFMachPortRef = *mut c_void;
pub(super) type CGEventRef = *mut c_void;
pub(super) type CGEventTapProxy = *mut c_void;
pub(super) type AXUIElementRef = *mut c_void;

pub(super) const K_CG_SESSION_EVENT_TAP: u32 = 1;
pub(super) const K_CG_HID_EVENT_TAP: u32 = 0;
pub(super) const K_CG_HEAD_INSERT_EVENT_TAP: u32 = 0;
pub(super) const K_CG_EVENT_TAP_OPTION_DEFAULT: u32 = 0;
pub(super) const K_CG_EVENT_KEY_DOWN: u32 = 10;
pub(super) const K_CG_EVENT_KEY_UP: u32 = 11;
pub(super) const K_CG_EVENT_TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
pub(super) const K_CG_EVENT_TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;
pub(super) const K_CG_KEYBOARD_EVENT_AUTOREPEAT: u32 = 8;
pub(super) const K_CG_KEYBOARD_EVENT_KEYCODE: u32 = 9;
pub(super) const K_CG_EVENT_SOURCE_USER_DATA: u32 = 42;
pub(super) const K_CG_EVENT_FLAG_MASK_SHIFT: u64 = 0x0002_0000;
pub(super) const K_CG_EVENT_FLAG_MASK_CONTROL: u64 = 0x0004_0000;
pub(super) const K_CG_EVENT_FLAG_MASK_ALTERNATE: u64 = 0x0008_0000;
pub(super) const K_CG_EVENT_FLAG_MASK_COMMAND: u64 = 0x0010_0000;
pub(super) const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
pub(super) const K_AX_VALUE_CGPOINT_TYPE: u32 = 1;
pub(super) const K_AX_VALUE_CGSIZE_TYPE: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Default)]
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
#[derive(Clone, Copy)]
pub(super) struct CFRange {
    pub(super) location: CFIndex,
    pub(super) length: CFIndex,
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    pub(super) static kCFRunLoopCommonModes: CFStringRef;
    pub(super) fn CFRelease(value: CFTypeRef);
    pub(super) fn CFGetTypeID(value: CFTypeRef) -> CFTypeId;
    pub(super) fn CFMachPortCreateRunLoopSource(
        allocator: CFTypeRef,
        port: CFMachPortRef,
        order: CFIndex,
    ) -> CFRunLoopSourceRef;
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
    pub(super) fn CFMachPortInvalidate(port: CFMachPortRef);
    pub(super) fn CFRunLoopRun();
    pub(super) fn CFRunLoopStop(run_loop: CFRunLoopRef);
    pub(super) fn CFStringCreateWithCString(
        allocator: CFTypeRef,
        string: *const c_char,
        encoding: u32,
    ) -> CFStringRef;
    pub(super) fn CFStringGetTypeID() -> CFTypeId;
    pub(super) fn CFStringGetLength(string: CFStringRef) -> CFIndex;
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
    pub(super) fn CGEventTapIsEnabled(tap: CFMachPortRef) -> bool;
    pub(super) fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
    pub(super) fn CGEventGetFlags(event: CGEventRef) -> u64;
    pub(super) fn CGEventCreateKeyboardEvent(
        source: CFTypeRef,
        virtual_key: u16,
        key_down: bool,
    ) -> CGEventRef;
    pub(super) fn CGEventSetFlags(event: CGEventRef, flags: u64);
    pub(super) fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
    pub(super) fn CGEventPost(tap: u32, event: CGEventRef);
    pub(super) fn CGPreflightListenEventAccess() -> bool;
    pub(super) fn CGPreflightPostEventAccess() -> bool;
}

// IsSecureEventInputEnabled is declared by HIToolbox/Events.h. Carbon is the
// public umbrella framework that links the HIToolbox subframework symbol.
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    pub(super) fn IsSecureEventInputEnabled() -> u8;
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    pub(super) fn AXIsProcessTrusted() -> u8;
    pub(super) fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    pub(super) fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> c_int;
    pub(super) fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut c_int) -> c_int;
    pub(super) fn AXValueGetType(value: CFTypeRef) -> u32;
    pub(super) fn AXValueGetValue(value: CFTypeRef, value_type: u32, output: *mut c_void) -> u8;
}
