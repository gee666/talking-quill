# Keyboard owner lifecycle contract

> Keyboard owner contract suite: `talking-quill-keyboard-owner-v1`
> Contract version: **1**
> Contract ID: `talking-quill-keyboard-owner-lifecycle-v1`
> Status: **normative target**

This document freezes target owner startup, controller loss, quit, update, rollback, uninstall, and failure behavior. Read it with `keyboard-owner-state-machine.md`, `keyboard-owner-protocol-v1.md`, `keyboard-owner-wire-reducer-mapping.md`, and `keyboard-owner-compatibility.md`. Maintenance coordination applies to the macOS owner. Windows uses installer-side wait-and-replace after the local owner reaches neutrality.

## Current Windows personal runtime

Electron supervises one windowless gateway at the signed-in user's integrity. The gateway first connects to the stable local endpoint for its WTS session. If no eligible owner exists, it starts the adjacent owner and verifies the connected server against the launched PID, process creation marker, retained executable, and immutable installed-release manifest. The owner publishes `\\.\pipe\TalkingQuill.KeyboardOwner.Personal.V2.<session>`. A named `Local\\TalkingQuill.KeyboardOwner.Personal.V1.<session>` mutex still elects one owner across gateway processes. The old mutex name is preserved for upgrade exclusion and is not a credential.

The same interactive user is inside the threat boundary. Same-user code can inspect or interfere with processes using rights Windows grants that user, and it can deny service by taking the singleton name. The design does not claim the retired service's same-user process isolation. It prevents accidental duplicate owners, ambient handle inheritance, pathname replacement during launch, durable credential reuse, cross-session election, and unauthenticated owner protocol access.

The gateway opens the owner image without write or delete sharing, hashes that retained file, and keeps it open through launch and authentication. Immediately after creation it checks the kernel-reported process image path, file identity, SHA-256, PID, and creation marker. For both a launched owner and an existing owner, Windows supplies pipe peer PID, process token, WTS session, user and logon SIDs, integrity, architecture, canonical image identity, and image hash. Handshake identity claims are compared with those facts and never act as their source.

On unexpected gateway loss, the owner closes admission and keeps native observation plus the session mutex while ownership is unresolved. A successor gateway reconnects to that same owner with bounded exponential backoff and performs disabled-first reconciliation. Planned quit, update, repair, and uninstall send `owner.exit_when_neutral`; replacement or removal proceeds only after a neutral response, process exit, and singleton release. Readiness stays unavailable until authentication and disabled-first reconciliation complete.

## 1. Process boundary and singleton

The Talking Quill helper remains a supervised, non-suppressing gateway. The Keyboard Owner is the only process that owns native suppression or injection. It survives abnormal Electron or gateway loss while it drains or awaits an authenticated successor. A planned application quit retires it only through neutral exit.

There is at most one enabled owner per interactive user login session. Platform singleton election prevents duplicate processes, while authenticated capture capability, non-wrapping lease epoch, exact build/install identity, and disabled-first reconciliation prevent stale authority. A spoofed or unauthenticated endpoint is never killed or bypassed; capture remains unavailable.

The Windows owner and gateway run at the interactive user's normal integrity. Electron starts the gateway, and the gateway starts the adjacent owner only when the stable session endpoint has no eligible owner. Windows installs no SCM service, LocalSystem role, enrollment, maintenance executable, or ProgramData runtime. The local-only named pipe has a SYSTEM and current-logon-SID ACL, rejects remote clients, enforces first-instance creation, and authenticates every connection with ephemeral P-256 material bound to kernel peer and release facts. This does not isolate the processes from hostile code running as the same user. macOS keeps its reviewed per-user LoginItem strategy.

## 2. Startup

Startup order is normative:

1. On Windows, connect to the stable per-WTS-session endpoint before considering launch. If no eligible endpoint exists, open and hash the adjacent owner while denying write and delete sharing, launch it, and retain the process and image identities through authentication. On macOS, launch follows the LoginItem contract;
2. verify immutable owner build mode, artifact/install identity, production/test endpoint separation, singleton, runtime latch, and peer-independent native prerequisites;
3. establish the authenticated owner channel. Windows verifies the local named-pipe peer from kernel process, token, session, and retained-image facts before trusting protocol data;
4. install or initialize the pass-through native observation loop;
5. seed an exact physical snapshot and fence pre-held keys so they cannot match a new configuration;
6. confirm startup snapshot seeding to the pure state reducer;
7. report startup complete only from closed admission and neutral ownership;
8. authenticate with ephemeral P-256 ECDH bound to the stable endpoint, exact process and image facts, WTS session, release digest, package-layout digest, and purpose. Windows stores no persistent IPC secret or durable reconnect credential;
9. grant only an initially disabled lease;
10. reconcile health, session capture `off`, and a complete configuration before any enable request.

No stale settings, target token, generation, lease, or queued heartbeat from a prior owner instance is accepted.

## 3. Lease liveness

Connection EOF is immediate controller loss. Only an accepted, current, capability-sequenced `lease.renew` updates capture liveness when processed before expiry; read-only or other authenticated frames do not. `maintenance.renew` is the corresponding maintenance-only rule. The gateway renews once per second. Production lease expiry is five seconds measured with monotonic time.

Transport and capability sequences must both be current. Queued stale renewals cannot extend an expired lease. There is no forced takeover while a valid lease exists. After loss, authenticated peers may observe draining, but no new capture lease is granted until predecessor ownership and admitted effects are known neutral.

Lease timing is configurable only in tests. Diagnostics may report aggregate expiry counts, never endpoint, identifier, timestamp, or peer identity.

## 4. Controller-loss order

EOF, heartbeat expiry, MAC fault, protocol fault, helper crash, and Electron crash all use this order:

1. atomically revoke the capability and close fresh callback admission;
2. wait for the closure barrier proving pre-barrier admitted work quiescent;
3. cancel/replay an unresolved candidate once under existing reducer semantics;
4. cancel paste that is still waiting and unclaimed;
5. retain and drain committed activation ups, session-key ups, accepted replay cleanup, and irreversible paste work;
6. report neutral only after every ownership/effect substate is authoritatively empty.

Loss reason may affect only coarse aggregate counters. It never selects different keyboard semantics.

### Transaction boundary behavior

- Before the first hidden down, current and future physical input passes.
- A replayable candidate is cancelled and replayed exactly once; accepted cleanup remains owned until complete.
- If activation delivery was not accepted into the bounded local owner queue, delivery failure cancels/replays according to the existing reducer.
- After local activation commit, semantic delivery may be lost, but input is not replayed; exact trigger/prefix ups remain owned.
- A hidden/delivered session Escape or Enter retains ownership of its exact physical up. No event is emitted without a current lease.
- Waiting paste is cancelled before claim.
- Claimed paste finishes or becomes authoritative `indeterminate`; it is never retried.

## 5. Normal application quit

Application shutdown remains producer-first. Electron stops capture producers, then asks the gateway for planned owner retirement. The gateway sends `owner.exit_when_neutral` before it cancels transport. That command atomically closes admission and returns `neutral` only after ownership and effects are empty. Electron treats `draining`, timeout, transport loss, or unconfirmed process exit as incomplete planned shutdown. Update, repair, uninstall, and ordinary quit do not overwrite or remove owner files until the owner process exits and the stable singleton is released.

If the gateway actor is blocked, shutdown cancels the current command and transport under one absolute budget. Cancellation never means neutral. The owner continues its fail-closed native drain if the gateway cannot confirm planned retirement.

## 6. Neutral idle exit

Unexpected gateway loss does not start a fixed ownership retirement timer. A planned `owner.exit_when_neutral` exits only after authoritative native closure, semantic neutrality, empty admitted effects, and no maintenance transaction requiring the owner.

Orphan/degraded/maintenance draining has no ownership timeout. Hook/tap removal and process exit are forbidden while ownership is non-neutral or unknown.

## 7. macOS maintenance acquisition

On macOS, capture and maintenance are disjoint capabilities. Maintenance acquisition is staged:

1. authenticate a maintenance-purpose signed peer;
2. prove/hold the platform maintenance guard for the intended replacement interval;
3. validate the immutable source-to-target release manifest or uninstall operation;
4. reserve the maintenance epoch and snapshot the predecessor capture route;
5. atomically seal/revoke capture, close admission, and complete dependent cancellation;
6. issue and confirm the tokenized durable two-phase maintenance-record write;
7. only after persistence, bind/return the random maintenance capability.

Any failure after sealing leaves capture sealed and enters degraded drain; it never restores predecessor authority. Exhaustion is checked before persistence where possible, but cannot leave capture enabled after a persisted or sealed maintenance intent.

The durable record contains only format version, phase, operation, source build, intended target build/hash (or uninstall), and random transaction ID. It contains no input state, target evidence, clipboard content/hash, or IPC secret.

Maintenance prepare waits without fake bounded success for closure and ownership/effects neutrality, then issues a tokenized native-adapter stop. Only authoritative hook/tap stop and unregister readiness creates the final response. Transport must write/flush that response on the same authenticated connection and confirm the flush before state emits owner exit. Installer/update success additionally requires independently observing process exit and stable singleton release.

A user-facing deadline may postpone update/uninstall and ask the user to release keys. It cannot force success, synthesize balancing input, retire ownership, remove the hook/tap, overwrite a running owner, or kill it.

## 8. macOS two-phase maintenance and crash recovery

The durable phases are:

1. `maintenance_in_progress` — persisted before maintenance authority is acknowledged;
2. `installation_complete` — written only by the exact newly installed signed target after replacement proofs pass.

The record is scoped to exact source build, target build/hash, operation, and transaction. A malformed, undecryptable, wrong-owner/mode, symlinked, or mismatched record leaves capture disabled and requires the matching signed repair path.

Maintenance connection loss never restores the revoked capture lease. While the platform maintenance guard remains, a newly authenticated maintenance-only client may reacquire the same transaction/operation with a new epoch. If the guard disappears, a known-neutral sealed owner exits and releases its singleton; the next owner starts disabled until the signed repair/update CLI completes or rolls back that exact transaction.

Crash before replacement leaves the old signed repair path able to resume only its immutable transaction. Crash after replacement leaves the exact target signed CLI able to verify and complete that transaction. No downloaded compatibility policy and no “same publisher” shortcut is accepted.

## 9. macOS authenticated update

The update sequence is normative:

1. stop new application sessions and drain application work;
2. the gateway opens a separate authenticated maintenance-purpose connection;
3. acquire staged maintenance with immutable source/target build IDs and target packaged-owner hash;
4. treat the correlated successful `maintenance.acquire` response as authoritative proof that capture is sealed/revoked; predecessor `lease.revoked` is best-effort status and delivery failure does not undo sealing; wait for owner `MaintenanceReady` through the maintenance connection;
5. send `maintenance.prepare` with the exact capability/transaction/operation;
6. owner confirms native adapter stop/unregister readiness, creates the response, and waits for transport flush;
7. gateway receives success, owner exits, and maintenance code independently observes process/singleton exit;
8. replace versioned payload/registration while holding the platform guard;
9. verify exact installed source/owner signatures, hashes, architecture, build, and installation identity;
10. mark only the matching transaction `installation_complete`;
11. start the target owner disabled, verify/clear the matching record, and perform full reconciliation before enablement.

Installer/update code never overwrites or deletes a running owner. Old and new owners contend on one stable per-session singleton. Protocol-incompatible recovery remains capture-disabled; it never starts the old in-process suppressing path or a second owner.

The first owner-enabled production release establishes a v10 maintenance-compatible recovery floor. Direct downgrade to a v8 installer is unsupported because v8 cannot drain/remove a future owner. A v8 install is allowed only after the v10 maintenance CLI proves neutral exit and removes owner runtime state.

## 10. Runtime rollback and maintenance rollback

Runtime rollback is a stricter one-way latch with no target build. It closes activation and session-key admission, drains, and cannot be cleared by update completion or readiness recovery. Only an explicit user-authorized recovery from the currently installed exact signed build, while neutral and outside maintenance, may clear it under a separately reviewed flow.

Maintenance rollback is an immutable source-to-target transaction and follows the same two-phase replacement rules as update. Neither path selects the old in-process suppressing helper. A feature-free v10 test/recovery role may authenticate for maintenance but cannot open fresh keyboard admission; it is not a canonical package artifact.

## 11. Windows install, repair, update, and uninstall

The elevated NSIS installer asks the installed Electron process to close, then waits for the gateway and owner to exit. It never terminates an owner whose neutrality is unknown. If the owner does not drain within the bound, replacement stops before moving machine files.

Install and repair move the fixed Program Files predecessor and obsolete ProgramData state to protected recovery locations. The replacement is copied before commit. Commit removes obsolete service registration and ProgramData state while recovery remains available. Recovery deletion is the final step. Any earlier failure runs rollback and restores the predecessor. Uninstall uses the same bounded runtime wait and preserves `%APPDATA%\\Talking Quill` unless the user selects and confirms personal-data removal.

In-app update elevation targets the trusted installed gateway binary, not the downloaded installer. After UAC, that bootstrap reopens the installer with write and delete sharing denied, verifies the expected SHA-256, creates it suspended, verifies the launched process image path, file identity, and hash, then resumes it while retaining the locked file handle. The package still contains only the gateway and owner native executables.

## 12. macOS update and uninstall

R5-M now owns the per-audit-session runtime directory, a `0600` Unix socket, and a separate `0600` regular-file singleton held with nonblocking exclusive `flock`. Before binding, it validates the running owner against the exact enrolled identifier, CDHash, signing identity, and designated requirement, while retaining the absolute componentwise no-symlink vnode/path and executable SHA as stable-snapshot corroboration. The absolute home/runtime path is opened component by component from `/`, rejecting `.`/`..`, symlinks, wrong owners/types, and group/other-writable home, `Library`, or `Application Support` ancestors with retained `openat(O_NOFOLLOW)` directory descriptors; lock/socket operations remain relative to those descriptors with `fstatat`/`unlinkat` no-follow policy. Before bind, R5-M proves a CSPRNG temporary name absent. For every temporary, published, and private broker-listener pathname it also performs an active nonce probe: Darwin `socket(SOCK_NONBLOCK|SOCK_CLOEXEC)` plus `connect`, deadline/cancellation-aware `poll`, and `SO_ERROR` replace blocking `UnixStream::connect`; the retained listener drains and retains queued candidates so a saturated backlog cannot deadlock or discard the racing probe. Each candidate retains fragmented nonce bytes across `recv(MSG_DONTWAIT)` calls and is classified only after all 32 bytes arrive or EOF/error/bounded idle expiry makes it terminal; partial response sends use the same deadline/cancellation-aware write loop. No-follow inode checks bracket the connection, the retained listener must accept that exact nonce and return the domain-separated response, and any mismatch is rejected/quarantined. Broker probing shares the absolute handshake deadline and shutdown cancellation. The published inode is rechecked on every endpoint poll; concurrent replacement closes acceptance and permanently poisons cleanup. Immediately after successful bind it arms a two-stage cleanup guard before `fstatat`: once identity is known cleanup is inode-checked; if identity acquisition fails, cleanup only atomically quarantines the active name and never unlinks a possible raced replacement. That rare unknown quarantine is retained for later explicit maintenance rather than guessed safe to delete. It validates the listener's bound Unix address independently with `getsockname`; Darwin listener-FD identity is never compared with the filesystem socket vnode. Publication uses exclusive rename and requires the published pathname to retain the captured device/inode. Endpoint shutdown closes accept and atomically quarantines the socket pathname, verifies the created inode, and unlinks only that inode while `ProductionRuntime` still holds the singleton; a replacement socket is never unlinked. Every armed publication/probe cleanup guard records whether identity-checked removal actually succeeded; guard `Drop` can no longer discard cleanup failure. That shared setup outcome is consumed by endpoint shutdown and `ProductionRuntime` setup-error handling. Cleanup uncertainty is sticky across construction failure, explicit shutdown, and `Drop`: the singleton/maintenance FDs move to process-lifetime poison storage and are never released for a replacement owner. Authentication is limited to four workers and a strict rolling maximum of sixteen starts in any one-second interval, each with one absolute three-second total deadline and a 256 MiB executable cap. Potentially unbounded Security.framework/Keychain work runs in a separately spawned exact owner broker process. Darwin `posix_spawn` first snapshots every source with `F_DUPFD_CLOEXEC` onto distinct descriptors above stdio and the fixed child range, then uses explicit `dup2` file actions to fixed child descriptors, `/dev/null` standard streams, a two-entry sanitized environment, and `POSIX_SPAWN_CLOEXEC_DEFAULT`; mapping order therefore remains correct even when stdio is closed or an original source occupies 100/101/102. IPC is created as Darwin `socketpair(SOCK_CLOEXEC)`—there is no post-creation `fcntl` window or globally inheritable-FD window. Before bind, startup fully validates the owner dynamic/static `SecCode`, exact CDHash, requirement, canonical path, retained vnode/hash, and local signing identity, and retains that exact executable FD for the endpoint lifetime. Each broker is spawned through `/dev/fd/<safe-duplicate>` from that retained vnode, so later pathname replacement cannot select different bytes. Before sending a request or trusting output, the parent accepts a connection on a private per-request runtime-directory socket and performs only deadline-checked kernel `getpeereid`/`LOCAL_PEERPID`/`LOCAL_PEERTOKEN` checks against the returned `posix_spawn` PID, console UID, and audit session—no Security.framework call occurs in the parent handshake or shutdown path. The already-validated sealed-resource bytes are carried inside the nonce/purpose-bound private request frame rather than reopened by pathname. The killable child parses and verifies that policy, validates its own dynamic/static signing and designated requirement without treating a `/dev/fd` pathname as policy authority, and performs gateway `SecCode` and Keychain work. Parent broker I/O uses nonblocking `poll` under the handshake deadline and an independent shutdown-cancellation atomic. Structured child observation clears PID ownership immediately when `waitpid` reaps the child or `ECHILD` proves it is no longer owned, so cleanup never signals an already-reaped PID. Timeout/cancellation sends `SIGKILL` only after a final non-reaping observation and transfers the PID to a dedicated reaper which retains it until `waitpid` succeeds; the runtime never joins that reaper, leaks a zombie, or waits indefinitely for framework completion. Runtime configuration is the fixed outer-app `Contents/Resources/keyboard-owner-r5m.json` resource, read through a retained no-follow vnode and authenticated by the pinned detached CMS signer. It intentionally does not live inside the nested owner bundle: adding a policy that contains the owner executable hash to that bundle's resource seal would mutate the owner's CodeDirectory and create a hash/signature cycle. Its detached CMS signer certificate SHA-256 is compiled into the owner with `TALKING_QUILL_MACOS_POLICY_SIGNER_SHA256`, not supplied by the resource. The strict resource schema pins release and installation digests, both role paths/hashes/local signing modes, and both release-policy blobs/signatures. Raw executable SHA-256 is signed corroborating artifact metadata only when that retained path snapshot is stable; macOS execution authority is the audit-token dynamic `SecCode`, designated requirement, and exact CDHash. R8-M must enroll the CDHash and signed artifact relation, inject the immutable signer pin at owner compilation, seal this exact resource, provision and natively prove the optional-access-group or per-item designated-requirement Keychain ACL acceptance/denial set, and implement registration/update/removal; R5-M deliberately performs no `SMAppService` or package/signing work.

R5-M atomically enters maintenance exclusion by taking and retaining a nonblocking shared `flock` on `maintenance.lock` before attempting the owner singleton's exclusive lock. The shared lock remains held through endpoint closure and authentication-worker cancellation/join. Once the endpoint is quiescent, shutdown releases the shared maintenance lock before releasing the exclusive owner-election lock. This gives an already-waiting R8-M exclusive maintenance coordinator an opportunity to obtain maintenance exclusion while replacement election is still blocked. R8-M maintenance must take the exclusive `maintenance.lock`, which therefore cannot overlap an elected or authenticating owner. R5-M does not claim a complete authenticated maintenance handoff, waiter fairness, transaction ownership, or completion. Although protocol-v1 can represent one-hop predecessor maintenance, R5-M has only current-code enrollment and behaviorally accepts maintenance only when client and owner policies are the exact current policy; capture additionally requires exact current `EnabledCandidate`, while observe permits either exact current feature-free test policy or enabled policy. R8-M must define and test predecessor enrollment, coordinator authentication, protocol sealing/drain, exclusive-lock acquisition, LoginItem unregistration, replacement/removal, and target completion.

macOS provides the accepted peer's audit session through `LOCAL_PEERTOKEN`. R5-M validates the owner's audit session and current console UID at startup, then polls the current process audit token, `CGSessionCopyCurrentDictionary` audit/on-console state, and `SCDynamicStoreCopyConsoleUser` every 250 ms; any mismatch is `SessionEnded`. `SIGHUP` is only a shutdown hint and is not session-end authority. R8-M must still prove LoginItem registration, launch provenance, and lifecycle behavior; R5-M makes no `SMAppService` authority claim.

App-controlled uninstall drains to neutral, unregisters `SMAppService`, removes owner socket/Keychain runtime state, then applies the user's app-data choice. Drag-to-Trash cannot invoke an app uninstaller. The signed platform spike must prove that a still-running owner can detect the absent outer signed bundle, close/drain, self-unregister its LoginItem, remove its audit-session socket and shared Keychain item, and exit. Failure blocks implementation; stale registration is not an accepted fallback.

## 13. Paste after controller loss

Paste remains one-shot.

- `Waiting` is cancellable and returns/falls back to clipboard-only if a response path remains.
- Once natively `Claimed`, the operation is irreversible. It completes exactly once or becomes `Indeterminate` when authoritative completion cannot be established.
- `Indeterminate` blocks new capture and maintenance-ready until authoritative completion or the documented hard-failure boundary.
- Electron retains clipboard fallback and never retries after claim.
- No durable clipboard text, clipboard hash, keystroke journal, or replay journal is introduced for reconnect.

The default for A1 and subsequent work is no claimed-paste replay/report recovery across controller reconnect.

## 14. Native faults and owner survival

IPC parser/writer failure revokes the lease but does not terminate the owner. Gateway EOF closes admission and revokes authority. A same-user owner continues drain without deadline, and a replacement begins only after authoritative neutral exit. Callback/owner-loop panics may be contained only at reviewed boundaries where reducer, journal, and ownership remain provably intact after recovery. A contained recoverable fault enters degraded drain and exits after neutrality; it never re-enables.

Unrecoverable hook/tap loss closes availability. If ownership was non-neutral and observation is lost, the guarantee boundary is explicit and release-blocking. A replacement process cannot claim inherited ownership. An abnormal-exit marker may record only bounded build/boot metadata and is not ownership recovery evidence.

## 15. Hard guarantee boundary

No user-mode design guarantees balance after forcible owner death by an equally privileged actor, `TerminateProcess`, `SIGKILL`, OS crash, power loss, login-session destruction, corrupted owner memory, or unrecoverable native observation/injection loss. These are limits, not states to conceal in metrics.

A watchdog may restart only after neutral failure policy permits it; it MUST NOT report successful drain, inherit a lease/journal, or enable based on predecessor claims. No lifecycle deadline ever permits synthetic balancing downs, false ownership retirement, hook/tap removal with held ownership, force-kill, or a success response.
