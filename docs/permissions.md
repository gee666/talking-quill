# Permissions

Talking Quill requests privileges only in response to a feature that needs them.

- **Microphone:** required to record dictation and run the input-level test. Audio remains local.
- **Accessibility/Input Monitoring (macOS):** required for the activation shortcut and paste injection. Grant it to the installed, signed Talking Quill app, then restart the app.
- **Screen Recording (macOS):** required only for On-Screen Awareness. A screenshot is captured at submission and sent only to the selected Smart provider; retention is off by default.

Use Info → Permissions to inspect status and open OS settings. If a grant appears stale, remove the old Talking Quill entry, reopen the signed app from Applications, and grant again. Windows elevated applications and secure fields may reject synthetic paste; Talking Quill leaves the result on the clipboard and reports that fallback.
