//! Worker-owned AX observer and focused-control notification context.

use super::*;

pub(super) struct TargetObserverContext {
    pub(super) shared: Arc<CacheShared>,
    pub(super) focused_control: OwnedCf,
}

pub(super) unsafe extern "C" fn target_observer_callback(
    _observer: ffi::AXObserverRef,
    element: ffi::AXUIElementRef,
    _notification: ffi::CFStringRef,
    refcon: *mut c_void,
) {
    if refcon.is_null() {
        return;
    }
    // SAFETY: WorkerObserver owns this boxed context until its source and
    // observer are removed on the same AX worker.
    let context = unsafe { &*refcon.cast::<TargetObserverContext>() };
    // Selected-text/range notifications are registered only on the retained
    // focused control. They advance an independent epoch as well as the broad
    // target epoch; app/window/focus notifications arrive on the application.
    if !element.is_null()
        && unsafe {
            ffi::CFEqual(
                element.cast_const().cast(),
                context.focused_control.as_type_ref(),
            ) != 0
        }
    {
        context.shared.invalidate_selected_range();
    } else {
        context.shared.invalidate_notification();
    }
}

pub(super) struct WorkerObserver {
    pub(super) observer: ffi::AXObserverRef,
    pub(super) source: ffi::CFRunLoopSourceRef,
    pub(super) run_loop: ffi::CFRunLoopRef,
    pub(super) context: *mut TargetObserverContext,
    pub(super) process_id: i32,
}

pub(super) fn observer_identity_matches(
    observer_process_id: i32,
    evidence_process_id: i32,
    focused_control_equal: bool,
) -> bool {
    observer_process_id == evidence_process_id && focused_control_equal
}

impl WorkerObserver {
    pub(super) fn observes_control(&self, evidence: &TargetEvidence) -> bool {
        let focused_control_equal = unsafe {
            // SAFETY: the observer context and candidate evidence are both
            // retained on this AX worker for the complete comparison.
            ffi::CFEqual(
                (*self.context).focused_control.as_type_ref(),
                evidence.focused_control.as_type_ref(),
            ) != 0
        };
        observer_identity_matches(self.process_id, evidence.process_id, focused_control_equal)
    }

    pub(super) fn install(
        run_loop: ffi::CFRunLoopRef,
        shared: &Arc<CacheShared>,
        resources: &TargetCaptureResources,
        evidence: &TargetEvidence,
    ) -> Result<Self, PlatformError> {
        let focused_control = evidence.focused_control.retained_clone()?;
        let context = Box::into_raw(Box::new(TargetObserverContext {
            shared: Arc::clone(shared),
            focused_control,
        }));
        let mut observer = null_mut();
        // SAFETY: observer output is writable and the boxed callback context
        // remains valid until WorkerObserver removes and releases the observer.
        if unsafe {
            ffi::AXObserverCreate(
                evidence.process_id,
                Some(target_observer_callback),
                &raw mut observer,
            )
        } != 0
            || observer.is_null()
        {
            unsafe { drop(Box::from_raw(context)) };
            return Err(PlatformError::NativeFailure);
        }
        let registrations = [
            (
                evidence.application.as_type_ref().cast_mut(),
                resources.focused_window_changed.as_type_ref(),
            ),
            (
                evidence.application.as_type_ref().cast_mut(),
                resources.focused_ui_element_changed.as_type_ref(),
            ),
            (
                evidence.focused_control.as_type_ref().cast_mut(),
                resources.selected_text_changed.as_type_ref(),
            ),
            (
                evidence.focused_control.as_type_ref().cast_mut(),
                resources.selected_text_range_changed.as_type_ref(),
            ),
        ];
        for (element, notification) in registrations {
            if unsafe {
                ffi::AXObserverAddNotification(
                    observer,
                    element,
                    notification,
                    context.cast::<c_void>(),
                )
            } != 0
            {
                unsafe {
                    ffi::CFRelease(observer.cast_const());
                    drop(Box::from_raw(context));
                }
                return Err(PlatformError::NativeFailure);
            }
        }
        let source = unsafe { ffi::AXObserverGetRunLoopSource(observer) };
        if source.is_null() {
            unsafe {
                ffi::CFRelease(observer.cast_const());
                drop(Box::from_raw(context));
            }
            return Err(PlatformError::NativeFailure);
        }
        unsafe { ffi::CFRunLoopAddSource(run_loop, source, ffi::kCFRunLoopDefaultMode) };
        Ok(Self {
            observer,
            source,
            run_loop,
            context,
            process_id: evidence.process_id,
        })
    }
}

impl Drop for WorkerObserver {
    fn drop(&mut self) {
        // SAFETY: installation and teardown run on the same AX worker. Removing
        // the source prevents another callback before its boxed refcon is freed;
        // the observer owns the borrowed source until the following release.
        unsafe {
            ffi::CFRunLoopRemoveSource(self.run_loop, self.source, ffi::kCFRunLoopDefaultMode);
            ffi::CFRelease(self.observer.cast_const());
            drop(Box::from_raw(self.context));
        }
    }
}
