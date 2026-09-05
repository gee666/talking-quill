use super::*;

fn test_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tmp")
        .join(format!(
            "talking-quill-key-security-{}-{name}",
            std::process::id()
        ))
}

fn create_hard_linked_exact_acl_key(key: &Path, linked: &Path) -> File {
    let ancestors =
        create_protected_directories(key.parent().expect("key parent")).expect("create parent");
    let sid = current_user_sid_string().expect("current user sid");
    let creation_descriptor = SecurityDescriptor::from_sddl(&format!(
        "O:{sid}D:P(A;;0x120089;;;SY)(A;;0x120089;;;BA)(A;;FA;;;{sid})"
    ))
    .expect("create writable descriptor");
    let final_descriptor = SecurityDescriptor::from_sddl(&format!(
        "O:{sid}D:P(A;;0x120089;;;SY)(A;;0x120089;;;BA)(A;;0x120089;;;{sid})"
    ))
    .expect("create final descriptor");
    let mut wide = key.as_os_str().encode_wide().collect::<Vec<_>>();
    wide.push(0);
    let attributes = creation_descriptor.attributes();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE | WRITE_DAC,
            FILE_SHARE_READ,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    assert_ne!(handle, INVALID_HANDLE_VALUE, "create writable key");
    let mut writable = unsafe { File::from_raw_handle(handle) };
    writable.write_all(&[1, 2, 3]).expect("write key");
    assert_ne!(
        unsafe { FlushFileBuffers(writable.as_raw_handle()) },
        0,
        "flush writable key"
    );
    drop(writable);
    drop(ancestors);
    std::fs::hard_link(key, linked).expect("create hard link while permitted");

    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE | WRITE_DAC,
            FILE_SHARE_READ,
            null_mut(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    assert_ne!(handle, INVALID_HANDLE_VALUE, "reopen writable key");
    let file = unsafe { File::from_raw_handle(handle) };
    assert_ne!(
        unsafe {
            SetKernelObjectSecurity(
                file.as_raw_handle(),
                DACL_SECURITY_INFORMATION,
                final_descriptor.0,
            )
        },
        0,
        "apply final key ACL"
    );
    file
}

#[test]
fn exact_acl_key_is_admitted_and_hard_link_is_rejected() {
    let directory = test_path("exact");
    let key = directory.join("key.der");
    let linked = directory.join("linked.der");
    let _ = std::fs::remove_dir_all(&directory);
    let retained = create_hard_linked_exact_acl_key(&key, &linked);
    validate_security(retained.as_raw_handle()).expect("validate exact protected ACL");
    assert_eq!(
        validate_private_key_handle(&retained),
        Err("private key file identity")
    );
    drop(retained);
    assert_eq!(
        open_validated_private_key(&key).err(),
        Some("private key file identity")
    );
    std::fs::remove_file(&linked).expect("remove hard link");
    let admitted = open_validated_private_key(&key).expect("admit single-link key");
    drop(admitted);
    delete_validated_private_key(&key).expect("handle-delete key");
    assert!(!key.exists());
    std::fs::remove_dir(&directory).expect("remove directory");
}
