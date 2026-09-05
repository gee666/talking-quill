//! Owned, aligned token information. Embedded SID pointers never escape storage.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Security::{
    GetTokenInformation, TOKEN_GROUPS, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_USER,
    TokenIntegrityLevel, TokenLogonSid, TokenPrimary, TokenSessionId, TokenType, TokenUser,
};
use windows_sys::Win32::System::Threading::OpenProcessToken;

use crate::peer::PeerError;

pub(crate) struct Token(OwnedHandle);

impl Token {
    pub(crate) fn open(process: HANDLE) -> Result<Self, PeerError> {
        let mut handle = std::ptr::null_mut();
        // SAFETY: process is only passed to the kernel; handle is writable output.
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut handle) } == 0 || handle.is_null() {
            return Err(PeerError::Token);
        }
        // SAFETY: successful OpenProcessToken transfers one owned handle to us.
        Ok(Self(unsafe { OwnedHandle::from_raw_handle(handle) }))
    }

    pub(crate) fn query(&self, class: i32, max_bytes: u32) -> Result<TokenInformation, PeerError> {
        let mut length = 0;
        // SAFETY: a null buffer with zero capacity requests the required size.
        unsafe {
            GetTokenInformation(
                self.0.as_raw_handle(),
                class,
                std::ptr::null_mut(),
                0,
                &mut length,
            )
        };
        if length == 0 || length > max_bytes {
            return Err(PeerError::Token);
        }
        let capacity = length;
        // Win32 token structures require pointer alignment, which Vec<u8> cannot promise.
        let mut information = TokenInformation {
            storage: vec![0; (length as usize).div_ceil(std::mem::size_of::<usize>())],
            length: length as usize,
        };
        // SAFETY: storage is aligned and holds at least capacity writable bytes.
        if unsafe {
            GetTokenInformation(
                self.0.as_raw_handle(),
                class,
                information.storage.as_mut_ptr().cast(),
                capacity,
                &mut length,
            )
        } == 0
            || length > capacity
            || length == 0
        {
            return Err(PeerError::Token);
        }
        information.length = length as usize;
        Ok(information)
    }

    pub(crate) fn facts(&self) -> Result<(Vec<u8>, Vec<u8>, u32, u32), PeerError> {
        let kind = self.query(TokenType, 64 * 1024)?;
        // SAFETY: i32 and u32 accept every initialized bit pattern.
        if unsafe { kind.header::<i32>()? } != TokenPrimary {
            return Err(PeerError::Token);
        }
        let user = self.query(TokenUser, 64 * 1024)?;
        // SAFETY: TOKEN_USER contains only integer and raw pointer fields.
        let user_sid = user
            .sid(unsafe { user.header::<TOKEN_USER>()? }.User.Sid)?
            .to_vec();
        let logon = self.query(TokenLogonSid, 64 * 1024)?;
        let logon_sid = logon.logon_sid()?.to_vec();
        // Preserve the peer SID length policy without imposing it on endpoint lookup.
        if user_sid.len() > 68 || logon_sid.len() > 68 {
            return Err(PeerError::Token);
        }
        let session = self.query(TokenSessionId, 64 * 1024)?;
        // SAFETY: u32 accepts every initialized bit pattern.
        let session = unsafe { session.header::<u32>()? };
        let integrity = self.query(TokenIntegrityLevel, 64 * 1024)?;
        // SAFETY: TOKEN_MANDATORY_LABEL contains only integer and raw pointer fields.
        let sid = integrity.sid(
            unsafe { integrity.header::<TOKEN_MANDATORY_LABEL>()? }
                .Label
                .Sid,
        )?;
        if sid[1] == 0 {
            return Err(PeerError::Token);
        }
        let rid = u32::from_le_bytes(
            sid[sid.len() - 4..]
                .try_into()
                .map_err(|_| PeerError::Token)?,
        );
        Ok((user_sid, logon_sid, session, rid))
    }
}

pub(crate) struct TokenInformation {
    storage: Vec<usize>,
    length: usize,
}

impl TokenInformation {
    fn bytes(&self) -> &[u8] {
        // SAFETY: storage is initialized and length is bounded by its byte capacity.
        unsafe { std::slice::from_raw_parts(self.storage.as_ptr().cast(), self.length) }
    }

    /// T must accept every bit pattern, including raw pointer values.
    unsafe fn header<T: Copy>(&self) -> Result<T, PeerError> {
        if self.length < std::mem::size_of::<T>() {
            return Err(PeerError::Token);
        }
        // SAFETY: the size was checked; caller guarantees bit validity. No reference
        // is formed, so this does not rely on T's alignment matching our storage.
        Ok(unsafe { self.storage.as_ptr().cast::<T>().read_unaligned() })
    }

    pub(crate) fn logon_sid(&self) -> Result<&[u8], PeerError> {
        // SAFETY: TOKEN_GROUPS contains only integer and raw pointer fields.
        let groups = unsafe { self.header::<TOKEN_GROUPS>()? };
        if groups.GroupCount != 1 {
            return Err(PeerError::Token);
        }
        self.sid(groups.Groups[0].Sid)
    }

    fn sid(&self, sid: windows_sys::Win32::Security::PSID) -> Result<&[u8], PeerError> {
        // Validate addresses numerically before accessing through our own allocation.
        let offset = (sid as usize)
            .checked_sub(self.storage.as_ptr() as usize)
            .ok_or(PeerError::Token)?;
        let bytes = self.bytes().get(offset..).ok_or(PeerError::Token)?;
        if bytes.len() < 8 {
            return Err(PeerError::Token);
        }
        bytes
            .get(..8 + usize::from(bytes[1]) * 4)
            .ok_or(PeerError::Token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn information(length: usize) -> TokenInformation {
        TokenInformation {
            storage: vec![0; 16],
            length,
        }
    }

    #[test]
    fn rejects_short_headers_and_out_of_bounds_sids() {
        let info = information(3);
        assert!(unsafe { info.header::<u32>() }.is_err());
        assert!(info.sid(std::ptr::null_mut()).is_err());
        let outside = info.storage.as_ptr().wrapping_add(info.storage.len());
        assert!(info.sid(outside as _).is_err());
    }

    #[test]
    fn sid_extent_is_checked() {
        let mut info = information(12);
        // SAFETY: the allocation has at least twelve initialized writable bytes.
        let bytes =
            unsafe { std::slice::from_raw_parts_mut(info.storage.as_mut_ptr().cast::<u8>(), 12) };
        bytes[0] = 1;
        bytes[1] = 1;
        bytes[8..12].copy_from_slice(&0x2000_u32.to_le_bytes());
        let sid = info.storage.as_ptr() as _;
        assert_eq!(info.sid(sid).unwrap().len(), 12);
        info.length = 11;
        assert!(info.sid(sid).is_err());
    }
}
