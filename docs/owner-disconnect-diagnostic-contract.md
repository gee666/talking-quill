# Owner disconnect diagnostic contract

Owner disconnect accounting is active only when the user enables detailed diagnostic logging before the helper starts. It uses a helper journal, a separate persistence worker, stderr replay, an Electron checkpoint, and framed acknowledgements. With diagnostics disabled, Electron does not configure the helper journal, persist replay dimensions, or send acknowledgements. Stderr is transport, not proof of delivery.

## Helper report contract

`report_owner_connection_diagnostic` returns `Result`. It adds the disconnect to the fixed in-memory dimension map, submits a commit waiter to the journal worker, and waits at most 250 ms. It returns `Ok` only when the worker has atomically committed a journal snapshot containing that exact cumulative value. Queue exhaustion, worker startup failure, or deadline expiry returns an error. The in-memory increment remains pending for retry after an error.

Owner startup, reconciliation, renewal, event polling, health refresh, and established operations treat this result as diagnostic status only. A storage or writer failure may lose diagnostic records, but it cannot close the callback gate, revoke a lease, stop reconnect, or change lifecycle state. Keyboard safety comes only from authenticated protocol and native ownership state.

The caller never performs filesystem I/O. The persistence worker owns journal reads, writes, retry timing, and commit replies. Its queue is capped at 64 waiters. A transient store error retries every 20 ms. The owner coordinator or service worker waits only for the 250 ms result deadline.

## Exclusive journal ownership

Electron passes one profile-specific journal path to the helper. `FileJournalStore` opens a sidecar lock file and holds a nonblocking OS-exclusive lock for the helper process lifetime. Windows uses `LockFileEx` with `LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY`. Unix uses `flock` with `LOCK_EX | LOCK_NB`.

An overlapping helper cannot read or replace the journal while its predecessor holds the lock. It keeps any new disconnect in bounded memory, retries lock acquisition on the persistence worker, and returns failure at the report deadline. The helper continues its normal supervised lifecycle without claiming that record was durable. A later process can take over the journal only after the operating system releases the predecessor's lock.

The journal has one stable 256-bit CSPRNG ID and a separate 256-bit CSPRNG nonce. Each successful process takeover creates a fresh 256-bit CSPRNG stream ID and then increments the journal generation. The helper rejects equal IDs and the former all-zero and all-`f` sentinel values. It has no deterministic identity fallback. Independent processes therefore rely only on fresh operating-system entropy, not shared machine or profile data, for new identities. These identifiers contain no machine, user, path, owner, or request data.

## Startup recovery

Initial access denial and lock contention remain in a recovery state. The persistence worker retries `load` in-process. If it later acquires an existing journal, it adopts that journal identity and merges any bounded pending counters before committing. It does not overwrite an unread predecessor journal.

A syntactically invalid, truncated, oversized, or schema-invalid journal is moved to a random `corrupt-<256-bit-id>.json` name under the same protected logs directory after exclusive ownership is acquired. The new journal starts with explicit durability-failure accounting. Quarantine names reveal no original content. If quarantine or replacement fails, recovery continues and reports cannot succeed.

A new or quarantined journal stays unavailable if the operating-system CSPRNG fails. The persistence worker retries entropy generation every 20 ms. It must obtain a fresh, pairwise-distinct journal ID, nonce, and process stream ID, then durably commit the journal and generation before `journal_available` becomes true. An existing valid journal keeps its stable ID and nonce, but a restarted helper still needs a fresh stream ID and a durable generation update. Until that completes, the writer emits no owner replay, ACK validation fails closed, and report calls cannot return `Ok`.

## Journal format and replay

The journal stores each exact allowlisted dimension tuple, cumulative `u128` total, acknowledgement high-water value, and overflow state. Every `u128` value is serialized as a decimal string. The tuple grammar has fewer than 100,000 possible values and the file has a 32 MiB cap. It never stores one entry per disconnect.

The stderr writer and journal worker are separate. Blocked stderr cannot delay journal commits or commit replies. The writer emits owner values only from a successful journal snapshot. A record is marked `durable: true` only when no newer journal revision is pending. The writer emits no owner replay while identity generation, initial installation, or storage recovery leaves the journal unavailable. A failed report therefore cannot produce a false durable record.

Writer spawn failure, recovered synchronization poisoning, and journal failures increment explicit counters. Writer failure does not erase the journal. A successor can replay it after safe lock takeover.

## Framed acknowledgement

`diagnostic.ack` is a strict framed JSON-RPC request on the existing helper stdin channel. Electron sends it only after `owner-connection-counts.json` has been atomically replaced and synced.

The RPC coordinator validates and enqueues the ACK update. The journal worker performs its filesystem work. The coordinator waits at most 250 ms for the commit reply. Timeout returns `{ "acknowledged": false }` without supervising or restarting the helper. The helper keeps replaying until a later ACK commit succeeds.

The helper checks journal ID, nonce, exact dimensions, and monotonic decimal count. It advances the durable acknowledgement only after the worker commits the updated journal.

This ordering closes both crash windows:

1. If Electron dies before its checkpoint commits, it sends no ACK. The journal remains unacknowledged and replays after the same helper continues or a later helper safely takes over the lock.
2. If Electron commits and dies before the helper commits the ACK, replay is deduplicated from Electron's persisted per-journal high-water value.

## Electron checkpoint and bounds

Electron deduplicates by stable journal identity and exact dimension. It retains independent high-water maps for interleaved journals and process generations. The version-2 checkpoint atomically stores totals, journal high-water values, stream metadata, helper failure counters, collision counters, and capacity rejects.

The sink retains at most 32 journal identities and never collects their high-water maps. A 33rd identity changes no totals and receives no ACK. Audit-only process stream metadata is capped at 256 entries. Collecting that metadata cannot affect counting because stream metadata is not a deduplication key.

Ordinary JSONL diagnostics use a separate 256-operation queue. Owner replay commits use a separate queue capped at 64. All accepted dimensions and checkpoint fields pass strict privacy schemas. Diagnostic export includes the Electron checkpoint, not the helper journal path or quarantined contents.

## Retention limit

Forced helper or Electron death loses no disconnect for which `report_owner_connection_diagnostic` returned `Ok`, provided the committed helper journal remains readable and its counters have not exceeded `u128::MAX`.

If storage stays unavailable beyond 250 ms, reporting returns failure. The pending value remains in bounded memory and can commit if storage recovers before process exit, but callers cannot treat it as durable. The failure never becomes capture, reconnect, shutdown, or process-lifecycle authority. If the process exits first, that failed report is outside the durable set.
