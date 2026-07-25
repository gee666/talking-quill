# Removing Talking Quill data

Talking Quill stores its settings, the dictation history, encrypted provider credentials, downloaded Whisper models, retained screenshots, diagnostic logs, and temporary files in its own application-data folder. It does not store data in an Ollama installation, an Ollama model directory, or Pi's separate configuration, authentication, package, cache, or session directories.

## Reset from the application

1. Open **Settings**.
2. Find **Privacy & data**.
3. Choose **Reset all application data**.
4. Type `RESET TALKING QUILL`, then choose **Reset and restart**.

Talking Quill writes a recovery journal, shuts down recording, providers, the Whisper worker, the native helper, SQLite, and other owned services, then requests a one-time renderer acknowledgement before a bounded restart. The restart completes deletion through the platform native identity-bound boundary before opening the first-run experience. Replacement links and mount/device transitions are never followed. If the process is interrupted after confirmation, the journal causes deletion to resume on the next launch; if the native boundary is unavailable, the journal and quarantined data are retained.

Reset removes only Talking Quill's application-data directory, including its optional Pi path preference. It never uninstalls, upgrades, downgrades, mutates, or resets Ollama or Pi, removes their models/packages, or deletes Pi authentication/configuration.

## macOS removal

For complete removal on macOS:

1. Use **Reset all application data** as described above.
2. Quit Talking Quill after it restarts.
3. Drag **Talking Quill.app** from Applications to the Trash.

If the application cannot be opened, remove `~/Library/Application Support/Talking Quill` manually, then move the application to the Trash. Do not remove Ollama folders; they are separate applications and are not owned by Talking Quill.

## Windows removal

The Windows uninstaller asks whether to also delete local Talking Quill data before installed files are removed. The checkbox is off by default. Leaving it off preserves the application-data folder for a later reinstall. When checked, Talking Quill first clears and confirms its launch-at-login registration, then runs the verified reset while the installed executable and bundled native helper still exist. Any verification or reset failure stops the uninstall rather than claiming deletion.
