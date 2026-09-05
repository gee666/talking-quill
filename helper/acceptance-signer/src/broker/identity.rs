use super::*;

pub(super) fn authorized_peer(
    pid: u32,
    child: &ProbeChild,
    input: &ProbeInput,
) -> Result<bool, &'static str> {
    let peer = match talking_quill_windows_owner_ipc::peer::VerifiedPeer::from_process_id(pid) {
        Ok(peer) => peer,
        Err(_) => return Ok(false),
    };
    let facts = &peer.facts;
    let expected = &child.facts;
    if !peer_identity_matches(
        facts,
        expected,
        input.executable_sha256,
        &input.source_commit,
        &input.source_tree,
    ) {
        return Ok(false);
    }
    if pid == child.pid {
        return Ok(facts.creation_marker == expected.creation_marker);
    }
    is_creation_bound_descendant(
        pid,
        facts.creation_marker,
        child.pid,
        expected.creation_marker,
    )
}

pub(super) fn peer_identity_matches(
    facts: &talking_quill_windows_owner_ipc::peer::PeerFacts,
    expected: &talking_quill_windows_owner_ipc::peer::PeerFacts,
    image_sha256: [u8; 32],
    source_commit: &str,
    source_tree: &str,
) -> bool {
    facts.user_sid == expected.user_sid
        && facts.logon_sid == expected.logon_sid
        && facts.wts_session_id == expected.wts_session_id
        && facts.integrity_rid == expected.integrity_rid
        && facts.architecture == expected.architecture
        && facts.canonical_image == expected.canonical_image
        && facts.file_identity == expected.file_identity
        && facts.image_sha256 == image_sha256
        && (facts.source_identity.is_none()
            || facts
                .source_identity
                .as_ref()
                .is_some_and(|source| source.commit == source_commit && source.tree == source_tree))
}

pub(super) fn creation_chain_reaches_root(
    chain: &[(u32, u64)],
    root: u32,
    root_creation: u64,
) -> bool {
    let mut later = u64::MAX;
    for &(pid, creation) in chain {
        if creation > later {
            return false;
        }
        if pid == root {
            return creation == root_creation;
        }
        later = creation;
    }
    false
}

fn is_creation_bound_descendant(
    mut pid: u32,
    mut creation: u64,
    root: u32,
    root_creation: u64,
) -> Result<bool, &'static str> {
    let mut chain = vec![(pid, creation)];
    for _ in 0..32 {
        let parent = process_parent(pid)?;
        if parent == 0 || parent == pid {
            return Ok(false);
        }
        let peer =
            match talking_quill_windows_owner_ipc::peer::VerifiedPeer::from_process_id(parent) {
                Ok(peer) => peer,
                Err(_) => return Ok(false),
            };
        pid = parent;
        creation = peer.facts.creation_marker;
        chain.push((pid, creation));
        if parent == root {
            return Ok(creation_chain_reaches_root(&chain, root, root_creation));
        }
    }
    Ok(false)
}

fn process_parent(pid: u32) -> Result<u32, &'static str> {
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    let snapshot = Handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) });
    if snapshot.0 == INVALID_HANDLE_VALUE {
        return Err("ancestry snapshot");
    }
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    if unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0 {
        loop {
            if entry.th32ProcessID == pid {
                return Ok(entry.th32ParentProcessID);
            }
            if unsafe { Process32NextW(snapshot.0, &mut entry) } == 0 {
                break;
            }
        }
    }
    Err("ancestry pid")
}
