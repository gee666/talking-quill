//! Callback-facing cache requests, freshness fences, and worker shutdown.

use super::*;

/// A cache populated and owned exclusively by the dedicated AX worker. Event
/// callbacks reserve only a nondestructive scalar publication ID plus epochs;
/// they never move, retain, compare, or release CF evidence. Validation returns
/// scalar handles, and all retained evidence teardown stays on the AX worker.
pub(in crate::platform::macos) struct TargetCache {
    pub(super) shared: Arc<CacheShared>,
    pub(super) validation_pool: Arc<ValidationPool>,
    pub(super) requests: Sender<ValidationWork>,
    pub(super) insertions: Sender<InsertionWork>,
    pub(super) next_request_id: AtomicU64,
    pub(super) worker: Option<JoinHandle<()>>,
    pub(super) worker_completion: Receiver<()>,
    #[cfg(test)]
    pub(super) _test_request_receiver: Option<Receiver<ValidationWork>>,
}

impl TargetCache {
    pub(in crate::platform::macos) fn start() -> Result<Self, PlatformError> {
        let shared = Arc::new(CacheShared::new());
        let validation_pool = Arc::new(ValidationPool::new());
        let (requests, request_receiver) = bounded(VALIDATION_QUEUE_CAPACITY);
        let (insertions, insertion_receiver) = bounded(1);
        let worker_shared = Arc::clone(&shared);
        let worker_pool = Arc::clone(&validation_pool);
        let (worker_completion_tx, worker_completion) = bounded(1);
        let worker = thread::Builder::new()
            .name("talking-quill-helper-macos-ax-cache".into())
            .spawn(move || {
                target_cache_worker(
                    worker_shared,
                    worker_pool,
                    request_receiver,
                    insertion_receiver,
                );
                let _ = worker_completion_tx.try_send(());
            })
            .map_err(|_| PlatformError::ThreadStopped)?;
        Ok(Self {
            shared,
            validation_pool,
            requests,
            insertions,
            next_request_id: AtomicU64::new(1),
            worker: Some(worker),
            worker_completion,
            #[cfg(test)]
            _test_request_receiver: None,
        })
    }

    pub(in crate::platform::macos) fn invalidate_boundary(&self) {
        self.shared.invalidate_boundary();
    }

    pub(in crate::platform::macos) fn reserve_activation(&self) -> Option<ActivationReservation> {
        self.reserve_activation_at(Instant::now())
    }

    #[cfg(test)]
    pub(super) fn request_validation(&self) -> Option<ValidationRequest> {
        self.request_validation_from(self.current_epoch(), 1)
    }

    pub(in crate::platform::macos) fn request_target_validation(
        &self,
        target: TargetHandle,
    ) -> Option<ValidationRequest> {
        self.request_validation_from(self.current_epoch(), target.publication_id)
    }

    pub(in crate::platform::macos) fn request_activation_validation(
        &self,
        reservation: &ActivationReservation,
    ) -> Option<ValidationRequest> {
        self.request_validation_from(reservation.notification_epoch, reservation.publication_id)
    }

    /// Callback-safe scalar fence for candidate-start evidence. Broad focus and
    /// selected-range observers invalidate these epochs before any replay or
    /// target-sensitive operation may use the retained publication.
    pub(in crate::platform::macos) fn reservation_is_current(
        &self,
        reservation: &ActivationReservation,
    ) -> bool {
        if self.current_epoch() != reservation.notification_epoch
            || self.current_selected_range_epoch() != reservation.selected_range_epoch
            || self.shared.published_id.load(Ordering::Acquire) != reservation.publication_id
            || self.shared.stopping.load(Ordering::Acquire)
        {
            return false;
        }
        let slot = match self.shared.slot.try_lock() {
            Ok(slot) => slot,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return false,
        };
        slot.as_ref().is_some_and(|cached| {
            cached.publication_id == reservation.publication_id
                && cached.notification_epoch == reservation.notification_epoch
                && cached.selected_range_epoch == reservation.selected_range_epoch
                && Instant::now().saturating_duration_since(cached.captured_at)
                    <= TARGET_CACHE_MAX_AGE
        })
    }

    pub(in crate::platform::macos) fn prepare_insertion(
        &self,
        target: TargetHandle,
        notification_epoch: u64,
        boundary_epoch: u64,
        selected_range_epoch: u64,
        expected_clipboard_sha256: ClipboardTextHash,
        deadline: Instant,
    ) -> Option<InsertionRequest> {
        self.handle_is_current(
            target,
            notification_epoch,
            boundary_epoch,
            selected_range_epoch,
        )
        .then(|| {
            let state = Arc::new(AtomicU8::new(INSERTION_PENDING));
            InsertionRequest {
                work: Some(InsertionWork {
                    publication_id: target.publication_id,
                    notification_epoch,
                    boundary_epoch,
                    selected_range_epoch,
                    expected_clipboard_sha256,
                    deadline,
                    state: Arc::clone(&state),
                }),
                state,
            }
        })
    }

    pub(in crate::platform::macos) fn submit_insertion(
        &self,
        request: &mut InsertionRequest,
    ) -> bool {
        request.submit(&self.insertions)
    }

    pub(super) fn request_validation_from(
        &self,
        start_epoch: u64,
        expected_publication_id: u64,
    ) -> Option<ValidationRequest> {
        // Callback-safe: slots and the work queue were allocated at startup.
        // Acquisition is a bounded atomic scan; Arc clone and try_send neither
        // allocate nor wait, and every failure recycles the claimed slot.
        let request_id = self.next_request_id.fetch_add(1, Ordering::AcqRel);
        let ticket = ValidationTicket {
            request_id,
            start_epoch,
            start_boundary_epoch: self.current_boundary_epoch(),
        };
        let (request, work) = self
            .validation_pool
            .acquire_expected(ticket, expected_publication_id)?;
        if self.requests.try_send(work).is_ok() {
            Some(request)
        } else {
            drop(request);
            None
        }
    }

    pub(in crate::platform::macos) fn request_stop(&self) {
        self.shared.stopping.store(true, Ordering::Release);
        self.validation_pool.stop();
        self.shared.invalidate_notification();
    }

    pub(in crate::platform::macos) fn current_epoch(&self) -> u64 {
        self.shared.current_notification_epoch()
    }

    pub(in crate::platform::macos) fn current_boundary_epoch(&self) -> u64 {
        self.shared.current_boundary_epoch()
    }

    pub(in crate::platform::macos) fn current_selected_range_epoch(&self) -> u64 {
        self.shared.current_selected_range_epoch()
    }

    pub(in crate::platform::macos) fn handle_is_current(
        &self,
        target: TargetHandle,
        validated_epoch: u64,
        validated_boundary_epoch: u64,
        validated_selected_range_epoch: u64,
    ) -> bool {
        self.current_epoch() == validated_epoch
            && self.current_boundary_epoch() == validated_boundary_epoch
            && self.current_selected_range_epoch() == validated_selected_range_epoch
            && self.shared.published_id.load(Ordering::Acquire) == target.publication_id
            && !self.shared.stopping.load(Ordering::Acquire)
    }

    pub(super) fn reserve_activation_at(&self, now: Instant) -> Option<ActivationReservation> {
        let notification_before = self.shared.current_notification_epoch();
        let boundary_before = self.shared.current_boundary_epoch();
        let selected_range_before = self.shared.current_selected_range_epoch();
        let slot = match self.shared.slot.try_lock() {
            Ok(slot) => slot,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return None,
        };
        let cached = slot.as_ref()?;
        let notification_after = self.shared.current_notification_epoch();
        let boundary_after = self.shared.current_boundary_epoch();
        let selected_range_after = self.shared.current_selected_range_epoch();
        if notification_before != notification_after
            || boundary_before != boundary_after
            || selected_range_before != selected_range_after
            || cached.notification_epoch != notification_after
            || cached.boundary_epoch != boundary_after
            || cached.selected_range_epoch != selected_range_after
            || cached.publication_id == 0
            || self.shared.published_id.load(Ordering::Acquire) != cached.publication_id
            || now.saturating_duration_since(cached.captured_at) > TARGET_CACHE_MAX_AGE
        {
            return None;
        }
        Some(ActivationReservation {
            notification_epoch: cached.notification_epoch,
            boundary_epoch: cached.boundary_epoch,
            selected_range_epoch: cached.selected_range_epoch,
            publication_id: cached.publication_id,
        })
    }

    #[cfg(test)]
    pub(in crate::platform::macos) fn install_current_handle_for_test(
        &self,
        target: TargetHandle,
        notification_epoch: u64,
        boundary_epoch: u64,
    ) {
        self.shared
            .notification_epoch
            .store(notification_epoch, Ordering::Release);
        self.shared
            .boundary_epoch
            .store(boundary_epoch, Ordering::Release);
        self.shared.selected_range_epoch.store(1, Ordering::Release);
        self.shared
            .published_id
            .store(target.publication_id, Ordering::Release);
    }

    #[cfg(test)]
    pub(in crate::platform::macos) fn with_open_validation_queue_for_test() -> Self {
        let (requests, request_receiver) = bounded(VALIDATION_QUEUE_CAPACITY);
        let (insertions, _insertion_receiver) = bounded(1);
        Self {
            shared: Arc::new(CacheShared::new()),
            validation_pool: Arc::new(ValidationPool::new()),
            requests,
            insertions,
            next_request_id: AtomicU64::new(1),
            worker: None,
            worker_completion: bounded(1).1,
            _test_request_receiver: Some(request_receiver),
        }
    }

    #[cfg(test)]
    pub(super) fn without_worker() -> Self {
        Self::with_open_validation_queue_for_test()
    }

    #[cfg(test)]
    pub(super) fn publish_for_test(&self, evidence: TargetEvidence, captured_at: Instant) {
        let notification_epoch = self.shared.current_notification_epoch();
        let publication_id = self
            .shared
            .published_id
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        *self.shared.slot.lock().unwrap() = Some(CachedTarget {
            evidence,
            captured_at,
            notification_epoch,
            boundary_epoch: self.shared.current_boundary_epoch(),
            selected_range_epoch: self.shared.current_selected_range_epoch(),
            publication_id,
        });
    }
}

impl Drop for TargetCache {
    fn drop(&mut self) {
        self.request_stop();
        if let Some(worker) = self.worker.take()
            && (self
                .worker_completion
                .recv_timeout(Duration::from_millis(500))
                .is_ok()
                || worker.is_finished())
        {
            let _ = worker.join();
        }
        // A nonresponsive AX process cannot retain helper shutdown forever.
        // Every AX message is separately bounded; if the worker still misses
        // this outer deadline its Arc-owned evidence/refcons outlive this cache
        // safely and process exit terminates the detached worker.
    }
}
