# Keyboard-owner compatibility

Windows production packages use one runtime model. Electron supervises `talking-quill-helper.exe`, the non-suppressing gateway. The gateway reconnects to an eligible owner through the stable per-WTS-session local named pipe or starts the adjacent `talking-quill-keyboard-owner.exe` when no eligible endpoint exists.

The protocol authenticates fresh ephemeral P-256 material against independently derived pipe peer PID, process creation marker, token and WTS session, user and logon SIDs, integrity, architecture, retained image identity, and immutable installed-release digests before capture can open. Code running as the same Windows user remains inside the threat boundary and can deny service.

A Windows package contains exactly these two native roles. The retired SCM service, LocalSystem launcher, enrollment store, inherited owner bootstrap, public authority pipe, and maintenance executable are not runtime compatibility targets. The singleton remains `Local\\TalkingQuill.KeyboardOwner.Personal.V1.<session>` so upgrades cannot create a second owner under a renamed mutex.

macOS keeps its LoginItem owner, authenticated local transport, signing, Keychain, TCC, and maintenance bridge behavior.

Protocol compatibility remains fail closed. A version, feature, release policy, image, endpoint binding, session, or owner-association mismatch leaves fresh capture admission closed.
