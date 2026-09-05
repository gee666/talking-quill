use super::*;

pub(super) fn validate_security(handle: HANDLE) -> Result<(), &'static str> {
    let mut owner: PSID = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 || descriptor.is_null() || owner.is_null() || dacl.is_null() {
        if !descriptor.is_null() {
            unsafe { LocalFree(descriptor as _) };
        }
        return Err("private key security query");
    }
    let descriptor = SecurityDescriptor(descriptor);
    validate_descriptor(descriptor.0, owner, dacl)
}

fn validate_descriptor(
    descriptor: PSECURITY_DESCRIPTOR,
    owner: PSID,
    dacl: *mut ACL,
) -> Result<(), &'static str> {
    let current = current_user_sid()?;
    let system = well_known_sid(WinLocalSystemSid)?;
    let administrators = well_known_sid(WinBuiltinAdministratorsSid)?;
    if unsafe { EqualSid(owner, current.as_ptr() as PSID) } == 0 {
        return Err("private key owner");
    }
    let mut control = 0u16;
    let mut revision = 0u32;
    if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
        || control & SE_DACL_PROTECTED == 0
        || unsafe { (*dacl).AceCount } != 3
    {
        return Err("private key dacl policy");
    }
    let expected = [current, system, administrators];
    let mut seen = [false; 3];
    for index in 0..3u32 {
        let mut raw: *mut c_void = null_mut();
        if unsafe { GetAce(dacl, index, &mut raw) } == 0 || raw.is_null() {
            return Err("private key ace query");
        }
        let ace = unsafe { &*(raw as *const ACCESS_ALLOWED_ACE) };
        if u32::from(ace.Header.AceType) != ACCESS_ALLOWED_ACE_TYPE
            || u32::from(ace.Header.AceFlags) & INHERITED_ACE != 0
            || ace.Mask != FILE_GENERIC_READ
        {
            return Err("private key ace policy");
        }
        let sid = addr_of!(ace.SidStart) as PSID;
        let Some(position) = expected
            .iter()
            .position(|candidate| unsafe { EqualSid(sid, candidate.as_ptr() as PSID) } != 0)
        else {
            return Err("private key principal");
        };
        if seen[position] {
            return Err("private key duplicate principal");
        }
        seen[position] = true;
    }
    if !seen.into_iter().all(|value| value) {
        return Err("private key principal set");
    }
    Ok(())
}

pub(super) fn current_user_sid_string() -> Result<String, &'static str> {
    let sid = current_user_sid()?;
    let mut text = null_mut();
    if unsafe { ConvertSidToStringSidW(sid.as_ptr() as PSID, &mut text) } == 0 || text.is_null() {
        return Err("current user sid string");
    }
    let mut length = 0usize;
    while unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    let value = String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })
        .map_err(|_| "current user sid string")?;
    unsafe { LocalFree(text as _) };
    Ok(value)
}

fn current_user_sid() -> Result<Vec<u8>, &'static str> {
    let mut token: HANDLE = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err("current token");
    }
    let mut bytes = 0u32;
    unsafe { GetTokenInformation(token, TokenUser, null_mut(), 0, &mut bytes) };
    if bytes < size_of::<TOKEN_USER>() as u32 {
        unsafe { CloseHandle(token) };
        return Err("current user sid size");
    }
    let mut buffer = vec![0u8; bytes as usize];
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr() as *mut c_void,
            bytes,
            &mut bytes,
        )
    };
    unsafe { CloseHandle(token) };
    if ok == 0 {
        return Err("current user sid");
    }
    let user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
    copy_sid(user.User.Sid)
}

fn well_known_sid(kind: i32) -> Result<Vec<u8>, &'static str> {
    let mut buffer = vec![0u8; SECURITY_MAX_SID_SIZE as usize];
    let mut bytes = buffer.len() as u32;
    if unsafe { CreateWellKnownSid(kind, null_mut(), buffer.as_mut_ptr() as PSID, &mut bytes) } == 0
    {
        return Err("well known sid");
    }
    buffer.truncate(bytes as usize);
    Ok(buffer)
}

fn copy_sid(sid: PSID) -> Result<Vec<u8>, &'static str> {
    use windows_sys::Win32::Security::GetLengthSid;
    let bytes = unsafe { GetLengthSid(sid) };
    if bytes == 0 || bytes > SECURITY_MAX_SID_SIZE {
        return Err("sid length");
    }
    let mut output = vec![0u8; bytes as usize];
    unsafe { std::ptr::copy_nonoverlapping(sid as *const u8, output.as_mut_ptr(), bytes as usize) };
    Ok(output)
}
