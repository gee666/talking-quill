# Security review notes

## Pi process-provider boundary

Talking Quill treats Pi as user-installed executable code. It locates or accepts an executable, resolves the real path, and performs bounded `--version` and `--help` capability checks. It does not authenticate package ownership, versions, dependencies, source bytes, configuration, credentials, models, native modules, or extensions, and it does not sandbox Pi.

Processes are spawned natively with `shell: false`. Windows `.cmd`/`.bat` shims use the minimum fixed `C:\Windows\System32\cmd.exe /d /s /c` bridge Windows requires. The executable path is quoted; remaining values are fixed flags, a strict `provider/model`, or a fixed thinking enum. Prompt text is bounded and sent only on stdin. Supported `--no-tools`, `--no-session`, `--no-context-files`, and `--no-approve` flags are passed. Output is bounded, operations have hard deadlines, and cancellation terminates the process tree.

Pi inherits the normal user environment and profile, including `PI_CODING_AGENT_DIR`. Normal Pi configuration, authentication, models, and extensions load and may update their own state. Pi and extensions have the user's file, network, and process permissions. Users must trust the selected Pi executable and its configuration.

Pi's upstream is conservatively classified as cloud. Talking Quill cannot apply its HTTP adapters' DNS pinning, redirect, or SSRF controls inside Pi. `--list-models` sends no transcript, but Pi or extensions may still access configured services.

## Application boundaries

Renderer IPC remains schema-validated and role-scoped. Provider prompts and responses are excluded from diagnostic logs. The packaged application keeps RunAsNode, Node-options, and inspector fuses disabled. Native helper, Whisper worker, credential-vault, update, and data-removal controls are unchanged by the external Pi adapter.
