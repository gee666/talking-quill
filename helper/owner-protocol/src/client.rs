//! Transport-independent owner protocol client library.
//!
//! Endpoint discovery, process launch, credential loading, reconnect, and
//! mutation retry behavior deliberately live outside this crate.

use std::{fmt, marker::PhantomData};

use thiserror::Error;

use crate::schema::Request;
use crate::session::{GatewayMessage, GatewaySessionCodec, SessionCodecError};
use crate::transport::{FlushReceipt, OrderedTransport, Progress, ReceiveResult, TransportError};

pub struct OwnerProtocolClient<'a> {
    transport: Box<dyn OrderedTransport>,
    codec: GatewaySessionCodec,
    pending_flush: Option<FlushReceipt>,
    closing: bool,
    closed: bool,
    lifetime: PhantomData<&'a ()>,
}

impl fmt::Debug for OwnerProtocolClient<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OwnerProtocolClient(<redacted>)")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientPoll {
    Empty,
    PeerClosed,
    Message(GatewayMessage),
}

impl<'a> OwnerProtocolClient<'a> {
    #[must_use]
    pub const fn supports_feature(&self, feature: u64) -> bool {
        self.codec.supports_feature(feature)
    }

    pub fn new<T>(transport: T, codec: GatewaySessionCodec) -> Result<Self, ClientError>
    where
        T: OrderedTransport + 'static,
    {
        if transport.is_test_only() != codec.is_test_only() {
            return Err(ClientError::TransportAuthenticationBoundary);
        }
        Ok(Self {
            transport: Box::new(transport),
            codec,
            pending_flush: None,
            closing: false,
            closed: false,
            lifetime: PhantomData,
        })
    }

    /// Encodes and accepts a request exactly once. A temporarily pending flush
    /// is serviced by [`Self::poll`]; callers must never retry the mutation.
    pub fn send_request(&mut self, request: &Request) -> Result<u64, ClientError> {
        if self.closed || self.closing {
            return Err(ClientError::Closed);
        }
        if self.pending_flush.is_some() {
            return Err(ClientError::Backpressured);
        }
        let encoded = self.codec.encode_request(request).map_err(|error| {
            self.fatal_close();
            ClientError::Codec(error)
        })?;
        let sequence = encoded.transport_sequence();
        let receipt = self
            .transport
            .try_send(encoded.into_frame())
            .map_err(|error| {
                self.fatal_close();
                ClientError::Transport(error)
            })?;
        match self.transport.flush(receipt).map_err(|error| {
            self.fatal_close();
            ClientError::Transport(error)
        })? {
            Progress::Complete => {}
            Progress::Pending => self.pending_flush = Some(receipt),
        }
        Ok(sequence)
    }

    pub fn poll(&mut self) -> Result<ClientPoll, ClientError> {
        if self.closed {
            return Ok(ClientPoll::PeerClosed);
        }
        if let Some(receipt) = self.pending_flush {
            match self.transport.flush(receipt).map_err(|error| {
                self.fatal_close();
                ClientError::Transport(error)
            })? {
                Progress::Pending => return Ok(ClientPoll::Empty),
                Progress::Complete => self.pending_flush = None,
            }
        }
        if self.closing {
            return match self.transport.close().map_err(ClientError::Transport)? {
                Progress::Pending => Ok(ClientPoll::Empty),
                Progress::Complete => {
                    self.closed = true;
                    Ok(ClientPoll::PeerClosed)
                }
            };
        }
        match self.transport.try_receive().map_err(|error| {
            self.fatal_close();
            ClientError::Transport(error)
        })? {
            ReceiveResult::Empty => Ok(ClientPoll::Empty),
            ReceiveResult::PeerClosed => {
                self.closed = true;
                Ok(ClientPoll::PeerClosed)
            }
            ReceiveResult::Frame(frame) => {
                let message = self.codec.receive_owner_frame(&frame).map_err(|error| {
                    self.fatal_close();
                    ClientError::Codec(error)
                })?;
                Ok(ClientPoll::Message(message))
            }
        }
    }

    /// Begins a graceful close. Pending accepted bytes are flushed before the
    /// connected stream is released. Call [`Self::poll`] while pending.
    pub fn close(&mut self) -> Result<Progress, ClientError> {
        if self.closed {
            return Ok(Progress::Complete);
        }
        self.closing = true;
        let progress = self.transport.close()?;
        if progress == Progress::Complete {
            self.closed = true;
        }
        Ok(progress)
    }

    pub fn abort(&mut self) {
        self.fatal_close();
    }

    #[must_use]
    pub const fn is_closed(&self) -> bool {
        self.closed
    }

    #[must_use]
    pub fn outstanding_requests(&self) -> usize {
        self.codec.outstanding_requests()
    }

    fn fatal_close(&mut self) {
        self.transport.abort();
        self.pending_flush = None;
        self.closed = true;
    }
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("owner-protocol client is closed")]
    Closed,
    #[error("test authentication cannot cross a production transport boundary")]
    TransportAuthenticationBoundary,
    #[error("owner-protocol client has an accepted write awaiting flush")]
    Backpressured,
    #[error(transparent)]
    Codec(#[from] SessionCodecError),
    #[error(transparent)]
    Transport(#[from] TransportError),
}
