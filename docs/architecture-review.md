# Architecture review

## Changes

- Split Electron startup, shutdown, helper communication, recording, Echo sessions, persistence, updates, and IPC into smaller modules. Non-provider main-process files are at most 350 lines.
- Separated Pi provider setup, completion, RPC handling, and transport policy. Kept existing public exports and provider behavior.
- Split renderer components, shared schemas, IPC contracts, and styles without intentionally changing markup, styling, or interaction.
- Removed audio/Echo runtime import cycles and added an import-graph regression test.
- Split native keyboard platform code, protocol code, gateway services, and installer modules. The macOS event-tap entry module is now 284 lines instead of 11,869.
- Tightened native resource ownership and removed redundant logic. Unrelated functional fixes identified during review were reverted.
- Split package policies and personal-install tooling. Updated source-contract tests and exact-path audit exceptions for moved implementations without removing their security assertions.

No commits were created. This is a substantial refactoring pass, not a claim that every architectural issue has been eliminated.

## Validation

Passed on the combined working tree:

- TypeScript typechecks, ESLint, source boundaries, and Knip.
- Rust helper formatting and strict Clippy.
- Release audit, third-party notices, model manifest, audio fixtures, and network boundaries.
- Source formatting for app, scripts, tests, and root configuration files.
- Production application build.
- JavaScript tests excluding the blocked machine-lock wrapper suite: 231 suites passed, 2 skipped; 2,456 tests passed, 15 skipped.

The earlier unrestricted JavaScript run completed with 230 suites passing, one failing, and two skipped. All 112 failures were in `tests/unit/windows-machine-lock-wrapper.test.ts`. An additional import-graph suite was added afterward.

Reviewers also ran targeted native tests, strict installer Clippy, Windows architecture checks, and macOS cross-compilation. Those checks do not replace the blocked full native suites or platform runtime testing.

## Elevated validation follow-up

After closing the running application and preserving the empty legacy evidence directory under `tmp/`, elevated native validation passed. Three stale child-test filters in `windows/stale_tests.rs` were corrected to reference their extracted module.

- Installer: 37 default tests and 53 stale-cleanup feature tests passed, plus both strict Clippy checks.
- Full Rust helper workspace test command passed.
- Windows machine-lock wrapper: 114 tests passed in a separate run.
- Remaining JavaScript suites: 2,459 tests passed, 12 opt-in tests skipped, using one worker.
- Root formatting, lint, typechecks, release checks, and production build passed.

The combined JavaScript run hit Windows DLL initialization failures near the end of the crash-recovery suite. Separate reruns passed without weakening assertions. Logs are `tmp/machine-wrapper-focused.log`, `tmp/elevated-app-tests.log`, `tmp/elevated-helper-tests.log`, and `tmp/installer-investigation-after.log`.

Personal installer packaging requires a clean committed source snapshot. The main checkout remains uncommitted; the installer was built from a separate snapshot under `tmp/refactor-installer-source/` without disabling provenance checks.

The x64 installer is `release/architecture-refactor/Talking-Quill-0.0.73-win-x64-setup.exe`. `pnpm personal:win:check` passed, including recursive package extraction, native-image checks, Electron fuses, and provenance verification. The adjacent `SHA256SUMS.txt` and `build-info.json` record its checksum and source snapshot. The installer has not been run.

## Earlier validation blockers

The machine-lock wrapper rejects an existing legacy evidence directory whose ACL differs from its required protected ACL. Many tests then wait for readiness despite the child having exited, making the unrestricted run take about 31 minutes.

Both `pnpm test:helper` and `pnpm installer:check` stop at the same ACL guard. The current terminal is not elevated. Earlier native checks also reported running release application processes. Those processes, application data, and machine permissions were left untouched.

`pnpm format:check` cannot traverse an inaccessible directory under `tmp/`. The explicit source-format check passes; the root command itself remains blocked.

Evidence is in the local `tmp/` directory:

- `refactor-tests-final.log`: unrestricted test results.
- `refactor-tests-unblocked.log`: final run excluding the blocked suite.
- `refactor-hang-diagnosis.md`: failure diagnosis.
- `refactor-helper-tests.log` and `refactor-installer-check.log`: native validation blockers.
- `refactor-build-final.log`: production build.

## Remaining work

Some large native lifecycle, maintenance, test, and release-tooling files remain. Examples include `helper/src/machine_lock_test_namespace.rs`, `helper/keyboard-owner/src/macos/endpoint.rs`, and `scripts/run-machine-lock-isolated-tests.mjs`. This pass does not meet a universal 350-line limit.

The renderer still has three oversized lifecycle modules: `capture-engine.ts`, `KeyboardShortcutInput.tsx`, and `useProviderConfiguration.ts`. Provider catalogs retain large declarative tables. These need separate state-ownership review rather than arbitrary splitting.

Live keyboard callbacks, macOS runtime behavior, native installer UI, and visual regression testing remain unverified. Installer packaging follows the elevated validation results above.

The empty rejected test-evidence directory was preserved, not deleted. Security guards remain unchanged. Build the x64 personal installer with `pnpm personal:win:build` from a clean source snapshot.
