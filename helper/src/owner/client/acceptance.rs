//! Feature-gated observability without acquiring a capture lease.

use super::{ConnectedOwner, OwnerClientError};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use talking_quill_owner_protocol::{
    GatewayMessage,
    client::ClientPoll,
    schema::{Empty, Request, Response, SuccessResult},
};

#[cfg(feature = "windows-installed-acceptance")]
pub fn acceptance_observability_without_lease(
    connected: ConnectedOwner,
    deadline: Instant,
    cancelled: &AtomicBool,
) -> Result<talking_quill_owner_protocol::schema::ObservabilityResult, OwnerClientError> {
    let mut client = connected.client;
    let request = Request::ObservabilityGet(Empty {});
    let correlation = client
        .send_request(&request)
        .map_err(|_| OwnerClientError::Transport)?;
    loop {
        if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
            client.abort();
            return Err(OwnerClientError::Cancelled);
        }
        match client.poll() {
            Ok(ClientPoll::Empty) => std::thread::sleep(Duration::from_millis(1)),
            Ok(ClientPoll::PeerClosed) => return Err(OwnerClientError::Disconnected),
            Ok(ClientPoll::Message(GatewayMessage::Response {
                correlation_sequence,
                response,
            })) if correlation_sequence == correlation => match response {
                Response::Success(SuccessResult::Observability(value)) => return Ok(*value),
                Response::Success(_) => return Err(OwnerClientError::Protocol),
                Response::Error(error) => return Err(OwnerClientError::Rejected(error.code())),
            },
            Ok(ClientPoll::Message(GatewayMessage::Response { .. })) => {
                client.abort();
                return Err(OwnerClientError::Protocol);
            }
            Ok(ClientPoll::Message(_)) => {}
            Err(_) => {
                client.abort();
                return Err(OwnerClientError::Transport);
            }
        }
    }
}
