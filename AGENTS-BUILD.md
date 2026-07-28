# Build and installation instructions for AI agents

Your goal is to leave the user with a native, easy-to-install Talking Quill package—not merely compiled source or an unpacked application directory. Work from the repository root, explain any system-level changes before making them, and stop with a clear error if a required command fails.

## 1. Identify the environment

Inspect the host before installing or building anything:

- **Windows:** use PowerShell and check `$env:PROCESSOR_ARCHITECTURE` plus `[System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture`.
- **macOS:** use `uname -s` and `uname -m`. `arm64` is Apple Silicon; `x86_64` is Intel.
- **WSL:** confirm both `uname -s` and `/proc/version` (or `WSL_DISTRO_NAME`). WSL is not a supported packaging host because the native helper intentionally refuses cross-platform builds. Do not attempt to ship a Linux package or cross-build the Windows helper from WSL. Switch to native Windows PowerShell, preferably using a clone in a normal Windows path, and continue there.

Talking Quill supports Windows and macOS only. Build the package matching the user's native OS and processor unless the user explicitly requests another architecture. Windows packages must be built on Windows and macOS packages on macOS.

Also inspect `git status` before working. Do not discard unrelated user changes, and never place credentials in the repository or logs.

## 2. Install the required toolchain

The pinned versions in the repository are authoritative:

- Git
- Node.js **24.15.0** (`.node-version` and `package.json`)
- pnpm **11.13.0** (`package.json`)
- Rust **1.97.1** with Cargo, rustfmt, and Clippy (`rust-toolchain.toml`)

After installation, open a fresh shell if PATH changed and verify:

```text
node --version
pnpm --version
rustup --version
cargo --version
git --version
```

Use a Node version manager when one is already available. Otherwise install the exact Node release from nodejs.org (or an OS package manager that provides that exact version), then enable the pinned pnpm version:

```text
corepack enable
corepack prepare pnpm@11.13.0 --activate
```

If Corepack is unavailable, install it or run `npm install --global pnpm@11.13.0`. Do not silently use a different Node or pnpm version.

Install Rust with rustup from <https://rustup.rs/>. Running Cargo in this repository selects and installs the pinned toolchain automatically. If needed, force it with:

```text
rustup toolchain install 1.97.1 --profile minimal --component rustfmt --component clippy
```

### Windows prerequisites

Install **Visual Studio 2022 Build Tools** with **Desktop development with C++**, MSVC v143, and a current Windows 10 or 11 SDK. For an ARM64 package, include the MSVC ARM64 build tools. `winget` may be used when available, but obtain the user's permission before installing system software.

Use native PowerShell—not WSL, Git Bash, or an MSYS Node installation—for dependency installation and packaging.

### macOS prerequisites

Install the Xcode command-line tools when absent:

```bash
xcode-select --install
```

Confirm `xcode-select -p`, `clang --version`, and `codesign --version` work. A package intended for normal distribution should be signed with a Developer ID Application certificate and notarized. Request signing/notarization values through the user's secure environment or secret manager; never ask the user to commit them:

- `CSC_LINK`
- `CSC_KEY_PASSWORD`
- `APPLE_ID`
- `APPLE_APP_SPECIFIC_PASSWORD`
- `APPLE_TEAM_ID`

If these credentials are unavailable, clearly label the result as an unsigned local/test build and warn that Gatekeeper may block it. Do not claim that an unsigned package is release-ready.

## 3. Install dependencies

From the repository root, install exactly what the lockfile specifies:

```text
pnpm install --frozen-lockfile
```

Do not use npm or yarn for workspace dependencies, do not regenerate `pnpm-lock.yaml`, and do not bypass failed native dependency builds. Diagnose and fix the toolchain instead.

## 4. Validate and build

Run the repository's complete validation before packaging:

```text
pnpm validate
```

This checks formatting, linting, TypeScript, tests, the Rust helper, generated metadata, boundaries, and the production build. If a failure is caused by pre-existing user changes, report it rather than overwriting those changes. Do not package a build with unexplained validation failures.

## 5. Produce the native installer

The packaging scripts compile the Electron application, build the Rust helper for the selected architecture, package native dependencies, inspect the package, and restore the development native module afterward.

### Windows

In native PowerShell at the repository root:

```powershell
# Most Intel/AMD Windows computers
pnpm package:win

# Windows on ARM
pnpm package:win:arm64
```

Deliver the NSIS installer from `release/`:

- x64: `release/Talking-Quill-<version>-win-x64.exe`
- ARM64: `release/Talking-Quill-<version>-win-arm64.exe`

Do not give the user `win-unpacked/` or `win-arm64-unpacked/` as the primary deliverable. The `.exe` installer supports a per-user installation, an installation-directory choice, and Start menu/desktop shortcuts.

### macOS

On a native Mac at the repository root, with distribution credentials available when producing a release package:

```bash
# Apple Silicon
pnpm package:mac:arm64

# Intel Mac
pnpm package:mac:x64
```

Deliver the architecture-matched DMG from `release/`:

- Apple Silicon: `release/Talking-Quill-<version>-mac-arm64.dmg`
- Intel: `release/Talking-Quill-<version>-mac-x64.dmg`

The ZIP is useful for updates or archival, but the DMG is the easiest interactive installation package. Verify signing and notarization for a release build:

```bash
codesign --verify --deep --strict --verbose=2 "release/mac-arm64/Talking Quill.app" # Apple Silicon
# Use release/mac for Intel.
spctl --assess --type execute --verbose=2 "release/mac-arm64/Talking Quill.app"
xcrun stapler validate release/Talking-Quill-*-mac-*.dmg
```

## 6. Verify and hand off

1. Confirm the expected installer exists, is non-empty, and has the correct architecture.
2. Generate a SHA-256 checksum (`Get-FileHash -Algorithm SHA256` on Windows or `shasum -a 256` on macOS).
3. When practical, install the package in a clean local user account or disposable VM, launch Talking Quill, and confirm the Welcome screen appears. Do not alter the user's regular installation without permission.
4. Give the user the absolute installer path, filename, architecture, checksum, validation results, and any signing/notarization limitation.
5. Include short install directions: run the Windows installer, or open the macOS DMG and drag Talking Quill to Applications.

A successful handoff ends with an installer the user can click, not with a development command or an unpacked directory.
