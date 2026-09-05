//! Kernel token identity for normal, relocated, and UAC-elevated setup peers.
use super::*;
use windows_sys::Win32::Security::{TOKEN_LINKED_TOKEN, TokenLinkedToken};

#[derive(Debug, Clone, PartialEq, Eq)]
struct PeerClaims {
    user_sid: Vec<u8>,
    authentication_id: (u32, i32),
    session_id: u32,
    integrity_rid: u32,
}

fn token_information(token: std::os::windows::io::RawHandle, class: i32) -> Result<Vec<usize>> {
    let mut length = 0;
    unsafe { GetTokenInformation(token, class, ptr::null_mut(), 0, &mut length) };
    if length == 0 {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot size setup peer token information.",
        ));
    }
    let mut bytes = vec![0_usize; (length as usize).div_ceil(mem::size_of::<usize>())];
    if unsafe { GetTokenInformation(token, class, bytes.as_mut_ptr().cast(), length, &mut length) }
        == 0
    {
        return Err(fail(
            EXIT_REJECTED,
            "Cannot read setup peer token information.",
        ));
    }
    Ok(bytes)
}

fn process_token(pid: u32) -> Result<OwnedHandle> {
    let process_raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process_raw.is_null() {
        return Err(fail(EXIT_REJECTED, "Cannot inspect setup peer token."));
    }
    let process = unsafe { OwnedHandle::from_raw_handle(process_raw) };
    let mut token_raw = ptr::null_mut();
    if unsafe { OpenProcessToken(process.as_raw_handle(), TOKEN_QUERY, &mut token_raw) } == 0 {
        return Err(fail(EXIT_REJECTED, "Cannot open setup peer token."));
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(token_raw) })
}

#[cfg(feature = "stale-schema2-cleanup")]
pub(super) fn process_integrity(pid: u32) -> Result<u32> {
    token_claims(&process_token(pid)?).map(|claims| claims.integrity_rid)
}

fn token_claims(token: &OwnedHandle) -> Result<PeerClaims> {
    let user = token_information(token.as_raw_handle(), TokenUser)?;
    let user = unsafe { &*(user.as_ptr().cast::<TOKEN_USER>()) };
    let sid_length = unsafe { GetLengthSid(user.User.Sid) } as usize;
    if sid_length == 0 {
        return Err(fail(EXIT_REJECTED, "Setup peer user SID is invalid."));
    }
    let user_sid =
        unsafe { std::slice::from_raw_parts(user.User.Sid.cast::<u8>(), sid_length) }.to_vec();
    let statistics = token_information(token.as_raw_handle(), TokenStatistics)?;
    let statistics = unsafe { &*(statistics.as_ptr().cast::<TOKEN_STATISTICS>()) };
    let session = token_information(token.as_raw_handle(), TokenSessionId)?;
    if session.is_empty() {
        return Err(fail(EXIT_REJECTED, "Setup peer session is invalid."));
    }
    let session_id = u32::try_from(session[0] & u32::MAX as usize)
        .map_err(|_| fail(EXIT_REJECTED, "Setup peer session is invalid."))?;
    let integrity = token_information(token.as_raw_handle(), TokenIntegrityLevel)?;
    let integrity = unsafe { &*(integrity.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()) };
    let count = unsafe { *GetSidSubAuthorityCount(integrity.Label.Sid) } as u32;
    if count == 0 {
        return Err(fail(EXIT_REJECTED, "Setup peer integrity SID is invalid."));
    }
    let integrity_rid = unsafe { *GetSidSubAuthority(integrity.Label.Sid, count - 1) };
    Ok(PeerClaims {
        user_sid,
        authentication_id: (
            statistics.AuthenticationId.LowPart,
            statistics.AuthenticationId.HighPart,
        ),
        session_id,
        integrity_rid,
    })
}

pub(super) fn verify_peer_claims(controller: u32, worker: u32) -> Result<[u8; 32]> {
    let controller_token = process_token(controller)?;
    let controller_claims = token_claims(&controller_token)?;
    let worker_claims = token_claims(&process_token(worker)?)?;
    // UAC split tokens may have distinct AuthenticationId values. Read the
    // relationship from Windows, never from data supplied over the setup pipe.
    let linked_claims = if controller_claims.authentication_id != worker_claims.authentication_id {
        linked_token(&controller_token)?
            .as_ref()
            .map(token_claims)
            .transpose()?
    } else {
        None
    };
    if !same_lineage(&controller_claims, &worker_claims, linked_claims.as_ref()) {
        return Err(fail(
            EXIT_REJECTED,
            "Setup peers do not share the required user, logon, session, and integrity lineage.",
        ));
    }
    let mut hash = Sha256::new();
    hash.update(&controller_claims.user_sid);
    hash.update(controller_claims.authentication_id.0.to_le_bytes());
    hash.update(controller_claims.authentication_id.1.to_le_bytes());
    hash.update(controller_claims.session_id.to_le_bytes());
    hash.update(controller_claims.integrity_rid.to_le_bytes());
    hash.update(worker_claims.authentication_id.0.to_le_bytes());
    hash.update(worker_claims.authentication_id.1.to_le_bytes());
    hash.update(worker_claims.integrity_rid.to_le_bytes());
    Ok(hash.finalize().into())
}

fn linked_token(token: &OwnedHandle) -> Result<Option<OwnedHandle>> {
    let mut linked: TOKEN_LINKED_TOKEN = unsafe { mem::zeroed() };
    let mut length = 0;
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenLinkedToken,
            (&mut linked as *mut TOKEN_LINKED_TOKEN).cast(),
            mem::size_of::<TOKEN_LINKED_TOKEN>() as u32,
            &mut length,
        )
    } == 0
    {
        let error = unsafe { GetLastError() };
        // Non-split tokens legitimately have no linked token.
        if error == windows_sys::Win32::Foundation::ERROR_NO_SUCH_LOGON_SESSION {
            return Ok(None);
        }
        return Err(fail(
            EXIT_REJECTED,
            format!("Cannot inspect setup elevation lineage (Windows error {error})."),
        ));
    }
    if linked.LinkedToken.is_null() {
        return Ok(None);
    }
    Ok(Some(unsafe {
        OwnedHandle::from_raw_handle(linked.LinkedToken)
    }))
}

fn same_lineage(controller: &PeerClaims, worker: &PeerClaims, linked: Option<&PeerClaims>) -> bool {
    controller.user_sid == worker.user_sid
        && controller.session_id == worker.session_id
        && worker.integrity_rid >= controller.integrity_rid
        && (controller.authentication_id == worker.authentication_id
            || linked.is_some_and(|claims| claims == worker))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims() -> PeerClaims {
        PeerClaims {
            user_sid: vec![1, 2, 3],
            authentication_id: (10, 0),
            session_id: 1,
            integrity_rid: 8192,
        }
    }

    #[test]
    fn linked_uac_logon_is_accepted_but_unrelated_logons_are_rejected() {
        let controller = claims();
        let mut worker = controller.clone();
        worker.authentication_id = (11, 0);
        worker.integrity_rid = 12288;
        assert!(!same_lineage(&controller, &worker, None));
        assert!(same_lineage(&controller, &worker, Some(&worker)));
        for invalid in [
            PeerClaims {
                user_sid: vec![9],
                ..worker.clone()
            },
            PeerClaims {
                session_id: 2,
                ..worker.clone()
            },
            PeerClaims {
                integrity_rid: 4096,
                ..worker.clone()
            },
            PeerClaims {
                authentication_id: (12, 0),
                ..worker.clone()
            },
        ] {
            assert!(!same_lineage(&controller, &invalid, Some(&worker)));
        }
        assert!(same_lineage(&controller, &controller, None));
    }

    #[test]
    fn windows_current_process_and_its_actual_linked_token_are_accepted() {
        let token = process_token(std::process::id()).unwrap();
        let current = token_claims(&token).unwrap();
        assert!(verify_peer_claims(std::process::id(), std::process::id()).is_ok());
        if let Some(linked) = linked_token(&token).unwrap() {
            let linked = token_claims(&linked).unwrap();
            let (controller, worker) = if current.integrity_rid <= linked.integrity_rid {
                (&current, &linked)
            } else {
                (&linked, &current)
            };
            assert!(same_lineage(controller, worker, Some(worker)));
            eprintln!(
                "Windows split token verification: current logon {:?}, linked logon {:?}",
                current.authentication_id, linked.authentication_id
            );
        }
    }
}
