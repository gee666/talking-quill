# Install Talking Quill

Official downloads are published only at the canonical [Talking Quill releases page](https://github.com/gee666/talking-quill/releases). Select the artifact whose operating system and CPU match the device, then follow [release verification](release-verification.md) before opening it.

## Windows

Download `Talking-Quill-<version>-win-x64.exe` for most Intel/AMD PCs or `...-win-arm64.exe` for Windows on Arm. Verify `SHA256SUMS.txt` and the trusted, timestamped Authenticode publisher, then run the NSIS installer as the current user. Choose an install directory if needed. Windows may ask for microphone access on first recording.

The per-user NSIS uninstaller offers an optional application-data removal page. Selecting it removes Talking Quill settings, credentials, history, screenshots, models, logs, and the optional Pi path preference; it never removes or mutates Ollama, Pi, or their packages/models/authentication. See [data removal](data-removal.md) for scope and manual alternatives.

## macOS

Download the DMG matching Apple Silicon (`arm64`) or Intel (`x64`). Verify `SHA256SUMS.txt`, the Developer ID identity, hardened runtime, and stapled notarization ticket. Open the DMG, drag Talking Quill to Applications, then launch it from Applications so permission grants bind to the installed signed identity. Follow [permissions](permissions.md) for microphone, Accessibility/Input Monitoring, and optional Screen Recording access.

## First launch

The six-step Welcome flow remains compact at the supported 960×600 minimum and covers: privacy/free-use summary, microphone test, local model download, shortcut test, optional Smart provider, and Ready guidance. Completed steps show a check, the current step has a distinct focus marker, and setup can be resumed or reopened from Info.

The Whisper model is deliberately excluded from installers. Setup displays its size, verifies free disk space, and downloads pinned checksummed files. Raw mode is offline after this download.

Do not bypass an invalid signature, notarization, checksum, or package warning. Continue with [shortcuts](shortcuts.md), [providers](providers.md), the [Ollama guide](ollama.md), [privacy](privacy.md), and [troubleshooting](troubleshooting.md).
