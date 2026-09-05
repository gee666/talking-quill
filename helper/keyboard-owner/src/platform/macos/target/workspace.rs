//! Workspace notification registration and admitted-callback retirement.

use super::*;

pub(super) struct WorkspaceCallbackEntry {
    pub(super) shared: Arc<CacheShared>,
    pub(super) active: usize,
    pub(super) retiring: bool,
}

#[derive(Default)]
pub(super) struct WorkspaceCallbackRegistry {
    pub(super) entries: HashMap<usize, WorkspaceCallbackEntry>,
}

pub(super) fn workspace_callback_registry() -> &'static (Mutex<WorkspaceCallbackRegistry>, Condvar)
{
    static REGISTRY: OnceLock<(Mutex<WorkspaceCallbackRegistry>, Condvar)> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        (
            Mutex::new(WorkspaceCallbackRegistry::default()),
            Condvar::new(),
        )
    })
}

pub(super) struct WorkspaceInvocation {
    pub(super) observer: usize,
    pub(super) shared: Arc<CacheShared>,
}

impl WorkspaceInvocation {
    pub(super) fn begin(observer: ffi::ObjcId) -> Option<Self> {
        let observer = observer as usize;
        let (registry, _) = workspace_callback_registry();
        let mut registry = registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = registry.entries.get_mut(&observer)?;
        if entry.retiring {
            return None;
        }
        // Hold an Objective-C +1 before leaving registry admission. Teardown
        // cannot reach its final release until this active count drains, so
        // the receiver cannot deallocate during the selector body.
        unsafe {
            let _ = ffi::objc_msgSend(
                observer as ffi::ObjcId,
                ffi::sel_registerName(c"retain".as_ptr()),
            );
        }
        entry.active += 1;
        Some(Self {
            observer,
            shared: Arc::clone(&entry.shared),
        })
    }
}

impl Drop for WorkspaceInvocation {
    fn drop(&mut self) {
        let (registry, drained) = workspace_callback_registry();
        let mut registry = registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = registry.entries.get_mut(&self.observer) {
            entry.active = entry.active.saturating_sub(1);
            if entry.active == 0 {
                drained.notify_all();
            }
        }
        drop(registry);
        // SAFETY: begin acquired this exact callback-owned +1 while teardown
        // was excluded by the registry lock.
        unsafe {
            let _ = ffi::objc_msgSend(
                self.observer as ffi::ObjcId,
                ffi::sel_registerName(c"release".as_ptr()),
            );
        }
    }
}

pub(super) unsafe extern "C" fn workspace_activation_callback(
    observer: ffi::ObjcId,
    _selector: ffi::ObjcSel,
    _notification: ffi::ObjcId,
) {
    // The registry admission is the callback-lifetime synchronization point.
    // Teardown unregisters first, closes this admission, waits for every
    // admitted invocation, and only then releases the Objective-C receiver.
    if let Some(invocation) = WorkspaceInvocation::begin(observer) {
        invocation.shared.invalidate_notification();
    }
}

pub(super) struct WorkspaceObserver {
    pub(super) observer: ffi::ObjcId,
    pub(super) notification_center: ffi::ObjcId,
}

impl WorkspaceObserver {
    pub(super) fn install(shared: &Arc<CacheShared>) -> Result<Self, PlatformError> {
        // SAFETY: Objective-C runtime class and selector names are static C
        // strings. Registration happens once per helper process.
        let mut class = unsafe { ffi::objc_getClass(WORKSPACE_OBSERVER_CLASS.as_ptr()) };
        if class.is_null() {
            let superclass = unsafe { ffi::objc_getClass(c"NSObject".as_ptr()) };
            if superclass.is_null() {
                return Err(PlatformError::NativeFailure);
            }
            class = unsafe {
                ffi::objc_allocateClassPair(superclass, WORKSPACE_OBSERVER_CLASS.as_ptr(), 0)
            };
            let callback_selector =
                unsafe { ffi::sel_registerName(WORKSPACE_CALLBACK_SELECTOR.as_ptr()) };
            if class.is_null()
                || callback_selector.is_null()
                || !unsafe {
                    ffi::class_addMethod(
                        class,
                        callback_selector,
                        workspace_activation_callback as *const () as *const c_void,
                        c"v@:@".as_ptr(),
                    )
                }
            {
                return Err(PlatformError::NativeFailure);
            }
            unsafe { ffi::objc_registerClassPair(class) };
        }

        let observer =
            unsafe { ffi::objc_msgSend(class.cast(), ffi::sel_registerName(c"new".as_ptr())) };
        if observer.is_null() {
            return Err(PlatformError::NativeFailure);
        }
        let workspace_class = unsafe { ffi::objc_getClass(c"NSWorkspace".as_ptr()) };
        let shared_workspace = unsafe {
            ffi::objc_msgSend(
                workspace_class.cast(),
                ffi::sel_registerName(c"sharedWorkspace".as_ptr()),
            )
        };
        let notification_center = unsafe {
            ffi::objc_msgSend(
                shared_workspace,
                ffi::sel_registerName(c"notificationCenter".as_ptr()),
            )
        };
        if workspace_class.is_null() || shared_workspace.is_null() || notification_center.is_null()
        {
            unsafe {
                let _ = ffi::objc_msgSend(observer, ffi::sel_registerName(c"release".as_ptr()));
            }
            return Err(PlatformError::NativeFailure);
        }

        let (registry, _) = workspace_callback_registry();
        registry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entries
            .insert(
                observer as usize,
                WorkspaceCallbackEntry {
                    shared: Arc::clone(shared),
                    active: 0,
                    retiring: false,
                },
            );
        unsafe {
            let _ = ffi::objc_msgSend(
                notification_center,
                ffi::sel_registerName(c"addObserver:selector:name:object:".as_ptr()),
                observer,
                ffi::sel_registerName(WORKSPACE_CALLBACK_SELECTOR.as_ptr()),
                ffi::NSWorkspaceDidActivateApplicationNotification,
                null_mut::<c_void>(),
            );
        }
        Ok(Self {
            observer,
            notification_center,
        })
    }
}

pub(super) fn retire_workspace_callback(observer: usize) {
    let (registry, drained) = workspace_callback_registry();
    let mut registry = registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(entry) = registry.entries.get_mut(&observer) {
        entry.retiring = true;
    }
    while registry
        .entries
        .get(&observer)
        .is_some_and(|entry| entry.active != 0)
    {
        registry = drained
            .wait(registry)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }
    registry.entries.remove(&observer);
}

impl Drop for WorkspaceObserver {
    fn drop(&mut self) {
        // NSNotificationCenter removal closes future selector dispatch. Registry
        // retirement then drains every callback that already acquired its Arc;
        // the receiver's final +1 is released only after that drain completes.
        unsafe {
            let _ = ffi::objc_msgSend(
                self.notification_center,
                ffi::sel_registerName(c"removeObserver:".as_ptr()),
                self.observer,
            );
        }
        retire_workspace_callback(self.observer as usize);
        unsafe {
            let _ = ffi::objc_msgSend(self.observer, ffi::sel_registerName(c"release".as_ptr()));
        }
    }
}
