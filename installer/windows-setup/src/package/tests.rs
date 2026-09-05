use super::*;

fn fixture() -> Vec<u8> {
    let payloads: [(&str, &[u8]); 4] = [
        ("resources/helper/talking-quill-helper.exe", b"gateway"),
        (
            "resources/helper/talking-quill-keyboard-owner.exe",
            b"owner",
        ),
        (
            "resources/helper/talking-quill-update-recovery-launcher.exe",
            b"recovery-launcher",
        ),
        ("resources/keyboard-owner-release-v1.json", b"release"),
    ];
    let blocks: Vec<Vec<u8>> = payloads
        .iter()
        .map(|(_, bytes)| zstd::stream::encode_all(*bytes, 3).unwrap())
        .collect();
    let mut tree = Sha256::new();
    let mut files = Vec::new();
    for ((path, content), block) in payloads.iter().zip(&blocks) {
        let content_hash = hex(&Sha256::digest(content));
        for value in [*path, "0", &content.len().to_string(), &content_hash] {
            frame(&mut tree, value);
        }
        files.push(serde_json::json!({"blockOffset":0,"blockSize":block.len(),"mode":0,"path":path,"sha256":content_hash,"size":content.len()}));
    }
    let mut value = serde_json::json!({
        "architecture":"x64", "files":files,
        "faultPhase":null, "packageMode":"fresh", "predecessor":null, "schemaVersion":2, "sourceCommit":"ab".repeat(20),
        "sourceTree":"cd".repeat(20), "target":{"gatewaySha256":hex(&Sha256::digest(b"gateway")),"ownerSha256":hex(&Sha256::digest(b"owner")),"recoveryLauncherSha256":hex(&Sha256::digest(b"recovery-launcher")),"releaseBuildDigest":"33".repeat(32)},
        "treeSha256":hex(&tree.finalize()), "version":"0.0.69"
    });
    let mut manifest = serde_json::to_vec(&value).unwrap();
    for _ in 0..4 {
        let mut offset = manifest.len();
        for (file, block) in value["files"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .zip(&blocks)
        {
            file["blockOffset"] = serde_json::json!(offset);
            offset += block.len();
        }
        manifest = serde_json::to_vec(&value).unwrap();
    }
    let mut image = vec![0_u8; 512];
    image[..2].copy_from_slice(b"MZ");
    image[60..64].copy_from_slice(&64_u32.to_le_bytes());
    image[64..68].copy_from_slice(b"PE\0\0");
    image[88..90].copy_from_slice(&0x20b_u16.to_le_bytes());
    let package_offset = image.len() as u64;
    let mut package = manifest.clone();
    for block in blocks {
        package.extend_from_slice(&block);
    }
    image.extend_from_slice(&package);
    let mut footer = [0_u8; FOOTER_SIZE];
    footer[..8].copy_from_slice(FOOTER_MAGIC);
    footer[8..12].copy_from_slice(&2_u32.to_le_bytes());
    footer[16..24].copy_from_slice(&package_offset.to_le_bytes());
    footer[24..32].copy_from_slice(&(package.len() as u64).to_le_bytes());
    footer[32..40].copy_from_slice(&(manifest.len() as u64).to_le_bytes());
    footer[40..72].copy_from_slice(&Sha256::digest(&package));
    footer[72..104].copy_from_slice(&Sha256::digest(&manifest));
    image.extend_from_slice(&footer);
    image
}

#[test]
fn parses_and_extracts_a_canonical_full_tree() {
    let bytes = fixture();
    let mut reader = std::io::Cursor::new(&bytes);
    let parsed = parse(&mut reader, bytes.len() as u64).unwrap();
    let mut output = Vec::new();
    extract_file(&mut reader, &parsed, &parsed.manifest.files[0], &mut output).unwrap();
    assert_eq!(output, b"gateway");
}

#[test]
fn rejects_package_and_manifest_mutation() {
    let mut package_mutation = fixture();
    package_mutation[512] ^= 1;
    assert!(matches!(
        parse(
            &mut std::io::Cursor::new(&package_mutation),
            package_mutation.len() as u64
        ),
        Err(PackageError::Digest)
    ));
    let mut footer_mutation = fixture();
    let position = footer_mutation.len() - FOOTER_SIZE + 8;
    footer_mutation[position] = 3;
    assert!(matches!(
        parse(
            &mut std::io::Cursor::new(&footer_mutation),
            footer_mutation.len() as u64
        ),
        Err(PackageError::Footer)
    ));
    let mut reserved_mutation = fixture();
    let position = reserved_mutation.len() - FOOTER_SIZE + 104;
    reserved_mutation[position] = 1;
    assert!(matches!(
        parse(
            &mut std::io::Cursor::new(&reserved_mutation),
            reserved_mutation.len() as u64
        ),
        Err(PackageError::Footer)
    ));
}

#[test]
fn rejects_concatenated_valid_compressed_frames() {
    let content = b"payload";
    let mut block = zstd::stream::encode_all(&content[..], 3).unwrap();
    block.extend_from_slice(&zstd::stream::encode_all(&b"second"[..], 3).unwrap());
    let file = ManifestFile {
        path: "a".into(),
        mode: 0,
        size: content.len() as u64,
        sha256: hex(&Sha256::digest(content)),
        block_offset: 0,
        block_size: block.len() as u64,
    };
    let package = ParsedPackage {
        package_offset: 0,
        manifest: Manifest {
            schema_version: 2,
            architecture: "x64".into(),
            version: "0.0.69".into(),
            source_commit: "11".repeat(20),
            source_tree: "22".repeat(20),
            package_mode: "fresh".into(),
            predecessor: None,
            target: TargetIdentity {
                release_build_digest: "33".repeat(32),
                gateway_sha256: "44".repeat(32),
                owner_sha256: "55".repeat(32),
                recovery_launcher_sha256: "66".repeat(32),
            },
            fault_phase: None,
            tree_sha256: "66".repeat(32),
            files: vec![file.clone()],
        },
    };
    assert_eq!(
        extract_file(
            &mut std::io::Cursor::new(block),
            &package,
            &file,
            &mut Vec::new()
        ),
        Err(PackageError::Block)
    );
}

#[test]
fn rejects_mismatched_modes_targets_and_fault_seams() {
    let bytes = fixture();
    let mut reader = std::io::Cursor::new(&bytes);
    let mut parsed = parse(&mut reader, bytes.len() as u64).unwrap();
    let footer = bytes.len() - FOOTER_SIZE;
    let package_size = u64::from_le_bytes(bytes[footer + 24..footer + 32].try_into().unwrap());
    let manifest_size = u64::from_le_bytes(bytes[footer + 32..footer + 40].try_into().unwrap());
    parsed.manifest.package_mode = "release".into();
    assert_eq!(
        validate_manifest(&parsed.manifest, package_size, manifest_size),
        Err(PackageError::Identity)
    );
    parsed.manifest.package_mode = "fresh".into();
    parsed.manifest.predecessor = Some(Predecessor {
        version: "0.0.68".into(),
        release_build_digest: "11".repeat(32),
        gateway_sha256: "22".repeat(32),
        owner_sha256: "33".repeat(32),
    });
    assert_eq!(
        validate_manifest(&parsed.manifest, package_size, manifest_size),
        Err(PackageError::Identity)
    );
    parsed.manifest.predecessor = None;
    parsed.manifest.package_mode = "repair".into();
    for phase in [
        "staged",
        "prepared",
        "predecessorMoved",
        "publishing",
        "publishedBeforePersist",
        "published",
        "registered",
        "committed",
        "legacyRetiring",
        "legacyRetired",
    ] {
        parsed.manifest.fault_phase = Some(phase.into());
        assert_eq!(
            validate_manifest(&parsed.manifest, package_size, manifest_size),
            if cfg!(feature = "acceptance-faults") {
                Ok(())
            } else {
                Err(PackageError::Identity)
            },
            "{phase}"
        );
    }
    parsed.manifest.fault_phase = Some("arbitrary".into());
    assert_eq!(
        validate_manifest(&parsed.manifest, package_size, manifest_size),
        Err(PackageError::Identity)
    );
}

#[test]
fn nullable_manifest_fields_are_required_on_the_wire() {
    let bytes = fixture();
    let footer = bytes.len() - FOOTER_SIZE;
    let offset = u64::from_le_bytes(bytes[footer + 16..footer + 24].try_into().unwrap()) as usize;
    let size = u64::from_le_bytes(bytes[footer + 32..footer + 40].try_into().unwrap()) as usize;
    let value: serde_json::Value = serde_json::from_slice(&bytes[offset..offset + size]).unwrap();
    for field in ["predecessor", "faultPhase"] {
        let mut mutation = value.clone();
        mutation.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<Manifest>(mutation).is_err(),
            "{field}"
        );
    }
}

#[test]
fn rejects_windows_path_aliases() {
    for path in [
        "../app.exe",
        "a\\b",
        "a:b",
        "CON",
        "dir/nul.txt",
        "a./b",
        "AUX.txt",
        "/root",
    ] {
        assert_eq!(validate_path(path), Err(PackageError::Path), "{path}");
    }
    assert_eq!(validate_path("resources/app.asar"), Ok(()));
}

#[test]
fn rejects_case_collisions_and_noncanonical_json() {
    let files = vec![
        ManifestFile {
            path: "A.txt".into(),
            mode: 0,
            size: 1,
            sha256: "00".repeat(32),
            block_offset: 1,
            block_size: 1,
        },
        ManifestFile {
            path: "a.TXT".into(),
            mode: 0,
            size: 1,
            sha256: "00".repeat(32),
            block_offset: 2,
            block_size: 1,
        },
    ];
    let manifest = Manifest {
        schema_version: 2,
        architecture: "x64".into(),
        version: "0.0.69".into(),
        source_commit: "00".repeat(20),
        source_tree: "11".repeat(20),
        package_mode: "fresh".into(),
        predecessor: None,
        target: TargetIdentity {
            release_build_digest: "33".repeat(32),
            gateway_sha256: "44".repeat(32),
            owner_sha256: "55".repeat(32),
            recovery_launcher_sha256: "66".repeat(32),
        },
        fault_phase: None,
        tree_sha256: "22".repeat(32),
        files,
    };
    assert_eq!(
        validate_manifest(&manifest, 3, 1),
        Err(PackageError::Collision)
    );
}
