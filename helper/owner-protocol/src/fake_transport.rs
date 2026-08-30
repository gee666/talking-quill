//! Deterministic, bounded, in-memory implementation of the ordered transport.
//!
//! It has no endpoint discovery, launch, credential, or persistence behavior.

use std::collections::VecDeque;
use std::fmt;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::transport::{
    DEFAULT_QUEUE_CAPACITY, FlushReceipt, MAX_FRAME_LENGTH, MAX_QUEUE_CAPACITY, OrderedTransport,
    Progress, validate_complete_frame,
};

pub use crate::transport::{ReceiveResult, TransportError, TransportError as FakeTransportError};

static NEXT_ENDPOINT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Side {
    Client,
    Server,
}

impl Side {
    const fn index(self) -> usize {
        match self {
            Self::Client => 0,
            Self::Server => 1,
        }
    }

    const fn peer(self) -> Self {
        match self {
            Self::Client => Self::Server,
            Self::Server => Self::Client,
        }
    }
}

#[derive(Default)]
struct PendingFrame {
    sequence: u64,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct DirectionState {
    pending: VecDeque<PendingFrame>,
    delivered: VecDeque<Vec<u8>>,
    sent_high_water: u64,
    flushed_high_water: u64,
}

struct LinkState {
    open: [bool; 2],
    capacity: usize,
    directions: [DirectionState; 2],
}

struct SharedLink {
    state: Mutex<LinkState>,
}

/// Out-of-band test control used to model process death and race boundaries.
#[derive(Clone)]
pub struct FakeTransportControl {
    shared: Arc<SharedLink>,
    side: Side,
}

impl fmt::Debug for FakeTransportControl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FakeTransportControl(<redacted>)")
    }
}

impl FakeTransportControl {
    pub fn abort(&self) {
        let mut state = self.shared.state.lock().expect("fake transport mutex");
        state.open[self.side.index()] = false;
        state.directions[self.side.index()].pending.clear();
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        let state = self.shared.state.lock().expect("fake transport mutex");
        state.open[self.side.index()]
    }
}

/// One endpoint of a fake ordered full-duplex stream.
pub struct FakeOrderedEndpoint {
    shared: Arc<SharedLink>,
    side: Side,
    endpoint_id: u64,
    peer_closed_reported: bool,
}

impl fmt::Debug for FakeOrderedEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FakeOrderedEndpoint(<redacted>)")
    }
}

#[must_use]
pub fn fake_ordered_transport_pair() -> (FakeOrderedEndpoint, FakeOrderedEndpoint) {
    fake_ordered_transport_pair_with_capacity(
        NonZeroUsize::new(DEFAULT_QUEUE_CAPACITY).expect("nonzero default"),
    )
    .expect("default fake transport capacity is valid")
}

pub fn fake_ordered_transport_pair_with_capacity(
    capacity: NonZeroUsize,
) -> Result<(FakeOrderedEndpoint, FakeOrderedEndpoint), FakeTransportError> {
    if capacity.get() > MAX_QUEUE_CAPACITY {
        return Err(TransportError::InvalidCapacity);
    }
    let first_id = NEXT_ENDPOINT_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(2)
        })
        .map_err(|_| TransportError::SequenceExhausted)?;
    let second_id = first_id
        .checked_add(1)
        .ok_or(TransportError::SequenceExhausted)?;
    let shared = Arc::new(SharedLink {
        state: Mutex::new(LinkState {
            open: [true, true],
            capacity: capacity.get(),
            directions: [DirectionState::default(), DirectionState::default()],
        }),
    });
    Ok((
        FakeOrderedEndpoint {
            shared: Arc::clone(&shared),
            side: Side::Client,
            endpoint_id: first_id,
            peer_closed_reported: false,
        },
        FakeOrderedEndpoint {
            shared,
            side: Side::Server,
            endpoint_id: second_id,
            peer_closed_reported: false,
        },
    ))
}

impl Drop for FakeOrderedEndpoint {
    fn drop(&mut self) {
        if let Ok(mut state) = self.shared.state.lock() {
            state.open[self.side.index()] = false;
        }
    }
}

impl FakeOrderedEndpoint {
    #[must_use]
    pub fn control(&self) -> FakeTransportControl {
        FakeTransportControl {
            shared: Arc::clone(&self.shared),
            side: self.side,
        }
    }

    pub fn try_send(&mut self, frame: Vec<u8>) -> Result<FlushReceipt, FakeTransportError> {
        <Self as OrderedTransport>::try_send(self, frame)
    }

    /// Fake-only receiver framing-fault seam.
    #[doc(hidden)]
    pub fn try_send_receiver_fault(
        &mut self,
        frame: Vec<u8>,
    ) -> Result<FlushReceipt, FakeTransportError> {
        self.try_send_bounded(frame)
    }

    pub fn confirm_flushed(&mut self, receipt: FlushReceipt) -> Result<(), FakeTransportError> {
        match <Self as OrderedTransport>::flush(self, receipt)? {
            Progress::Complete => Ok(()),
            Progress::Pending => unreachable!("fake flush is immediate"),
        }
    }

    #[must_use]
    pub fn is_flushed(&self, receipt: FlushReceipt) -> bool {
        if receipt.transport_id() != self.endpoint_id {
            return false;
        }
        let state = self.shared.state.lock().expect("fake transport mutex");
        state.directions[self.side.index()].flushed_high_water >= receipt.write_sequence()
    }

    pub fn try_receive(&mut self) -> ReceiveResult {
        <Self as OrderedTransport>::try_receive(self).expect("fake receive cannot fail")
    }

    pub fn close(&mut self) {
        let _ = <Self as OrderedTransport>::close(self);
    }

    pub fn abort(&mut self) {
        <Self as OrderedTransport>::abort(self);
    }

    #[must_use]
    pub fn queued_outbound(&self) -> usize {
        let state = self.shared.state.lock().expect("fake transport mutex");
        state.directions[self.side.index()].pending.len()
    }
}

impl FakeOrderedEndpoint {
    fn try_send_bounded(&mut self, frame: Vec<u8>) -> Result<FlushReceipt, TransportError> {
        if frame.len() > MAX_FRAME_LENGTH {
            return Err(TransportError::InvalidFrame);
        }
        let mut state = self.shared.state.lock().expect("fake transport mutex");
        if !state.open[self.side.index()] {
            return Err(TransportError::Closed);
        }
        if !state.open[self.side.peer().index()] {
            return Err(TransportError::PeerClosed);
        }
        let capacity = state.capacity;
        let direction = &mut state.directions[self.side.index()];
        if direction.pending.len() >= capacity {
            return Err(TransportError::QueueFull);
        }
        let sequence = direction
            .sent_high_water
            .checked_add(1)
            .ok_or(TransportError::SequenceExhausted)?;
        direction.sent_high_water = sequence;
        direction.pending.push_back(PendingFrame {
            sequence,
            bytes: frame,
        });
        Ok(FlushReceipt::new(self.endpoint_id, sequence))
    }
}

impl OrderedTransport for FakeOrderedEndpoint {
    fn is_test_only(&self) -> bool {
        true
    }

    fn try_send(&mut self, frame: Vec<u8>) -> Result<FlushReceipt, TransportError> {
        validate_complete_frame(&frame)?;
        self.try_send_bounded(frame)
    }

    fn flush(&mut self, receipt: FlushReceipt) -> Result<Progress, TransportError> {
        if receipt.transport_id() != self.endpoint_id {
            return Err(TransportError::WrongReceipt);
        }
        let mut state = self.shared.state.lock().expect("fake transport mutex");
        if !state.open[self.side.index()] {
            return Err(TransportError::Closed);
        }
        if !state.open[self.side.peer().index()] {
            return Err(TransportError::PeerClosed);
        }
        let direction = &mut state.directions[self.side.index()];
        if receipt.write_sequence() == 0
            || receipt.write_sequence() > direction.sent_high_water
            || receipt.write_sequence() <= direction.flushed_high_water
        {
            return Err(TransportError::UnknownReceipt);
        }
        while direction
            .pending
            .front()
            .is_some_and(|frame| frame.sequence <= receipt.write_sequence())
        {
            let frame = direction.pending.pop_front().expect("checked front");
            direction.delivered.push_back(frame.bytes);
        }
        direction.flushed_high_water = receipt.write_sequence();
        Ok(Progress::Complete)
    }

    fn try_receive(&mut self) -> Result<ReceiveResult, TransportError> {
        let mut state = self.shared.state.lock().expect("fake transport mutex");
        let inbound = &mut state.directions[self.side.peer().index()];
        if let Some(frame) = inbound.delivered.pop_front() {
            return Ok(ReceiveResult::Frame(frame));
        }
        if !state.open[self.side.peer().index()] {
            if self.peer_closed_reported {
                Ok(ReceiveResult::Empty)
            } else {
                self.peer_closed_reported = true;
                Ok(ReceiveResult::PeerClosed)
            }
        } else {
            Ok(ReceiveResult::Empty)
        }
    }

    fn close(&mut self) -> Result<Progress, TransportError> {
        let mut state = self.shared.state.lock().expect("fake transport mutex");
        if !state.open[self.side.peer().index()]
            && !state.directions[self.side.index()].pending.is_empty()
        {
            return Err(TransportError::PeerClosed);
        }
        let direction = &mut state.directions[self.side.index()];
        while let Some(frame) = direction.pending.pop_front() {
            direction.flushed_high_water = frame.sequence;
            direction.delivered.push_back(frame.bytes);
        }
        state.open[self.side.index()] = false;
        Ok(Progress::Complete)
    }

    fn abort(&mut self) {
        self.control().abort();
    }
}
