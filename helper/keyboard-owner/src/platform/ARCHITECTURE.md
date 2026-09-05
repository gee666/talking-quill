# Native platform modules

`mod.rs` defines the shared platform contract. `callback` owns callback admission,
delivery leases, and terminal signaling. `observability` owns the serialized
aggregate schema and atomic snapshots. `tap_recovery` is the pure macOS recovery
policy. None of these modules adds a native worker or protocol queue.

## macOS

`macos.rs` owns shared state. Owner commands, paste commands, submission, and the
platform implementation are separate modules. Commands retain their original
claim/cancel state transitions and completion deadlines.

`event_tap.rs` owns the callback context and its module map. Its children separate
run-loop startup and resource lifetime, input normalization, transaction effects,
deferred journals, paste observation, gap/panic recovery, and shutdown. They
borrow the same context rather than creating independent copies of transaction
state. Tests are grouped by the contracts they exercise.

`injection` owns native event allocation, marker authentication, replay records,
and paste barriers. `target` owns retained AX evidence, cache epochs, validation,
observers, clipboard samples, and insertion. The event tap continues to use
scalar reservations and the existing bounded AX-worker handoff.

`cf::OwnedCf` adopts raw references through an unsafe constructor with an explicit
ownership contract. String conversion and hashing borrow an owner so the CF
object remains retained during conversion. The `Send` implementations for target
evidence and the native event pool retain their documented thread restrictions.

Two files intentionally exceed the 350-line target:

- `ffi.rs`, 359 lines, keeps the shared ABI types and framework declarations in one
  catalog. Splitting this small catalog would add visibility plumbing without
  separating ownership or behavior.
- `event_tap/recovery_classifier.rs`, 359 lines, keeps the ordered recovery
  disposition decision tree together. Branch ordering determines which native
  obligations may be retired.

## Windows

`hook` separates owner-thread lifetime, command claims, callback dispatch, input
tracking, transaction effects, paste, and shutdown. Its tests are grouped by
input and lifecycle contracts. `injection` separates marker identity, native
record encoding, paste planning, and cleanup. `target` separates token registry
policy from native focus evidence. `audio_devices` separates COM declarations,
callback objects, topology sampling, monitor ownership, and the worker loop.

The PowerToys attribution stays with `neutralize_menu` in
`windows/injection/replay.rs`. Its complete MIT grant remains in
`docs/attribution/powertoys-mit.txt` at the repository root, and the notice generator
still includes that grant in `app/assets/THIRD_PARTY_NOTICES.txt`. Attribution
checks must read the implementation module, not just `windows/injection.rs`.

Native registration, unregistration, callback lifetime, and input ordering remain
unchanged. In particular, failed native unregistration retains the existing
callback-lifetime assumptions; this refactor does not attempt a recovery-policy
change.

## Verification limits

Windows unit tests and macOS cross-target checks validate Rust behavior and
compilation, not permissioned macOS execution or OS callback races. Native
acceptance still needs macOS event-tap/AX/TCC runs and Windows hook, SendInput,
desktop-switch, and Core Audio notification runs. Local-unsigned and native-test
features are intentionally mutually exclusive.
