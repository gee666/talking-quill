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

mod stream;
pub use stream::StreamOrderedTransport;
