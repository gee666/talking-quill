# Task 5 native-helper test evidence

This record is implementation evidence only. It does not mark Task 5 complete and does not replace the gate owner's Windows/macOS physical-input acceptance pass.

## 2026-07-20 — Windows x64 development host

Environment: Windows 10.0.26200 x64, Node.js 24.15.0, pnpm 11.13.0, rustc/cargo 1.97.1 from the pinned toolchain.

| Check | Result |
| --- | --- |
| `cargo fmt --manifest-path helper/Cargo.toml --all -- --check` | Passed |
| `cargo clippy --manifest-path helper/Cargo.toml --locked --all-targets -- -D warnings` | Passed with zero warnings |
| `cargo test --manifest-path helper/Cargo.toml --locked` | Passed after review fixes: 81 tests, 0 failed |
| `cargo build --manifest-path helper/Cargo.toml --locked --release` | Passed |
| `pnpm test` | Passed after review fixes: 70 Vitest tests, 0 failed |
| `pnpm lint` | Passed, including source/bundle boundary checks |
| `pnpm typecheck` | Passed root and app strict TypeScript checks |
| `pnpm build` | Passed Electron main/preload/renderer production build |
| `pnpm test:e2e` | Passed: 2 source Electron Playwright tests |
| `pnpm package:win:dir` | Passed; staged and packaged `x86_64-pc-windows-msvc` helper |
| `pnpm package:inspect -- release/win-unpacked` | Passed allowlist, x64 helper/application header match, regular-file check, and Electron fuse checks |
| Packaged Playwright smoke | Passed: 1 test; app/helper handshake surfaced Available, renderer had no helper API, persistence/restart passed |
| `pnpm test:helper:harness` | Passed before and after review fixes; protocol 1/helper 1.0.0, hook `ready`, ping healthy, Windows permissions `not_applicable`, clean shutdown |
| Safe `front_app.get` probe | Reported the actual foreground process `explorer.exe` and its visible File Explorer title |

The safe harness confirmed that this host already owns Alt+Z, received the helper's native-unavailable conflict response, then successfully registered and immediately unregistered Alt+Q. It exited with activation disabled, generated no input, and did not invoke paste, so it did not alter the agent host's keyboard or clipboard contents.

## Physical and cross-platform evidence still required at the gate

This coding environment has no human-controlled desktop input, so physical Alt/Option gesture swallowing, hardware repeat, session Esc/Enter swallowing, and visual paste confirmation in Notepad could not be truthfully executed. The guided harness and exact Windows/macOS matrix are in `native-helper.md`; no synthetic-input result is substituted because the helper intentionally ignores injected events.

A native macOS host was unavailable. The macOS backend and both target configurations are CI-wired, but runtime CGEventTap, Accessibility/Input Monitoring/event-post TCC states, TextEdit insertion, Developer ID signing identity, and notarization must be verified on macOS. Credentialed Authenticode/Developer ID signatures were also unavailable; the local Windows package was an unsigned development package. Electron-builder did discover and process the helper as a signable `.exe`, and package inspection verified its architecture and placement.
