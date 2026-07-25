# Talking Quill

Talking Quill is a free, account-free desktop dictation app for Windows and macOS. Speech is transcribed locally with Whisper; after the model is downloaded, Raw mode works offline and sends no audio or transcript over the network. Optional Smart mode sends text—and, only when enabled, one screenshot—to the provider you configure. That provider may charge for its service.

## Features

- Quick and Extended system-wide dictation with editable Raw/Smart profiles and exact shortcuts
- local Whisper small/large models with resumable, verified downloads
- clipboard-preserving insertion into the focused application
- 38 Smart providers, including Ollama, Pi, and OpenAI API-key authentication
- optional On-Screen Awareness, voice commands, custom vocabulary, and a local dictation history
- encrypted credential storage, scoped data deletion, no accounts or telemetry

## Install and use

Official release metadata is configured once in [`release.config.json`](release.config.json); the canonical release repository is [`gee666/talking-quill`](https://github.com/gee666/talking-quill). The release workflow intentionally fails on the legacy local origin or any other repository.

Talking Quill 1.0.2 runs a compatible user-installed `pi` command in place. It does not copy, authenticate, pin, or modify Pi's package files. Pi uses the normal user profile, configuration, authentication, models, and extensions, including `PI_CODING_AGENT_DIR`. Talking Quill sends transcript text through bounded stdin and disables Pi tools, sessions, context files, and approval when the installed CLI supports those flags. Because Pi and its extensions run with your permissions, install and configure only code you trust.

See [installation](docs/install.md), [permissions](docs/permissions.md), [shortcuts](docs/shortcuts.md), [providers](docs/providers.md), the [Ollama guide](docs/ollama.md), [privacy](docs/privacy.md), [data removal](docs/data-removal.md), and [troubleshooting](docs/troubleshooting.md).

Official release artifacts **must be** signed and, on macOS, notarized; follow [release verification](docs/release-verification.md). Local builds are not published.

## Development

Requirements: Node.js 24.15.0, pnpm 11.13.0, Rust 1.97.1, and platform C/C++ build tools.

```text
pnpm install --frozen-lockfile
pnpm validate
pnpm test:coverage
pnpm test:e2e
pnpm security:release-gate
pnpm package:win
pnpm package:mac
```

`app/` contains Electron/React code, `helper/` the narrowly scoped Rust native helper, `build/` packaging policy, `scripts/` deterministic gates, `tests/` automated tests, and `docs/` user/release documentation. Generated data and test profiles stay under ignored `tmp/`; packages are written to ignored `release/`.

## License

Talking Quill is MIT licensed; see [LICENSE](LICENSE). Third-party and model notices, including the preserved Mintplex Labs attribution for MIT-licensed source consulted during implementation, are generated into `app/assets/THIRD_PARTY_NOTICES.txt`.
