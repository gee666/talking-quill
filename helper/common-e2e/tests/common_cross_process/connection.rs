use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;
use talking_quill_helper::owner::client::{ConnectError, ConnectedOwner, OwnerConnector};
use talking_quill_owner_protocol::client::OwnerProtocolClient;
use talking_quill_owner_protocol::schema::{
    Binding, BindingShortcut, Bindings, Letter, Modifiers, ProfileId, Purpose,
};
use talking_quill_owner_protocol::{
    Bytes32, FakeAuthenticatedMaterial, FlushReceipt, OrderedTransport, ReceiveResult,
    StreamOrderedTransport, TransportError, TransportProgress,
};
pub(super) const OWNER_INSTANCE: [u8; 32] = [42; 32];
pub(super) const FIRST_FAKE_OWNER_TAG: u8 = 51;
pub(super) const REPLACEMENT_FAKE_OWNER_TAG: u8 = 52;
pub(super) const EXPECTED_FAKE_OWNER_CRASH_EXIT: i32 = 86;

#[derive(Debug)]
pub(super) struct TestTcpTransport(pub(super) StreamOrderedTransport<TcpStream>);

impl OrderedTransport for TestTcpTransport {
    fn is_test_only(&self) -> bool {
        true
    }

    fn try_send(&mut self, frame: Vec<u8>) -> Result<FlushReceipt, TransportError> {
        self.0.try_send(frame)
    }

    fn flush(&mut self, receipt: FlushReceipt) -> Result<TransportProgress, TransportError> {
        self.0.flush(receipt)
    }

    fn try_receive(&mut self) -> Result<ReceiveResult, TransportError> {
        self.0.try_receive()
    }

    fn close(&mut self) -> Result<TransportProgress, TransportError> {
        self.0.close()
    }

    fn abort(&mut self) {
        self.0.abort();
    }
}

pub(super) fn material(purpose: Purpose, owner_tag: u8) -> FakeAuthenticatedMaterial {
    let purpose_tag = match purpose {
        Purpose::Capture => 1,
        Purpose::Maintenance => 2,
        Purpose::Observe => 3,
    };
    FakeAuthenticatedMaterial::new_with_owner_instance(
        Bytes32::new([owner_tag.wrapping_add(purpose_tag); 32]),
        Bytes32::new([owner_tag; 32]),
        purpose,
        [owner_tag.wrapping_add(10 + purpose_tag); 32],
        [owner_tag.wrapping_add(20 + purpose_tag); 32],
    )
}

pub(super) fn wire_bindings() -> Bindings {
    Bindings::new(vec![Binding::new(
        ProfileId::new("general".into()).unwrap(),
        BindingShortcut::new(Modifiers::new(false, true, false, false), vec![Letter::X]).unwrap(),
    )])
    .unwrap()
}

/// Test-only connector using `FakeAuthenticatedMaterial`; it does not exercise
/// Windows/macOS production peer, code-identity, or credential authentication.
pub(super) struct TcpConnector {
    address: SocketAddr,
    owner_tag: u8,
    rotate_capture_identity: bool,
    capture_attempt: u8,
}

impl TcpConnector {
    pub(super) fn fixed(address: SocketAddr, owner_tag: u8) -> Self {
        Self {
            address,
            owner_tag,
            rotate_capture_identity: false,
            capture_attempt: 0,
        }
    }

    pub(super) fn rotating_fake_owners(address: SocketAddr) -> Self {
        Self {
            address,
            owner_tag: FIRST_FAKE_OWNER_TAG,
            rotate_capture_identity: true,
            capture_attempt: 0,
        }
    }

    fn connected(&self, purpose: Purpose, owner_tag: u8) -> Result<ConnectedOwner, ConnectError> {
        const CONNECT_BUDGET: Duration = Duration::from_millis(25);
        let mut stream = TcpStream::connect_timeout(&self.address, CONNECT_BUDGET)
            .map_err(|_| ConnectError::Unavailable)?;
        stream
            .set_write_timeout(Some(CONNECT_BUDGET))
            .map_err(|_| ConnectError::Unavailable)?;
        stream
            .write_all(&[match purpose {
                Purpose::Capture => 1,
                Purpose::Maintenance => 2,
                Purpose::Observe => 3,
            }])
            .map_err(|_| ConnectError::Unavailable)?;
        stream
            .set_nonblocking(true)
            .map_err(|_| ConnectError::Unavailable)?;
        let (gateway, _) = material(purpose, owner_tag)
            .codecs()
            .map_err(|_| ConnectError::Authentication)?;
        Ok(ConnectedOwner {
            client: OwnerProtocolClient::new(
                TestTcpTransport(
                    StreamOrderedTransport::new(stream).map_err(|_| ConnectError::Unavailable)?,
                ),
                gateway,
            )
            .map_err(|_| ConnectError::Authentication)?,
            build_id: "common-e2e-build".into(),
        })
    }
}

impl OwnerConnector for TcpConnector {
    fn connect_capture(&mut self) -> Result<ConnectedOwner, ConnectError> {
        let owner_tag = if self.rotate_capture_identity {
            self.owner_tag
                .checked_add(self.capture_attempt)
                .ok_or(ConnectError::Unavailable)?
        } else {
            self.owner_tag
        };
        let connected = self.connected(Purpose::Capture, owner_tag);
        if connected.is_ok() && self.rotate_capture_identity {
            self.capture_attempt = self
                .capture_attempt
                .checked_add(1)
                .ok_or(ConnectError::Unavailable)?;
        }
        connected
    }

    fn connect_maintenance(&mut self) -> Result<ConnectedOwner, ConnectError> {
        self.connected(Purpose::Maintenance, self.owner_tag)
    }
}
