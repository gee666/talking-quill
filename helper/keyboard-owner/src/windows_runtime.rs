//! Windows per-WTS-session singleton and session-lifetime signals.

use std::fmt;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::System::RemoteDesktop::{
    ProcessIdToSessionId, WTS_CURRENT_SERVER_HANDLE, WTSActive, WTSConnectState, WTSFreeMemory,
    WTSQuerySessionInformationW,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcessId, ReleaseMutex, WaitForSingleObject,
};

use crate::runtime::{
    OsRuntimeSignals, RuntimeError, RuntimeSignal, RuntimeSignalSource, SingletonCoordinator,
    SingletonError,
};

pub struct WindowsSessionSingleton {
    handle: HANDLE,
    held: bool,
    preserve_until_process_exit: bool,
}

unsafe impl Send for WindowsSessionSingleton {}

impl fmt::Debug for WindowsSessionSingleton {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WindowsSessionSingleton(<redacted>)")
    }
}

impl WindowsSessionSingleton {
    pub fn for_current_session() -> Result<Self, SingletonError> {
        let mut session = 0_u32;
        if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) } == 0 {
            return Err(SingletonError);
        }
        Self::with_name(format!(
            "Local\\TalkingQuill.KeyboardOwner.Personal.V1.{session}"
        ))
    }

    fn with_name(name: String) -> Result<Self, SingletonError> {
        let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(SingletonError);
        }
        Ok(Self {
            handle,
            held: false,
            preserve_until_process_exit: false,
        })
    }
}

impl SingletonCoordinator for WindowsSessionSingleton {
    fn try_acquire(&mut self) -> Result<bool, SingletonError> {
        if self.held {
            return Ok(true);
        }
        match unsafe { WaitForSingleObject(self.handle, 0) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => {
                self.held = true;
                Ok(true)
            }
            WAIT_TIMEOUT => Ok(false),
            _ => Err(SingletonError),
        }
    }

    fn release(&mut self) {
        if self.held && !self.preserve_until_process_exit {
            unsafe { ReleaseMutex(self.handle) };
            self.held = false;
        }
    }

    fn preserve_process_lifetime(&mut self) {
        self.preserve_until_process_exit = true;
    }
}

impl Drop for WindowsSessionSingleton {
    fn drop(&mut self) {
        self.release();
        if self.preserve_until_process_exit {
            return;
        }
        if !self.handle.is_null() {
            unsafe { CloseHandle(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

#[derive(Debug)]
pub struct WindowsSessionRuntimeSignals {
    os: OsRuntimeSignals,
    session_id: u32,
    next_poll: Instant,
}

impl WindowsSessionRuntimeSignals {
    pub fn install() -> Result<Self, RuntimeError> {
        let mut session_id = 0;
        if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session_id) } == 0
            || !session_is_active(session_id)
        {
            return Err(RuntimeError::SignalInstall);
        }
        Ok(Self {
            os: OsRuntimeSignals::install()?,
            session_id,
            next_poll: Instant::now(),
        })
    }
}

impl RuntimeSignalSource for WindowsSessionRuntimeSignals {
    fn poll_signal(&mut self) -> Option<RuntimeSignal> {
        if let Some(signal) = self.os.poll_signal() {
            return Some(signal);
        }
        let now = Instant::now();
        if now < self.next_poll {
            return None;
        }
        self.next_poll = now + Duration::from_millis(250);
        (!session_is_active(self.session_id)).then_some(RuntimeSignal::SessionEnded)
    }
}

fn session_is_active(session_id: u32) -> bool {
    let mut buffer = std::ptr::null_mut();
    let mut bytes = 0;
    let ok = unsafe {
        WTSQuerySessionInformationW(
            WTS_CURRENT_SERVER_HANDLE,
            session_id,
            WTSConnectState,
            &mut buffer,
            &mut bytes,
        )
    };
    if ok == 0 || buffer.is_null() || bytes < std::mem::size_of::<i32>() as u32 {
        if !buffer.is_null() {
            unsafe { WTSFreeMemory(buffer.cast()) };
        }
        return false;
    }
    let state = unsafe { *buffer.cast::<i32>() };
    unsafe { WTSFreeMemory(buffer.cast()) };
    state == WTSActive
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    use super::*;

    const FIXTURE_TEST: &str = "windows_runtime::tests::singleton_subprocess_fixture";

    fn singleton_test_name(label: &str) -> String {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).expect("test namespace randomness");
        let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        format!("Local\\TalkingQuill.KeyboardOwner.Test.{suffix}.{label}")
    }

    fn run_contender(name: &str, expected: &str) {
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", FIXTURE_TEST, "--nocapture"])
            .env("TQ_SINGLETON_FIXTURE", "contend")
            .env("TQ_SINGLETON_NAME", name)
            .env("TQ_SINGLETON_EXPECTED", expected)
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn wait_for_fixture_state(path: &Path, expected: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if std::fs::read_to_string(path).is_ok_and(|value| value == expected) {
                return;
            }
            assert!(Instant::now() < deadline, "singleton fixture timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn spawn_holder(name: &str, poison: bool) -> (std::process::Child, PathBuf) {
        let state = PathBuf::from("../tmp").join(format!(
            "singleton-fixture-{}-{}.txt",
            std::process::id(),
            if poison { "poison" } else { "release" }
        ));
        let _ = std::fs::remove_file(&state);
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", FIXTURE_TEST, "--nocapture"])
            .env("TQ_SINGLETON_FIXTURE", "hold")
            .env("TQ_SINGLETON_NAME", name)
            .env("TQ_SINGLETON_POISON", if poison { "1" } else { "0" })
            .env("TQ_SINGLETON_STATE", &state)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        wait_for_fixture_state(&state, "READY");
        (child, state)
    }

    #[test]
    fn singleton_subprocess_fixture() {
        let Ok(action) = std::env::var("TQ_SINGLETON_FIXTURE") else {
            return;
        };
        let name = std::env::var("TQ_SINGLETON_NAME").unwrap();
        let mut singleton = WindowsSessionSingleton::with_name(name).unwrap();
        match action.as_str() {
            "contend" => {
                let acquired = singleton.try_acquire().unwrap();
                assert_eq!(
                    acquired,
                    std::env::var("TQ_SINGLETON_EXPECTED").unwrap() == "1"
                );
                if acquired {
                    singleton.release();
                }
            }
            "hold" => {
                assert!(singleton.try_acquire().unwrap());
                if std::env::var("TQ_SINGLETON_POISON").as_deref() == Ok("1") {
                    singleton.preserve_process_lifetime();
                }
                let state = std::env::var("TQ_SINGLETON_STATE").unwrap();
                std::fs::write(&state, "READY").unwrap();
                let mut command = String::new();
                std::io::stdin().read_line(&mut command).unwrap();
                if command.trim() == "release" {
                    singleton.release();
                    std::fs::write(&state, "RELEASED").unwrap();
                    command.clear();
                    std::io::stdin().read_line(&mut command).unwrap();
                }
            }
            _ => panic!("unknown singleton fixture action"),
        }
    }

    #[test]
    fn singleton_blocks_takeover_until_explicit_release() {
        let name = singleton_test_name("takeover");
        let (mut holder, state) = spawn_holder(&name, false);
        run_contender(&name, "0");
        writeln!(holder.stdin.as_mut().unwrap(), "release").unwrap();
        holder.stdin.as_mut().unwrap().flush().unwrap();
        wait_for_fixture_state(&state, "RELEASED");
        run_contender(&name, "1");
        drop(holder.stdin.take());
        assert!(holder.wait().unwrap().success());
        std::fs::remove_file(state).unwrap();
    }

    #[test]
    fn singleton_poison_is_held_for_process_lifetime() {
        let name = singleton_test_name("poison");
        let (mut holder, state) = spawn_holder(&name, true);
        run_contender(&name, "0");
        drop(holder.stdin.take());
        assert!(holder.wait().unwrap().success());
        run_contender(&name, "1");
        std::fs::remove_file(state).unwrap();
    }

    #[test]
    fn current_wts_session_is_active_for_native_tests() {
        let mut session = 0;
        assert_ne!(
            unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) },
            0
        );
        assert!(session_is_active(session));
    }
}
