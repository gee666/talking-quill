# Keyboard owner state-machine contract

> Keyboard owner contract suite: `talking-quill-keyboard-owner-v1`
> Contract version: **1**
> Contract ID: `talking-quill-keyboard-owner-state-v1`
> Status: **normative target**

This document is the normative lifecycle and safety target for the out-of-process Talking Quill Keyboard Owner. `.agent-plans/keyboard-owner.md` is design history, not an implementation contract. Changes to capability, ordering, ownership, admission, or neutrality semantics require safety review. A wire-visible change also requires review of the owner protocol major or compatibility epoch.

`helper/keyboard-owner/src/state.rs` is the history-moved pure, non-production reference prototype. It has no transport or suppression authority, is compiled only in the inert, feature-free owner test library, and is not certified as the W1 conformance implementation. Its known gaps are release-blocking in `keyboard-owner-conformance-status.md`. The root helper is now a non-suppressing structural gateway and cannot depend on the owner package.

## 1. Scope and caller obligations

The reducer is pure. It owns no transport, authentication, timers, entropy, process launch, native callback, persistence, or platform maintenance guard. Before invoking it, outer layers must authenticate the peer, validate platform identity and compatibility, allocate cryptographically random identifiers, and durably persist any required maintenance transaction.

A reducer transition returns ordered required actions. The caller MUST:

1. execute those actions in returned order;
2. preserve each action's owner-instance scope;
3. feed only authoritative, matching native confirmations back to the reducer; and
4. treat actions attached to a `TransitionError` as mandatory fail-closed work, not diagnostic advice.

Transport code may represent additional read-only waiters. The reducer represents only the authenticated observer eligible to acquire authority.

## 2. Orthogonal state

The model has four axes. No layer may collapse them into one permissive enum.

### 2.1 Controller

- `NoController` — no represented peer has authority.
- `AuthenticatedObserver` — authenticated and read-only; this state does not perform authentication.
- `CaptureLeaseDisabled` — has capture capability, but fresh native admission is not open.
- `CaptureLeaseEnabled` — has capture capability and a confirmed open admission barrier.
- `MaintenanceExclusive` — has a disjoint maintenance capability; capture/configuration/paste methods are not representable for this capability.

Only one controller is represented at a time. A wrong connection cannot disturb the active controller. A protocol fault from the active connection permanently revokes that capability.

### 2.2 Native admission

- `Closed` — fresh callback admission is authoritatively closed and pre-barrier admitted effects are quiescent.
- `Opening` — an open action was issued but not confirmed.
- `Open` — the native linearization point confirmed fresh admission open.
- `Closing` — closure is requested but not yet authoritatively confirmed.
- `Unknown` — an action completed indeterminately; availability is closed and emergency closure is required.

Admission is a separate axis because a requested native action is not yet a native fact.

### 2.3 Native ownership

Only bounded aggregate facts enter this model; key identity, physical snapshots, replay journals, target evidence, and paste content do not.

| Field | Values / bound |
| --- | --- |
| candidate | `None`, `Active`, `Cancelling` |
| activation drain keys | `0..=26` unique A–Z physical generations |
| session drain keys | `0..=2` unique Escape/Enter physical generations |
| combined physical drain keys | `0..=28` |
| replay cleanup edges | `0..=26` unique balancing letter releases |
| paste | `None`, `Waiting`, `Cancelling`, `Claimed`, `Indeterminate` |
| admitted effects | `0..=8` fixed owner-admission queue budget |

Ownership is neutral only when every field is empty/zero. Counts are safety bounds, not permission to retain event content. The grammar derives 26 activation letters, two session keys, and at most one balancing cleanup up per unique letter. A0 fixes the admitted-effect budget at eight; W3 must implement exactly that bounded admission capacity and fail closed before a ninth item. W1 must source these constants from the shared keyboard core and boundary-test max/max+1 rather than duplicate unexplained literals.

### 2.4 Process and health

- `Starting`
- `Healthy`
- `RollbackLatched`
- `Degraded`
- `Stopping`

`Degraded` is one-way. Recovery means authoritative close and drain followed by owner exit, never re-enable in the same process.

Readiness has three independent booleans: eligible build, eligible permissions, and healthy native hook/tap. All three must be true to open admission. Readiness recovery never auto-enables.

## 3. Exact initial state and bounds

A new owner starts with no controller, neutral ownership, process `Starting`, admission `Closed`, all readiness dimensions false, no seeded startup snapshot, no reconciliation, no maintenance transaction, no pending action, and no native-uncertainty latch.

The following bounds are normative:

- capability ID: exactly 32 bytes and not all zero;
- maintenance transaction ID: exactly 32 bytes and not all zero;
- capability epoch: nonzero `u64`, monotonically increasing in its capture or maintenance domain, never wrapping;
- capability command sequence: nonzero `u64`, first value 1, contiguous, never wrapping;
- native action ID: process-scoped nonzero `u64`, monotonically increasing, never wrapping;
- pending dependent tokenized native actions: at most one; later steps are not issued before confirmation.

An identifier/action allocation overflow is a fail-closed fault. Capability and maintenance identifiers must remain redacted from debug output and diagnostics.

## 4. Startup and disabled-first enablement

Startup physical snapshot seeding and startup completion are distinct confirmations. Startup completion requires a seeded exact physical snapshot, closed admission, and neutral ownership. The native adapter fences pre-held keys; raw snapshot data never enters this aggregate model and pre-held keys cannot match a new configuration.

Capture lease acquisition requires:

- process `Healthy`;
- the acquiring connection is the represented authenticated observer;
- no maintenance seal;
- known neutral ownership;
- admission `Closed`;
- no pending actions.

Readiness is not required for a disabled lease. Each new capture lease starts with session-off and configuration reconciliation reset.

Opening fresh admission requires all of the following:

1. a current disabled capture lease;
2. admission `Closed`;
3. the latest positive, strictly increasing requested configuration revision authoritatively applied;
4. session capture authoritatively reconciled `off`;
5. all readiness dimensions eligible;
6. neutral ownership, no pending effects, and no native uncertainty;
7. no rollback latch or maintenance seal.

`OpenFreshAdmission` does not enable capture. Only its matching native confirmation changes the controller to enabled and admission to `Open`. A readiness loss closes first, resets reconciliation, and cannot auto-enable after recovery.

## 5. Capability command sequencing

Capture and maintenance capabilities have separate command high-watermarks. The only accepted sequence is previous + 1. The first mutation after acquire is 1. `Renew` consumes this same sequence. There is no retransmit or result cache.

A valid capability command consumes its sequence before semantic validation. Therefore a semantically rejected command MUST NOT be retried with the same sequence. While tokenized native work is pending, `Renew` is allowed; ordinary mutations are rejected after their valid sequence is consumed. Priority `runtime.rollback` is the exception: it latches immediately, supersedes/merges with pending open/close/config/session/paste work, and cannot fail with `AdmissionTransitionPending`.

Configuration revision has a separate positive, strictly increasing per-capture-epoch high-water. Readiness loss or application uncertainty may invalidate reconciliation/applied state but never lowers that high-water. The native application identity is `(captureEpoch,revision)` plus the complete owner-retained snapshot and pre-held-key fence.

A wrong connection returns `WrongController` without changing the active authority. From the active connection, a wrong capability ID/epoch, duplicate, skipped, zero, or wrapped sequence is a protocol fault: revoke the capability, close fresh admission, and use normal cancel/drain behavior. Reconnection begins with a new disabled capability and full-state reconciliation; it never retries an uncertain mutation.

## 6. Admission barriers and action ordering

`CloseFreshAdmission` is stronger than “disable the hook.” Its confirmation is the native linearization proof that:

1. no new callback can admit work; and
2. every effect admitted before the barrier is quiescent.

A close confirmation while admitted effects are nonzero is a native-confirmation fault. Opening, closing, unknown admission, pending actions, or native uncertainty forbid neutral/maintenance-ready reporting and clean exit.

A valid open completion already superseded by a close is consumed but never enables capture. Multiple close intents merge behind a single outstanding close action. After authoritative closure, dependent work MUST remain in this order:

1. `CancelCandidate`;
2. `CancelWaitingPaste`;
3. `ApplyConfiguration` with complete snapshot/fence identity;
4. `ContinueNativeDrain`.

Predecessor `lease.revoked` is not a reducer/native dependent action. After sealing, the protocol server may enqueue it exactly once as nonblocking best-effort terminal status on the retained route. Writer acceptance/failure is tracked as an admitted notification effect that must quiesce, but it never delays cancellation or controls sealing. Only the next dependent native token is returned. Its confirmation emits the following step. Failure retires undispatched later work; uncertain cancellation can never be followed by configuration application.

`ApplySessionCaptureOff` and `OpenFreshAdmission` are separately tokenized reconciliation/admission actions. `ContinueNativeDrain` and `StopNativeOwner` are ordered directives without transferable capability tokens.

A native action token is owner-instance-local executor correlation. It is not a wire capability and cannot be accepted by a new owner instance or capability epoch.

## 7. Ownership transition matrices

`Open` and `Closing` form the admission-active authority window because already admitted work may still become owned before the close barrier. `Opening`, `Closed`, and `Unknown` use post-close restrictions.

### 7.1 Candidate

| Previous | Admission-active next | Post-close next |
| --- | --- | --- |
| `None` | `None`, `Active` | `None` |
| `Active` | `None`, `Active`, `Cancelling` | invalid (closure first changes it to `Cancelling`) |
| `Cancelling` | `Cancelling`, `None` | `Cancelling`, `None` |

Replay cleanup edges may decrease at any time. They may increase only while resolving an `Active` or `Cancelling` candidate in the admission-active window, or from a `Cancelling` candidate after closure.

### 7.2 Paste

| Previous | Admission-active next | Post-close next |
| --- | --- | --- |
| `None` | `None`, `Waiting` | `None`, or `Waiting` only through a separately authorized `BeginPaste` transition |
| `Waiting` | `Waiting`, `Claimed`, `Indeterminate`, `None` | invalid (closure first changes it to `Cancelling`) |
| `Cancelling` | `Cancelling`, `None` | `Cancelling`, `None` |
| `Claimed` | `Claimed`, `Indeterminate`, `None` | `Claimed`, `Indeterminate`, `None` |
| `Indeterminate` | `Indeterminate`, `None` | `Indeterminate`, `None` |

After keyboard admission is not active, activation/session drain counts and keyboard-admitted effect counts cannot increase, an active candidate cannot appear, and cancelled waiting paste cannot claim late. Separately authorized paste admission is legal with closed keyboard admission under an exact current capture capability, paste readiness, and no rollback/maintenance/degraded seal; it is the only closed-state `None → Waiting` path. Claimed or indeterminate paste blocks neutrality until authoritative completion; a timeout cannot invent completion.

An impossible native snapshot is retained, not discarded. The owner degrades, closes admission, and continues drain from the conservative observed facts.

## 8. Native action failure contract

| Action | `FailedNotApplied` | `Indeterminate` |
| --- | --- | --- |
| close admission | issue a new close token | degrade, set admission unknown, issue exactly one emergency close |
| open admission | return to closed if still opening | degrade, set admission unknown, issue emergency close |
| session-off/configuration | invalidate reconciliation | invalidate reconciliation |
| candidate/paste cancellation | degrade and latch native state unknown | same |

A stale, replayed, swapped, or kind-mismatched completion is a native-confirmation fault: degrade, latch native-state uncertainty, and request closure if it is not already authoritative.

`AdmissionState::Unknown` and the native-state-unknown latch are distinct. Successful emergency closure may resolve unknown admission. Confirmation mismatch, cancellation uncertainty, or action-allocation failure latches native-state uncertainty. That latch forbids neutral, new capture leases, maintenance-ready, and clean exit.

## 9. Controller loss, release, and rollback

EOF, heartbeat expiry, MAC fault, protocol fault, and gateway connection loss use identical safety transitions; only aggregate counters may distinguish the reason. Loss atomically revokes authority, closes fresh admission, cancels candidate/waiting paste after the barrier, and drains retained ownership. A disconnected or expired lease can never be revived.

`lease.release` returns no disposition before a required close barrier. After closure it returns `Neutral` only when admission is closed, ownership is neutral, pending actions are empty, and native state is known; otherwise it returns `Draining`. `Draining` permits gateway exit, not owner exit.

Runtime rollback closes activation and session-key admission, resets reconciliation, and permanently latches rollback for the process. It drains; it never force-stops the owner or clears through readiness recovery.

## 10. Maintenance

Before maintenance, transport/platform code MUST authenticate a maintenance-eligible peer and hold the platform maintenance guard. The transaction ID and operation (`update`, `uninstall`, or `rollback`) are immutable together.

Maintenance acquisition is staged: reserve epoch; snapshot predecessor capture route; seal capture and close/cancel sequentially; persist the exact two-phase transaction with a tokenized action; only after persistence confirmation install/return the maintenance capability. Persistence/epoch/action failure after sealing never restores capture and enters sealed degraded drain. Capture and maintenance capabilities never coexist.

The predecessor revocation action carries its original connection, capability, and capture epoch; it never inherits the maintenance epoch. Reacquiring after maintenance connection loss requires the same transaction ID/operation and receives a new maintenance epoch.

`Prepare` requires closed, known-neutral state, then uses explicit tokenized adapter-stop/unregister confirmation, final-response creation, transport-flush confirmation, and only then owner exit. Maintenance connection loss never restores capture. `MaintenanceReady` may remain because the persisted transaction remains sealed. Guard loss permits sealed owner exit only with no active maintainer and authoritative known neutrality.

## 11. Derived reported states

Priority is exact:

1. `Starting`;
2. `Stopping`;
3. `DegradedDraining`;
4. maintenance-derived state;
5. controller-derived state.

| Conditions | Reported state |
| --- | --- |
| healthy, no authority, closed, known neutral, no pending actions | `IdleNeutral` |
| disabled lease, closed, known neutral, no pending actions | `LeaseDisabled` |
| enabled lease with confirmed open admission | `LeaseEnabled` |
| disabled lease but not quiescent | `LeaseDraining` |
| no controller with candidate cancelling | `OrphanCancelling` |
| no controller otherwise non-quiescent | `OrphanDraining` |
| maintenance sealed but non-quiescent | `MaintenanceDraining` |
| maintenance sealed, closed, known neutral, no pending actions | `MaintenanceReady` |
| process degraded | `DegradedDraining` |
| native stopping/response flushing/process exiting | `Stopping` |

Opening a lease reports `LeaseDraining`. Paste cancellation alone reports `OrphanDraining`, not `OrphanCancelling`. `LeaseDisabled` does not claim reconciliation eligibility. The external owner status also reports `processState`, `rollbackLatched`, `nativeStateUnknown`, and `maintenanceSealed` orthogonally; rollback must not be hidden behind `IdleNeutral`/`LeaseDisabled`, and availability never derives from the projection alone.

## 12. Release-blocking safety invariants

The following identifiers are stable references for tests, review findings, and exact-artifact evidence.

1. **KBO-INV-001 — Input balance.** Every key-down that Talking Quill admits, hides, replays, or injects has exactly one corresponding foreground-visible key-up while continued native observation/injection for that event remains operational. This does not claim ownership of unrelated device/OS input.
2. **KBO-INV-002 — Owned-up suppression.** The owner suppresses an up only when it owns the corresponding hidden down or has atomically replaced the original edge with a complete tagged replay batch.
3. **KBO-INV-003 — Journal disposition.** A captured journal is either committed to one activation or replayed once in original order; never both and never neither unless the exact held downs remain explicitly in drain ownership.
4. **KBO-INV-004 — Unified cancellation.** Lease loss, controller disconnect, callback-delivery failure, configuration replacement, permission loss, update, and uninstall all use the same reducer cancellation/drain semantics; none gets a release-only shortcut.
5. **KBO-INV-005 — No deadline escape.** No shutdown/update/uninstall deadline authorizes synthetic balancing downs, false ownership retirement, hook/tap removal with held ownership, or a success response.
6. **KBO-INV-006 — Synthetic isolation.** Replayed, paste, dummy, external injected, and test-only events cannot mutate physical state or activate Talking Quill.
7. **KBO-INV-007 — Modifier integrity.** Physical modifiers are never synthesized as user-owned releases. Windows Alt/Win neutralization remains one tagged dummy pair per applicable modifier cycle.
8. **KBO-INV-008 — AltGr isolation.** AltGr cannot accidentally satisfy plain Ctrl+Alt.
9. **KBO-INV-009 — Acceptance fencing.** Configuration revisions and lease epochs cannot reinterpret keys held before their acceptance boundary.
10. **KBO-INV-010 — Predecessor quiescence.** A new controller cannot enable capture until predecessor ownership is neutral and all predecessor notifications/effects are quiescent.
11. **KBO-INV-011 — Exclusive native authority.** Only the owner process installs a suppressing hook/tap or performs replay, dummy, target-specific paste, or global keyboard injection.
12. **KBO-INV-012 — Per-session singleton.** There is at most one enabled owner for a user login session. A mutex/launchd singleton is not sufficient by itself; lease epoch checks also reject stale commands.
13. **KBO-INV-013 — Capability sequencing.** Capture and maintenance use disjoint capabilities. `lease.acquire` and `maintenance.acquire` are authenticated pre-capability control messages governed only by contiguous transport-frame sequence; each returns its random capability ID/epoch. The first later mutation for that capability has command sequence/request ID 1, then exactly increments. Any command sequence `<=` or `!= previous+1` at that capability's command high-watermark is a replay/protocol fault; sequences never wrap, no result cache/retransmit exists, and reconnect never retries an uncertain mutation—it starts disabled and sends full state.
14. **KBO-INV-014 — Authenticate first.** IPC authentication completes before any configuration, target token, permission mutation, or capture capability is accepted.
15. **KBO-INV-015 — Peer rejection.** Cross-session, wrong-image, wrong endpoint/release/process-creation binding, malformed, replayed-handshake, and invalid-MAC connections fail closed without changing capture state. Same-user code remains inside the Windows threat boundary.
16. **KBO-INV-016 — No lease revival.** A disconnected/expired lease can never be revived. Reconnection creates a new lease only after neutral ownership and a full disabled-first reconciliation.
17. **KBO-INV-017 — Instance-scoped effects.** Target tokens and activation generations are scoped to an owner instance. Tokens from an old owner instance or lease epoch are rejected without native dispatch.
18. **KBO-INV-018 — Loss order.** Controller loss first closes fresh callback admission, then cancels/replays unresolved candidates, then drains committed/session ownership. Process exit is allowed only after ownership and irreversible paste work are neutral.
19. **KBO-INV-019 — Truthful reporting.** The owner must not report `neutral`, `maintenance_ready`, clean shutdown, or successful lease release until semantic ownership and admitted effects are quiescent.
20. **KBO-INV-020 — Degrade closed.** If native ownership cannot be proved, the owner remains in a non-enableable degraded/draining state. Availability is sacrificed before keyboard correctness.
21. **KBO-INV-021 — One-way runtime rollback.** Runtime rollback disables both activation and session-key capture, is one-way for the lease/process, and drains existing ownership rather than killing the owner.
22. **KBO-INV-022 — One-shot paste.** Paste remains one-shot: after native claim, timeout/unknown completion is `indeterminate`; Electron retains clipboard fallback and never retries.
23. **KBO-INV-023 — Capability conjunction.** Production capability is false unless the package build mode, owner artifact identity, IPC authentication, owner lease, platform permissions, and native hook/tap health all agree.
24. **KBO-INV-024 — Exact evidence.** An exact-artifact evidence record names the artifact SHA-256, contained helper/owner SHA-256, signing identities, platform/architecture, workflow run/attempt, and test result. Evidence for different bytes is unusable.

## 13. Hard guarantee boundary

No user-mode design can preserve these guarantees after forcible owner death, power loss, OS crash, login-session destruction, `TerminateProcess`, `SIGKILL`, corrupted owner memory, or loss of native observation while ownership is non-neutral. A watchdog or replacement owner MUST NOT claim inherited ownership or successful drain. Recoverable callback/loop faults may be contained only where ownership state remains provably intact; otherwise the failure is explicit and release-blocking.
