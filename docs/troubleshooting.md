# Troubleshooting

Start with [installation](install.md), [permissions](permissions.md), and [shortcuts](shortcuts.md). Verify downloaded assets using [release verification](release-verification.md); never bypass a signature, notarization, or checksum failure.

- **Needs Setup:** complete the model download and check that the native helper is present. Use Info to open logs; logs are redacted.
- **Welcome step is incomplete:** setup progress is saved after each successful Continue action. Reopen Welcome from Info. At 960×600 all six tracker markers must remain on one row; use the latest release if controls or the footer are clipped.
- **No microphone:** choose an input in Settings, reconnect it, and review OS microphone permission. Recording teardown intentionally releases the stream.
- **Shortcut not detected on macOS:** grant Accessibility/Input Monitoring to the installed signed app and restart it. Secure Input can temporarily block global hooks.
- **Text was copied but not inserted:** paste manually. Secure fields and elevated Windows windows can reject synthetic input.
- **Model corrupt/offline:** use delete and re-download when online. Downloads resume into a temporary directory and are accepted only after SHA-256 verification.
- **Smart fallback:** Raw text was inserted because the provider timed out, rejected credentials, returned an invalid response, or was unreachable. Run Connection test; secrets and response bodies are not logged. Consult [providers](providers.md) or the [Ollama guide](ollama.md).
- **Pi not found:** run `npm install -g @earendil-works/pi-coding-agent`, then open Settings → Smart processing → **Pi installation path** and choose **Auto-detect**. Auto-detect checks PATH and standard npm/pnpm global locations without requiring a restart.
- **Configured Pi path invalid:** choose **Browse…**, paste an absolute `.cmd`, `.bat`, `.exe`, extensionless executable, symlink, or containing directory, or choose **Auto-detect**. A configured path never silently falls through to another Pi.
- **Pi incompatible:** the command did not advertise print mode, model/thinking selection, or model listing during bounded `--help` and `--version` checks. Update Pi or select another compatible executable; there is no version whitelist.
- **Pi launch failure:** run the selected command's `--version`, `--help`, and `--list-models` directly. Talking Quill runs that installation in place and does not repair, copy, authenticate, or pin its files.
- **No authenticated Pi models:** run `pi`, authenticate the desired provider, then retry discovery. No authentication data is copied into Talking Quill settings.
- **Pi model missing or future list format:** use **Discover models** or enter a strict `provider/model` ID manually. Talking Quill retains the exact saved selection even when it is absent from, or cannot parse, the catalog. **Test connection** sends a minimal fixed prompt so Pi validates that exact model; it may contact or charge the configured provider. Smart failures still fall back to Raw text.
- **On-Screen Awareness unavailable:** select a vision-capable model and grant Screen Recording on macOS. Generic endpoints require an explicit override and image test.
- **Update check unavailable:** the public check requires access to the canonical GitHub repository's latest stable release. Drafts and prereleases are intentionally ignored; an authenticated draft verification in release CI is separate from the public update endpoint.
- **Removal/reset questions:** review [data removal](data-removal.md) before destructive actions.

When reporting a problem, include app version, OS/architecture, artifact SHA-256, redacted error category, and reproduction steps. Never include keys, transcripts, screenshots, private endpoints, usernames, or machine-specific secrets.
