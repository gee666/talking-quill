# Native helper development and manual verification

`talking-quill-helper` is a required, narrowly privileged Rust process. Its long-running RPC mode owns only the global activation hook, session-only Esc/Enter capture, Ctrl/Cmd+V dispatch, front-application inspection, and macOS permission probes. It has no clipboard reader, arbitrary key injector, filesystem RPC, shell, command execution, or generic native-call method. A separate one-shot `--remove-owned-tree <path> <device:inode>` mode is invoked only after reset journaling and service quiescence: Windows uses identity-checked non-reparse handles, and macOS uses descriptor-relative no-follow traversal. It fails closed on replacement or mount/device changes.

## Build and protocol

Requirements are pinned by `rust-toolchain.toml`. Build the current native architecture and stage the binary for Electron:

```text
pnpm build:helper
cargo fmt --manifest-path helper/Cargo.toml --all -- --check
cargo clippy --manifest-path helper/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path helper/Cargo.toml --locked
```

`tests/native/helper-harness.mjs` and `app/src/main/helper/HelperClient` use four-byte big-endian framing followed by one UTF-8 JSON-RPC 2.0 object, limited to 16 KiB. Protocol version 2 accepts exactly:

- `initialize`
- `activation.configure`
- `session.set_capture`
- `paste.inject`
- `front_app.get`
- `permissions.get`
- `ping`
- `shutdown`

The Rust contract is documented in `helper/src/protocol/mod.rs`; the matching strict Zod schemas are in `app/src/shared/helper/protocol.ts`.

Run the non-destructive native probe (it tries Z, then a bounded Q/J fallback on a genuine OS hotkey conflict; the selected exact pair is immediately unregistered without generating input, and paste is not invoked):

```text
pnpm test:helper:harness
```

Run the guided physical-input test only on an interactive desktop:

```text
pnpm test:helper:manual
```

`activation.configure` accepts at most ten distinct `{key, shift}` bindings. Shift is an exact part of each binding rather than a mode toggle. Configuration is bounded and validated on both sides of the protocol.

The helper fails open: initialization is required before capture, activation defaults disabled, queue/stdio failures close the callback gate, and shutdown disables capture before acknowledging. `HelperClient` restores only the last successful activation configuration after a supervised restart; it never restores session capture.

On Windows, exact configured chords use `RegisterHotKey`/`MOD_NOREPEAT`, so Windows consumes Alt/Option plus each binding's explicit Shift state and letter without altering unrelated Alt input. The low-level hook balances the physical key-up and owns session Esc/Enter interception. Registration conflicts fail atomically and preserve the prior configuration.

## Windows physical matrix

Use a normal unelevated build first, then repeat the stated edge cases.

1. Open Notepad with an empty document and run the interactive harness in a terminal.
2. Outside session capture, type Enter and Esc. Enter must reach Notepad; Esc must retain normal Notepad behavior.
3. During the activation phase, press and release Alt+Z. No `z` or application command may reach Notepad; exactly one down and one up notification must appear.
4. Hold Alt+Z long enough for keyboard repeat. Repeats must not emit additional activation-down notifications.
5. Repeat with the configured Shift binding and verify `shift: true`.
6. Hold Ctrl or a Windows key while pressing Alt+Z. The chord must pass through and emit no activation event. Test AltGr+Z on a layout with AltGr; it must also pass through.
7. During session capture, press Esc, main Enter, and keypad Enter. The keys must not reach Notepad and paired down/up notifications must appear. Immediately after capture is disabled, they must pass through.
8. Copy distinctive Unicode and multiline text (for example `Zażółć 🦊\n第二行`), select Notepad in the harness countdown, invoke paste, and verify exact text appears once.
9. Confirm `front_app.get` reports `Notepad.exe` and the visible document title.
10. Repeat paste against an elevated Notepad. A blocked dispatch is an expected OS boundary and must not wedge Ctrl or V.
11. Repeat activation after sleep/wake and with a second monitor/DPI setting.

Also test Word, Chrome, VS Code, and Terminal before release; secure/password fields may reject synthetic paste and must never receive raw keystroke fallbacks.

## macOS physical matrix

Run an unsigned development binary only for implementation checks. Release permission evidence must use the final signed/notarized identity because TCC grants are code-identity-specific.

1. In System Settings → Privacy & Security, deny Accessibility and Input Monitoring. Start the helper and verify denied states are reported without prompting and unrelated keys pass through.
2. Grant Input Monitoring and Accessibility, restart the app/helper, and verify the hook reports ready. Revoke each permission and repeat.
3. In TextEdit, repeat the Option+Z, Option+Shift+Z, repeat, extra-Control/Command, and session Esc/Return/keypad Enter checks from the Windows matrix.
4. Copy `Zażółć 🦊\n第二行`, dispatch paste into TextEdit, and verify exact text appears once.
5. Confirm `front_app.get` reports TextEdit and its focused-window title.
6. Leave Secure Keyboard Entry enabled in Terminal and confirm `paste.inject` returns `secure_input` without posting Cmd+V and without a fallback keystroke path.
7. Exercise sleep/wake, tap timeout recovery, multiple displays, and permission revocation while the helper is running.

Repeat insertion in Pages, Chrome, VS Code, and Terminal before release.

## Packaging and signing

The build script maps targets as follows and stages exactly one binary before each package:

| Electron target | Rust target |
| --- | --- |
| Windows x64 | `x86_64-pc-windows-msvc` |
| Windows arm64 | `aarch64-pc-windows-msvc` |
| macOS Intel | `x86_64-apple-darwin` |
| macOS Apple Silicon | `aarch64-apple-darwin` |

Packaged locations are `resources/helper/talking-quill-helper.exe` on Windows and `Contents/Resources/helper/talking-quill-helper` on macOS. `after-pack.cjs` rejects missing, empty, symlinked, or wrong-architecture helpers before signing. Package inspection repeats that check and requires executable mode on macOS. Electron-builder is configured to include `.exe` files in Windows signing and to use the macOS hardened runtime with audio input permission; when signing credentials are configured it signs nested Mach-O code before the app bundle. Every native packaging CI job runs the safe protocol/health/permission/front-app harness before packaging. CI explicitly disables credential discovery and therefore produces unsigned test packages. Authenticode certificate verification, Developer ID verification, and notarization remain credentialed release gates.
