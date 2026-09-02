#[cfg(not(windows))]
fn main() {
    std::process::exit(64);
}

#[cfg(windows)]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RetainedNamespace {
    path: std::path::PathBuf,
    root_identity: String,
    parent_identity: String,
}

#[cfg(windows)]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct NamespaceSessionRoot {
    kind: String,
    path: std::path::PathBuf,
    parent_identity: String,
    ownership_prefix: String,
    binding_path: std::path::PathBuf,
    binding_nonce: String,
}

#[cfg(windows)]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct NamespaceSessionRequest {
    command: String,
    namespace_id: String,
    record_path: std::path::PathBuf,
    child_stdout_log_path: std::path::PathBuf,
    child_stderr_log_path: std::path::PathBuf,
    control_nonce: String,
    #[serde(default)]
    probe_control_handle: bool,
    roots: Vec<NamespaceSessionRoot>,
}

#[cfg(windows)]
fn main() {
    use std::io::Read;

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Entry {
        relative_path: String,
        directory: bool,
        identity: String,
    }

    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if let [mode] = arguments.as_slice()
        && mode == "--namespace-session"
    {
        use std::io::BufRead;
        let mut request = String::new();
        if std::io::BufReader::new(std::io::stdin())
            .read_line(&mut request)
            .is_err()
        {
            std::process::exit(65);
        }
        let Ok(request) = serde_json::from_str::<NamespaceSessionRequest>(&request) else {
            std::process::exit(65);
        };
        match run_namespace_session(request) {
            Ok(()) => std::process::exit(0),
            Err(error) => {
                eprintln!("namespace session failed: {error}");
                std::process::exit(78)
            }
        }
    }
    if let [mode, command, namespaces] = arguments.as_slice()
        && mode == "--supervise"
    {
        let (Some(command), Some(namespaces)) = (command.to_str(), namespaces.to_str()) else {
            std::process::exit(64);
        };
        let Ok(namespaces) = serde_json::from_str::<Vec<RetainedNamespace>>(namespaces) else {
            std::process::exit(65);
        };
        match supervise(command, &namespaces) {
            Ok(code) => std::process::exit(code as i32),
            Err(()) => std::process::exit(74),
        }
    }
    if let [mode, value] = arguments.as_slice()
        && mode == "--assert-supervisor-process-protected"
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{
            CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_NOT_ALL_ASSIGNED,
            GetLastError, HANDLE, SetLastError,
        };
        use windows_sys::Win32::Security::{
            AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW,
            SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
        };
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_DUP_HANDLE,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
        };
        let Some(pid) = value.to_str().and_then(|value| value.parse::<u32>().ok()) else {
            std::process::exit(64);
        };
        let mut token = std::ptr::null_mut();
        if unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
                &mut token,
            )
        } == 0
        {
            std::process::exit(78);
        }
        for privilege_name in [
            "SeDebugPrivilege",
            "SeTakeOwnershipPrivilege",
            "SeRestorePrivilege",
        ] {
            let name = std::ffi::OsStr::new(privilege_name)
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>();
            let mut privilege: LUID_AND_ATTRIBUTES = unsafe { std::mem::zeroed() };
            if unsafe {
                LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut privilege.Luid)
            } == 0
            {
                unsafe { CloseHandle(token) };
                std::process::exit(78);
            }
            privilege.Attributes = SE_PRIVILEGE_ENABLED;
            let state = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [privilege],
            };
            unsafe { SetLastError(0) };
            let adjusted = unsafe {
                AdjustTokenPrivileges(
                    token,
                    0,
                    &state,
                    size_of::<TOKEN_PRIVILEGES>() as u32,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            let adjust_error = unsafe { GetLastError() };
            if adjusted == 0 || adjust_error != ERROR_NOT_ALL_ASSIGNED {
                eprintln!("{privilege_name} remains assignable in the supervised child");
                unsafe { CloseHandle(token) };
                std::process::exit(79);
            }
        }
        unsafe { CloseHandle(token) };
        let synchronized = unsafe { OpenProcess(0x0010_0000, 0, pid) };
        if synchronized.is_null() {
            std::process::exit(78);
        }
        let query = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if query.is_null() {
            unsafe { CloseHandle(synchronized) };
            std::process::exit(78);
        }
        unsafe { CloseHandle(query) };
        for access in [
            PROCESS_DUP_HANDLE,
            PROCESS_VM_READ,
            0x0004_0000,
            0x0008_0000,
        ] {
            let opened = unsafe { OpenProcess(access, 0, pid) };
            if !opened.is_null() {
                eprintln!("dangerous supervisor process access remained open: {access:#x}");
                unsafe { CloseHandle(opened) };
                unsafe { CloseHandle(synchronized) };
                std::process::exit(79);
            }
        }
        let mut duplicate: HANDLE = std::ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                synchronized,
                4usize as HANDLE,
                GetCurrentProcess(),
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } != 0
        {
            eprintln!("cross-process DuplicateHandle unexpectedly succeeded");
            unsafe { CloseHandle(duplicate) };
            unsafe { CloseHandle(synchronized) };
            std::process::exit(79);
        }
        unsafe { CloseHandle(synchronized) };
        return;
    }
    if let [mode, value] = arguments.as_slice()
        && mode == "--assert-handle-not-inherited"
    {
        use windows_sys::Win32::Foundation::{
            CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let Some(value) = value.to_str().and_then(|value| value.parse::<usize>().ok()) else {
            std::process::exit(64);
        };
        let process = unsafe { GetCurrentProcess() };
        let mut duplicate: HANDLE = std::ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                process,
                value as HANDLE,
                process,
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } != 0
        {
            unsafe { CloseHandle(duplicate) };
            std::process::exit(79);
        }
        return;
    }
    if let [mode, path, expected_sha256] = arguments.as_slice()
        && mode == "--publish-record"
    {
        let mut bytes = Vec::new();
        if std::io::stdin().read_to_end(&mut bytes).is_err() {
            std::process::exit(65);
        }
        let Some(expected_sha256) = expected_sha256.to_str() else {
            std::process::exit(64);
        };
        let expected_sha256 = (expected_sha256 != "-").then_some(expected_sha256);
        match talking_quill_helper::machine_lock_test_namespace::publish_cleanup_record(
            std::path::Path::new(path),
            &bytes,
            expected_sha256,
        ) {
            Ok(()) => return,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(78)
            }
        }
    }
    if let [mode, path] = arguments.as_slice()
        && mode == "--recover-record-backup"
    {
        match talking_quill_helper::machine_lock_test_namespace::recover_cleanup_record_backup(
            std::path::Path::new(path),
        ) {
            Ok(()) => return,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(78)
            }
        }
    }
    if let [mode, path] = arguments.as_slice()
        && mode == "--inspect-record-temp"
    {
        match talking_quill_helper::machine_lock_test_namespace::inspect_cleanup_record_pending(
            std::path::Path::new(path),
        ) {
            Ok(bytes) => {
                use std::io::Write;
                if std::io::stdout().write_all(&bytes).is_err() {
                    std::process::exit(74);
                }
                return;
            }
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(78)
            }
        }
    }
    if let [mode, path, expected_sha256] = arguments.as_slice()
        && mode == "--recover-record-temp"
    {
        let Some(expected_sha256) = expected_sha256.to_str() else {
            std::process::exit(64);
        };
        match talking_quill_helper::machine_lock_test_namespace::recover_cleanup_record_pending(
            std::path::Path::new(path),
            expected_sha256,
        ) {
            Ok(_) => return,
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(78)
            }
        }
    }
    match arguments.as_slice() {
        [mode, path, record_id, stream, parent_identity] if mode == "--inspect-cleanup-log" => {
            let (Some(record_id), Some(stream), Some(parent_identity)) = (
                record_id.to_str(),
                stream.to_str(),
                parent_identity.to_str(),
            ) else {
                std::process::exit(64);
            };
            match talking_quill_helper::machine_lock_test_namespace::inspect_cleanup_log(
                std::path::Path::new(path),
                record_id,
                stream,
                parent_identity,
            ) {
                Ok(Some(log)) => {
                    println!("{}", serde_json::to_string(&log).unwrap());
                    return;
                }
                Ok(None) => std::process::exit(3),
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [
            mode,
            path,
            record_id,
            expected_sha256,
            expected_byte_length,
            expected_file_identity,
            stream,
            parent_identity,
        ] if mode == "--delete-cleanup-log" => {
            let (
                Some(record_id),
                Some(expected_sha256),
                Some(expected_byte_length),
                Some(expected_file_identity),
                Some(stream),
                Some(parent_identity),
            ) = (
                record_id.to_str(),
                expected_sha256.to_str(),
                expected_byte_length.to_str(),
                expected_file_identity.to_str(),
                stream.to_str(),
                parent_identity.to_str(),
            )
            else {
                std::process::exit(64);
            };
            let expected_sha256 = (expected_sha256 != "-").then_some(expected_sha256);
            let expected_byte_length = if expected_byte_length == "-" {
                None
            } else {
                expected_byte_length.parse().ok()
            };
            let expected_file_identity =
                (expected_file_identity != "-").then_some(expected_file_identity);
            if expected_sha256.is_some()
                != (expected_byte_length.is_some() && expected_file_identity.is_some())
            {
                std::process::exit(64);
            }
            match talking_quill_helper::machine_lock_test_namespace::delete_cleanup_log(
                std::path::Path::new(path),
                record_id,
                expected_sha256,
                expected_byte_length,
                expected_file_identity,
                stream,
                parent_identity,
            ) {
                Ok(()) => return,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [mode, path] if mode == "--protect-legacy-evidence-root" => {
            match talking_quill_helper::machine_lock_test_namespace::protect_legacy_evidence_root(
                std::path::Path::new(path),
            ) {
                Ok(()) => return,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [mode, path, record_id, stream, pending] if mode == "--create-legacy-evidence-fixture" => {
            let (Some(record_id), Some(stream), Some(pending)) =
                (record_id.to_str(), stream.to_str(), pending.to_str())
            else {
                std::process::exit(64);
            };
            let pending = match pending {
                "final" => false,
                "pending" => true,
                _ => std::process::exit(64),
            };
            let mut bytes = Vec::new();
            if std::io::Read::read_to_end(&mut std::io::stdin(), &mut bytes).is_err() {
                std::process::exit(74);
            }
            match talking_quill_helper::machine_lock_test_namespace::create_legacy_evidence_fixture(
                std::path::Path::new(path),
                record_id,
                stream,
                pending,
                &bytes,
            ) {
                Ok(()) => return,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [mode, path] if mode == "--inspect-legacy-evidence-root" => {
            match talking_quill_helper::machine_lock_test_namespace::inspect_legacy_evidence_root(
                std::path::Path::new(path),
            ) {
                Ok(root) => {
                    println!("{}", serde_json::to_string(&root).unwrap());
                    return;
                }
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [mode, path, record_id, stream, pending, root_identity]
            if mode == "--inspect-legacy-evidence" =>
        {
            let (Some(record_id), Some(stream), Some(pending), Some(root_identity)) = (
                record_id.to_str(),
                stream.to_str(),
                pending.to_str(),
                root_identity.to_str(),
            ) else {
                std::process::exit(64);
            };
            let pending = match pending {
                "final" => false,
                "pending" => true,
                _ => std::process::exit(64),
            };
            match talking_quill_helper::machine_lock_test_namespace::inspect_legacy_evidence(
                std::path::Path::new(path),
                record_id,
                stream,
                pending,
                root_identity,
            ) {
                Ok(Some(log)) => {
                    println!("{}", serde_json::to_string(&log).unwrap());
                    return;
                }
                Ok(None) => std::process::exit(3),
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [
            mode,
            path,
            record_id,
            stream,
            pending,
            root_identity,
            sha256,
            byte_length,
            file_identity,
        ] if mode == "--delete-legacy-evidence" => {
            let (
                Some(record_id),
                Some(stream),
                Some(pending),
                Some(root_identity),
                Some(sha256),
                Some(byte_length),
                Some(file_identity),
            ) = (
                record_id.to_str(),
                stream.to_str(),
                pending.to_str(),
                root_identity.to_str(),
                sha256.to_str(),
                byte_length.to_str(),
                file_identity.to_str(),
            )
            else {
                std::process::exit(64);
            };
            let pending = match pending {
                "final" => false,
                "pending" => true,
                _ => std::process::exit(64),
            };
            let sha256 = (sha256 != "-").then_some(sha256);
            let byte_length = if byte_length == "-" {
                None
            } else {
                byte_length.parse().ok()
            };
            let file_identity = (file_identity != "-").then_some(file_identity);
            if sha256.is_some() != (byte_length.is_some() && file_identity.is_some()) {
                std::process::exit(64);
            }
            match talking_quill_helper::machine_lock_test_namespace::delete_legacy_evidence(
                std::path::Path::new(path),
                record_id,
                stream,
                pending,
                root_identity,
                talking_quill_helper::machine_lock_test_namespace::ExpectedStreamedLog {
                    sha256,
                    byte_length,
                    file_identity,
                },
            ) {
                Ok(()) => return,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [mode, path, root_identity] if mode == "--remove-empty-legacy-evidence-root" => {
            let Some(root_identity) = root_identity.to_str() else {
                std::process::exit(64);
            };
            match talking_quill_helper::machine_lock_test_namespace::remove_empty_legacy_evidence_root(
                std::path::Path::new(path),
                root_identity,
            ) {
                Ok(()) => return,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [mode, path] if mode == "--delete-cleanup-record" => {
            match talking_quill_helper::machine_lock_test_namespace::delete_cleanup_record(
                std::path::Path::new(path),
            ) {
                Ok(()) => return,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [mode, path, parent_identity, prefix, binding, nonce]
            if mode == "--create-protected-root" =>
        {
            let (Some(parent_identity), Some(prefix), Some(nonce)) =
                (parent_identity.to_str(), prefix.to_str(), nonce.to_str())
            else {
                std::process::exit(64);
            };
            match talking_quill_helper::machine_lock_test_namespace::create_protected_root(
                std::path::Path::new(path),
                parent_identity,
                prefix,
                std::path::Path::new(binding),
                nonce,
            ) {
                Ok((identity, ads_sha256)) => {
                    println!(
                        "{}",
                        serde_json::json!({"identity": identity, "adsSha256": ads_sha256})
                    );
                    return;
                }
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [mode, path, prefix, binding, nonce] if mode == "--remove-interrupted-root" => {
            let (Some(prefix), Some(nonce)) = (prefix.to_str(), nonce.to_str()) else {
                std::process::exit(64);
            };
            match talking_quill_helper::machine_lock_test_namespace::remove_interrupted_root(
                std::path::Path::new(path),
                prefix,
                std::path::Path::new(binding),
                nonce,
            ) {
                Ok(()) => return,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [mode, binding, prefix, nonce] if mode == "--delete-creation-artifacts" => {
            let (Some(prefix), Some(nonce)) = (prefix.to_str(), nonce.to_str()) else {
                std::process::exit(64);
            };
            match talking_quill_helper::machine_lock_test_namespace::delete_interrupted_creation_artifacts(
                std::path::Path::new(binding),
                prefix,
                nonce,
            ) {
                Ok(()) => return,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [mode, path, prefix, binding, nonce, identity, hash] if mode == "--verify-root-binding" => {
            let (Some(prefix), Some(nonce), Some(identity), Some(hash)) = (
                prefix.to_str(),
                nonce.to_str(),
                identity.to_str(),
                hash.to_str(),
            ) else {
                std::process::exit(64);
            };
            match talking_quill_helper::machine_lock_test_namespace::verify_root_binding(
                std::path::Path::new(path),
                std::path::Path::new(binding),
                prefix,
                nonce,
                identity,
                hash,
            ) {
                Ok(()) => return,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [mode, binding, prefix, nonce, identity, hash]
            if mode == "--verify-deleted-root-binding" =>
        {
            let (Some(prefix), Some(nonce), Some(identity), Some(hash)) = (
                prefix.to_str(),
                nonce.to_str(),
                identity.to_str(),
                hash.to_str(),
            ) else {
                std::process::exit(64);
            };
            match talking_quill_helper::machine_lock_test_namespace::verify_deleted_root_binding(
                std::path::Path::new(binding),
                prefix,
                nonce,
                identity,
                hash,
            ) {
                Ok(()) => return,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [mode, binding, prefix, nonce, identity, hash] if mode == "--delete-root-binding" => {
            let (Some(prefix), Some(nonce), Some(identity), Some(hash)) = (
                prefix.to_str(),
                nonce.to_str(),
                identity.to_str(),
                hash.to_str(),
            ) else {
                std::process::exit(64);
            };
            match talking_quill_helper::machine_lock_test_namespace::delete_cleanup_binding(
                std::path::Path::new(binding),
                prefix,
                nonce,
                identity,
                hash,
            ) {
                Ok(()) => return,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [mode, source, target] if mode == "--force-directory-replacement" => {
            match talking_quill_helper::machine_lock_test_namespace::directory_replacement_is_blocked(
                std::path::Path::new(source),
                std::path::Path::new(target),
            ) {
                Ok(true) => return,
                Ok(false) => std::process::exit(79),
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [mode, path] if mode == "--stream-inventory" => {
            match talking_quill_helper::machine_lock_test_namespace::stream_inventory(
                std::path::Path::new(path),
            ) {
                Ok(inventory) => {
                    println!("{}", serde_json::to_string(&inventory).unwrap());
                    return;
                }
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [mode] if mode == "--registry-create-empty-root-fixture" => {
            match talking_quill_helper::machine_lock_test_namespace::create_empty_registry_root_fixture() {
                Ok(()) => return,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [mode] if mode == "--registry-create-link-fixture" => {
            match talking_quill_helper::machine_lock_test_namespace::create_registry_link_fixture()
            {
                Ok(()) => return,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [mode] if mode == "--registry-remove-link-fixture" => {
            match talking_quill_helper::machine_lock_test_namespace::remove_registry_link_fixture()
            {
                Ok(()) => return,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(78)
                }
            }
        }
        [mode] if mode == "--registry-delete-empty-root" => {
            match talking_quill_helper::machine_lock_test_namespace::delete_empty_registry_root() {
                Ok(()) => return,
                Err(_) => std::process::exit(78),
            }
        }
        [mode] if mode == "--registry-root-inventory" => {
            match talking_quill_helper::machine_lock_test_namespace::registry_root_inventory() {
                Ok(inventory) => {
                    println!("{}", serde_json::to_string(&inventory).unwrap());
                    return;
                }
                Err(_) => std::process::exit(78),
            }
        }
        [mode, namespace] if mode == "--registry-create" => {
            let Some(namespace) = namespace.to_str() else {
                std::process::exit(64);
            };
            match talking_quill_helper::machine_lock_test_namespace::create_registry_namespace(
                namespace,
            ) {
                Ok(()) => return,
                Err(_) => std::process::exit(78),
            }
        }
        [mode, namespace] if mode == "--registry-inventory" => {
            let Some(namespace) = namespace.to_str() else {
                std::process::exit(64);
            };
            match talking_quill_helper::machine_lock_test_namespace::registry_inventory(namespace) {
                Ok(inventory) => {
                    println!("{}", serde_json::to_string(&inventory).unwrap());
                    return;
                }
                Err(_) => std::process::exit(78),
            }
        }
        [mode, namespace] if mode == "--registry-delete-exact" => {
            let Some(namespace) = namespace.to_str() else {
                std::process::exit(64);
            };
            let mut json = String::new();
            if std::io::stdin().read_to_string(&mut json).is_err() {
                std::process::exit(65);
            }
            let Ok(inventory) = serde_json::from_str(&json) else {
                std::process::exit(65);
            };
            match talking_quill_helper::machine_lock_test_namespace::delete_registry_exact(
                namespace, &inventory,
            ) {
                Ok(()) => return,
                Err(_) => std::process::exit(78),
            }
        }
        _ => {}
    }
    let result = match arguments.as_slice() {
        [mode, path] if mode == "--flush-directory" => {
            talking_quill_helper::owned_tree::flush_owned_directory(std::path::Path::new(path))
        }
        [mode, path] if mode == "--identity" => {
            match talking_quill_helper::owned_tree::owned_tree_identity(std::path::Path::new(path))
            {
                Ok(identity) => {
                    println!("{identity}");
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }
        [mode, path, identity] if mode == "--exact" || mode == "--resume-exact" => {
            let Some(identity) = identity.to_str() else {
                std::process::exit(64);
            };
            let mut json = String::new();
            if std::io::stdin().read_to_string(&mut json).is_err() {
                std::process::exit(65);
            }
            let Ok(entries) = serde_json::from_str::<Vec<Entry>>(&json) else {
                std::process::exit(65);
            };
            let entries = entries
                .into_iter()
                .map(
                    |entry| talking_quill_helper::owned_tree::ExactOwnedTreeEntry {
                        relative_path: entry.relative_path,
                        directory: entry.directory,
                        identity: entry.identity,
                    },
                )
                .collect::<Vec<_>>();
            if mode == "--resume-exact" {
                talking_quill_helper::owned_tree::remove_remaining_exact_owned_tree(
                    std::path::Path::new(path),
                    identity,
                    &entries,
                )
            } else {
                talking_quill_helper::owned_tree::remove_exact_owned_tree(
                    std::path::Path::new(path),
                    identity,
                    &entries,
                )
            }
        }
        [path, identity] => {
            let Some(identity) = identity.to_str() else {
                std::process::exit(64);
            };
            talking_quill_helper::owned_tree::remove_owned_tree(
                std::path::Path::new(path),
                identity,
            )
        }
        _ => std::process::exit(64),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(78);
    }
}

#[cfg(windows)]
fn session_crash_at(phase: &str) {
    if std::env::var("TQ_MACHINE_LOCK_TEST_CRASH_AFTER").as_deref() == Ok(phase) {
        std::process::exit(197);
    }
}

#[cfg(windows)]
fn update_session_record(
    path: &std::path::Path,
    update: impl FnOnce(&mut serde_json::Value) -> Result<(), &'static str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let current_bytes = std::fs::read(path)?;
    let mut record = serde_json::from_slice::<serde_json::Value>(&current_bytes)?;
    update(&mut record).map_err(std::io::Error::other)?;
    let revision = record["revision"]
        .as_u64()
        .ok_or_else(|| std::io::Error::other("cleanup record revision is invalid"))?;
    record["revision"] = serde_json::Value::from(
        revision
            .checked_add(1)
            .ok_or_else(|| std::io::Error::other("cleanup record revision exhausted"))?,
    );
    use sha2::{Digest, Sha256};
    let expected_sha256 = Sha256::digest(&current_bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    talking_quill_helper::machine_lock_test_namespace::publish_cleanup_record(
        path,
        &serde_json::to_vec(&record)?,
        Some(&expected_sha256),
    )?;
    Ok(())
}

#[cfg(windows)]
fn emit_control(nonce: &str, value: serde_json::Value) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    println!("TQNS:{nonce}:{value}");
    std::io::stdout().flush()?;
    Ok(())
}

#[cfg(windows)]
fn run_namespace_session(
    request: NamespaceSessionRequest,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::BufRead;
    if request.roots.len() != 4 {
        return Err("namespace session requires four outer roots".into());
    }
    if request.control_nonce.len() != 32
        || !request
            .control_nonce
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("namespace session control nonce is invalid".into());
    }
    emit_control(
        &request.control_nonce,
        serde_json::json!({"event": "started"}),
    )?;
    let mut start = String::new();
    std::io::BufReader::new(std::io::stdin()).read_line(&mut start)?;
    if start.trim() != "create" {
        return Err("namespace session did not receive creation acknowledgement".into());
    }
    let publication_pause = std::env::var_os("TQ_MACHINE_LOCK_TEST_ROOT_PUBLICATION_PAUSE_FILE");
    let mut kinds = std::collections::BTreeSet::new();
    let mut roots = Vec::with_capacity(4);
    for plan in &request.roots {
        if !kinds.insert(plan.kind.clone()) {
            return Err("namespace session root kind is duplicated".into());
        }
        update_session_record(&request.record_path, |record| {
            record["creatingRoot"] = serde_json::Value::String(plan.kind.clone());
            Ok(())
        })?;
        if let Some(base) = &publication_pause {
            let mut path = base.clone();
            path.push(format!(".{}", plan.kind));
            unsafe {
                std::env::set_var("TQ_MACHINE_LOCK_TEST_ROOT_PUBLICATION_PAUSE_FILE", path);
            }
        }
        let root =
            talking_quill_helper::machine_lock_test_namespace::create_protected_root_retained(
                &plan.path,
                &plan.parent_identity,
                &plan.ownership_prefix,
                &plan.binding_path,
                &plan.binding_nonce,
            )?;
        roots.push((plan.kind.clone(), root));
    }
    match publication_pause {
        Some(path) => unsafe {
            std::env::set_var("TQ_MACHINE_LOCK_TEST_ROOT_PUBLICATION_PAUSE_FILE", path);
        },
        None => unsafe {
            std::env::remove_var("TQ_MACHINE_LOCK_TEST_ROOT_PUBLICATION_PAUSE_FILE");
        },
    }
    if std::env::var("TQ_MACHINE_LOCK_TEST_CRASH_AFTER").as_deref()
        == Ok("create-after-record-before-identity")
    {
        std::process::exit(197);
    }
    let registry =
        talking_quill_helper::machine_lock_test_namespace::create_registry_namespace_owned(
            &request.namespace_id,
        )?;
    for (_, root) in &mut roots {
        root.retain_low_access_guards()?;
    }
    let setup = roots
        .iter()
        .map(|(kind, root)| {
            serde_json::json!({
                "kind": kind,
                "identity": root.identity(),
                "adsSha256": root.ads_sha256(),
            })
        })
        .collect::<Vec<_>>();
    emit_control(
        &request.control_nonce,
        serde_json::json!({"event": "ready", "roots": setup}),
    )?;
    let mut line = String::new();
    std::io::BufReader::new(std::io::stdin()).read_line(&mut line)?;
    if line.trim() != "run" {
        return Err("namespace session did not receive run acknowledgement".into());
    }
    if std::env::var("TQ_MACHINE_LOCK_TEST_CRASH_AFTER").as_deref() == Ok("roots-created") {
        std::process::exit(197);
    }
    talking_quill_helper::machine_lock_test_namespace::harden_current_process_for_supervised_child(
    )?;
    let record_privacy =
        talking_quill_helper::machine_lock_test_namespace::retain_private_cleanup_record(
            &request.record_path,
            request
                .record_path
                .file_stem()
                .and_then(|value| value.to_str())
                .ok_or("cleanup record path has no identity")?,
            &request.control_nonce,
        )?;
    let supervisor_claim =
        talking_quill_helper::machine_lock_test_namespace::retain_supervisor_claim()?;
    unsafe {
        std::env::set_var(
            "TQ_MACHINE_LOCK_TEST_SUPERVISOR_CLAIM",
            supervisor_claim.claim(),
        );
    }
    unsafe {
        std::env::remove_var("TQ_MACHINE_LOCK_TEST_CONTROL_HANDLE_VALUE");
    }
    if request.probe_control_handle {
        use windows_sys::Win32::System::Console::{GetStdHandle, STD_OUTPUT_HANDLE};
        let control_handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) } as usize;
        unsafe {
            std::env::set_var(
                "TQ_MACHINE_LOCK_TEST_CONTROL_HANDLE_VALUE",
                control_handle.to_string(),
            );
        }
    }
    let child_result = supervise_job(
        &request.command,
        Some((
            &request.child_stdout_log_path,
            &request.child_stderr_log_path,
        )),
    );
    unsafe {
        std::env::remove_var("TQ_MACHINE_LOCK_TEST_SUPERVISOR_CLAIM");
        std::env::remove_var("TQ_MACHINE_LOCK_TEST_CONTROL_HANDLE_VALUE");
    }
    drop(record_privacy);
    drop(supervisor_claim);
    let (child_code, stdout_guard, stderr_guard) =
        child_result.map_err(|_| "job supervision failed")?;
    for (_, root) in &mut roots {
        root.restore_root_for_teardown()?;
    }
    let roots = roots
        .into_iter()
        .map(|(kind, root)| {
            root.inventory()
                .map(|inventory| (kind.clone(), root, inventory))
                .map_err(|error| std::io::Error::other(format!("{kind} inventory: {error}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let registry_inventory = registry.inventory()?;
    update_session_record(&request.record_path, |record| {
        let entries = record["roots"]
            .as_array_mut()
            .ok_or("cleanup record roots are absent")?;
        for (kind, _, inventory) in &roots {
            let entry = entries
                .iter_mut()
                .find(|entry| entry["kind"].as_str() == Some(kind))
                .ok_or("cleanup record root kind is absent")?;
            entry["inventory"] = serde_json::Value::Array(
                inventory
                    .iter()
                    .map(|item| {
                        serde_json::json!({
                            "relativePath": item.relative_path,
                            "directory": item.directory,
                            "identity": item.identity,
                        })
                    })
                    .collect(),
            );
        }
        record["registryInventory"] = serde_json::to_value(&registry_inventory)
            .map_err(|_| "registry inventory serialization failed")?;
        record["phase"] = serde_json::Value::String("inventory-sealed".to_owned());
        Ok(())
    })?;
    if std::env::var("TQ_MACHINE_LOCK_TEST_CRASH_AFTER").as_deref() == Ok("inventory-sealed") {
        std::process::exit(197);
    }
    for (kind, root, inventory) in roots.into_iter().rev() {
        update_session_record(&request.record_path, |record| {
            record["phase"] = serde_json::Value::String("deleting-root".to_owned());
            record["deletingRoot"] = serde_json::Value::String(kind.clone());
            Ok(())
        })?;
        root.delete_handle_bound(&inventory).map_err(|error| {
            std::io::Error::other(format!("{kind} handle-bound deletion: {error}"))
        })?;
        update_session_record(&request.record_path, |record| {
            record["deletingRoot"] = serde_json::Value::Null;
            record["deletedRoots"]
                .as_array_mut()
                .ok_or("cleanup record deletedRoots are absent")?
                .push(serde_json::Value::String(kind.clone()));
            Ok(())
        })?;
        if std::env::var("TQ_MACHINE_LOCK_TEST_CRASH_AFTER").as_deref()
            == Ok(&format!("deleted-root:{kind}"))
        {
            std::process::exit(197);
        }
    }
    update_session_record(&request.record_path, |record| {
        record["phase"] = serde_json::Value::String("deleting-registry".to_owned());
        Ok(())
    })?;
    registry
        .delete_handle_bound(&registry_inventory)
        .map_err(|error| std::io::Error::other(format!("registry deletion: {error}")))?;
    update_session_record(&request.record_path, |record| {
        record["phase"] = serde_json::Value::String("registry-deleted".to_owned());
        Ok(())
    })?;
    if std::env::var("TQ_MACHINE_LOCK_TEST_CRASH_AFTER").as_deref() == Ok("registry-deleted") {
        std::process::exit(197);
    }
    emit_control(
        &request.control_nonce,
        serde_json::json!({"event": "completed", "childExitCode": child_code}),
    )?;
    let mut drained = String::new();
    std::io::BufReader::new(std::io::stdin()).read_line(&mut drained)?;
    if drained.trim() != "drained" {
        return Err("namespace session did not receive log drain acknowledgement".into());
    }
    unsafe {
        windows_sys::Win32::Foundation::CloseHandle(stdout_guard);
        windows_sys::Win32::Foundation::CloseHandle(stderr_guard);
    }
    Ok(())
}

#[cfg(windows)]
fn supervise(command: &str, namespaces: &[RetainedNamespace]) -> Result<u32, ()> {
    let _namespace_handles = namespaces
        .iter()
        .map(|namespace| {
            talking_quill_helper::machine_lock_test_namespace::retain_namespace_handles(
                &namespace.path,
                &namespace.root_identity,
                &namespace.parent_identity,
            )
            .map_err(|_| ())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (code, stdout, stderr) = supervise_job(command, None)?;
    unsafe {
        if !stdout.is_null() {
            windows_sys::Win32::Foundation::CloseHandle(stdout);
        }
        if !stderr.is_null() {
            windows_sys::Win32::Foundation::CloseHandle(stderr);
        }
    }
    Ok(code)
}

#[cfg(windows)]
fn supervise_job(
    command: &str,
    child_log_paths: Option<(&std::path::Path, &std::path::Path)>,
) -> Result<
    (
        u32,
        windows_sys::Win32::Foundation::HANDLE,
        windows_sys::Win32::Foundation::HANDLE,
    ),
    (),
> {
    use std::{mem::zeroed, os::windows::ffi::OsStrExt, ptr::null_mut};
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
        },
        Security::{
            AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW,
            SE_PRIVILEGE_REMOVED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
        },
        Storage::FileSystem::{
            CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE,
            FlushFileBuffers, OPEN_EXISTING,
        },
        System::{
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
                QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
            },
            Threading::{
                CREATE_SUSPENDED, CreateProcessW, DeleteProcThreadAttributeList,
                EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE,
                InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST, OpenProcessToken,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, ResumeThread,
                STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW, TerminateProcess,
                UpdateProcThreadAttribute, WaitForSingleObject,
            },
        },
    };

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    struct AttributeList(LPPROC_THREAD_ATTRIBUTE_LIST);
    impl Drop for AttributeList {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { DeleteProcThreadAttributeList(self.0) };
            }
        }
    }

    let remove_dangerous_privileges = |process: HANDLE| -> Result<(), ()> {
        let mut token = null_mut();
        if unsafe { OpenProcessToken(process, TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token) }
            == 0
        {
            return Err(());
        }
        let token = Handle(token);
        for privilege_name in [
            "SeDebugPrivilege",
            "SeTakeOwnershipPrivilege",
            "SeRestorePrivilege",
        ] {
            let name = std::ffi::OsStr::new(privilege_name)
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>();
            let mut privilege: LUID_AND_ATTRIBUTES = unsafe { zeroed() };
            if unsafe { LookupPrivilegeValueW(null_mut(), name.as_ptr(), &mut privilege.Luid) } == 0
            {
                return Err(());
            }
            privilege.Attributes = SE_PRIVILEGE_REMOVED;
            let state = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [privilege],
            };
            if unsafe {
                AdjustTokenPrivileges(
                    token.0,
                    0,
                    &state,
                    size_of::<TOKEN_PRIVILEGES>() as u32,
                    null_mut(),
                    null_mut(),
                )
            } == 0
            {
                return Err(());
            }
        }
        Ok(())
    };

    let job = Handle(unsafe { CreateJobObjectW(null_mut(), null_mut()) });
    if job.0.is_null() {
        return Err(());
    }
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } == 0
    {
        return Err(());
    }
    let mut command_line = std::ffi::OsStr::new(&format!("cmd.exe /d /s /c \"{command}\""))
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut inherited_handles = Vec::new();
    let mut stdout_handle = None;
    let mut stderr_handle = None;
    let mut input_handle = None;
    let mut attribute_storage = Vec::<usize>::new();
    let mut attribute_list = AttributeList(null_mut());
    let mut startup_ex: STARTUPINFOEXW = unsafe { zeroed() };
    let mut startup: STARTUPINFOW = unsafe { zeroed() };
    let (startup_pointer, creation_flags) = if let Some((stdout_log_path, stderr_log_path)) =
        child_log_paths
    {
        let mut exact_security =
            talking_quill_helper::machine_lock_test_namespace::exact_inheritable_file_security()
                .map_err(|_| ())?;
        let security = exact_security.inheritable_attributes();
        let create_log = |path: &std::path::Path| {
            let path = path
                .as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>();
            Handle(unsafe {
                CreateFileW(
                    path.as_ptr(),
                    GENERIC_WRITE,
                    FILE_SHARE_READ,
                    &security,
                    CREATE_NEW,
                    FILE_ATTRIBUTE_NORMAL,
                    null_mut(),
                )
            })
        };
        let output = create_log(stdout_log_path);
        session_crash_at("child-stdout-log-created");
        let error = create_log(stderr_log_path);
        session_crash_at("child-stderr-log-created");
        talking_quill_helper::owned_tree::flush_owned_directory(
            stdout_log_path.parent().ok_or(())?,
        )
        .map_err(|_| ())?;
        session_crash_at("child-log-directory-flushed");
        let nul = std::ffi::OsStr::new("NUL")
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let input = Handle(unsafe {
            CreateFileW(
                nul.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                &security,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            )
        });
        if output.0 == INVALID_HANDLE_VALUE
            || error.0 == INVALID_HANDLE_VALUE
            || input.0 == INVALID_HANDLE_VALUE
        {
            return Err(());
        }
        inherited_handles.extend([input.0, output.0, error.0]);
        let mut attribute_bytes = 0usize;
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut attribute_bytes);
        }
        attribute_storage.resize(attribute_bytes.div_ceil(size_of::<usize>()), 0);
        attribute_list.0 = attribute_storage.as_mut_ptr().cast();
        if unsafe {
            InitializeProcThreadAttributeList(attribute_list.0, 1, 0, &mut attribute_bytes)
        } == 0
            || unsafe {
                UpdateProcThreadAttribute(
                    attribute_list.0,
                    0,
                    PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                    inherited_handles.as_ptr().cast(),
                    size_of_val(inherited_handles.as_slice()),
                    null_mut(),
                    null_mut(),
                )
            } == 0
        {
            return Err(());
        }
        startup_ex.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup_ex.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup_ex.StartupInfo.hStdInput = input.0;
        startup_ex.StartupInfo.hStdOutput = output.0;
        startup_ex.StartupInfo.hStdError = error.0;
        startup_ex.lpAttributeList = attribute_list.0;
        stdout_handle = Some(output);
        stderr_handle = Some(error);
        input_handle = Some(input);
        (
            (&raw const startup_ex.StartupInfo),
            CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT,
        )
    } else {
        startup.cb = size_of::<STARTUPINFOW>() as u32;
        (&raw const startup, CREATE_SUSPENDED)
    };
    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    if unsafe {
        CreateProcessW(
            null_mut(),
            command_line.as_mut_ptr(),
            null_mut(),
            null_mut(),
            1,
            creation_flags,
            null_mut(),
            null_mut(),
            startup_pointer,
            &mut process,
        )
    } == 0
    {
        return Err(());
    }
    drop(input_handle);
    drop(attribute_list);
    drop(attribute_storage);
    let process_handle = Handle(process.hProcess);
    let thread_handle = Handle(process.hThread);
    if remove_dangerous_privileges(process_handle.0).is_err()
        || unsafe { AssignProcessToJobObject(job.0, process_handle.0) } == 0
    {
        unsafe { TerminateProcess(process_handle.0, 74) };
        return Err(());
    }
    if unsafe { ResumeThread(thread_handle.0) } == u32::MAX {
        unsafe { TerminateJobObject(job.0, 74) };
        return Err(());
    }
    if unsafe { WaitForSingleObject(process_handle.0, INFINITE) } != WAIT_OBJECT_0 {
        return Err(());
    }
    loop {
        let mut accounting: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { zeroed() };
        if unsafe {
            QueryInformationJobObject(
                job.0,
                JobObjectBasicAccountingInformation,
                (&raw mut accounting).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                null_mut(),
            )
        } == 0
        {
            return Err(());
        }
        if accounting.ActiveProcesses == 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    for (index, log) in [&stdout_handle, &stderr_handle].into_iter().enumerate() {
        if let Some(log) = log {
            if unsafe { FlushFileBuffers(log.0) } == 0 {
                return Err(());
            }
            session_crash_at(if index == 0 {
                "child-stdout-log-flushed"
            } else {
                "child-stderr-log-flushed"
            });
        }
    }
    let stdout_guard = stdout_handle
        .take()
        .map(|handle| {
            let raw = handle.0;
            std::mem::forget(handle);
            raw
        })
        .unwrap_or(null_mut());
    let stderr_guard = stderr_handle
        .take()
        .map(|handle| {
            let raw = handle.0;
            std::mem::forget(handle);
            raw
        })
        .unwrap_or(null_mut());
    let mut exit_code = 0;
    if unsafe { GetExitCodeProcess(process_handle.0, &mut exit_code) } == 0 {
        return Err(());
    }
    Ok((exit_code, stdout_guard, stderr_guard))
}
