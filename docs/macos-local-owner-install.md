# macOS local Keyboard Owner installation

The local Keyboard Owner supports macOS 13 or newer on Intel x64 and Apple Silicon ARM64. It does **not** require an Apple Developer account, Developer ID, notarization, or a paid signing service. It is not represented as notarized software.

## Recommended identity

Use one locally generated self-signed code-signing certificate that is explicitly trusted on the friend's Mac. Keep the same certificate, application path (`/Applications/Talking Quill.app`), bundle identifiers, and installation identity across updates. This gives macOS a stable designated requirement for LoginItem, Keychain, and TCC decisions.

Ad-hoc signing is supported for fresh-install diagnostics only. Updates and rollbacks reject ad-hoc outer applications. Every changed ad-hoc executable has a new CodeDirectory identity, so Accessibility/Input Monitoring permission and the Keychain ACL may have to be enrolled again. A local CMS signing certificate is still required for the sealed keyboard-owner policy.

Never copy private keys, Keychain exports, passwords, or the 32-byte owner IPC secret into the repository or `tmp/`.

## Build variables

The local package commands are `pnpm package:mac:owner:x64` and `pnpm package:mac:owner:arm64`. The native Mac build environment supplies:

- `TALKING_QUILL_MACOS_POLICY_SIGNER_SHA256`: SHA-256 of the certificate compiled into the owner as the detached-policy trust pin;
- `TALKING_QUILL_MACOS_POLICY_CMS_IDENTITY`: exact Keychain identity used by `security cms`;
- `TALKING_QUILL_MACOS_INSTALLATION_ID`: stable random 32-byte installation identity as 64 lowercase hex characters;
- `TALKING_QUILL_MACOS_LOCAL_IDENTITY`: local code-signing identity name for the preferred self-signed mode;
- `TALKING_QUILL_MACOS_LOCAL_CERT_SHA256` and `TALKING_QUILL_MACOS_LOCAL_CERT_SHA1`: exact local certificate hashes;
- optionally `TALKING_QUILL_MACOS_LOCAL_SIGNING_MODE=adhoc`; canonical successor packages require `TALKING_QUILL_PREDECESSOR_VERSION` plus exact predecessor build/gateway/owner hashes for the same architecture.

The local-owner workflow uses a controlled encrypted PKCS#12 secret, imports it into a new runner-temporary Keychain, restricts it to `codesign`/`security`, derives the actual code-signing leaf and CMS signer fingerprints from that Keychain, rejects any mismatch with the declared/compiled pins, and deletes that Keychain in an `always()` step. Hosted runners do not generate an identity or persist one between jobs. Configure the `TALKING_QUILL_MACOS_LOCAL_P12_BASE64`, `TALKING_QUILL_MACOS_LOCAL_P12_PASSWORD`, and `TALKING_QUILL_MACOS_BUILD_KEYCHAIN_PASSWORD` secrets together with the exact public identity/hash values above.

Install the resulting app at `/Applications/Talking Quill.app`. A package built for another path fails closed.

## First launch and Login Items

On first launch, Talking Quill verifies both pinned detached CMS signatures, identical authenticated policy, and both native code identities before provisioning two fixed Keychain items with trusted-application ACLs for only the exact gateway and owner. Registration and unregistration are performed only by the fixed native bridge in the outer application's `Contents/MacOS`; neither the Resources gateway nor nested LoginItem calls `SMAppService` directly. The owner starts and retains a narrow removal-only bridge process while the outer bundle still exists, so a later Drag-to-Trash cleanup request remains in the supported outer-bundle context. macOS may show **Login Items** approval. Approve it in **System Settings → General → Login Items**, then restart Talking Quill. The owner never runs as root and has no Dock icon.

If registration, policy, ACL, code identity, audit session, or socket verification fails, keyboard capture remains unavailable. Do not weaken this to UID-only authentication or put a credential in a file/environment variable.

## TCC permissions

The owner—not Electron and not the old helper—owns the event tap and paste insertion. Grant permissions to **Talking Quill Keyboard Owner**:

1. System Settings → Privacy & Security → Accessibility;
2. System Settings → Privacy & Security → Input Monitoring;
3. approve event posting/automation prompts if macOS presents them.

Quit and reopen Talking Quill after changing permission. Revoking permission closes fresh capture and drains already-owned keys. Existing permission for an older Talking Quill/helper identity does not transfer. With ad-hoc updates, remove the stale entry and grant the new owner identity again. Secure Event Input/password controls deliberately fall back to clipboard-only behavior.

## Updates and removal

Never replace the app while a shortcut, Escape, or Enter key remains held. Release all shortcut/session keys and retry. The packaged updater first extracts and validates the exact ZIP, requires its policy to enroll the installed build as its one-hop predecessor, and asks the owner to seal and persist the transaction. The owner writes a fixed `maintenance_in_progress` Keychain record with an owner-generated random handoff before acknowledging preparation. A detached native finalizer must match every argv field to that exact record, validates both detached CMS signatures against the compiled signer pin, requires identical decoded gateway/owner policies, and retains a verified predecessor backup. Under the exclusive audit-session lock it unregisters the LoginItem, validates replacement, refreshes the exact ACL, and writes `installation_complete`; it then releases the lock before registering/probing the target. Target owner startup remains disabled for `maintenance_in_progress`, accepts only an exact policy/predecessor-bound `installation_complete`, atomically clears the record, and starts disabled before the authenticated probe. Failure restores the predecessor and uses the same exact `rolled_back` startup reconciliation. Rollback candidates use the same path and must enroll the currently installed build; arbitrary downgrades remain rejected. A controlled rollback is started with `open -a "Talking Quill" --args --rollback-local-owner=/absolute/path/to/Talking-Quill.zip`. Keep the matching `release-identity-mac-x64.json` or `release-identity-mac-arm64.json` from the same staged release beside the ZIP. Local update and rollback reject a missing sidecar, a sidecar for another architecture or predecessor, and any ZIP whose SHA-256 differs. The sidecar is evidence, not signing authority. Changing the ZIP and updating the adjacent sidecar cannot authorize the new bytes. The native finalizer derives the installed predecessor outer application's certificate-backed designated requirement, bundle identifier, and Team ID when one exists. It rejects ad-hoc candidates and requires the extracted and installed replacement to retain that exact outer identity. After drain, the finalizer waits for Electron to exit, verifies the complete outer bundle signature and architecture, atomically swaps the validated bundle, retains the predecessor until target authentication succeeds, and restores that predecessor if target completion fails.

A controlled local-owner uninstall can be started with `open -a "Talking Quill" --args --uninstall-local-owner`. It uses the same held-key drain and native finalizer, moves the outer app to Trash, then deletes only uniquely validated fixed Keychain items after ServiceManagement confirms unregistration. Before that irreversible boundary, the finalizer moves its predecessor tombstone beneath the private fixed `KeyboardOwner/cleanup-v1` root and durably records strict transaction metadata plus the pinned-CMS-authenticated source policy. If the finalizer or machine stops after commit, the next packaged reinstall/launch runs the signed native `--macos-owner-resume-cleanup` mode before provisioning or updater startup; it needs no deleted Keychain item and can delete only the exact transaction-derived tombstone and matching no-follow journal. Cleanup is idempotent across repeated crashes. If the application is truly absent, no application code can execute, so cleanup cannot run while it remains absent; the durable journal intentionally remains pending and the next reinstall or launch always resumes it.

Moving the outer app to Trash is detected by a still-running owner at its health boundary. It closes capture, drains, drops endpoint/socket/singleton/maintenance handles, creates a durable removal poison, requests unregistration through the outer native bridge, and remains alive with capture permanently closed while it retries fixed Keychain, socket, lock, and runtime-directory cleanup at bounded intervals until the poison can be durably removed. Relaunch recovery is an additional crash/reboot path, not the primary retry mechanism. If macOS denies self-unregistration or exact cleanup cannot be proved, the owner stays failed closed; use a package-matched repair rather than killing it while a key is held.

## Native acceptance required

Before relying on an artifact, test its exact hashes on the friend's native architecture: LoginItem register/launch/unregister, gateway and owner Keychain access, Electron/unrelated-process denial, TCC grant/revoke/regrant, owner survival after Electron/gateway exit, held-key drain, Option/Command replay, paste, update, app removal, logout, and cleanup. The native package workflow executes the exact extracted signed ZIP, packaged owner/gateway/outer bridge, registration lifecycle, authenticated gateway allow path, and packaged outer-application denial probe; missing identities, tools, approval, or artifacts fail the job. User-mediated Accessibility, Input Monitoring, event-posting prompts, and behavioral held-key/TCC grant-revoke-regrant exercises remain manual: CI must not claim or automate those privacy approvals. CI can prove that stable self-signed certificate identity persists across distinct role bytes and that changed binaries receive distinct CodeDirectory hashes; actual TCC persistence and ad-hoc permission refresh remain manual macOS observations. Cross-compilation and package inspection are not native TCC or ServiceManagement evidence.
