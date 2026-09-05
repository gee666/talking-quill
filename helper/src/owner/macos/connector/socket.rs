use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use talking_quill_owner_protocol::schema::Purpose;

use crate::owner::client::ConnectError;

use super::MacosOwnerConnector;

impl MacosOwnerConnector {
    pub(super) fn connect_socket_bounded(
        &self,
        path: &std::path::Path,
        purpose: Purpose,
        deadline: Instant,
    ) -> Result<UnixStream, ConnectError> {
        self.check_capture_connect_allowed(purpose)?;
        let bytes = path.as_os_str().as_bytes();
        let mut address = unsafe { std::mem::zeroed::<libc::sockaddr_un>() };
        if bytes.is_empty() || bytes.len() >= address.sun_path.len() {
            return Err(ConnectError::Unavailable);
        }
        address.sun_family = libc::AF_UNIX as libc::sa_family_t;
        address.sun_len = u8::try_from(std::mem::size_of::<libc::sockaddr_un>())
            .map_err(|_| ConnectError::Unavailable)?;
        for (target, source) in address.sun_path.iter_mut().zip(bytes.iter().copied()) {
            *target = source as libc::c_char;
        }
        let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
        if raw < 0 {
            return Err(ConnectError::Unavailable);
        }
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } != 0
            || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } != 0
        {
            return Err(ConnectError::Unavailable);
        }
        let connected = unsafe {
            libc::connect(
                fd.as_raw_fd(),
                (&raw const address).cast::<libc::sockaddr>(),
                std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t,
            )
        };
        if connected != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINPROGRESS) {
                return Err(ConnectError::Unavailable);
            }
            loop {
                self.check_capture_connect_allowed(purpose)?;
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(ConnectError::Unavailable);
                }
                let mut poll_fd = libc::pollfd {
                    fd: fd.as_raw_fd(),
                    events: libc::POLLOUT,
                    revents: 0,
                };
                let timeout = remaining.min(Duration::from_millis(25)).as_millis() as libc::c_int;
                let ready = unsafe { libc::poll(&raw mut poll_fd, 1, timeout) };
                if ready < 0 {
                    return Err(ConnectError::Unavailable);
                }
                if ready == 0 {
                    continue;
                }
                let mut socket_error = 0;
                let mut length = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
                if unsafe {
                    libc::getsockopt(
                        fd.as_raw_fd(),
                        libc::SOL_SOCKET,
                        libc::SO_ERROR,
                        (&raw mut socket_error).cast(),
                        &raw mut length,
                    )
                } != 0
                    || socket_error != 0
                {
                    return Err(ConnectError::Unavailable);
                }
                break;
            }
        }
        Ok(UnixStream::from(fd))
    }
}
