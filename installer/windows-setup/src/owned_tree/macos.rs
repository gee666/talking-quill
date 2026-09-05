use super::OwnedTreeError;
use std::{
    ffi::{CStr, CString, OsStr},
    io,
    mem::zeroed,
    os::unix::ffi::OsStrExt,
    path::Path,
};

struct Fd(libc::c_int);
impl Drop for Fd {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.0);
        }
    }
}
struct Dir(*mut libc::DIR);
impl Drop for Dir {
    fn drop(&mut self) {
        unsafe {
            libc::closedir(self.0);
        }
    }
}

pub fn remove(
    path: &Path,
    expected_device: u64,
    expected_inode: u64,
) -> Result<(), OwnedTreeError> {
    let parent = path.parent().ok_or(OwnedTreeError::InvalidPath)?;
    let name = path.file_name().ok_or(OwnedTreeError::InvalidPath)?;
    let parent_fd = open_path(parent)?;
    let name = c_name(name)?;
    let root_fd = open_at(parent_fd.0, &name)?;
    let root = stat_fd(root_fd.0)?;
    if root.st_dev as u64 != expected_device || root.st_ino != expected_inode {
        return Err(OwnedTreeError::IdentityMismatch);
    }
    remove_contents(root_fd.0, root.st_dev)?;
    if unsafe { libc::unlinkat(parent_fd.0, name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    if unsafe { libc::fsync(parent_fd.0) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

fn remove_contents(fd: libc::c_int, root_device: libc::dev_t) -> Result<(), OwnedTreeError> {
    let duplicate = unsafe { libc::dup(fd) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let directory = unsafe { libc::fdopendir(duplicate) };
    if directory.is_null() {
        unsafe {
            libc::close(duplicate);
        }
        return Err(io::Error::last_os_error().into());
    }
    let directory = Dir(directory);
    loop {
        unsafe {
            *libc::__error() = 0;
        }
        let entry = unsafe { libc::readdir(directory.0) };
        if entry.is_null() {
            let error = unsafe { *libc::__error() };
            if error == 0 {
                break;
            }
            return Err(io::Error::from_raw_os_error(error).into());
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        let mut metadata: libc::stat = unsafe { zeroed() };
        if unsafe { libc::fstatat(fd, name.as_ptr(), &mut metadata, libc::AT_SYMLINK_NOFOLLOW) }
            != 0
        {
            return Err(io::Error::last_os_error().into());
        }
        let kind = metadata.st_mode & libc::S_IFMT;
        if kind == libc::S_IFDIR {
            if metadata.st_dev != root_device {
                return Err(OwnedTreeError::MountBoundary);
            }
            let child = open_at(fd, name)?;
            let opened = stat_fd(child.0)?;
            if opened.st_dev != metadata.st_dev || opened.st_ino != metadata.st_ino {
                return Err(OwnedTreeError::IdentityMismatch);
            }
            remove_contents(child.0, root_device)?;
            if unsafe { libc::unlinkat(fd, name.as_ptr(), libc::AT_REMOVEDIR) } != 0 {
                return Err(io::Error::last_os_error().into());
            }
        } else if unsafe { libc::unlinkat(fd, name.as_ptr(), 0) } != 0 {
            return Err(io::Error::last_os_error().into());
        }
    }
    Ok(())
}

fn open_path(path: &Path) -> Result<Fd, OwnedTreeError> {
    let path =
        CString::new(path.as_os_str().as_bytes()).map_err(|_| OwnedTreeError::InvalidPath)?;
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(Fd(fd))
}
fn open_at(parent: libc::c_int, name: &CStr) -> Result<Fd, OwnedTreeError> {
    let fd = unsafe {
        libc::openat(
            parent,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(Fd(fd))
}
fn stat_fd(fd: libc::c_int) -> Result<libc::stat, OwnedTreeError> {
    let mut metadata = unsafe { zeroed() };
    if unsafe { libc::fstat(fd, &mut metadata) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(metadata)
}
fn c_name(name: &OsStr) -> Result<CString, OwnedTreeError> {
    CString::new(name.as_bytes()).map_err(|_| OwnedTreeError::InvalidPath)
}
