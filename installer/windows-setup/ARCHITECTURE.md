# Native Windows setup

This crate is an independent workspace. Keep installer policy here, not in a configuration shared with the application or helper.

## Execution and ownership

- `windows.rs` routes entry points and assembles the Windows-private implementation. `controller` owns user interaction, image relocation, and elevated launch. Internal worker and service roles do not display UI.
- `channel` owns the authenticated controller/worker protocol. Its child modules separate worker connection and transcript construction. `peer_identity` checks token lineage. `pipe_io` owns overlapped operation completion.
- `worker`, `installation`, and `recovery` coordinate installation and durable recovery. `recovery/uninstall` owns machine retirement. `state` defines existing JSON records without changing their schemas or phase strings.
- `machine_lock` owns the lifecycle lock. Its children separate legacy mutexes, registry publication, directory initialization, and atomic markers. `machine_lock_namespace` selects production or supervised test names and ACLs.
- `registration` separates path resolution, maintenance images, finalizer publication, and registry registration. Path resolution can publish a maintenance generation when elevated; it is not a pure lookup.
- `terminal_service` separates SCM security/configuration, registration verification, cleanup, and retirement. `terminal_service_host` owns SCM callbacks. `terminal_finalizer` separates tombstones, relaunch ownership, and residue removal.
- `namespace_inventory`, `registry_security`, `stale_objects`, `stale_audit`, `stale_diagnostic`, and `stale_cleanup` admit only the exact legacy fixture. Diagnostic logging has its own module and never calls cleanup as a dry run.
- `package` separates byte transport from manifest/path validation and tests. `owned_tree` separates platform implementations and identity-bound deletion tests.

Child modules use Windows-private reexports to preserve existing callers. This is a compatibility boundary, not a new public API. More explicit imports can replace parent wildcard imports incrementally.

## Safety and durability rules

Do not shorten retained-handle lifetimes during extraction. In particular:

- Cancelled overlapped pipe operations must be drained before their buffers, event, or `OVERLAPPED` storage leave scope.
- File and directory identity checks must retain the handles used for deletion. Reparse-point rejection is not interchangeable with canonical-path comparison.
- Keep registry flushes, file flushes, durable renames, audit events, and crash-injection points in order. Stale cleanup retires registry publication last.
- SCM handles use `ServiceHandle`, not file-handle ownership. Service configuration must stay alive through start, and service handles must close before the manager.
- Security descriptors returned by conversion APIs need `LocalFree`, including error paths. This refactor releases the expected service descriptor before propagating conversion failures.
- Production and test namespace feature gates are authority checks. Never make the fixed fixture SID, hash, generation, or ACL environment-configurable.

`terminal_service/security` owns the restart policy constants used by both configuration and verification. `terminal_policy` owns terminal names and ACLs. Existing timeout values and wire constants are unchanged.

## Deliberate size exceptions

Most production modules are below 350 lines. The two stale cleanup/diagnostic coordinators remain larger because they retain objects across staged admission and irreversible retirement. Splitting them needs explicit lifetime design, not arbitrary line cuts.

The large existing `windows/tests.rs` and `windows/stale_tests.rs` remain together. Several tests launch themselves by exact Rust test name and depend on the supervised namespace wrapper. Test discovery names must remain stable.

## Follow-up review items

These pre-existing issues need dedicated security/acceptance work rather than a mechanical refactor:

- Hardened service verification does not request `READ_CONTROL`, which DACL querying requires. Adding that right changes observable behavior and is deferred.
- Maintenance temporary-file recognition omits generation-qualified names produced by the copy path. Keep a correction separate from module extraction.
- The worker delegates a same-access process handle before token-lineage verification completes. Narrowing that handle and changing authentication order needs cross-process UAC tests.
- Pathname hashing alone does not prove the identity of the image mapped into a process. Image sharing policy and launch-time binding need review together.
- The existing-service branch configures and starts a terminal service before subsequent registration verification. Verify existing identity before mutation/start in a separately tested change.
- Registry code still contains manually owned handles and repeated bounded string readers. Convert those incrementally while preserving missing-key behavior and explicit flush points.

## Validation

Use `cargo fmt`, production and feature-specific `cargo check`, and `cargo clippy --all-features --all-targets -- -D warnings` with this crate's manifest. Place build output under the repository's `tmp/` directory.

Run the full Windows test suite from an elevated PowerShell at the repository root, after existing Talking Quill processes have exited normally:

```powershell
$env:TEMP = Join-Path (Get-Location) 'tmp'
$env:TMP = $env:TEMP
node scripts/run-machine-lock-isolated-tests.mjs -- cargo test --manifest-path installer/windows-setup/Cargo.toml --all-features --target-dir tmp/installer-architecture-check -- --test-threads=1
```

Do not bypass the wrapper's process or namespace admission checks, remove its cleanup records manually, or set a fabricated namespace environment. The wrapper owns namespace creation and cleanup. Existing protected cleanup records may prevent it from starting when relevant processes are alive. The ACL tests require elevation to set administrator-owned security descriptors.

Pure package and protocol tests can run separately without a machine namespace. The TypeScript source-contract tests read extracted module trees and bound order assertions to their owning functions instead of relying on concatenation order. Full service lifecycle and UI acceptance still require the installed-acceptance environment.
