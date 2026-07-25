# Smart providers

Talking Quill includes 38 provider adapters. Raw transcription remains local; Smart processing sends the transcript, and an optional screenshot only when On-Screen Awareness is enabled, to the selected provider. Provider charges and privacy terms still apply.

## Pi

Talking Quill runs a compatible user-installed `pi` executable in place. Automatic discovery uses the system `where.exe` on Windows, scans `PATH`/`PATHEXT` case-insensitively, and checks standard npm and pnpm user homes. Settings also accepts an absolute `.cmd`, `.bat`, `.exe`, extensionless executable, symlink, or containing directory.

Talking Quill does not require a package name or version and does not copy, materialize, hash, authenticate, or pin Pi, its dependencies, configuration, credentials, model store, native modules, or extensions. It performs bounded `--version` and `--help` capability checks. Future compatible versions are accepted.

Model discovery runs the same installed command as `pi --list-models`. The bounded parser tolerates spacing and column changes. If a future output format cannot be parsed, users may enter a strict `provider/model` ID manually; Test Connection and runtime then let Pi validate it.

A Smart request is launched without `shell: true` as:

```text
pi -p --model <provider/model> --thinking <enum> [supported safety flags]
```

The only optional safety flags are `--no-tools`, `--no-session`, `--no-context-files`, and `--no-approve`, and only flags advertised by that Pi installation are passed. The prompt never appears in argv: it is sent through bounded stdin. Model IDs use a strict `provider/model` grammar and thinking uses the fixed application enum. Windows `.cmd`/`.bat` shims use only the fixed system command processor `/d /s /c` bridge required by Windows; the executable path is quoted and all arguments are application-generated or grammar-validated.

Pi inherits the normal user environment and profile, including `PI_CODING_AGENT_DIR`, and loads normal Pi configuration, authentication, models, and extensions. Pi may update its own state normally. Pi and extensions execute with the user's permissions and may access files, the network, or processes. Talking Quill does not sandbox or attest them. Hard deadlines, bounded output, cancellation, and process-tree termination still apply.
