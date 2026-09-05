//! Give the native bootstrap its own Windows version and execution policy.
use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=WindowsSdkDir");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = env::var("CARGO_PKG_VERSION").expect("package version");
    let mut numbers: Vec<u16> = version
        .split('.')
        .map(|part| part.parse().expect("numeric version"))
        .collect();
    numbers.resize(4, 0);
    let numeric = numbers
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("build output"));
    let manifest = output.join("setup.manifest");
    fs::write(
        &manifest,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3"><security><requestedPrivileges>
    <requestedExecutionLevel level="asInvoker" uiAccess="false" />
  </requestedPrivileges></security></trustInfo>
</assembly>"#,
    )
    .expect("write setup manifest");
    let resource = output.join("setup.rc");
    let manifest_path = manifest.to_string_lossy().replace('\\', "\\\\");
    fs::write(
        &resource,
        format!(
            r#"
1 24 "{manifest_path}"
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEFLAGSMASK 0x3fL
FILEFLAGS 0
FILEOS 0x40004L
FILETYPE 1
FILESUBTYPE 0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "FileDescription", "Talking Quill Setup\0"
      VALUE "FileVersion", "{version}\0"
      VALUE "InternalName", "talking-quill-windows-setup\0"
      VALUE "OriginalFilename", "talking-quill-windows-setup.exe\0"
      VALUE "ProductName", "Talking Quill\0"
      VALUE "ProductVersion", "{version}\0"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
        ),
    )
    .expect("write setup resource");
    let compiled = output.join("setup.res");
    let compiler = resource_compiler();
    let status = Command::new(&compiler)
        .arg("/nologo")
        .arg("/fo")
        .arg(&compiled)
        .arg(&resource)
        .status()
        .unwrap_or_else(|error| panic!("cannot run {}: {error}", compiler.display()));
    assert!(status.success(), "Windows resource compilation failed");
    println!("cargo:rustc-link-arg={}", compiled.display());
    // The resource above owns the manifest; prevent LINK from generating a second one.
    println!("cargo:rustc-link-arg=/MANIFEST:NO");
}

fn resource_compiler() -> PathBuf {
    if let Some(path) = env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|directory| directory.join("rc.exe"))
            .find(|path| path.is_file())
    }) {
        return path;
    }
    let sdk = env::var_os("WindowsSdkDir")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env::var_os("ProgramFiles(x86)").expect("Windows SDK installation"))
                .join("Windows Kits/10")
        });
    let mut versions: Vec<_> = fs::read_dir(sdk.join("bin"))
        .expect("Windows SDK bin directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    versions.sort();
    let host = if env::var("HOST").unwrap_or_default().starts_with("aarch64") {
        "arm64"
    } else {
        "x64"
    };
    versions
        .into_iter()
        .rev()
        .map(|version| version.join(host).join("rc.exe"))
        .find(|path| path.is_file())
        .expect("Install the Windows SDK resource compiler (rc.exe)")
}
