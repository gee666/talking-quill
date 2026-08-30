# Keyboard shortcut event semantics

## Windows automation

On Windows, untagged `SendInput` and UI Automation keyboard records are input-equivalent to physical input: they participate in ordered shortcut matching, suppression, cancellation, and exact replay. `LLKHF_INJECTED` and `LLKHF_LOWER_IL_INJECTED` do not by themselves exclude a record. Talking Quill tags only its own bounded replay, paste, and menu-neutralization records with private `dwExtraInfo` classes; those self-generated records bypass matching to prevent recursion. These tags grant no owner/protocol capability and external automation still crosses the ordinary configured shortcut policy.

Status: **current normative contract for protocol v10 and the out-of-process keyboard owner**. The keywords **MUST**, **MUST NOT**, **SHOULD**, and **MAY** are requirements for the transactional shortcut engine and its platform adapters.

Windows uses tagged scan-code replay and dummy-key menu neutralization. macOS uses separately tagged Core Graphics replay, exact side-specific modifier tracking, and Accessibility target identity. Both use the same bounded transactional matcher, activation-generation protocol, target-validated paste path, and runtime rollback switch. Canonical Windows and macOS packages contain the enabled out-of-process owner and exactly one suppression-capable native role. Feature-free structural test binaries remain inert and pass every physical keyboard event; package inspection rejects those test markers from canonical artifacts.

## Production fail-safe and owner lifetime

The keyboard owner's process-lifetime gate covers **all** native suppression: activation letters, Escape, main Enter, and numpad Enter. Protocol v10 reports `keyboardCapture.activationAvailable` and `keyboardCapture.sessionKeyCaptureAvailable`; both MUST agree with build mode, runtime rollback, authenticated owner readiness, and native health. A contradictory handshake is malformed. A closed gate returns activation `enabled:false` and session mode `off` before native dispatch. The owner adapter applies the same gate again before native work.

The non-suppressing gateway may stop while the detached owner still holds an exact suppressed down. The owner closes fresh admission, cancels or replays unresolved candidates, and remains alive until committed/session ownership drains through observed physical ups. Neither adapter synthesizes a balancing down. Gateway shutdown may report the owner as `draining`; it reports `neutral` only after authoritative native ownership and admitted effects are quiescent. Forcible owner death while an edge is hidden remains the documented user-mode hard limit.

## B1 shortcut grammar

The first transactional release preserves the v27 model:

- A shortcut has an exact Ctrl/Control, Alt/Option, Shift, and Meta/Windows/Command modifier mask.
- At least one modifier is required.
- A shortcut has 1 through 26 unique physical A–Z keys.
- Non-modifier keys are ordered and remain physically held while later keys are pressed. `Alt+X+P` means Alt down, X down, then P down before X is released.
- The final ordered key is the trigger.
- Left/right modifier sides are tracked independently even though the v27 wire model stores a combined mask.
- A binding belongs to exactly one profile. Once admitted by configuration validation, built-in and custom profiles use the same exact matching semantics.

Digits, punctuation, function, navigation, and numpad keys require a later versioned key model. OS-reserved input and input unavailable to an ordinary process cannot be promised by this grammar.

## Shared prefixes

Protocol v10 and the current profile schema admit arbitrary shared-prefix families while retaining the v27 key grammar. Every family uses these completion rules rather than a profile-specific matcher:

- A longer binding resolves immediately when its final fresh physical key-down is accepted.
- A shorter exact binding that has longer descendants remains pending.
- The pending shorter binding resolves when its trigger is released without a longer binding having completed.
- A repeat is never a fresh sequence step and cannot resolve a binding.
- There is no timeout in the held-key grammar.
- Wrong order, an extra key, a combined modifier-mask change, a binding revision, gate closure, or another impossible continuation cancels the unresolved candidate.

Pressing or releasing a second side of an already-held modifier does not, by itself, change the v27 combined mask or cancel a candidate. Every side-specific edge still passes and remains balanced. AltGr classification is distinct and can fence a candidate even when its combined bits resemble Ctrl+Alt.

For example, `Alt+X` resolves on X-up when a longer `Alt+X+P` binding is configured, while `Alt+X+P` resolves on P-down.

## Normalized input

The pure reducer receives a normalized event containing at least:

- physical key identity and platform scan/key code;
- `down`, `repeat`, or `up` phase;
- an exact **post-event** physical snapshot containing all held A–Z keys, every modifier side, the combined wire mask, and AltGr classification;
- monotonic time;
- binding revision and immutable binding snapshot;
- activation-gate state; and
- injection class: physical, helper replay, helper paste, helper dummy/menu neutralization, test traffic, or external injection.

A fresh `down` is valid only when that physical key was not held; `repeat` and `up` are valid only when it was held. The reducer compares every edge with the required post-event snapshot. A phase violation or snapshot disagreement closes fresh admission, fences the exact recovered state, and uses cancellation/reconciliation rather than guessing from stale edges.

At native-owner startup, the adapter MUST seed the reducer from an exact snapshot. Every seeded key and modifier is foreground-visible but revision-fenced until release. After a secure-desktop transition, callback gap, or native recovery, the adapter submits an owner-linearized reconciliation snapshot. Candidate input is replayed before reconciliation; accepted replay downs for keys absent from the recovered snapshot receive helper-owned cleanup ups. Committed input is never replayed and retains drain ownership only for keys still physically held.

Replay, paste, and menu-dummy events MUST pass without changing physical state, entering the matcher, or activating Talking Quill. Externally injected modifier and non-modifier events MUST pass without supplying ownership or sequence steps. Test traffic MAY exercise the physical path only in an explicit native test build; its source and marker MUST be compiled out of release helpers. A release helper treats any otherwise unknown injected marker as external input. Windows release builds reject the test feature at compile time, and helper staging plus package inspection scan the produced executable for the forbidden marker.

## Ownership and visibility

The implementation tracks these separately:

1. **Physical state** — hardware keys currently held.
2. **Foreground logical state** — events already passed or replayed to the foreground application.
3. **Talking Quill ownership** — captured events awaiting commit or replay.

A foreground-visible down MUST have exactly one foreground-visible up. Talking Quill MUST NOT suppress a physical up unless it owns the corresponding down or atomically replaces that event with a complete tagged replay. Physical modifier ups are not selectively swallowed.

Candidate non-modifier events are journaled in original order. A journal is committed to exactly one activation or replayed exactly once; it is never both committed and replayed. Configuration validation MUST reject a shortcut whose bounded down/up representation cannot fit the journal.

Physical key repeat is not bounded by shortcut length, so repeat capacity has an explicit callback-time safety rule: before an append would exceed the fixed journal, the reducer cancels the candidate and requests replay of the complete existing journal. The overflowing original repeat is not journaled; it passes only after that replay is fully accepted, and every still-held candidate key is fenced until release. A rejected or partial replay enters the same terminal/degraded cleanup and ownership-drain path as any other replay failure. The journal never allocates, wraps, overwrites, or truncates an accepted record.

## Transaction boundaries

### Candidate start

Modifier downs normally pass through. The first non-modifier down that can begin a binding is captured and starts a bounded transaction. Its repeats and owned up are captured while resolution is pending.

### Successful activation

Before committing an Alt or Windows-key shortcut on Windows, the adapter emits at most one tagged `VK 0xFF` down/up pair for the relevant modifier cycle. Failure to neutralize is transaction failure.

Activation delivery is part of the commit:

- If initial activation-down delivery succeeds, captured shortcut events are committed and discarded, matching owned ups remain captured, and physical modifier ups pass normally.
- If initial activation-down delivery fails, the journal is replayed and the callback gate closes according to terminal policy. No captured ownership remains.
- If activation-up delivery fails after a successful down commit, committed input MUST NOT be replayed. The owned physical up remains swallowed, a synthetic protocol up MAY be attempted only through an already-reserved bounded path, and fresh activation admission closes terminally. The adapter enters an ownership-drain state: every still-held key whose down was committed remains captured through its physical up. Only after those owned keys drain may the native owner uninstall.

Ownership drain is a narrow bypass of the closed callback gate. It performs only fixed-time physical-state bookkeeping and capture of exact outstanding owned ups; unrelated input passes immediately and cannot match or activate. After a proven native callback gap, the macOS owner reconciles transactional and Escape/Enter ownership against HID-system physical key state, retries retained cleanup, and keeps hidden-up tombstones until a tagged down/up pipeline barrier proves all older source edges have drained. A delayed old up or genuine nonrepeat fresh physical down for the same key retires that tombstone; queued autorepeat downs do not. Native shutdown never reports success while ownership is unknown. Gateway waits are bounded, but their expiry does not terminate or falsely neutralize a draining owner. A fully drained owner permits clean shutdown. Remaining ownership produces an explicit draining or terminal result: no synthetic down is posted and no ownership is falsely retired.

### Failed or cancelled candidate

The adapter replays the journal as one tagged ordered batch while any foreground-visible modifiers are still logically down. Any physical terminating keyboard edge is appended to that same batch and its original is suppressed, so it cannot overtake replay while the native callback is in flight. A mouse-down focus boundary likewise cancels the candidate and suppresses/copies the exact mouse scalar into deferred ownership; HID replay must complete exact tap observation before the tagged mouse pair is reposted. Before enabling the macOS tap, the owner creates a fixed native CGEvent pool with disjoint maximum-journal, recovery-barrier, paste-neutrality-barrier, and paste-pair ranges and caches the Mach timebase. Callback effects only mutate retained events; they perform no CGEvent/CF creation or Rust allocation. Immediately before every replay, cleanup, barrier, or paste event post, the owner samples `mach_absolute_time`, converts its ticks with the startup-cached `mach_timebase_info` ratio to `CGEventTimestamp` nanoseconds, clamps against the preceding post timestamp, and applies it with `CGEventSetTimestamp`. Replay effects never equate pipeline submission with completion: the reducer continuation remains installed until every exact tagged HID event is observed at the tap. Every replay/cleanup batch, recovery barrier, paste-neutrality barrier, and paste pair receives a never-reused operation generation encoded into its marker; observations require the exact current generation, process nonce lineage, own source PID, shape, flags, and ordered state. Delayed events from an older operation are terminal stale input and cannot satisfy a newer expectation. Any partial pool-creation failure releases its created prefix and fails hook startup, and successful pools are released on the owner only after strict drain teardown.

A partial native replay is terminal/degraded: only helper-owned injected downs may be cleaned up. The reducer retains the exact unaccepted suffix of every cleanup request and exposes a bounded retry plan; a cleanup attempt is not successful while either replay cleanup or partial menu-neutralization cleanup remains. If replay was not fully accepted, an unjournaled overflowing repeat/up owned by the candidate remains captured rather than being passed after cleanup. A physical modifier is never synthetically released merely because replay failed.

Effect outcomes distinguish native pipeline submission from synchronous accepted counts. Windows uses the required/default menu-neutralization policy and its actual `SendInput` count; acceptance of only the dummy down creates a retained dummy-up cleanup obligation. Windows paste makes exactly one initial semantic Ctrl+V `SendInput` call, records the exact accepted-prefix cleanup obligation, and atomically publishes failure or irreversible commitment before issuing any cleanup `SendInput` or performing blocking work. Acceptance through V-down commits immediately; later cleanup failure can degrade/terminalize native admission but cannot revoke or replace that result. macOS selects `NotRequired`, requests no neutralization effect, and never fabricates dummy records or accepted counts. For replay cleanup, only submitted helper-injected downs receive releases, in reverse down order. Cleanup failure never authorizes synthetic release of a physical key and never erases the retained helper-injected obligation.

Nonterminal candidate cancellation and configuration replacement use this replay path. Shutdown and terminal admission closure (including helper disconnect, secure-desktop failure, timeout, or gate failure) instead discard the unexposed journal, retain exact drain-only ownership, and wait only for authoritative physical ups; they never post candidate downs or report resource teardown as semantic drain.

## Development and rollback gate

All native keyboard suppression has one process-lifetime gate before native configuration:

- Feature-free structural test builds are inert and pass every physical event; they are not package candidates.
- Explicit `transactional-shortcuts-dev` and `windows-native-test-input` feature builds open the gate only for native acceptance; release/package checks reject them.
- Setting `TALKING_QUILL_DISABLE_ACTIVATION_CAPTURE=1` before launch closes the same all-suppression gate, including Escape/Enter. The historical environment name is retained for compatibility.
- Either build disable or runtime rollback wins; no protocol request can reopen a closed gate.
- A closed gate converts activation to `enabled:false` and session capture to `mode:"off"` before native dispatch, returns those effective values, and increments separate blocked-request counters.
- Paste remains a separate target-validated operation and does not authorize keyboard capture.
- Changing the runtime switch requires a helper restart.

The safe rollback is “no native keyboard suppression,” never partial trigger, release-only, Escape-only, or Alt-up suppression.

## Paste completion

Activation and paste are coordinated but separate transactions. Caller cancellation is guaranteed only before the `paste.inject` frame is dispatched. After dispatch, the bounded native operation wins or fails authoritatively; the host keeps the request and clipboard ownership pending, and a `paste.committed` notification wins over a concurrent abort. A captured activation MUST retain evidence strong enough for the platform adapter to revalidate the original insertion target; inability to obtain required candidate evidence makes that first edge uncapturable, while later token-registration failure yields targetless clipboard fallback. Each platform uses a fixed 32-entry generation registry and one-shot token consumption; eviction fails safely. Windows coherently double-samples PID, foreground HWND/thread, focused-control/caret HWND, input-desktop identity, and a monotonic target-change epoch before suppressing the first candidate edge, and revalidates that evidence before every continuation, neutralization, activation-start delivery, and replay. Activation-up delivery deliberately uses the frozen accepted context without target validation so protocol down/up remains balanced. Paste tokens retain candidate-start caret HWND/rectangle evidence under a CSPRNG process epoch. Focus, foreground, and caret WinEvents advance a monotonic candidate epoch, so an observed A-to-B-to-A transition remains invalid even when HWNDs return to their original values. Controls without distinct native focus and caret evidence—including many Chromium/Electron/custom virtual controls—are currently uncapturable and remain ordinary clipboard/manual-paste targets; Windows does not claim UI Automation support. Synchronous UIA provider calls are forbidden in the low-level hook, and a safe asynchronous UIA cache/epoch worker has not yet been implemented. macOS tokens include an independent Security-framework CSPRNG process epoch and reference worker-owned exact Accessibility application/window/control objects plus scalar CFRange caret location/length and PID evidence through a scalar publication handle; secure-random failure disables target tokens rather than falling back to predictable material. All macOS AX messaging runs on a dedicated cache worker, never in the CGEvent tap callback or its owner run loop. The worker double-samples the complete tuple, requires the focused control’s AXWindow to equal the sampled focused window, and publishes only identical coherent samples carrying the exact notification and event-boundary epochs validated around capture; stale evidence is never relabelled with a later epoch. NSWorkspace application-activation notifications, per-application AX focused-window/focused-control observers, and exact tuple changes advance the notification epoch. Mouse, keyboard, replay, paste, and callback-gap edges separately advance the event-boundary epoch so a publication racing a callback cannot repopulate stale evidence. At an activation callback, the owner nonblockingly reserves only an observer-confirmed pre-boundary publication index and epochs, establishes the boundary, and defers only the transactional delivery continuation while the worker flushes notifications and captures fresh post-boundary evidence. Strong targeting requires unchanged notification epoch and exact pre/post tuple equality; otherwise the activation is delivered targetless. The owner performs no AX call or wait, and any later physical event first resolves an outstanding continuation targetless before entering the restored transaction engine. Before paste, the native owner waits for every physical Ctrl, Shift, Alt, and Meta side to become neutral within a bounded deadline and revalidates that token at the injection boundary. Windows installs one absolute injection deadline at command submission and never resets it on owner admission. The command remains Waiting and caller-cancellable throughout modifier/target validation, clipboard Open/GetClipboardData, bounded canonical UTF-16-to-UTF-8 hashing, and stable sequence sampling. After revalidating target/modifiers/gate, a minimal final closure rechecks the absolute deadline and sequence, then CASes Waiting-to-Injecting immediately adjacent to one fully helper-owned, paste-tagged semantic Ctrl+V SendInput batch so non-QWERTY layouts retain paste behavior. Caller timeout winning Waiting-to-Cancelled prevents every future injection; Injecting winning makes that exact SendInput result authoritative and the caller cannot report ordinary clipboard-only failure while it may complete. The caller's post-CAS wait is also bounded; expiry closes keyboard admission and returns the explicit `indeterminate` failure, which forbids retry while avoiding a false `paste.committed` notification. Electron propagates that authority as an `indeterminate` session completion and tells the user to check the target rather than claiming clipboard-only success. Missing, locked, inaccessible, invalid, oversized, changed, or mismatched text remains clipboard-only and no custom/file format is read or rewritten. It cleans only accepted helper-owned downs after a partial count and retains any rejected cleanup suffix in the local native owner. Acceptance through V-down is conservatively reported as committed because the target may already have pasted; every partial initial batch degrades fresh keyboard admission. macOS sends explicit validation requests to the AX worker and accepts only coherent responses tied to unchanged application/window/control, typed CFRange, broad notification, event-boundary, and selected-range notification epochs. The worker registers focused-window/focused-control notifications on the application and selected-text/selected-text-range notifications on the exact retained control. A same-PID focused-control identity change retires and reinstalls that observer, invalidates broad and range epochs, and requires a fresh confirmed capture; an old-control callback can only invalidate, never authorize, a replacement control. After the authenticated neutral-modifier barrier, its callback rechecks the retained target handle, selected-range epoch, event boundary, modifier epoch, deadline, and Secure Event Input, then submits preallocated target-specific work to the AX worker. Immediately before claim, the worker rechecks the retained control, exact CFRange, workspace/target/range epochs, real Accessibility/Input Monitoring/event-post grants, real Secure Event Input, deadline, and enough budget for the configured 0.2-second AX timeout plus result margin. It also reads one immutable NSPasteboard NSString under equal before/after `changeCount`, enforces the application’s 1,000,000-byte transcript/insertion UTF-8 limit using checked CFString length/maximum-size bounds, hashes through fixed-size chunks before claim, retains that bounded string, and compares the 32-byte digest with the request’s canonical lowercase SHA-256. A mismatch or oversize value is clipboard-only while still preclaim. After claim, the worker sets the exact retained typed `AXSelectedTextRange` value back on the retained control, then verifies exact control/range/epochs, permissions, Secure Input, and the retained scalar `changeCount` immediately before setting `AXSelectedText` from the retained preclaim string. No second full conversion, hash, or large allocation occurs after claim. `kAXErrorCannotComplete`, timeout, and unknown outcomes from either set call are terminal ambiguity; only documented definitive rejections may produce exact failure. It exposes `Pending`, `Claimed`, exact success/failure, and terminal ambiguity to the owner. After claim, cancellation cannot overwrite authority; the owner retains the request until confirmed completion, and missing bounded completion proof closes admission terminally without publishing an ordinary failure. Confirmed success alone authorizes `paste.committed`. No global Command+V or paste-tagged CGEvent is posted. Electron writes the intended fallback text once and performs no later snapshot/restore, preventing torn partial-format restoration or overwrite of a newer custom/file clipboard change.

### macOS target, paste, recovery, and shutdown hardening

Strong macOS target evidence additionally requires `AXSelectedTextRange` to report exactly `kAXValueCFRangeType`; the AX worker extracts its `CFRange` location and length into scalar evidence, requires both scalars to be nonnegative, requires `location.checked_add(length)` not to overflow `isize`, and revalidates exact scalar equality. Wrong/unsupported types, extraction failure, negative scalars, or overflowing endpoints remain targetless. Every system/application/window/control AX element receives a finite `AXUIElementSetMessagingTimeout` before attribute or PID messaging, including `front_app.get`, so an unresponsive target cannot hold an RPC indefinitely. The AX worker owns every retained AX/CF object. Callback reservation is a nondestructive publication index plus exact notification/boundary epochs, performed only when the pure engine says the current letter edge can activate; worker validation compares the indexed pre-boundary tuple with a fresh post-boundary tuple and returns only a scalar handle. Cache invalidation never releases CF evidence in callback, and worker shutdown clears retained evidence on that worker.

One absolute two-second native paste deadline covers owner wake, asynchronous AX validation, modifier neutrality, the ordered neutral barrier, target-specific AX insertion, and result acknowledgement. The final barrier callback performs scalar/atomic checks only and nonblockingly queues the already-prepared AX work. Selected-text/range notifications and exact retained control/range checks reject same-control caret moves and control switches before insertion.


Secure Event Input and permission state are monitored while candidate, session, committed, retained cleanup, tombstone, barrier, pending validation/paste, or submitted-but-unobserved ownership exists. Once a replay/cleanup has been accepted into HID and has an exact observation cursor with an outcome-less in-flight continuation, that operation is immutable authority: timeout, Secure Input/permission loss, tap disable, gap reconciliation, shutdown, CloseAdmission, Reconcile, and RetryCleanup may atomically close admission/terminalize but cannot clear the cursor, replace/resume the continuation, begin another reducer turn, or resubmit. Exact observations alone advance it. At the absolute terminal deadline the tap is disabled before that authority is retired, so its already-submitted unobserved suffix—including the reserved full-capacity current repeat/terminator—passes to the foreground once; reposting that suffix would duplicate it and is forbidden. Escape/Enter protocol balancing is separate from native suppression ownership: a synthetic UI up may be delivered on suspension or terminal failure, but native ownership remains until the matching physical up or HID-hidden-up plus ordered barrier proof. Any transition closes admission terminally, reconciles authoritative HID state, and keeps/re-enables only strict drain behavior. Every tap-disable path performs the same reconciliation and replacement-barrier protocol. One conservative `pending_native_work` authority governs every shutdown admission, drain poll, callback stop decision, semantic-drain decision, and resource teardown. It includes transactional candidate/journal and committed state, native Escape/Enter ownership, retained cleanup, tombstones, all operation barriers/observations, pending activation/paste continuations, queued owner work, and submitted but unobserved events. Shutdown uses one absolute 1.5-second native drain deadline on each native owner. The protocol closes the callback gate, runs the bounded platform stop, waits for already-admitted callback delivery leases, and enqueues one final shutdown response only after clean quiescence. Exactly one separately reserved final-response slot is unavailable to the 256-entry ordinary queue. The production writer drains every accepted ordinary frame in FIFO order, writes that response last, and closes, so a saturated ordinary queue cannot drop or overtake shutdown completion. At deadline either platform terminalizes and exits without posting a balancing down, clearing native ownership as if drained, or emitting success. Candidate target change commits no replay and moves hidden downs to capture-only drain; no shutdown path injects into either the original or replacement target. Unresolved ownership is a terminal/incomplete semantic drain and remains a release blocker for every suppression-enabled test build. Forced termination cannot safely solve a previously hidden-down/future-up pair without an out-of-process owner; canonical packages therefore retain ownership in that owner, while feature-free native tests kill an inert helper between a visible down and up to prove the foreground pair remains balanced.

Electron serializes native RPC dispatch: at most one request frame is dispatched until its response arrives or its bounded timeout is ignored during drain; notifications continue to parse independently. A correlated response does not release dispatched authority until its method-specific result schema and paste commitment ordering are fully validated. Malformed correlated output releases only that slot without pumping ordinary work; the synchronous fault decision first drains/rejects ordinary successors and then deliberately dispatches reserved shutdown. A malformed predecessor observed during stop remains terminal even if shutdown itself responds and the process exits cleanly. `beginDraining` rejects queued ordinary work, lets the one dispatched predecessor finish, and reserves/prioritizes shutdown next even behind 256 accepted requests. A bounded predecessor-drain envelope is separate from the full three-second platform shutdown envelope, which starts only when the shutdown frame is actually written. Late ignored predecessor and final shutdown responses remain independently parseable even in one stdout chunk. A successful stop requires the final matching response and process close; timeout permits best-effort process termination only after the applicable envelope elapsed. There are no admission, ownership, manual-recovery, or drained notifications and no raw transition latch. The AX worker is stopped during native shutdown. On the owner, a native-resource RAII guard owns the live tap, tap source, command source, maintenance timer, callback refcon lifetime, target cache, and native pool. Normal return and contained owner unwind use the same tested order: stop AX, disable tap, remove sources/timer, invalidate callbacks/tap, drop target evidence, release CF objects, then drop the pooled events; clean shutdown reaches it after strict semantic ownership drain; terminal/incomplete shutdown may still tear resources down but is reported separately and never presented as semantic drain. Owner startup also publishes permanently retained command-source, maintenance-timer, and run-loop wake resources; `request_stop` performs only atomic loads plus source/timer signalling and wake-up. It creates no detached wake thread, takes no mutex, and a missing command source conservatively arms the permanent timer before wake-up. Callback unwind first publishes a fixed atomic `recovery_pending` state, retains the authoritative engine snapshot and exact deferred continuation/in-flight effect/current-edge outcome, closes admission, and enters replay/reconciliation/drain instead of stopping the run loop directly. After every recovery attempt, source, timer, event, and owner-lifecycle paths reacquire `recovery_pending` before any shutdown control, queued control, effect, admission, or ordinary callback work. While it remains set, owner paths only rearm permanent wake resources. Event callbacks use a dedicated allocation-free/nonblocking drain classifier: exact installed replay/cleanup remains Pass and advances once; helper barriers remain Owned and advance once; paste down remains Pass-but-uncommitted while exact up publishes the complete-pair result; and exact old-generation transactional or Escape/Enter ownership retires only fixed drain state. It never invokes matching/admission or submits effects. An explicit `recovery_deferred_mode` is release-published only when callback or owner-lifecycle recovery has retained ordering work. Owner recovery computes this requirement before publishing/clearing `recovery_pending`, so physical input cannot overtake an already-submitted replay, cleanup, or in-flight effect. A normal transactional candidate does not set the mode: activation triggers, unrelated terminating keys, side-specific modifier changes, and mouse cancellation continue through the ordinary reducer/effect path. Candidate journals reserve the final fixed slot for the current repeat/terminator, so overflow replay always contains that current exactly once. macOS posts replay through the HID stream and waits for every exact tap observation before reducer finalization; an external or mouse current that is not representable in that journal is copied into the deferred scalar queue and reposted only after observed replay. Once recovery mode is set, every other keyboard, modifier, external-injected, and mouse down/up edge is copied as lossless scalars into a startup-preallocated recovery journal and its original is suppressed until retained replay/cleanup drains. Two independent 64-edge fixed buffers hold the submitted batch and callback-time tail; two disjoint native-pool banks post repeated rollovers without mutating an observed batch or allocating. The journal stores source class, key/button generation and phase, repeat, full `u64` flags, keyboard type, original timestamp/marker/PID, and mouse location/button/click/number/pressure/deltas/instant-mouser/subtype fields. Every recorded scalar is validated on the exact tagged observation. Exact deferred callbacks pass once in original order, then the tail rolls over as the next authenticated batch. External edges are phase-independent: a lone external down is immediately eligible once predecessor native ordering drains, and a delayed up passes normally or joins the next batch without holding shutdown for a pair. A bounded collection deadline converts any external suffix blocked behind an unexposed physical half into recoverable overflow, retaining the external scalars while fencing only that physical half. Recovery mode clears only after replay/cleanup, both deferred buffers, exact observations, physical fences, paste work, and native ownership drain. Reposts receive fresh monotonic native timestamps because they occur after replay, while the original timestamp remains recorded as evidence. Mouse-up is in the tap mask, so a deferred down can never bypass its balancing up.

Exact owned releases and fresh nonrepeat downs use per-source, per-key generations. Recovery retires only the old engine ownership generation; a newer held generation remains physical foreground/fenced state. Its down, repeats, and matching up stay together in the deferred journal. Physical `flagsChanged` transitions use the exact side's HID state (or the maintained test-physical side tracker); external streams use a bounded PID-keyed side model whose slots are pinned by pending, submitted, tail, overflow, exposure, and fence references. Aggregate Ctrl/Alt/Shift/Command flags are never treated as evidence for one side, so either side can remain held while the other releases. If every external slot is pinned, ordinary input is still normalized as an external cancellation boundary, while recovery stores and reposts the exact scalar event without requiring a side phase model. If a required keyboard lock is temporarily unavailable, the independent journal marks the edge ownership-uncertain and resolves it against authoritative ownership before submission.

Capacity overflow enters recoverable terminal admission rather than a permanent global suppression fault. Wholly hidden, unexposed physical generations are discarded; fixed physical key/button fences are authoritative `pending_native_work`, keep shutdown drain-only, and consume the matching half or clear only after HID key/button state plus an exact ordered gap barrier proves release. External edges are never converted into indefinite pair fences: every retained external scalar is replayed in original order. Only exact physical up suffixes required to balance an edge already visible to the foreground—including a deferred down observed before a malformed suffix—are retained and reposted. Submitted prefixes continue under their exact token. Unpaired external downs therefore do not retain the process indefinitely, while concrete physical/native ownership still drains through HID reconciliation, tombstones, barriers, and exact observations. Once concrete balances and physical fences drain, overflow and deferred mode clear and owner teardown is allowed. Candidate mouse downs are never proxy-reposted on replay submission alone. HID replay must be exactly observed first; the original mouse down is copied into recovery-deferred ownership and its up remains ordered with it. Replay preparation/submission failure retains the same balanced deferred ownership. A failed or second-panicking recovery keeps poison, in-flight effect, generation evidence, and concrete deferred observations installed and arms the permanent wake resource for retry. Poison and `recovery_pending` clear only after authoritative continuation/effect/reconciliation state commits. Shutdown control authority is installed before executing its turn, preventing unwind recovery from beginning it twice. Current-edge disposition is independently atomic, so a panic while holding the keyboard mutex cannot erase pass/owned/proxy-replaced status. Callback-critical mutexes recover poisoned authoritative contents with `PoisonError::into_inner`; poison is cleared only after continuation, pending activation/paste, pool access, reconciliation, and strict drain recovery all commit. Recovery is itself unwind-contained, and a second panic leaves poison plus drain ownership for owner retry instead of escaping FFI or making `pending_native_work` permanently busy. Suspension/enqueue/signalling failures are terminal and cannot be ignored.

## Acceptance examples

The notation below uses `FG:` for foreground-visible events, `CAP:` for captured originals, `REP:` for tagged replay, `ACT:` for activation delivery, and `NEUTRAL:` for the Windows dummy event. Modifier events include their ordinary physical down/up unless stated otherwise.

### A1 — unambiguous `Ctrl+Shift+G`

Configured: General = `Ctrl+Shift+G`.

```text
FG: Ctrl down
FG: Shift down
CAP: G down
ACT: General down (accepted snapshot)
CAP: G up
ACT: General up
FG: Shift up
FG: Ctrl up
```

Acceptance: no G event reaches the foreground; each visible modifier down has one visible up.

### A2 — longer built-in `Alt+X+P`

Configured: General = `Alt+X`; Prompt = `Alt+X+P`.

```text
FG: Alt down
CAP: X down                 # General is pending
CAP: P down
NEUTRAL: tagged VK 0xFF down/up
ACT: Prompt down
CAP: P up
ACT: Prompt up
CAP: X up
FG: Alt up
```

Acceptance: neither X nor P reaches the foreground, Prompt activates once, Alt remains balanced, and no menu command is produced.

### A3 — shorter shared prefix

Configured as A2; input is Alt down, X down, X up, Alt up.

```text
FG: Alt down
CAP: X down                 # pending while longer descendants exist
CAP: X up
NEUTRAL: tagged VK 0xFF down/up
ACT: General complete (held duration from X down to X up)
FG: Alt up
```

Acceptance: General activates exactly once on X-up and X is not typed.

### A4 — invalid continuation replays

Configured as A2; input continues with Y rather than P.

```text
FG: Alt down
CAP: X down
CAP: Y down                 # proves that no binding can match
REP: X down, Y down         # one accepted ordered batch
FG: physical Y up
FG: physical X up
FG: Alt up
```

Acceptance: the unsuccessful input is visible exactly once and in order, no profile activates, and all visible downs are balanced. If an adapter journals the later ups instead, its one replay batch includes those ups in the same physical order.

### A5 — callback queue failure

Input reaches the successful boundary from A2, but `ACT: Prompt down` cannot be queued.

Acceptance: no activation is committed; captured events are replayed once; the callback gate closes; future physical input passes through; no captured ownership survives.

### A6 — revision fence

X is physically down under revision 10. Configuration changes to revision 11 before the remaining steps.

Acceptance: the held X cannot begin or complete a revision-11 binding. Matching resumes only after every key fenced by the revision has been released.

### A7 — replay and paste classification

A tagged replay of `Alt+X+P` and a helper-owned paste chord pass through the native callback.

Acceptance: neither stream mutates physical state, enters shortcut matching, emits activation, or is reported as user input.

### A8 — closed rollback gate

The host requests activation plus recording session capture while either rollback control is active.

Acceptance: native-owner configuration receives `enabled:false` and `mode:"off"`; every letter, modifier, Escape, and Enter down/up passes unchanged; no activation or session notification is emitted. Killing the helper after a visible down does not affect its later visible matching up.

### A9 — paste target changed

A shortcut activates in editor A, but focus moves to editor B before transcription finishes.

Acceptance: clipboard content is retained, no paste chord is injected, and editor B receives no text.

### A10 — activation-up queue failure

Prompt down from A2 was delivered and committed, but its matching activation-up notification cannot be queued.

Acceptance: P-up remains swallowed because Talking Quill owns P-down; the later X-up also remains swallowed because captured X-down was committed; committed X/P input is not replayed; fresh activation admission closes. Unrelated input passes immediately during drain. If X remains held through the absolute shutdown deadline, shutdown is terminal/incomplete: no synthetic X-down is sent, ownership is not reported retired, semantic drain is false, and no shutdown success is produced. This state is reachable only in an explicit suppression-enabled test build until a surviving owner exists.

### A11 — target token unavailable

The platform cannot capture a target token strong enough to distinguish the original focused control.

Acceptance: transcription may complete to the clipboard, but no paste chord is injected. The current foreground control receives no text.

### A12 — external and test injection

An external injector emits the configured key sequence. Separately, a native harness uses the test-traffic source.

Acceptance: external events pass but cannot supply modifiers or sequence steps. Test traffic can enter the physical matcher only in an explicit `transactional-shortcuts-dev` build whose owner cached the opt-in before enabling the tap. The trusted harness source traverses CoreGraphics and the production event-tap adapter; callback work remains fixed-size and nonblocking. A release build contains no test source/marker and treats the same records as external.

### Trusted macOS behavior runner

Permissioned macOS runners exercise protocol v10 against the gateway and detached owner with real Accessibility, Input Monitoring, event-posting, Secure Input, replay, and target-specific insertion behavior. These runs provide native acceptance evidence for the tested bytes. Local unsigned owner builds remain functional without a commercial promotion or certification switch; missing runner evidence does not silently convert an enabled local build into a different product mode. Test reports belong under repository `tmp/` and are never packaged.

### A13 — dual modifier sides

Left Alt is down, then X begins the canonical candidate. Right Alt goes down; left Alt goes up while right Alt remains down; P completes Prompt; right Alt goes up.

Acceptance: the unchanged combined Alt mask preserves the candidate; all four side-specific Alt edges pass exactly once; Prompt activates once. Reversing left/right order has identical semantics. AltGr remains excluded.

## Invariants checked by later reducer and native suites

1. Every foreground-visible down has exactly one foreground-visible up.
2. No unowned physical up is suppressed.
3. Replay and paste cannot activate Talking Quill or mutate physical state.
4. Captured input is committed once or replayed once, never both.
5. Failed delivery, cancellation, revision change, and gate closure leave no stale ownership.
6. AltGr does not accidentally satisfy plain Ctrl+Alt.
7. Modifier sides remain balanced across success, failure, shutdown, and recovery.
8. Dummy neutralization occurs at most once per relevant modifier cycle.
9. Paste requires neutral physical modifiers and a still-valid target.
