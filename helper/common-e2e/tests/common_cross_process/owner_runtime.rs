use super::connection::{OWNER_INSTANCE, TestTcpTransport, material};
use super::native::{Capabilities, FakeNativeAdapter};
use super::process::address_from_env;
use std::io::Read;
use std::net::TcpListener;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use talking_quill_keyboard_owner::{
    ActivationCaptureGate as OwnerCaptureGate, NativeAdapterExecutor, NoRuntimeSignals,
    OwnerProtocolServer, OwnerRuntime, ProcessSingleton,
};
use talking_quill_owner_protocol::StreamOrderedTransport;
use talking_quill_owner_protocol::schema::Purpose;

/// Test-only connection source: it bypasses OS peer/code identity and supplies
/// a fake authenticated codec over loopback. It exercises the real owner
/// runtime/state machine, not production endpoint authentication.
#[derive(Debug)]
struct FakeAuthenticatedTcpConnectionSource {
    listener: TcpListener,
    neutral_release: Arc<AtomicBool>,
}
impl talking_quill_keyboard_owner::AuthenticatedConnectionSource
    for FakeAuthenticatedTcpConnectionSource
{
    fn poll_authenticated(
        &mut self,
    ) -> Result<
        Option<talking_quill_keyboard_owner::AuthenticatedConnection>,
        talking_quill_keyboard_owner::ConnectionSourceError,
    > {
        match self.listener.accept() {
            Ok((mut stream, _)) => {
                let mut discriminator = [0_u8; 1];
                stream
                    .read_exact(&mut discriminator)
                    .map_err(|_| talking_quill_keyboard_owner::ConnectionSourceError)?;
                let purpose = match discriminator[0] {
                    1 => Purpose::Capture,
                    2 => Purpose::Maintenance,
                    3 => Purpose::Observe,
                    4 => {
                        self.neutral_release.store(true, Ordering::Release);
                        return Ok(None);
                    }
                    _ => return Err(talking_quill_keyboard_owner::ConnectionSourceError),
                };
                stream
                    .set_nonblocking(true)
                    .map_err(|_| talking_quill_keyboard_owner::ConnectionSourceError)?;
                let (_, codec) = material(purpose, OWNER_INSTANCE[0])
                    .codecs()
                    .map_err(|_| talking_quill_keyboard_owner::ConnectionSourceError)?;
                Ok(Some(
                    talking_quill_keyboard_owner::AuthenticatedConnection::new(
                        Box::new(TestTcpTransport(
                            StreamOrderedTransport::new(stream)
                                .map_err(|_| talking_quill_keyboard_owner::ConnectionSourceError)?,
                        )),
                        codec,
                    ),
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(_) => Err(talking_quill_keyboard_owner::ConnectionSourceError),
        }
    }
    fn shutdown_endpoint(
        &mut self,
    ) -> Result<(), talking_quill_keyboard_owner::ConnectionSourceError> {
        Ok(())
    }
}

pub(super) fn run_owner_runtime_with_fake_auth_process() {
    let listener = TcpListener::bind(address_from_env()).unwrap();
    listener.set_nonblocking(true).unwrap();
    let neutral_release = Arc::new(AtomicBool::new(false));
    let executor = NativeAdapterExecutor::new_for_test(
        FakeNativeAdapter::new(Arc::clone(&neutral_release)),
        Capabilities::default(),
        OwnerCaptureGate::open_for_test_harness(),
    );
    let server = OwnerProtocolServer::start(
        talking_quill_keyboard_owner::state::OwnerInstanceId::new(OWNER_INSTANCE).unwrap(),
        executor,
    )
    .unwrap();
    let mut runtime = OwnerRuntime::from_parts(
        server,
        FakeAuthenticatedTcpConnectionSource {
            listener,
            neutral_release,
        },
        NoRuntimeSignals,
        ProcessSingleton::default(),
    )
    .unwrap();
    runtime.run().unwrap();
}
