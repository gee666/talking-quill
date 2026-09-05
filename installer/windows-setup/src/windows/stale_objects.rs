//! Retain and diagnose stale lifecycle objects before cleanup.
use super::*;

pub(super) struct RetainedStaleObject {
    pub(super) file: File,
    pub(super) identity: String,
    pub(super) path: PathBuf,
    pub(super) directory: bool,
}

impl RetainedStaleObject {
    pub(super) fn open(path: &Path, directory: bool) -> Result<Self> {
        Self::open_with_share(path, directory, 0)
    }

    pub(super) fn open_lifecycle(path: &Path) -> Result<Self> {
        // Delete sharing permits POSIX unlink while a duplicate retains the same verified file
        // object. Read and write sharing remain denied, so an active lifecycle owner conflicts.
        Self::open_with_share(path, false, FILE_SHARE_DELETE)
    }

    pub(super) fn open_with_share(path: &Path, directory: bool, share: u32) -> Result<Self> {
        let raw = unsafe {
            CreateFileW(
                wide(path.as_os_str()).as_ptr(),
                FILE_GENERIC_READ | DELETE,
                share,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT
                    | if directory {
                        FILE_FLAG_BACKUP_SEMANTICS
                    } else {
                        0
                    },
                ptr::null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(fail(
                EXIT_REJECTED,
                "A stale fixture object is active or unavailable.",
            ));
        }
        let file = unsafe { File::from_raw_handle(raw) };
        let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { mem::zeroed() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0
            || (information.dwFileAttributes & 0x10 != 0) != directory
            || information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            return Err(fail(
                EXIT_REJECTED,
                "A retained stale fixture identity is invalid.",
            ));
        }
        let identity = file_identity_text(&file)?;
        Ok(Self {
            file,
            identity,
            path: path.to_owned(),
            directory,
        })
    }

    pub(super) fn verify(&self) -> Result<()> {
        if file_identity_text(&self.file)? != self.identity {
            return Err(fail(
                EXIT_REJECTED,
                "A retained stale fixture identity changed.",
            ));
        }
        let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { mem::zeroed() };
        if unsafe { GetFileInformationByHandle(self.file.as_raw_handle(), &mut information) } == 0
            || (information.dwFileAttributes & 0x10 != 0) != self.directory
            || information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            return Err(fail(
                EXIT_REJECTED,
                "A retained stale fixture type changed.",
            ));
        }
        Ok(())
    }

    pub(super) fn verify_protected_acl(&self) -> Result<()> {
        if !protected_handle_acl_is_exact(&self.file, self.directory)? {
            return Err(fail(
                EXIT_REJECTED,
                "A retained stale fixture ACL is not exact.",
            ));
        }
        Ok(())
    }

    pub(super) fn names(&self) -> Result<Vec<String>> {
        if !self.directory {
            return Err(fail(
                EXIT_REJECTED,
                "A retained file has no child inventory.",
            ));
        }
        let mut names = retained_directory_names(self.file.as_raw_handle())
            .map_err(|error| fail(EXIT_REJECTED, error.to_string()))?;
        names.sort_unstable();
        Ok(names)
    }

    pub(super) fn read_all(&mut self) -> Result<Vec<u8>> {
        self.file.seek(SeekFrom::Start(0)).map_err(io_failure)?;
        let mut bytes = Vec::new();
        self.file.read_to_end(&mut bytes).map_err(io_failure)?;
        Ok(bytes)
    }

    pub(super) fn rename(&mut self, destination: &Path) -> Result<()> {
        rename_handle(self.file.as_raw_handle(), destination)?;
        self.path = destination.to_owned();
        Ok(())
    }

    pub(super) fn mark_posix_deleted(&self) -> Result<()> {
        let disposition = FILE_DISPOSITION_INFO_EX {
            Flags: FILE_DISPOSITION_FLAG_DELETE
                | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS
                | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
        };
        if unsafe {
            SetFileInformationByHandle(
                self.file.as_raw_handle(),
                FileDispositionInfoEx,
                (&disposition as *const FILE_DISPOSITION_INFO_EX).cast(),
                mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
            )
        } == 0
        {
            return Err(io_failure(std::io::Error::last_os_error()));
        }
        Ok(())
    }

    pub(super) fn mark_deleted(&self) -> Result<()> {
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        if unsafe {
            SetFileInformationByHandle(
                self.file.as_raw_handle(),
                FileDispositionInfo,
                (&disposition as *const FILE_DISPOSITION_INFO).cast(),
                mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        } == 0
        {
            return Err(io_failure(std::io::Error::last_os_error()));
        }
        Ok(())
    }

    pub(super) fn finish_deleted(self) -> Result<()> {
        let path = self.path.clone();
        drop(self);
        if path_present(&path)? {
            return Err(fail(
                EXIT_FAILURE,
                "Windows retained a handle-deleted stale object.",
            ));
        }
        Ok(())
    }

    pub(super) fn delete(self) -> Result<()> {
        self.mark_deleted()?;
        self.finish_deleted()
    }
}

mod diagnostic;
pub(super) use diagnostic::*;
