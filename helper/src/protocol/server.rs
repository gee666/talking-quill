use std::sync::Arc;

use crossbeam_channel::Sender;
use serde::Deserialize;

use super::{PROTOCOL_VERSION, messages};
use crate::{
    CriticalDelivery,
    gateway::{ActivationCaptureGate, CallbackGate, GatewayBackend, TerminalSignal},
};
use messages::{Outbound, RequestId};
use observability::PasteCounters;

mod delivery;
mod dispatch;
mod lifecycle;
mod observability;
mod params;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PrepareMaintenanceParams {
    operation: MaintenanceOperation,
    transaction_id: String,
    source_build_id: String,
    #[serde(default)]
    target_build_id: Option<String>,
    #[serde(default)]
    target_owner_sha256: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MaintenanceOperation {
    Update,
    Uninstall,
    Rollback,
}

impl PrepareMaintenanceParams {
    fn valid(&self) -> bool {
        #[cfg(not(target_os = "macos"))]
        let bounded = |value: &str| !value.is_empty() && value.len() <= 128;
        let hash = |value: &str| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        };
        let canonical_id = |value: &str| {
            #[cfg(target_os = "macos")]
            {
                hash(value)
            }
            #[cfg(not(target_os = "macos"))]
            {
                bounded(value)
            }
        };
        canonical_id(&self.transaction_id)
            && canonical_id(&self.source_build_id)
            && match self.operation {
                MaintenanceOperation::Uninstall => {
                    self.target_build_id.is_none() && self.target_owner_sha256.is_none()
                }
                MaintenanceOperation::Update | MaintenanceOperation::Rollback => {
                    self.target_build_id.as_deref().is_some_and(canonical_id)
                        && self.target_owner_sha256.as_deref().is_some_and(hash)
                }
            }
    }
}

fn decode_sha256(value: &str) -> Result<[u8; 32], ()> {
    let mut bytes = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let nibble = |value| match value {
            b'0'..=b'9' => Ok(value - b'0'),
            b'a'..=b'f' => Ok(value - b'a' + 10),
            _ => Err(()),
        };
        bytes[index] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) enum HandleOutcome {
    Continue,
    Shutdown(RequestId),
    Stop,
}

impl HandleOutcome {
    const fn from_keep_running(keep_running: bool) -> Self {
        if keep_running {
            Self::Continue
        } else {
            Self::Stop
        }
    }
}

pub struct Server<P: GatewayBackend> {
    platform: P,
    outbound: Sender<Outbound>,
    critical_outbound: Sender<CriticalDelivery>,
    final_outbound: Sender<Outbound>,
    gate: Arc<CallbackGate>,
    terminal: Arc<TerminalSignal>,
    activation_capture_gate: ActivationCaptureGate,
    initialized: bool,
    platform_stopped: bool,
    final_response_sent: bool,
    activation_enable_requests_blocked: u64,
    session_capture_requests_blocked: u64,
    paste_counters: PasteCounters,
    terminal_observability_collected: bool,
    terminal_observability_authoritative: bool,
}
