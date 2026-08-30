# Talking Quill Windows diagnostic collector

`Collect-TalkingQuillDiagnostics.ps1` collects a privacy-limited support bundle for the installed Talking Quill release. It derives the observed version and architecture from the validated installed release manifest and reports them as unavailable when that manifest is missing or invalid. Run it as the signed-in user while the restart loop is visible. If Windows denies access to service or event details, the script opens one UAC prompt for a small elevated worker. The rest of the collector stays at the user's normal integrity.

Save the script in Downloads. Open PowerShell and run:

```powershell
Unblock-File "$env:USERPROFILE\Downloads\Collect-TalkingQuillDiagnostics.ps1"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$env:USERPROFILE\Downloads\Collect-TalkingQuillDiagnostics.ps1"
```

Keep Talking Quill open during the 60-second observation window. By default, the ZIP is written to Downloads:

```text
%USERPROFILE%\Downloads\Talking-Quill-diagnostics-YYYYMMDD-HHMMSS.zip
```

To use another local directory, pass it explicitly:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$env:USERPROFILE\Downloads\Collect-TalkingQuillDiagnostics.ps1" -OutputDirectory "C:\Support\TalkingQuill"
```

The ZIP has a `README.txt` and `integrity.txt`. The integrity file contains SHA-256 hashes for every report and the collector script.

The collector does not read or copy recordings, model bytes, settings contents, history, screenshots, voice commands, typed keys, credentials, or tokens. It keeps only Talking Quill processes and relevant events. It hashes model entry names and records file metadata only. Profile paths, the username, secrets in command lines, and account SIDs are redacted.

To run the built-in redaction and JSON-shape check on the affected machine:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$env:USERPROFILE\Downloads\Collect-TalkingQuillDiagnostics.ps1" -SelfTest
```
