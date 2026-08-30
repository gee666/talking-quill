#![cfg(target_os = "macos")]

use std::ffi::{CStr, CString};
#[cfg(test)]
use std::fs;
use std::mem::zeroed;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
#[cfg(test)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::MacosEndpointError;
use super::native_identity::current_audit_token;

pub const SOCKET_FILE: &str = "owner-v1.sock";
pub const SINGLETON_FILE: &str = "owner-v1.lock";
pub const MAINTENANCE_FILE: &str = "maintenance.lock";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SocketInode {
    pub device: u64,
    pub inode: u64,
}

#[derive(Clone, Debug)]
pub struct MacosRuntimeDirectory {
    path: PathBuf,
    directory: Arc<OwnedFd>,
    audit_session_id: u32,
    uid: u32,
}

impl MacosRuntimeDirectory {
    pub fn for_current_audit_session() -> Result<Self, MacosEndpointError> {
        let token = current_audit_token()?;
        let uid = unsafe { libc::geteuid() };
        if token.effective_uid() != uid || token.audit_session_id() == 0 {
            return Err(MacosEndpointError::RuntimeDirectory);
        }
        let home = home_directory(uid)?;
        let home_fd = open_absolute_directory(&home, uid, false)?;
        let library_fd = open_child_directory(home_fd.as_raw_fd(), "Library", uid, false, false)?;
        let support_fd = open_child_directory(
            library_fd.as_raw_fd(),
            "Application Support",
            uid,
            false,
            false,
        )?;
        let managed_fd =
            open_child_directory(support_fd.as_raw_fd(), "Talking Quill", uid, true, true)?;
        let owner_fd =
            open_child_directory(managed_fd.as_raw_fd(), "KeyboardOwner", uid, true, true)?;
        let run_fd = open_child_directory(owner_fd.as_raw_fd(), "run-v1", uid, true, true)?;
        let session_name = token.audit_session_id().to_string();
        let session_fd = open_child_directory(run_fd.as_raw_fd(), &session_name, uid, true, true)?;
        let path = home
            .join("Library")
            .join("Application Support")
            .join("Talking Quill")
            .join("KeyboardOwner")
            .join("run-v1")
            .join(session_name);
        Ok(Self {
            path,
            directory: Arc::new(session_fd),
            audit_session_id: token.audit_session_id(),
            uid,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn socket_path(&self) -> PathBuf {
        self.path.join(SOCKET_FILE)
    }

    #[must_use]
    pub fn maintenance_path(&self) -> PathBuf {
        self.path.join(MAINTENANCE_FILE)
    }

    #[must_use]
    pub const fn audit_session_id(&self) -> u32 {
        self.audit_session_id
    }

    #[must_use]
    pub const fn uid(&self) -> u32 {
        self.uid
    }

    pub fn revalidate(&self) -> Result<(), MacosEndpointError> {
        validate_directory_fd(self.directory.as_raw_fd(), self.uid, true)
    }

    pub(crate) fn open_lock_file(&self, name: &str) -> Result<OwnedFd, MacosEndpointError> {
        self.revalidate()?;
        let name = CString::new(name).map_err(|_| MacosEndpointError::RuntimeDirectory)?;
        // SAFETY: retained directory fd and one-component NUL-terminated name
        // are valid. O_NOFOLLOW prevents substituting a symlink.
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_CREAT | libc::O_RDWR | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if fd < 0 {
            return Err(MacosEndpointError::RuntimeDirectory);
        }
        // SAFETY: fd is newly owned on this path.
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        let stat = fstat(owned.as_raw_fd())?;
        if (stat.st_mode & libc::S_IFMT) != libc::S_IFREG
            || stat.st_uid != self.uid
            || stat.st_nlink != 1
            || stat.st_mode & 0o077 != 0
        {
            return Err(MacosEndpointError::RuntimeDirectory);
        }
        Ok(owned)
    }

    pub(crate) fn socket_inode(&self) -> Result<Option<SocketInode>, MacosEndpointError> {
        self.entry_inode(SOCKET_FILE, true)
    }

    pub(crate) fn socket_inode_named(
        &self,
        name: &str,
    ) -> Result<Option<SocketInode>, MacosEndpointError> {
        self.entry_inode(name, true)
    }

    pub(crate) fn verify_listener_address(
        &self,
        listener_fd: libc::c_int,
        expected_path: &Path,
    ) -> Result<(), MacosEndpointError> {
        self.revalidate()?;
        let expected = expected_path.as_os_str().as_bytes();
        let mut address: libc::sockaddr_un = unsafe { zeroed() };
        let mut length = std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t;
        if unsafe {
            libc::getsockname(
                listener_fd,
                (&raw mut address).cast::<libc::sockaddr>(),
                &raw mut length,
            )
        } != 0
            || address.sun_family != libc::AF_UNIX as libc::sa_family_t
        {
            return Err(MacosEndpointError::RuntimeDirectory);
        }
        let path_offset = std::mem::offset_of!(libc::sockaddr_un, sun_path);
        let available = usize::try_from(length)
            .ok()
            .and_then(|value| value.checked_sub(path_offset))
            .ok_or(MacosEndpointError::RuntimeDirectory)?
            .min(address.sun_path.len());
        let actual = unsafe {
            std::slice::from_raw_parts(address.sun_path.as_ptr().cast::<u8>(), available)
        };
        let actual_length = actual
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(actual.len());
        if &actual[..actual_length] != expected {
            return Err(MacosEndpointError::RuntimeDirectory);
        }
        Ok(())
    }

    pub(crate) fn verify_published_socket_inode(
        &self,
        expected: SocketInode,
    ) -> Result<(), MacosEndpointError> {
        if self.socket_inode()? == Some(expected) {
            Ok(())
        } else {
            Err(MacosEndpointError::RuntimeDirectory)
        }
    }

    pub(crate) fn set_socket_private(&self, name: &str) -> Result<(), MacosEndpointError> {
        let name = CString::new(name).map_err(|_| MacosEndpointError::RuntimeDirectory)?;
        // SAFETY: operation is scoped to the retained private directory.
        if unsafe {
            libc::fchmodat(
                self.directory.as_raw_fd(),
                name.as_ptr(),
                0o600,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Err(MacosEndpointError::RuntimeDirectory);
        }
        Ok(())
    }

    pub(crate) fn install_socket_name(&self, temporary: &str) -> Result<(), MacosEndpointError> {
        let temporary =
            CString::new(temporary).map_err(|_| MacosEndpointError::RuntimeDirectory)?;
        let socket = CString::new(SOCKET_FILE).expect("fixed socket name");
        // SAFETY: names are beneath the retained private directory and EXCL
        // proves no public endpoint is overwritten.
        if unsafe {
            libc::renameatx_np(
                self.directory.as_raw_fd(),
                temporary.as_ptr(),
                self.directory.as_raw_fd(),
                socket.as_ptr(),
                libc::RENAME_EXCL,
            )
        } != 0
        {
            return Err(MacosEndpointError::RuntimeDirectory);
        }
        Ok(())
    }

    pub(crate) fn quarantine_socket_name(&self, name: &str) -> Result<(), MacosEndpointError> {
        self.revalidate()?;
        static NEXT_UNKNOWN_QUARANTINE: std::sync::atomic::AtomicU64 =
            std::sync::atomic::AtomicU64::new(1);
        let sequence = NEXT_UNKNOWN_QUARANTINE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let source = CString::new(name).map_err(|_| MacosEndpointError::RuntimeDirectory)?;
        let quarantine = CString::new(format!(
            ".owner-v1.sock.unknown-{}-{sequence}",
            std::process::id()
        ))
        .map_err(|_| MacosEndpointError::RuntimeDirectory)?;
        if unsafe {
            libc::renameatx_np(
                self.directory.as_raw_fd(),
                source.as_ptr(),
                self.directory.as_raw_fd(),
                quarantine.as_ptr(),
                libc::RENAME_EXCL,
            )
        } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT)
        {
            Ok(())
        } else {
            Err(MacosEndpointError::RuntimeDirectory)
        }
    }

    /// Atomically moves the pathname to a private quarantine name, verifies the
    /// moved inode, and unlinks only that verified inode. If a replacement won
    /// the race, it is restored when possible and is never unlinked.
    pub(crate) fn unlink_socket_if_matches(
        &self,
        expected: SocketInode,
    ) -> Result<bool, MacosEndpointError> {
        self.unlink_named_socket_if_matches(SOCKET_FILE, expected)
    }

    pub(crate) fn unlink_named_socket_if_matches(
        &self,
        name: &str,
        expected: SocketInode,
    ) -> Result<bool, MacosEndpointError> {
        self.revalidate()?;
        static NEXT_QUARANTINE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let sequence = NEXT_QUARANTINE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let quarantine = format!(".owner-v1.sock.cleanup-{}-{sequence}", std::process::id());
        let socket = CString::new(name).map_err(|_| MacosEndpointError::RuntimeDirectory)?;
        let quarantine_c =
            CString::new(quarantine.as_str()).map_err(|_| MacosEndpointError::RuntimeDirectory)?;
        // SAFETY: both names are one-component paths beneath the retained fd;
        // RENAME_EXCL prevents clobbering an existing quarantine entry.
        if unsafe {
            libc::renameatx_np(
                self.directory.as_raw_fd(),
                socket.as_ptr(),
                self.directory.as_raw_fd(),
                quarantine_c.as_ptr(),
                libc::RENAME_EXCL,
            )
        } != 0
        {
            return if std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
                Ok(false)
            } else {
                Err(MacosEndpointError::RuntimeDirectory)
            };
        }
        let moved = self.entry_inode(&quarantine, true)?;
        if moved == Some(expected) {
            // SAFETY: quarantine now names the exact verified inode.
            if unsafe { libc::unlinkat(self.directory.as_raw_fd(), quarantine_c.as_ptr(), 0) } != 0
            {
                return Err(MacosEndpointError::RuntimeDirectory);
            }
            return Ok(true);
        }
        // A replacement was moved. Restore without replacing anything that may
        // now occupy the public socket name; never unlink the moved replacement.
        let _ = unsafe {
            libc::renameatx_np(
                self.directory.as_raw_fd(),
                quarantine_c.as_ptr(),
                self.directory.as_raw_fd(),
                socket.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        Ok(false)
    }

    fn entry_inode(
        &self,
        name: &str,
        require_socket: bool,
    ) -> Result<Option<SocketInode>, MacosEndpointError> {
        let name = CString::new(name).map_err(|_| MacosEndpointError::RuntimeDirectory)?;
        let mut stat: libc::stat = unsafe { zeroed() };
        // SAFETY: retained directory, fixed name, and writable stat are valid.
        let status = unsafe {
            libc::fstatat(
                self.directory.as_raw_fd(),
                name.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if status != 0 {
            return if std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
                Ok(None)
            } else {
                Err(MacosEndpointError::RuntimeDirectory)
            };
        }
        if (require_socket && (stat.st_mode & libc::S_IFMT) != libc::S_IFSOCK)
            || stat.st_uid != self.uid
        {
            return Err(MacosEndpointError::RuntimeDirectory);
        }
        Ok(Some(SocketInode {
            device: stat.st_dev as u64,
            inode: stat.st_ino,
        }))
    }

    #[cfg(test)]
    pub(crate) fn for_test_path(path: PathBuf) -> Result<Self, MacosEndpointError> {
        let token = current_audit_token()?;
        let uid = unsafe { libc::geteuid() };
        fs::create_dir_all(&path).map_err(|_| MacosEndpointError::RuntimeDirectory)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .map_err(|_| MacosEndpointError::RuntimeDirectory)?;
        let directory = open_absolute_directory(&path, uid, true)?;
        Ok(Self {
            path,
            directory: Arc::new(directory),
            audit_session_id: token.audit_session_id(),
            uid,
        })
    }
}

fn home_directory(uid: u32) -> Result<PathBuf, MacosEndpointError> {
    // SAFETY: startup immediately copies getpwuid's process-owned result.
    let entry = unsafe { libc::getpwuid(uid) };
    if entry.is_null() || unsafe { (*entry).pw_dir }.is_null() {
        return Err(MacosEndpointError::RuntimeDirectory);
    }
    let bytes = unsafe { CStr::from_ptr((*entry).pw_dir) }.to_bytes();
    if bytes.is_empty() {
        return Err(MacosEndpointError::RuntimeDirectory);
    }
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

fn open_absolute_directory(
    path: &Path,
    uid: u32,
    private: bool,
) -> Result<OwnedFd, MacosEndpointError> {
    use std::path::Component;

    if !path.is_absolute()
        || path
            .as_os_str()
            .as_bytes()
            .split(|byte| *byte == b'/')
            .any(|component| matches!(component, b"." | b".."))
    {
        return Err(MacosEndpointError::RuntimeDirectory);
    }
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return Err(MacosEndpointError::RuntimeDirectory);
    }
    let root = CString::new("/").expect("fixed root");
    let fd = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(MacosEndpointError::RuntimeDirectory);
    }
    let mut owned = unsafe { OwnedFd::from_raw_fd(fd) };
    for component in components {
        let Component::Normal(name) = component else {
            return Err(MacosEndpointError::RuntimeDirectory);
        };
        let name =
            CString::new(name.as_bytes()).map_err(|_| MacosEndpointError::RuntimeDirectory)?;
        let child = unsafe {
            libc::openat(
                owned.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if child < 0 {
            return Err(MacosEndpointError::RuntimeDirectory);
        }
        owned = unsafe { OwnedFd::from_raw_fd(child) };
    }
    validate_directory_fd(owned.as_raw_fd(), uid, private)?;
    Ok(owned)
}

fn open_child_directory(
    parent: libc::c_int,
    name: &str,
    uid: u32,
    create: bool,
    private: bool,
) -> Result<OwnedFd, MacosEndpointError> {
    let name = CString::new(name).map_err(|_| MacosEndpointError::RuntimeDirectory)?;
    // SAFETY: retained parent and one-component name are valid.
    let mut fd = unsafe {
        libc::openat(
            parent,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 && create && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
        // SAFETY: mkdirat is scoped to the retained parent and fixed component.
        if unsafe { libc::mkdirat(parent, name.as_ptr(), 0o700) } != 0
            && std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST)
        {
            return Err(MacosEndpointError::RuntimeDirectory);
        }
        fd = unsafe {
            libc::openat(
                parent,
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
    }
    if fd < 0 {
        return Err(MacosEndpointError::RuntimeDirectory);
    }
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    validate_directory_fd(owned.as_raw_fd(), uid, private)?;
    Ok(owned)
}

fn validate_directory_fd(
    fd: libc::c_int,
    uid: u32,
    private: bool,
) -> Result<(), MacosEndpointError> {
    let stat = fstat(fd)?;
    if (stat.st_mode & libc::S_IFMT) != libc::S_IFDIR
        || stat.st_uid != uid
        || stat.st_mode & 0o022 != 0
        || (private && stat.st_mode & 0o077 != 0)
    {
        return Err(MacosEndpointError::RuntimeDirectory);
    }
    Ok(())
}

fn fstat(fd: libc::c_int) -> Result<libc::stat, MacosEndpointError> {
    let mut stat: libc::stat = unsafe { zeroed() };
    // SAFETY: fd is retained and stat is writable.
    if unsafe { libc::fstat(fd, &mut stat) } != 0 {
        Err(MacosEndpointError::RuntimeDirectory)
    } else {
        Ok(stat)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_directory_open_rejects_parent_and_symlink_components() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("tmp")
            .join(format!("runtime-path-{}", std::process::id()));
        let real = root.join("real");
        let link = root.join("link");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&real).expect("real directory");
        std::os::unix::fs::symlink(&real, &link).expect("directory symlink");
        let uid = unsafe { libc::geteuid() };
        assert!(open_absolute_directory(&root.join("real/../real"), uid, false).is_err());
        assert!(open_absolute_directory(&link, uid, false).is_err());
        fs::remove_dir_all(root).expect("remove runtime fixture");
    }

    #[test]
    fn runtime_ancestors_reject_group_or_other_writable_modes() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("tmp")
            .join(format!("runtime-modes-{}", std::process::id()));
        let home = root.join("home");
        let library = home.join("Library");
        let support = library.join("Application Support");
        fs::create_dir_all(&support).unwrap();
        let uid = unsafe { libc::geteuid() };
        fs::set_permissions(&home, fs::Permissions::from_mode(0o702)).unwrap();
        assert!(open_absolute_directory(&home, uid, false).is_err());
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
        for path in [&home, &library, &support] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o720)).unwrap();
            if path == &home {
                assert!(open_absolute_directory(path, uid, false).is_err());
            } else {
                let parent = path.parent().unwrap();
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).unwrap();
                let parent_fd = open_absolute_directory(parent, uid, false).unwrap();
                assert!(
                    open_child_directory(
                        parent_fd.as_raw_fd(),
                        path.file_name().unwrap().to_str().unwrap(),
                        uid,
                        false,
                        false,
                    )
                    .is_err()
                );
            }
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::remove_dir_all(root).unwrap();
    }
}
