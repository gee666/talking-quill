use super::{Server, messages::*, params::EmptyParams};
use crate::{
    CriticalDelivery,
    gateway::{GatewayBackend, PlatformError, TerminalReason},
};
use crossbeam_channel::{Sender, bounded};
use serde::{Serialize, de::DeserializeOwned};
use std::time::Duration;

const CRITICAL_ACQUISITION_TIMEOUT: Duration = Duration::from_millis(250);

impl<P: GatewayBackend> Server<P> {
    pub(super) fn params<T: DeserializeOwned>(&self, request: &Request) -> Result<T, bool> {
        match serde_json::from_str(request.params.get()) {
            Ok(params) => Ok(params),
            Err(_) => Err(self.send_error(request.id.clone(), RpcError::invalid_params())),
        }
    }

    pub(super) fn empty_params(&self, request: &Request) -> Result<(), bool> {
        self.params::<EmptyParams>(request).map(|_| ())
    }

    pub(super) fn send_success<T: Serialize>(&self, id: RequestId, value: T) -> bool {
        self.send(success_outbound(id, value))
    }

    pub(super) fn send_platform_error(&self, id: RequestId, error: PlatformError) -> bool {
        let error = match error {
            PlatformError::OwnerUnavailable | PlatformError::NativeFailure => {
                RpcError::native_unavailable()
            }
            PlatformError::OwnerAuthentication => RpcError::owner_authentication(),
            PlatformError::OwnerIncompatible => RpcError::owner_incompatible(),
            PlatformError::OwnerBusy => RpcError::owner_busy(),
            PlatformError::OwnerSingletonCollision => RpcError::owner_singleton_collision(),
            PlatformError::OwnerDraining => RpcError::owner_draining(),
            PlatformError::OwnerRollback => RpcError::owner_rollback(),
            PlatformError::OwnerSecurityFault => RpcError::owner_security_fault(),
            PlatformError::Indeterminate => RpcError::indeterminate(),
        };
        self.send_error(id, error)
    }

    pub(super) fn send_error(&self, id: RequestId, error: RpcError) -> bool {
        self.send(Outbound::Response(RpcResponse::error(Some(id), error)))
    }

    pub(super) fn reserve_critical_delivery(&self) -> Option<Sender<Vec<Outbound>>> {
        let (acquired_tx, acquired_rx) = bounded(1);
        let (completion_tx, completion_rx) = bounded(1);
        if self
            .critical_outbound
            .try_send(CriticalDelivery::new(acquired_tx, completion_rx))
            .is_err()
            || acquired_rx
                .recv_timeout(CRITICAL_ACQUISITION_TIMEOUT)
                .is_err()
        {
            self.terminal
                .trigger(TerminalReason::OutboundQueueUnavailable);
            return None;
        }
        if self.terminal.is_triggered() {
            return None;
        }
        Some(completion_tx)
    }

    pub(super) fn complete_critical_delivery(
        &self,
        delivery: Sender<Vec<Outbound>>,
        batch: Vec<Outbound>,
    ) -> bool {
        if delivery.send(batch).is_ok() {
            true
        } else {
            self.terminal.trigger(TerminalReason::StdoutDisconnected);
            false
        }
    }

    pub(super) fn send(&self, outbound: Outbound) -> bool {
        self.send_to(&self.outbound, outbound)
    }

    pub(super) fn send_final(&self, outbound: Outbound) -> bool {
        self.send_to(&self.final_outbound, outbound)
    }

    fn send_to(&self, sender: &Sender<Outbound>, outbound: Outbound) -> bool {
        // Final guard for every server-produced message. Oversized success
        // results have already been replaced; never recurse if even a fallback
        // or future message violates the bound.
        if encode_outbound(&outbound).is_err() {
            self.terminal
                .trigger(TerminalReason::OutboundEncodingUnavailable);
            return false;
        }
        if sender.try_send(outbound).is_ok() {
            true
        } else {
            self.terminal
                .trigger(TerminalReason::OutboundQueueUnavailable);
            false
        }
    }
}

pub(super) fn success_outbound<T: Serialize>(id: RequestId, value: T) -> Outbound {
    let response_id = id.clone();
    match RpcResponse::success(id, value) {
        Ok(response) => {
            let outbound = Outbound::Response(response);
            match encode_outbound(&outbound) {
                Ok(_) => outbound,
                Err(OutboundEncodingError::FrameTooLarge(_)) => Outbound::Response(
                    RpcResponse::error(Some(response_id), RpcError::response_too_large()),
                ),
                Err(OutboundEncodingError::Serialization(_)) => {
                    Outbound::Response(RpcResponse::error(None, RpcError::internal_error()))
                }
            }
        }
        Err(_) => Outbound::Response(RpcResponse::error(None, RpcError::internal_error())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::MAX_FRAME_BYTES;

    #[test]
    fn oversized_success_becomes_stable_bounded_error_without_recursion() {
        let outbound = success_outbound(RequestId::for_test(7), "\u{0001}".repeat(MAX_FRAME_BYTES));
        let payload = encode_outbound(&outbound).unwrap();
        assert!(payload.len() < MAX_FRAME_BYTES);
        let response: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(response["id"], 7);
        assert_eq!(response["error"]["code"], -32_004);
        assert_eq!(response["error"]["message"], "Response too large");
        assert!(response.get("result").is_none());
    }
}
