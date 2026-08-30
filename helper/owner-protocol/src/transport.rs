//! Ordered, bounded transport for already-connected owner-protocol streams.
//!
//! This module deliberately does not discover, create, or authenticate an OS
//! endpoint. Platform code supplies an already-connected stream. Authenticated
//! transport sequence numbers and MAC validation remain in [`crate::session`].

use std::collections::VecDeque;
use std::fmt;
use std::io::{self, Read, Write};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};

use thiserror::Error;

use crate::framing::{LENGTH_PREFIX_BYTES, MAX_BODY_LENGTH, decode_outer_frame};

pub const DEFAULT_QUEUE_CAPACITY: usize = 16;
pub const MAX_QUEUE_CAPACITY: usize = 64;
pub const MAX_FRAME_LENGTH: usize = LENGTH_PREFIX_BYTES + MAX_BODY_LENGTH;

static NEXT_TRANSPORT_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn validate_complete_frame(frame: &[u8]) -> Result<(), TransportError> {
    if frame.len() > MAX_FRAME_LENGTH || decode_outer_frame(frame).is_err() {
        return Err(TransportError::InvalidFrame);
    }
    Ok(())
}

/// Receipt for one atomically accepted frame. Receipts are connection-bound.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct FlushReceipt {
    transport_id: u64,
    write_sequence: u64,
}

impl FlushReceipt {
    pub(crate) const fn new(transport_id: u64, write_sequence: u64) -> Self {
        Self {
            transport_id,
            write_sequence,
        }
    }

    pub(crate) const fn transport_id(self) -> u64 {
        self.transport_id
    }

    pub(crate) const fn write_sequence(self) -> u64 {
        self.write_sequence
    }
}

impl fmt::Debug for FlushReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FlushReceipt(<redacted>)")
    }
}

/// Result of a nonblocking receive attempt.
#[derive(Debug, Eq, PartialEq)]
pub enum ReceiveResult {
    Frame(Vec<u8>),
    Empty,
    PeerClosed,
}

/// Result of flushing through a receipt or gracefully closing a writer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Progress {
    Complete,
    Pending,
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("owner-protocol transport queue capacity must be between 1 and 64")]
    InvalidCapacity,
    #[error("owner-protocol transport endpoint is closed")]
    Closed,
    #[error("owner-protocol transport peer is closed")]
    PeerClosed,
    #[error("owner-protocol transport outbound queue is full")]
    QueueFull,
    #[error("owner-protocol transport frame is invalid")]
    InvalidFrame,
    #[error("owner-protocol transport write sequence exhausted")]
    SequenceExhausted,
    #[error("owner-protocol flush receipt does not belong to this writer")]
    WrongReceipt,
    #[error("owner-protocol flush receipt is unknown or stale")]
    UnknownReceipt,
    #[error("owner-protocol transport ended inside a frame")]
    TruncatedFrame,
    #[error("owner-protocol transport I/O failed")]
    Io(#[source] io::Error),
}

impl PartialEq for TransportError {
    fn eq(&self, other: &Self) -> bool {
        use TransportError::{
            Closed, InvalidCapacity, InvalidFrame, Io, PeerClosed, QueueFull, SequenceExhausted,
            TruncatedFrame, UnknownReceipt, WrongReceipt,
        };
        match (self, other) {
            (InvalidCapacity, InvalidCapacity)
            | (Closed, Closed)
            | (PeerClosed, PeerClosed)
            | (QueueFull, QueueFull)
            | (InvalidFrame, InvalidFrame)
            | (SequenceExhausted, SequenceExhausted)
            | (WrongReceipt, WrongReceipt)
            | (UnknownReceipt, UnknownReceipt)
            | (TruncatedFrame, TruncatedFrame) => true,
            (Io(left), Io(right)) => left.kind() == right.kind(),
            _ => false,
        }
    }
}

impl Eq for TransportError {}

/// One ordered full-duplex connection carrying complete outer frames.
///
/// `try_send` accepts a whole bounded frame or makes no change. `flush` reports
/// complete only after that frame and all preceding frames crossed the writer
/// boundary. `close` is graceful and therefore drains accepted frames first;
/// `abort` discards uncertain queued work.
pub trait OrderedTransport: fmt::Debug + Send {
    /// Brands deterministic test transports so fake authentication material
    /// cannot be attached to a production connected stream.
    #[must_use]
    fn is_test_only(&self) -> bool {
        false
    }

    fn try_send(&mut self, frame: Vec<u8>) -> Result<FlushReceipt, TransportError>;
    fn flush(&mut self, receipt: FlushReceipt) -> Result<Progress, TransportError>;

    /// Compatibility helper for coordinators that require an immediate writer
    /// boundary. New event loops should retain the receipt and retry `flush`
    /// when this reports queue backpressure.
    fn confirm_flushed(&mut self, receipt: FlushReceipt) -> Result<(), TransportError> {
        match self.flush(receipt)? {
            Progress::Complete => Ok(()),
            Progress::Pending => Err(TransportError::QueueFull),
        }
    }

    fn try_receive(&mut self) -> Result<ReceiveResult, TransportError>;
    fn close(&mut self) -> Result<Progress, TransportError>;
    fn abort(&mut self);
}

struct PendingFrame {
    sequence: u64,
    bytes: Vec<u8>,
    offset: usize,
}

/// Ordered transport over an already-connected, nonblocking byte stream.
///
/// The stream must translate temporary lack of readiness to
/// [`io::ErrorKind::WouldBlock`]. The adapter retains at most one bounded input
/// frame and `capacity` bounded output frames. It never creates an endpoint.
pub struct StreamOrderedTransport<S> {
    stream: Option<S>,
    id: u64,
    capacity: usize,
    outbound: VecDeque<PendingFrame>,
    sent_high_water: u64,
    written_high_water: u64,
    flushed_high_water: u64,
    inbound: Vec<u8>,
    expected_length: Option<usize>,
    peer_closed_reported: bool,
    closing: bool,
}

impl<S> fmt::Debug for StreamOrderedTransport<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StreamOrderedTransport(<redacted>)")
    }
}

impl<S: Read + Write + Send> StreamOrderedTransport<S> {
    pub fn new(stream: S) -> Result<Self, TransportError> {
        Self::with_capacity(
            stream,
            NonZeroUsize::new(DEFAULT_QUEUE_CAPACITY).expect("nonzero default capacity"),
        )
    }

    pub fn with_capacity(stream: S, capacity: NonZeroUsize) -> Result<Self, TransportError> {
        if capacity.get() > MAX_QUEUE_CAPACITY {
            return Err(TransportError::InvalidCapacity);
        }
        let id = NEXT_TRANSPORT_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| TransportError::SequenceExhausted)?;
        Ok(Self {
            stream: Some(stream),
            id,
            capacity: capacity.get(),
            outbound: VecDeque::with_capacity(capacity.get()),
            sent_high_water: 0,
            written_high_water: 0,
            flushed_high_water: 0,
            inbound: Vec::with_capacity(MAX_FRAME_LENGTH),
            expected_length: None,
            peer_closed_reported: false,
            closing: false,
        })
    }

    #[must_use]
    pub fn queued_outbound(&self) -> usize {
        self.outbound.len()
    }

    fn validate_receipt(&self, receipt: FlushReceipt) -> Result<(), TransportError> {
        if receipt.transport_id() != self.id {
            return Err(TransportError::WrongReceipt);
        }
        if receipt.write_sequence() == 0
            || receipt.write_sequence() > self.sent_high_water
            || receipt.write_sequence() <= self.flushed_high_water
        {
            return Err(TransportError::UnknownReceipt);
        }
        Ok(())
    }

    fn flush_through(&mut self, sequence: u64) -> Result<Progress, TransportError> {
        let stream = self.stream.as_mut().ok_or(TransportError::Closed)?;
        while self
            .outbound
            .front()
            .is_some_and(|frame| frame.sequence <= sequence)
        {
            let frame = self.outbound.front_mut().expect("checked front");
            while frame.offset < frame.bytes.len() {
                match stream.write(&frame.bytes[frame.offset..]) {
                    Ok(0) => return Err(TransportError::PeerClosed),
                    Ok(written) => frame.offset += written,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        return Ok(Progress::Pending);
                    }
                    Err(error) => return Err(TransportError::Io(error)),
                }
            }
            self.written_high_water = frame.sequence;
            self.outbound.pop_front();
        }
        loop {
            match stream.flush() {
                Ok(()) => {
                    if self.written_high_water >= sequence {
                        self.flushed_high_water = self.flushed_high_water.max(sequence);
                    }
                    return Ok(Progress::Complete);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    return Ok(Progress::Pending);
                }
                Err(error) => return Err(TransportError::Io(error)),
            }
        }
    }

    fn read_more(&mut self, target: usize) -> Result<Progress, TransportError> {
        let stream = self.stream.as_mut().ok_or(TransportError::Closed)?;
        let mut scratch = [0_u8; 4096];
        while self.inbound.len() < target {
            let wanted = (target - self.inbound.len()).min(scratch.len());
            match stream.read(&mut scratch[..wanted]) {
                Ok(0) if self.inbound.is_empty() => return Err(TransportError::PeerClosed),
                Ok(0) => return Err(TransportError::TruncatedFrame),
                Ok(read) => self.inbound.extend_from_slice(&scratch[..read]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    return Ok(Progress::Pending);
                }
                Err(error) => return Err(TransportError::Io(error)),
            }
        }
        Ok(Progress::Complete)
    }
}

impl<S: Read + Write + Send> OrderedTransport for StreamOrderedTransport<S> {
    fn try_send(&mut self, frame: Vec<u8>) -> Result<FlushReceipt, TransportError> {
        if self.stream.is_none() || self.closing {
            return Err(TransportError::Closed);
        }
        validate_complete_frame(&frame)?;
        if self.outbound.len() >= self.capacity {
            return Err(TransportError::QueueFull);
        }
        let sequence = self
            .sent_high_water
            .checked_add(1)
            .ok_or(TransportError::SequenceExhausted)?;
        self.sent_high_water = sequence;
        self.outbound.push_back(PendingFrame {
            sequence,
            bytes: frame,
            offset: 0,
        });
        Ok(FlushReceipt::new(self.id, sequence))
    }

    fn flush(&mut self, receipt: FlushReceipt) -> Result<Progress, TransportError> {
        self.validate_receipt(receipt)?;
        self.flush_through(receipt.write_sequence())
    }

    fn try_receive(&mut self) -> Result<ReceiveResult, TransportError> {
        if self.stream.is_none() {
            return Ok(ReceiveResult::PeerClosed);
        }
        if self.expected_length.is_none() {
            match self.read_more(LENGTH_PREFIX_BYTES) {
                Ok(Progress::Complete) => {}
                Ok(Progress::Pending) => return Ok(ReceiveResult::Empty),
                Err(TransportError::PeerClosed) => {
                    if self.peer_closed_reported {
                        return Ok(ReceiveResult::Empty);
                    }
                    self.peer_closed_reported = true;
                    return Ok(ReceiveResult::PeerClosed);
                }
                Err(error) => return Err(error),
            }
            let body_length = u32::from_be_bytes(
                self.inbound[..LENGTH_PREFIX_BYTES]
                    .try_into()
                    .expect("four-byte prefix"),
            ) as usize;
            if body_length == 0 || body_length > MAX_BODY_LENGTH {
                return Err(TransportError::InvalidFrame);
            }
            self.expected_length = Some(LENGTH_PREFIX_BYTES + body_length);
        }
        let expected = self.expected_length.expect("length established");
        if self.read_more(expected)? == Progress::Pending {
            return Ok(ReceiveResult::Empty);
        }
        self.expected_length = None;
        Ok(ReceiveResult::Frame(std::mem::take(&mut self.inbound)))
    }

    fn close(&mut self) -> Result<Progress, TransportError> {
        if self.stream.is_none() {
            return Ok(Progress::Complete);
        }
        self.closing = true;
        if self.flushed_high_water < self.sent_high_water
            && self.flush_through(self.sent_high_water)? == Progress::Pending
        {
            return Ok(Progress::Pending);
        }
        self.stream.take();
        Ok(Progress::Complete)
    }

    fn abort(&mut self) {
        self.outbound.clear();
        self.inbound.clear();
        self.stream.take();
        self.closing = true;
    }
}
