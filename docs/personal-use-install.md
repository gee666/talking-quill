# Fresh personal-use packages

These commands build **fresh-install, unsigned, owner-enabled** packages directly from this checkout. They do not need a previous release, lifecycle evidence, Authenticode, Developer ID, notarization, or an Apple account. Windows builds do require the reviewed stable updater verification key at `build/windows-update-public-key.sec1`; the build fails if that file is missing or malformed. They do not bypass updater publication checks. A package marked `freshInstall` cannot be staged as an updater. Update metadata binds the package hash and the exact predecessor version, gateway, and owner hashes. Windows has no enrollment or candidate build ID.

Run `corepack enable`, `pnpm install --frozen-lockfile`, and the commands below from the repository root. Build only code you trust. Unsigned OS warnings do not establish that code is safe.

## Windows x64 and ARM64

Build on the matching native Windows architecture. The updater and release metadata use separate architecture channels.

```powershell
# x64
pnpm personal:win:build

# ARM64
pnpm personal:win:arm64:build
```

The artifact is `release\Talking-Quill-<version>-win-<arch>-setup.exe`. **Double-click that normal `.exe` directly**; do not run a pnpm install command, set an environment variable, copy a SHA-256, or create/download a sidecar. On **Windows protected your PC**, select **More info**, confirm the displayed app/path is the artifact you just built, then select **Run anyway**. Approve the following **User Account Control (UAC)** prompt. The installer then installs normally. Declining either prompt makes no system change beyond any inert pre-consent backup and is safe to retry.

Release operators keep the P-256 PKCS#8 DER key in a protected file. Windows update staging accepts only its absolute `--update-private-key` path. It builds the signer, broker, bootstrap, and key tool from the clean reviewed checkout with `helper/Cargo.lock`, verifies their source commit, tree, x64 PE identity, file identity, and hashes, then publishes a protected identity-named snapshot. Callers cannot choose native paths or hashes. The native broker retains every key ancestor, rejects reparse points, and checks the key handle's owner, protected DACL, exact read-only principal set, and single-link identity before inheritance. Each x64 and ARM64 release still checks the exact predecessor gateway and requires the native signer to derive that inspected key.

The protected release workflow imports the reviewer-environment secret only in its narrow update-signing step. It removes the secret environment entry before starting the native importer, sends the key through stdin, stages with the resulting path only, and runs native validated deletion under `always()` before checking that the path is absent.

A key rotation uses one bridge release. The installed predecessor validates the bridge candidate with its own old primary key. The bridge candidate embeds only the new primary key, so an installed successor has no retired-key fallback. Release authorization still binds the exact package and canonical gateway and owner layout. The next release must be signed by the new key. Keep the old private key only long enough to authorize and test the bridge for both architectures.

The build embeds a canonical TQPKG2 full-tree manifest and compressed block for every application file. Package inspection rejects a mismatched path, digest, architecture, source identity, package mode, or predecessor. Windows updater publication also signs the exact outer native setup SHA-256 and canonical package-layout digest with the release-policy P-256 key whose public key is compiled into the installed gateway. The installed gateway verifies that signature, the exact architecture-specific predecessor, and the downloaded installer bytes before it resumes the staged elevated setup. Native setup then recomputes the extracted manifest layout and hashes the extracted gateway and owner before commit. The elevated setup keeps the previous Program Files tree in a protected sibling recovery directory until the replacement has been extracted and verified. A write-through transaction records staging, preparation, commit, and uninstall recovery. A failed or interrupted repair restores the predecessor before retry. Existing per-user data remains untouched.

`pnpm personal:win:check` remains an optional builder diagnostic that recursively extracts the generated installer, verifies the embedded identities, and prints its outer SHA-256. It is not an installation prerequisite.

On first run, start **Talking Quill** from Start, complete the welcome/model/microphone setup, choose a shortcut, and leave the app running. Launch at login can then be enabled in Settings.

After first launch, verify the package and the two running native roles:

```powershell
pnpm personal:win:verify
Get-Process talking-quill-helper,talking-quill-keyboard-owner -ErrorAction Stop
```

Personal builds do not run the interactive installer observer, which requires privileged Windows event access. Release packaging still requires it; builders can run `pnpm exec node scripts/run-windows-installer-ui-smoke.mjs` separately.

`personal:win:verify` checks the packaged role hashes and architecture, absence of the legacy service and ProgramData enrollment, and the authenticated gateway/owner runtime. For physical capture, focus Notepad, press the configured shortcut, dictate a short sentence, press Enter (or repeat the shortcut), and confirm text is inserted. Press Escape in a second attempt and confirm cancellation. If these fail, capture is not verified even when process checks pass.

### Repair, rollback, and uninstall

- **Repair/update:** rerun the full personal installer to replace an existing or broken installation. It first requests a graceful shutdown, then terminates remaining processes whose executable paths belong to the installation, using retained process handles. The elevated worker also removes obsolete keyboard service and scheduled-task state. The previous application tree remains recoverable until replacement and cleanup succeed. User data in `%APPDATA%\Talking Quill` is preserved. Downloaded automatic updates continue to require exact predecessor verification.
- **Rollback:** release-updater rollback remains predecessor-bound and separate from personal reinstall. Package metadata must identify the expected source and target release and exact package hash; arbitrary downgrade or rebuilt bytes fail closed.
- **Emergency capture switch:** start Talking Quill with `$env:TALKING_QUILL_DISABLE_ACTIVATION_CAPTURE='1'; & "$env:ProgramFiles\Talking Quill\Talking Quill.exe"`. This disables activation-key capture for that process; it does not weaken owner authentication.
- **Uninstall:** Settings -> Apps -> Installed apps -> Talking Quill -> Uninstall, or run `& "$env:ProgramFiles\Talking Quill\Uninstall Talking Quill.exe"`, then approve UAC. The separate prompt to remove settings, models, voice commands, recordings, dictionaries, and other personal data defaults to preservation and requires explicit confirmation. Both the installed uninstaller and registered maintenance command use one UAC prompt, wait for the authenticated cleanup worker, and return its final result. After elevation succeeds and the worker persists recovery authority, Windows renames the exact mapped image stream and commits deletion of its file namespace by retained handle before machine mutation starts. A successful result means registration, the canonical install tree, the quarantine, and the transaction journal are gone; only the kernel-owned mapped file remains until the calling process exits. Ordinary uninstall waits for local runtime exit, removes application binaries and obsolete service state, and preserves `%APPDATA%\Talking Quill` plus any separately configured model location. If an older broken uninstaller is missing protected maintenance, rerun the full personal installer, then uninstall.

Never update/repair/uninstall while shortcut, Enter, or Escape is held. Release all keys and retry.

## macOS 13+, Intel x64 or Apple Silicon ARM64

Create one stable local certificate and installation identity on the build Mac:

```bash
pnpm personal:mac:setup
```

This creates/imports a least-privilege, non-CA self-signed code-signing/CMS leaf in the login Keychain, marks only that leaf trusted locally, and writes non-secret stable build pins to `~/Library/Application Support/Talking Quill Local Build/signing.json`. Preserve the Keychain private identity and this file for future exact-predecessor updates; never send the private key or a Keychain export to a friend.

Build the architecture that matches **About This Mac**:

```bash
# Intel
pnpm personal:mac:x64:build
pnpm personal:mac:x64:check
pnpm personal:mac:x64:install

# Apple Silicon
pnpm personal:mac:arm64:build
pnpm personal:mac:arm64:check
pnpm personal:mac:arm64:install
```

Install copies and re-hashes the exact checked ZIP in private staging, validates its embedded identity, and runs the packaged native `SMAppService.status` probe. It proceeds only when the exact LoginItem is authoritatively `notRegistered` and no app, Keychain enrollment, durable owner/cleanup state, gateway, or owner remains.

For diagnostic ad-hoc code signing (less stable TCC identity), prefix the build with `TALKING_QUILL_MACOS_LOCAL_SIGNING_MODE=adhoc`. The same local certificate still seals the authenticated policy. Changed ad-hoc binaries normally require removing and re-adding privacy grants.

After installation, **Control-click** `/Applications/Talking Quill.app`, choose **Open**, then **Open** again. If macOS instead blocks it, use System Settings → Privacy & Security → scroll to the security message → **Open Anyway**, authenticate, and confirm **Open**. Do not remove quarantine with `xattr` as a substitute for reviewing this prompt.

On first run:

1. Approve **Talking Quill Keyboard Owner** under System Settings → General → Login Items, if requested.
2. Add/enable **Talking Quill Keyboard Owner** in Privacy & Security → **Accessibility** and **Input Monitoring**.
3. Approve the macOS **event-posting** prompt if macOS presents it. Grant **Automation** only if macOS separately presents a specific Apple Events prompt; it is not a substitute for event-posting permission.
4. Quit and reopen Talking Quill after changing grants; complete welcome/model/microphone/shortcut setup.

Verify the matching architecture:

```bash
pnpm personal:mac:x64:verify    # Intel
pnpm personal:mac:arm64:verify  # Apple Silicon
```

The command starts the app, requires both Gateway and Keyboard Owner processes, and validates the installed authenticated policy. Then focus TextEdit and perform the Enter/insertion and Escape/cancel capture checks described for Windows. Also revoke and regrant Accessibility/Input Monitoring once and confirm capture fails closed while revoked and returns only after reopen.

### Friend-Mac transfer

Send only the matching `release/Talking-Quill-<version>-mac-<arch>.zip` plus the public `Talking-Quill-Local-Code-Signing.cer`; do not send `signing.json`, a `.p12`, private key, password, or Keychain export. On the friend's Mac, compare a SHA-256 communicated over a separate channel:

```bash
shasum -a 256 Talking-Quill-*.zip
```

This transfer path is fresh-install-only: do not replace an existing app, Login Item, owner process, Keychain enrollment, or pending cleanup. Existing users must use an exact predecessor-bound updater or controlled uninstall first. Open Keychain Access, import the `.cer` into the **login** Keychain, open the certificate, set both **Trust → When using this certificate → Always Trust** (X.509/CMS) and **Code Signing → Always Trust**, and authenticate. Extract the ZIP, move Talking Quill to `/Applications`, then use **Control-click → Open**. When adding privacy entries, select the nested `/Applications/Talking Quill.app/Contents/Library/LoginItems/Talking Quill Keyboard Owner.app` (use Command-Shift-G in the file chooser), not only the outer Electron app. Grant Login Item/Accessibility/Input Monitoring/event-posting permissions as above. The builder should retain the identity and use exact predecessor metadata for future updates. Before the local certificate expires, create and test a planned replacement; rotation changes code identity and requires trusting the new public leaf plus TCC re-enrollment on each Mac.

### macOS repair, rollback, uninstall

- **Repair:** do not copy over a running or enrolled bundle. Use the controlled exact predecessor-bound update path where possible. Otherwise run controlled uninstall, wait for owner/Login Item cleanup, then fresh-install the exact ZIP and regrant stale permissions.
- **Rollback:** `open -a "Talking Quill" --args --rollback-local-owner=/absolute/path/to/Talking-Quill-previous-mac-<arch>.zip`. The ZIP must name the exact active build as predecessor.
- **Emergency capture switch:** `TALKING_QUILL_DISABLE_ACTIVATION_CAPTURE=1 "/Applications/Talking Quill.app/Contents/MacOS/Talking Quill"`.
- **Uninstall:** `open -a "Talking Quill" --args --uninstall-local-owner`. Wait for Login Item/Keychain cleanup and Trash movement. Reinstalling resumes any durable interrupted cleanup.

## Troubleshooting

- **Build says predecessor is incomplete:** unset all `TALKING_QUILL_PREDECESSOR_*` and `TALKING_QUILL_MACOS_PREDECESSOR_*` variables for a fresh build. The personal command does this automatically. For an update, supply the complete exact set instead; never invent hashes.
- **Windows process missing or stale machine state:** rerun the full personal installer. It closes the app and forcibly stops remaining processes within the installation before replacing files. Approve UAC so it can clean protected legacy service state.
- **Windows warning has no Run anyway:** verify local policy permits unsigned apps or build on a developer machine you administer; do not disable SmartScreen globally.
- **macOS identity/config missing:** rerun setup only if the original identity truly cannot be recovered. A new identity requires trusting the new public certificate and regranting TCC on every Mac.
- **macOS owner missing:** approve Login Items, remove stale privacy entries, add the exact current Keyboard Owner, quit/reopen, then rerun verification. If the outer app was moved/deleted, complete controlled cleanup before a fresh install; the script refuses residual Keychain items, owner state, Login Item registration, or gateway/owner processes.
- **Capture does not insert:** check microphone/model readiness, focus a normal editable control, avoid password/Secure Event Input fields, verify all grants, and test Escape cancellation. Logs/process presence alone do not prove capture.
- **Held-key maintenance warning:** release every shortcut modifier or final key, Enter, and Escape. Retry rather than killing the owner.
