# Keyboard owner wire-to-state mapping

> Keyboard owner contract suite: `talking-quill-keyboard-owner-v1`
> Contract version: **1**
> Contract ID: `talking-quill-keyboard-owner-wire-state-v1`
> Status: **normative target**

This document makes protocol command consumption, native linearization, and response timing explicit. B2/C1 implement this mapping for the fake, non-executable owner library; authenticated transport, production adapters, process lifecycle, and platform endpoints remain runtime-open in `keyboard-owner-conformance-status.md`.

## 1. General rules

- Transport validates/authenticates the envelope before invoking state logic.
- A current capability's command sequence is consumed before semantic validation.
- Native action requests are not facts. Only matching owner-instance/capability-scoped confirmation changes authoritative state.
- Dependent tokenized actions are issued one at a time. The next is returned only after the prior action confirms.
- A response is created only at the response point below. Transport confirms final maintenance response flush before state emits process exit.
- Unknown mutating methods and active-capability sequence faults close/revoke. Typed semantic errors keep the connection unless a row says otherwise.
- Read-only requests are correlated but do not mutate or consume capability command sequence.

## 2. Mapping table

| Wire request | Sequence domain | State transition | Response point |
| --- | --- | --- | --- |
| `lease.acquire` | transport | authenticated observer → new disabled capture capability; reset reconciliation | disabled capability installed; ownership known neutral |
| `lease.renew` | capture | renew liveness | command accepted before monotonic expiry |
| `session.reconcile_off` | capture | issue tokenized native session-off action | matching native off confirmation |
| `session.set_mode` | capture | issue tokenized full mode action; non-off requires enabled keyboard admission | matching native mode confirmation |
| `capture.replace_configuration` | capture | reserve `(captureEpoch,revision)` high-water and retain full snapshot; close if needed; sequentially cancel then apply/fence | matching native application/fence confirmation |
| `capture.set_enabled(true)` | capture | validate full reconciliation/readiness; issue open action | matching open linearization confirmation |
| `capture.set_enabled(false)` | capture | close admission | matching close/quiescence barrier |
| `paste.inject` | capture | begin separate paste admission; issue native admit action | pre-claim refusal, accepted waiting result, or authoritative claim/result according to one-shot phases |
| `lease.release` | capture | close, sequential cancel/drain, revoke | close barrier determines `neutral`/`draining`; terminal events may follow predecessor route |
| `runtime.rollback` | capture priority | latch rollback immediately and merge/supersede pending open/close/config/session/paste work | latch installed and close initiated; never waits to decide whether latch applies |
| `maintenance.acquire` | transport | staged seal/acquire in section 5 | persisted capability installed, state sealed/draining |
| `maintenance.renew` | maintenance | renew liveness | accepted before expiry |
| `maintenance.prepare` | maintenance | staged stop/flush/exit in section 6 | adapter stopped/unregister-ready; response is then flushed before exit |
| read-only method | transport | snapshot only | correlated bounded response |

`configure.activation` is intentionally not a v1 owner method: replacement and enablement are separate commands/sequences. A configuration action carries or references an owner-retained complete immutable snapshot, never only a revision number.

## 3. Separate keyboard and paste admission

Fresh keyboard callback admission and paste admission are independent.

Fresh keyboard admission requires enabled owner build mode, current capture capability, complete session-off/configuration reconciliation, permissions, healthy hook/tap, no rollback/maintenance/degraded/unknown state, and authoritative open confirmation.

Paste admission requires current exact compatible capture capability, paste readiness, target/generation scope, no rollback/maintenance/degraded/unknown state, and no conflicting paste. It does not require a suppression-capable build or open keyboard admission. This preserves owner-mediated paste in M1 feature-free test builds (`safe_disabled` on the frozen wire).

Target W1 transitions are:

```text
BeginPaste(operation, generation scope)
→ AdmitPaste(token, operation)
→ confirm waiting | reject before claim
→ confirm claimed
→ confirm completed | confirm indeterminate
```

Disconnect before claim cancels. Claimed/indeterminate is retained until authoritative completion and is never retried. Keyboard `buildEligible` and `pasteReady` are separate health facts.

## 4. Priority rollback and process matrix

`runtime.rollback` and trusted local rollback latch are the same priority event. They apply while opening, closing, configuration/session/paste actions are pending, and while capture is enabled/disabled. They immediately:

1. latch rollback;
2. revoke enable authority;
3. merge a close behind any pending open;
4. cancel unclaimed paste and unresolved candidates sequentially after closure;
5. retain claimed/owned drain facts.

In `Degraded`, new config/session-enable/paste/keyboard-open commands are forbidden; only closure/drain confirmations, release bookkeeping, and authenticated maintenance repair sealing remain. In native-stopping, response-flushing, or exiting phases, all ordinary capability mutations/renewals are rejected; only exact pending stop/flush completion is accepted.

Externally report orthogonal status:

```text
{
  reportedState,
  processState,
  rollbackLatched,
  nativeStateUnknown,
  maintenanceSealed
}
```

Availability cannot be inferred from `reportedState` alone.

## 5. Staged maintenance acquire

1. Authenticate `maintenance` purpose and validate immutable policy/guard.
2. Reserve/check a new maintenance epoch before persistence.
3. Snapshot the revoked capture route `{connection,capabilityId,captureEpoch}`.
4. Seal capture and close admission; never restore capture after this point.
5. Protocol server enqueues exactly one nonblocking best-effort predecessor `lease.revoked` terminal status on the retained route; this is not a reducer/native token and is never emitted a second time.
6. Complete closure and dependent cancellation sequentially without waiting for status delivery.
7. Emit tokenized `PersistMaintenanceRecord` with exact transaction/operation/source/target.
8. On persistence success, install maintenance capability and create acquire response; that correlated response is authoritative sealing/revocation proof.
9. On persistence failure, remain sealed, closed/degraded, and capture-disabled; return failure if safe.

Epoch/action exhaustion after step 3 remains sealed/degraded. No persisted record can coexist with enabled capture. Best-effort status uses the predecessor capture route/epoch, never the new maintenance epoch; writer failure retires only the status effect and never restores capture.

## 6. Staged maintenance prepare and exit

1. Validate current maintenance capability, sequence, transaction, and operation.
2. Require known-neutral ownership/effects and closed admission.
3. Emit tokenized `StopNativeAdapter`.
4. Confirm hook/tap stopped, owner callback thread joined, and registration/unregister readiness.
5. Create final `maintenance.prepare` success response.
6. Transport writes and flushes that exact correlated response and confirms completion.
7. Emit `ExitOwner`.
8. Maintenance CLI independently observes process exit and singleton release before installer success.

Failure/indeterminate stop never creates success. Endpoint exit cannot race the final response.

## 7. Configuration identity and dependent actions

Configuration identity is `(captureEpoch,revision)`. Each epoch has a positive strictly increasing requested high-water distinct from requested/applied/reconciliation state. Readiness loss or indeterminate application clears applied/reconciliation validity but never lowers the high-water. A new capture epoch may restart at revision 1.

Native application atomically installs the complete owner-retained snapshot and fences pre-held keys. Stale completions from another epoch/revision degrade and close.

After close, dependent effects are issued/confirmed in this order:

1. candidate cancellation/replay acceptance;
2. waiting paste cancellation;
3. configuration application/fence;
4. continue native drain.

Only one dependent token is outstanding. If a step fails indeterminately, later steps were never dispatched and are retired; configuration cannot apply after uncertain cancellation.

## 8. Terminal predecessor events

Maintenance takeover/release stores the exact predecessor route before replacing controller authority. `lease.revoked`, `lease.draining`, `lease.neutral`, and terminal degraded/unavailable are the only events allowed after revocation, exactly scoped and monotonic on the original connection. All predecessor mutations and ordinary semantic events remain invalid.

## 9. C1 implementation boundary and handoffs

C1 is implemented only in the non-executable `talking-quill-keyboard-owner` library. `NativeAdapterExecutor<A,C>` implements the B2 `OwnerExecutor` contract where `A: NativeAdapter` and `C: CapabilityIdSource`. Its ordinary constructor always applies `ActivationCaptureGate::for_process()`; explicit open/test bypass APIs exist only under the build-script-emitted `talking_quill_unoptimized_test_support` cfg, which is absent for every `OPT_LEVEL>0` artifact. There is no `NativeAdapter` implementation for `platform::NativePlatform` and no Windows/macOS endpoint.

The exact semantic input is `AdapterEvent { AdapterEventId, BrokerEvent }`. IDs start at 1, are contiguous, never wrap, and receive exactly one `AdapterEventDisposition`. Duplicate/skipped IDs latch ownership-unknown adapter desynchronization and their event is never dispatched. `CloseFreshAdmission`/`EmergencyCloseFreshAdmission` complete only as `NativeEffectResult::AdmissionClosed { through_event }`; before reducer close confirmation, the coordinator drains and acknowledges every contiguous pre-close event through that inclusive watermark and flushes all locally admitted notifications. A missing/skipped event latches ownership unknown, while the independent native close fact may still confirm admission closed; unknown ownership can never become neutral/exit. `BrokerEvent` is closed over keyboard, audio-device change, bounded raw ownership observation, readiness, paste claim/finish/late resolution, and recoverable native fault. The exact effect output is `NativeEffect`; it preserves every B2 token and immutable configuration/paste/maintenance payload. `ExitOwner` is intentionally not a native effect.

Handoffs are frozen as follows:

- **C2:** retain `OwnerProtocolServer` as the sole wire→state coordinator and preserve `pump_native_adapter()`, `flush_admitted_events()`, `exit_requested()`, and the B2 final-response-flush ordering when replacing the supplied fake connection with an authenticated bounded transport. It must not re-map `BrokerEvent` or `NativeEffect`.
- **C4:** construct `NativeAdapterExecutor<A,C>` in the owner runtime, supply a cryptographically random nonzero `CapabilityIdSource`, serialize protocol/control and adapter pumps on one owner coordinator, and treat `ExitOwner` only as the post-flush outer-loop directive. It must not add an open-gate constructor to optimized builds.
- **D1:** implement `NativeAdapter` inside the owner package for the Windows owner loop. `execute` must return only the matching `NativeEffectResult`; `try_next_event`/`acknowledge_event` must implement contiguous one-shot admission. The active Windows path uses only the adjacent same-user owner and its authenticated stable per-WTS-session named pipe.
- **D2:** after the signed macOS platform gate, implement the same `NativeAdapter` contract for the event-tap owner run loop, including authoritative paste claim/indeterminate resolution and exact ownership observations. No socket/LoginItem/Keychain endpoint is part of C1.
- **E1:** the gateway depends only on `talking-quill-owner-protocol`; it consumes the existing strict owner-v1 responses/events and predecessor terminal route. It must not depend on `talking-quill-keyboard-owner`, `NativeAdapter`, `BrokerEvent`, `NativeEffect`, native platform modules, or capability-ID generation.
