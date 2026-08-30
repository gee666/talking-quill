# Keyboard-owner conformance status

## Windows

The active Windows runtime is Electron to one supervised non-suppressing gateway to one adjacent same-user keyboard owner.

The gateway first tries the stable local-only per-WTS-session named pipe. It launches the adjacent owner only when no eligible owner endpoint exists. A launched connection must match the retained child PID, creation marker, image identity, immutable installer manifest, and kernel-derived pipe peer facts. Every connection uses fresh ephemeral P-256 material. Windows has no owner ticket or persistent IPC secret.

Unexpected gateway loss closes fresh admission. The owner keeps native observation and the stable `Local\\TalkingQuill.KeyboardOwner.Personal.V1.<session>` singleton while it drains unresolved ownership and permits an authenticated successor to reconnect. Planned quit, update, repair, and uninstall use `owner.exit_when_neutral` and require neutral response, owner process exit, and singleton release before replacement.

Windows packaging accepts exactly `talking-quill-helper.exe` and `talking-quill-keyboard-owner.exe` as native keyboard roles. The retired SCM, LocalSystem runtime, enrollment, service executable, inherited owner bootstrap, and maintenance executable are not conforming paths. Installer cleanup still removes obsolete service and ProgramData state during committed upgrade retirement and uninstall.

Same-user interference is a documented Windows limit.

## macOS

The existing LoginItem owner, signing identity, Keychain, TCC, local authentication, maintenance bridge, and lifecycle rules remain unchanged.
