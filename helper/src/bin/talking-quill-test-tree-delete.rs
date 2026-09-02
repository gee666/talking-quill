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
    child_log_path: std::path::PathBuf,
    control_nonce: String,
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
    if let [mode, request] = arguments.as_slice()
        && mode == "--namespace-session"
    {
        let Some(request) = request.to_str() else {
            std::process::exit(64);
        };
        let Ok(request) = serde_json::from_str::<NamespaceSessionRequest>(request) else {
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
    if let [mode, path] = arguments.as_slice()
        && mode == "--consume-record-temp"
    {
        match talking_quill_helper::machine_lock_test_namespace::consume_cleanup_record_temp(
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
    match arguments.as_slice() {
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
fn session_record_crash_at(phase: &str) {
    if std::env::var("TQ_MACHINE_LOCK_TEST_CRASH_AFTER").as_deref() == Ok(phase) {
        std::process::exit(197);
    }
}

#[cfg(windows)]
fn update_session_record(
    path: &std::path::Path,
    update: impl FnOnce(&mut serde_json::Value) -> Result<(), &'static str>,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    let mut record = serde_json::from_slice::<serde_json::Value>(&std::fs::read(path)?)?;
    update(&mut record).map_err(std::io::Error::other)?;
    let temporary = path.with_extension("native-tmp");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    session_record_crash_at("native-record-temp-created");
    talking_quill_helper::machine_lock_test_namespace::protect_cleanup_record(&temporary)?;
    session_record_crash_at("native-record-temp-protected");
    file.write_all(&serde_json::to_vec(&record)?)?;
    session_record_crash_at("native-record-temp-written");
    file.sync_all()?;
    session_record_crash_at("native-record-temp-flushed");
    drop(file);
    std::fs::rename(&temporary, path)?;
    talking_quill_helper::owned_tree::flush_owned_directory(
        path.parent()
            .ok_or_else(|| std::io::Error::other("cleanup record has no parent"))?,
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
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_OUTPUT_HANDLE};
    let control_handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) } as usize;
    unsafe {
        std::env::set_var(
            "TQ_MACHINE_LOCK_TEST_CONTROL_HANDLE_VALUE",
            control_handle.to_string(),
        );
    }
    let child_result = supervise_job(&request.command, Some(&request.child_log_path));
    unsafe {
        std::env::remove_var("TQ_MACHINE_LOCK_TEST_CONTROL_HANDLE_VALUE");
    }
    let child_code = child_result.map_err(|_| "job supervision failed")?;
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
    supervise_job(command, None)
}

#[cfg(windows)]
fn supervise_job(command: &str, child_log_path: Option<&std::path::Path>) -> Result<u32, ()> {
    use std::{mem::zeroed, os::windows::ffi::OsStrExt, ptr::null_mut};
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
        },
        Security::SECURITY_ATTRIBUTES,
        Storage::FileSystem::{
            CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_DELETE, FILE_SHARE_READ,
            FILE_SHARE_WRITE, OPEN_EXISTING,
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
                InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, ResumeThread,
                STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW, UpdateProcThreadAttribute,
                WaitForSingleObject,
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
    let mut log_handle = None;
    let mut input_handle = None;
    let mut attribute_storage = Vec::<usize>::new();
    let mut attribute_list = AttributeList(null_mut());
    let mut startup_ex: STARTUPINFOEXW = unsafe { zeroed() };
    let mut startup: STARTUPINFOW = unsafe { zeroed() };
    let (startup_pointer, creation_flags) = if let Some(log_path) = child_log_path {
        let security = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null_mut(),
            bInheritHandle: 1,
        };
        let log_path = log_path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let log = Handle(unsafe {
            CreateFileW(
                log_path.as_ptr(),
                GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_DELETE,
                &security,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                null_mut(),
            )
        });
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
        if log.0 == INVALID_HANDLE_VALUE || input.0 == INVALID_HANDLE_VALUE {
            return Err(());
        }
        inherited_handles.extend([input.0, log.0]);
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
        startup_ex.StartupInfo.hStdOutput = log.0;
        startup_ex.StartupInfo.hStdError = log.0;
        startup_ex.lpAttributeList = attribute_list.0;
        log_handle = Some(log);
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
    drop(log_handle);
    drop(attribute_list);
    drop(attribute_storage);
    let process_handle = Handle(process.hProcess);
    let thread_handle = Handle(process.hThread);
    if unsafe { AssignProcessToJobObject(job.0, process_handle.0) } == 0
        || unsafe { ResumeThread(thread_handle.0) } == u32::MAX
    {
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
    let mut exit_code = 0;
    if unsafe { GetExitCodeProcess(process_handle.0, &mut exit_code) } == 0 {
        return Err(());
    }
    Ok(exit_code)
}
