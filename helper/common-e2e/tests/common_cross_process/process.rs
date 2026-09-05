use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
pub(super) const ROLE_ENV: &str = "TALKING_QUILL_COMMON_E2E_ROLE";
const ADDRESS_ENV: &str = "TALKING_QUILL_COMMON_E2E_ADDRESS";

pub(super) fn free_address() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

pub(super) fn connect_retry(address: SocketAddr) -> TcpStream {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match TcpStream::connect(address) {
            Ok(stream) => return stream,
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("could not connect to cross-process fixture: {error}"),
        }
    }
}

pub(super) fn release_authoritative_neutral(address: SocketAddr) {
    let mut control = connect_retry(address);
    control.write_all(&[4]).unwrap();
}

pub(super) struct FixtureChild {
    child: Option<Child>,
    label: String,
}

impl FixtureChild {
    fn new(child: Child, label: impl Into<String>) -> Self {
        Self {
            child: Some(child),
            label: label.into(),
        }
    }

    fn wait_status(&mut self) -> (std::process::ExitStatus, String) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let status = self
                .child
                .as_mut()
                .expect("fixture child present")
                .try_wait()
                .unwrap();
            if let Some(status) = status {
                let mut stderr = String::new();
                if let Some(pipe) = self
                    .child
                    .as_mut()
                    .expect("fixture child present")
                    .stderr
                    .as_mut()
                {
                    let _ = pipe.read_to_string(&mut stderr);
                }
                self.child.take();
                return (status, stderr);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.kill_and_reap();
        panic!("{} did not exit", self.label);
    }

    fn kill_and_reap(&mut self) -> Option<std::process::ExitStatus> {
        let mut child = self.child.take()?;
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        let _ = child.kill();
        child.wait().ok()
    }

    pub(super) fn wait_success(mut self) {
        let (status, stderr) = self.wait_status();
        assert!(
            status.success(),
            "{} failed with {status}: {stderr}",
            self.label
        );
    }

    pub(super) fn wait_exit_code(mut self, expected: i32) {
        let (status, stderr) = self.wait_status();
        assert_eq!(
            status.code(),
            Some(expected),
            "unexpected {} status: {status}; {stderr}",
            self.label
        );
    }

    pub(super) fn terminate(mut self) {
        let status = self.kill_and_reap().expect("fixture child present");
        assert!(
            !status.success(),
            "terminated {} unexpectedly succeeded",
            self.label
        );
    }
}

impl Drop for FixtureChild {
    fn drop(&mut self) {
        let _ = self.kill_and_reap();
    }
}

pub(super) fn spawn_role(role: &str, address: SocketAddr) -> FixtureChild {
    FixtureChild::new(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "common_cross_process_foundation", "--nocapture"])
            .env(ROLE_ENV, role)
            .env(ADDRESS_ENV, address.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
        role,
    )
}

pub(super) fn spawn_gateway_role(
    role: &str,
    gateway_address: SocketAddr,
    owner_address: SocketAddr,
) -> FixtureChild {
    FixtureChild::new(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "common_cross_process_foundation", "--nocapture"])
            .env(ROLE_ENV, role)
            .env(ADDRESS_ENV, gateway_address.to_string())
            .env(
                "TALKING_QUILL_COMMON_E2E_OWNER_ADDRESS",
                owner_address.to_string(),
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
        role,
    )
}

pub(super) fn address_from_env() -> SocketAddr {
    std::env::var(ADDRESS_ENV).unwrap().parse().unwrap()
}
