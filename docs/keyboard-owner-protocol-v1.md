# Keyboard owner protocol v1

> Keyboard owner contract suite: `talking-quill-keyboard-owner-v1`
> Contract version: **1**
> Contract ID: `talking-quill-keyboard-owner-protocol-v1`
> Status: **normative target**

This is private IPC between the non-suppressing gateway and the detached per-user/per-login-session Keyboard Owner. It is independent of Electron/helper protocol v10. No current protocol implementation is certified; see `keyboard-owner-conformance-status.md`.

## 1. Boundary and authority

The owner alone installs a suppressing hook/tap or performs replay, dummy, target-aware paste, or global keyboard injection. Electron/renderers never receive the endpoint, secret, capability, native action token, kernel peer identity, or raw native state.

Endpoint/private-channel policy, launch provenance, kernel peer identity, production code identity, exact artifact/install compatibility, platform credential binding, and HMAC proof are all mandatory. Authentication grants an authority ceiling, not a capability. A later acquire grants an initially disabled capture capability or a disjoint maintenance capability.

Each connection binds one immutable purpose: `observe`, `capture`, or `maintenance`. Purpose never changes after `hello`. Windows personal-runtime peers are a gateway and owner in the same interactive WTS session over the stable local-only named pipe. The same signed-in user is trusted at the operating-system process boundary.

## 2. Common outer frame

Every handshake and authenticated body uses:

```text
u32be body_length
body[body_length]
```

The prefix is not included in `body_length`. `body_length` is `1..=16_384`. Zero, oversize, truncation, partial EOF, or trailing bytes is fatal. Handshake bodies are exact strict-JSON objects. Post-authentication bodies use the binary envelope in section 7.

Strict JSON means one object, valid UTF-8, no duplicate/unknown fields, no batches, exact discriminators, finite bounded values, and no generic native map. Parsers never include source frames in errors/logs.

### Scalar encodings

- Every 32-byte ID/digest/proof/MAC represented in JSON is unpadded base64url, exactly 43 characters.
- A Windows P-256 public key is SEC1 uncompressed form (byte `0x04` plus 32-byte X and Y), unpadded base64url, exactly 87 characters. It is `null` on macOS.
- Every potentially full-width nonzero `u64` is a canonical decimal string: ASCII digits, no leading zero, range 1 through 18,446,744,073,709,551,615.
- Zero is allowed only where a schema explicitly says so. `Counter`/`Duration` is canonical decimal `0..=9,007,199,254,740,991`.
- Feature sets are lowercase `0x` plus exactly 16 hexadecimal digits.
- Bounded display/build strings are UTF-8, 1–64 bytes, with no control characters.
- Enum values are exact lowercase ASCII literals named by this document.

## 3. Protocol selection

Both peers advertise `{major, minor, compatibilityEpoch, supportedFeatureBits, requiredFeatureBits}`.

- initial values are major 1, minor 0, compatibility epoch 1;
- feature `0x1` is `BASE_V1` and required;
- optional feature `0x2` is `FRONT_APP_METADATA_V1`; it enables only `front_app.metadata.get` and never changes the strict legacy `front_app.get` response;
- optional feature `0x4` is `REGISTERED_INPUT_OBSERVABILITY_V1`; it enables only bounded aggregate registered-input counters and never exposes key codes, scan codes, timing, layout, modifiers, configured shortcuts, or raw event records;
- the platform credential binding is mandatory pre-handshake policy, not an optional negotiable feature;
- major and compatibility epoch must be equal;
- selected minor is `min(clientMinor, ownerMinor)`;
- selected features are the bitwise intersection;
- every feature required by either peer must occur in the intersection.

Unknown optional bits are ignored after intersection. Unknown required bits reject. Minor compatibility never overrides artifact/install/signing policy. There is no v8 keyboard downgrade.

## 4. Required authentication order

```text
kernel peer acquisition
→ untrusted hello/header feasibility
→ OS session, executable, signer, build, architecture, installation validation
→ challenge and protocol selection
→ client credential proof
→ authenticated owner finish
→ capability acquisition
```

No credential proof is sent before the gateway validates owner process, code, installed-release, session, and endpoint bindings from kernel-derived facts. No owner capability exists before the final authenticated frame is verified.

Each side uses a fresh OS-RNG 32-byte nonce. The owner also creates a fresh 32-byte session ID and process-scoped owner instance ID. Nonces/session IDs are single-use. Windows additionally requires fresh ephemeral P-256 key pairs on every connection.

### 4.1 Windows stable session pipe

Windows stores no DPAPI, ticket, or per-install owner secret. Each connection uses fresh ephemeral P-256 material. The transcript binds the stable endpoint, independently observed gateway and owner image hashes, process IDs and creation markers, WTS and token session, user and logon SID digests, integrity, architecture, purpose, release-build digest, package-layout digest, and installation identity. Reconnect creates fresh ECDH keys and protocol nonces.

The owner creates `\\.\pipe\TalkingQuill.KeyboardOwner.Personal.V2.<session>` before native readiness, with first-instance enforcement, byte mode, overlapped I/O, remote-client rejection, and a SYSTEM plus current-logon-SID ACL. Both sides derive peer PID from the connected pipe and retain verified process and image handles for the authenticated connection. A newly launched owner must also match the gateway's retained launched PID and creation marker. Handshake fields only confirm these facts.

This protocol authenticates the two cooperating roles and prevents credential replay or accidental ambient handle inheritance. It does not isolate them from hostile code already running as the same interactive user. Such code can inspect or interfere with same-user processes where Windows permits it, or deny service by taking the per-session singleton name.

## 5. Exact handshake payloads

Handshake objects have exactly the listed fields.

### `hello`

```text
{
  type: "hello",
  purpose: "observe" | "capture" | "maintenance",
  protocol: ProtocolHeader,
  clientNonce: Bytes32,
  platform: "windows" | "macos",
  architecture: "x64" | "arm64",
  releaseBuildDigest: Bytes32,
  executableSha256: Bytes32,
  installationIdentityDigest: Bytes32,
  signerPolicyDigest: Bytes32,
  osSessionBindingDigest: Bytes32,
  clientReleasePolicy: PolicyBlob,
  clientReleasePolicySignature: PolicySignature,
  clientReleasePolicyDigest: Bytes32,
  platformCredentialBindingDigest: Bytes32,
  clientEphemeralPublicKey: P256PublicKey | null
}
```

### `challenge`

```text
{
  type: "challenge",
  ownerProtocol: ProtocolHeader,
  selectedProtocol: SelectedProtocol,
  purpose: Purpose,
  authorityCeiling: "observer" | "capture" | "maintenance",
  ownerNonce: Bytes32,
  sessionId: Bytes32,
  ownerInstanceId: Bytes32,
  platform: Platform,
  architecture: Architecture,
  releaseBuildDigest: Bytes32,
  executableSha256: Bytes32,
  installationIdentityDigest: Bytes32,
  signerPolicyDigest: Bytes32,
  osSessionBindingDigest: Bytes32,
  ownerReleasePolicy: PolicyBlob,
  ownerReleasePolicySignature: PolicySignature,
  ownerReleasePolicyDigest: Bytes32,
  platformCredentialBindingDigest: Bytes32,
  ownerEphemeralPublicKey: P256PublicKey | null
}
```

### `authenticate`

```text
{ type: "authenticate", clientProof: Bytes32 }
```

### `authenticated`

`authenticated` is the first owner-to-gateway authenticated envelope (transport sequence 1, response kind, correlation zero) and the sole exception to section 8's ordinary `{ok,result}` response wrapper:

```text
{
  type: "authenticated",
  selectedProtocol: SelectedProtocol,
  purpose: Purpose,
  authorityCeiling: AuthorityCeiling,
  ownerProof: Bytes32
}
```

Claims are comparison inputs, never evidence. Both peers use kernel-bound process handles/audit tokens for validation. A failed/replayed/reordered/malformed handshake closes without changing owner state.

## 6. Exact transcript, proofs, and keys

The transcript is this fixed binary concatenation:

```text
ASCII "TQKO-AUTH-TRANSCRIPT-V1\0"
client major                    u16be
client minor                    u16be
client compatibility epoch      u32be
client supported features       u64be
client required features        u64be
owner major                     u16be
owner minor                     u16be
owner compatibility epoch       u32be
owner supported features        u64be
owner required features         u64be
selected major                  u16be
selected minor                  u16be
selected compatibility epoch    u32be
selected features               u64be
connection purpose              u8 (observe=1,capture=2,maintenance=3)
authority ceiling               u8 (observer=1,capture=2,maintenance=3)
platform                        u8 (windows=1,macos=2)
client architecture             u8 (x64=1,arm64=2)
owner architecture              u8 (x64=1,arm64=2)
reserved                        3 zero bytes
client nonce                    32 bytes
owner nonce                     32 bytes
session ID                      32 bytes
owner instance ID               32 bytes
client release build digest     32 bytes
client executable digest        32 bytes
owner release build digest      32 bytes
owner executable digest         32 bytes
installation identity digest    32 bytes
client signer-policy digest     32 bytes
owner signer-policy digest      32 bytes
OS session-binding digest       32 bytes
client immutable release-policy digest 32 bytes
owner immutable release-policy digest  32 bytes
platform credential-binding digest     32 bytes
key-agreement mode              u8 (macOS Keychain=1, retired ticket mode=2 reserved, Windows stable-pipe P-256=3)
reserved                        3 zero bytes
client P-256 public key         65 bytes (all zero on macOS)
owner P-256 public key          65 bytes (all zero on macOS)
```

The two peers must have equal installation/session digests; the transcript stores that accepted value. `PolicyBlob` is unpadded base64url of exactly 328 bytes (438 characters). On macOS, `PolicySignature` contains a 1–4,096-byte DER detached CMS signature over those exact policy bytes. In the unsigned Windows profile, the same field carries the fixed `TQKOWPR1` installed-manifest proof. Windows independently rebuilds the policy from the protected installer manifest and kernel-derived peer image hashes, then compares both the policy and proof before trusting any field. A DER-shaped digest is not a Windows signature or proof.

The exact `TQKO_RELEASE_POLICY_V1` blob layout is:

```text
offset size field
0      8    ASCII "TQKOPOL1"
8      2    format_version u16be = 1
10     1    platform (windows=1,macos=2)
11     1    architecture (x64=1,arm64=2)
12     1    owner_mode (safe_disabled=1,enabled_candidate=2)
13     1    predecessor_present (0|1)
14     2    zero
16     32   release_build_digest
48     32   gateway_executable_sha256
80     32   owner_executable_sha256
112    32   gateway_signer_policy_digest
144    32   owner_signer_policy_digest
176    24   gateway protocol: major u16be, minor u16be,
            epoch u32be, supported u64be, required u64be
200    24   owner protocol in the same layout
224    32   predecessor_release_build_digest
256    32   predecessor_gateway_sha256
288    32   predecessor_owner_sha256
320    1    predecessor_platform (0 if absent; otherwise platform tag)
321    1    predecessor_architecture (0 if absent; otherwise architecture tag)
322    6    zero
```

When predecessor is absent, bytes 224–327 are all zero. No blob contains its own digest or signature. The JSON digest field must equal `SHA256(decoded PolicyBlob)`. Each peer verifies its platform proof, exact length/reserved/tags, installed artifact metadata, role-policy digest, platform/architecture, and advertised headers before proofs. macOS verifies detached CMS. Windows reconstructs the policy from the protected manifest and retained peer image facts. For new-gateway→old-owner maintenance, the owner verifies that the client blob's predecessor owner fields name its own exact policy/artifact. For old-gateway→new-owner maintenance, the gateway verifies that the owner blob's predecessor gateway fields name its own exact policy/artifact. Both distinct digests are transcript-bound; handshake-supplied bytes are trusted only after detached-signature and self-field validation. Canonical vectors are frozen in `release-policy-vectors.json`.

```text
transcriptHash = SHA256(transcript)
platformIkm = macOS: perInstallKeychainSecret
            | Windows: rawP256AffineX32be
PRK = HKDF-Extract-SHA256(salt=transcriptHash, IKM=platformIkm)
clientProofKey = HKDF-Expand-SHA256(PRK, "TQKO client proof v1", 32)
ownerProofKey = HKDF-Expand-SHA256(PRK, "TQKO owner proof v1", 32)
gatewayFrameKey = HKDF-Expand-SHA256(PRK, "TQKO gateway-to-owner frame v1", 32)
ownerFrameKey = HKDF-Expand-SHA256(PRK, "TQKO owner-to-gateway frame v1", 32)
clientProof = HMAC-SHA256(clientProofKey,
  "TQKO CLIENT FINISH V1\0" || transcriptHash)
ownerProof = HMAC-SHA256(ownerProofKey,
  "TQKO OWNER FINISH V1\0" || transcriptHash || clientProof)
```

On Windows, each peer validates the SEC1 public point as finite, on NIST P-256, non-infinity, and in the correct subgroup before use. `rawP256AffineX32be` is the SP 800-56A raw ECDH affine X coordinate encoded as exactly 32 big-endian bytes, left-padded with zeroes. Hashed/provider-specific `DeriveKey*` output is not this field. W2 must freeze cross-language known-answer vectors for both public points, the raw shared value, complete transcript, HKDF keys, proofs, and frame MACs.

All comparisons are constant-time. The final envelope MAC plus `ownerProof` binds session ID, selected protocol, purpose, and authority ceiling.

The macOS per-install secret is exactly 32 OS-RNG bytes. R5-M fixes the generic-password service/account and optionally compiles `TALKING_QUILL_MACOS_KEYCHAIN_ACCESS_GROUP`. When present, that exact group is queried; when absent for a local no-paid-entitlement installation, the group selector is omitted. Both modes request all matching class/service/account data with `kSecUseAuthenticationUIFail` and accept exactly one 32-byte nonzero item; missing, duplicate, wrong selected group, malformed, or UI-requiring results fail closed, and runtime never creates an item. The group-less query can prove only these query/result properties: it cannot inspect or prove the externally provisioned per-item trusted-application/designated-requirement ACL. Native owner/gateway acceptance and Electron/other-same-user denial evidence for that ACL remains pending R8-M, and no owner-mediated fallback is claimed to work. R5-M performs the Security.framework identity and Keychain lookup in an exact-owner subprocess created by Darwin `posix_spawn` with `POSIX_SPAWN_CLOEXEC_DEFAULT`, explicit fixed-FD `dup2` actions, `/dev/null` standard streams, and only `PATH=/usr/bin:/bin` plus `LANG=C`. Parent descriptors remain permanently close-on-exec. Parent/child exchange a version-1 request/response frame with distinct eight-byte magic, exact big-endian payload length, purpose, a fresh 32-byte request nonce echoed by the response, the already-validated bounded sealed-policy bytes in the request, and connection binding plus secret in the fixed response; short, wrong-version/purpose/nonce/length, or trailing data fails closed, and every secret/result buffer is `Zeroizing`. Parent atomically-CLOEXEC socketpair I/O is nonblocking `poll` under the absolute deadline and shutdown cancellation. Timeout sends `SIGKILL` and hands the PID to a dedicated non-joined reaper which retains it until `waitpid` succeeds, so an uninterruptible child cannot hold an authentication worker or singleton shutdown indefinitely. This supplies a hard bound even though Security.framework does not document a universal completion deadline for every local trust/Keychain backend. R5-M thereby proves its own exact no-UI lookup behavior, not the provisioned ACL's gateway/owner-only negative set. R8-M must create and natively inspect/test the access group and ACL so the exact enrolled gateway and owner can read while Electron and other same-user code cannot. Before reading it, the owner requires matching `getpeereid`, `LOCAL_PEERPID`, and `LOCAL_PEERTOKEN` values from the accepted socket, obtains the audited dynamic `SecCode` with `kSecGuestAttributeAudit`, validates that exact running code directly with strict `SecCodeCheckValidity` and the designated requirement, then derives its exact `SecStaticCode` and repeats strict validation as an additional disk check. Dynamic and static signing information—including identifier, signing class/certificate identity, and the CodeDirectory unique hash/CDHash for both ad-hoc and self-signed modes—must agree exactly. The main-executable URL comes from the audited code's signing information. R5-M resolves that absolute path once, component by component from a retained root descriptor with `openat(O_NOFOLLOW)`, retains and hashes the resulting vnode under a 256 MiB cap, and rechecks that retained vnode after static validation; it performs no PID-path lookup and no post-validation pathname reopen. macOS does not expose a public API that directly returns an executable vnode from `SecCode`, so the one componentwise path resolution from Security.framework's URL is an explicit OS limitation; the audit-token dynamic `SecCode` remains the primary authority. Signing information supplies the actual identifier, ad-hoc CodeDirectory hash, or certificate chain. Local requirements are either an actual ad-hoc signature with exact CodeDirectory hash or a one-certificate locally trusted self-signed identity whose actual DER supplies both the enrolled SHA-256 and requirement SHA-1; root/leaf equality and trust are also enforced by the designated requirement. Expected policy values are never echoed as observed identity. Apple anchors, Developer ID chains, and other multi-certificate chains are rejected. Windows has no ticket or durable reconnect credential. Fresh ephemeral P-256 keys and nonces are bound to the matching stable endpoint, kernel peer facts, installed release, session, and purpose. Electron controls the gateway protocol but does not receive the owner pipe handle. The macOS `platformCredentialBindingDigest` is client-computable `SHA256(UTF8("talking-quill/macos-audit-token-binding/v1") || 0x00 || eight LOCAL_PEERTOKEN words in big-endian order || console UID in big-endian order)`. The gateway computes it from its own audit token; the owner recomputes it from the exact accepted socket's `LOCAL_PEERTOKEN` and `getpeereid`. It is not server-random or socket-inode-derived. Fresh nonces, the Keychain proof, and independent accepted-socket/SecCode verification prevent a copied Hello from authorizing another peer. No credential is accepted through argv/environment/renderer IPC/stdout/stderr/user-readable storage. macOS Keychain role isolation remains a separate platform requirement. Windows makes no protected-launch or same-user denial claim in the personal runtime.

## 7. Exact authenticated envelope

```text
offset  size  field
0       4     magic = ASCII "TQKO"
4       1     envelope_version = 1
5       1     message_kind
6       1     direction
7       1     flags = 0
8       32    session_id
40      8     transport_sequence, u64be
48      8     correlation_sequence, u64be
56      4     payload_length, u32be
60      N     exact strict UTF-8 JSON payload
60+N    32    HMAC-SHA-256
```

- kinds: request=1, response=2, event=3, predecessor-terminal-event=4;
- directions: gateway-to-owner=1, owner-to-gateway=2;
- `body_length = 92 + payload_length`, maximum payload 16,292 bytes;
- first sequence in each direction is 1, then exactly previous+1, never wrapping;
- request correlation is zero;
- response correlation echoes the gateway-to-owner transport sequence of its request;
- ordinary/terminal event correlation is zero;
- `authenticated` is the one response allowed correlation zero.

MAC input is:

```text
ASCII "TQKO-FRAME-V1\0"
|| u32be(body_length)
|| body bytes offsets 0 through 59+N
```

Use the directional frame key. The MAC field is excluded. Duplicate/skipped/wrapped/wrong-direction/wrong-session/wrong-length/nonzero-flags/invalid-MAC closes the connection and revokes its capability through common loss handling.

At most eight requests may await responses per connection. Responses may interleave with events; correlation is mandatory. The writer preserves each direction's sequence order. There is no response retransmit/cache.

## 8. Payload envelope and errors

Request payload:

```text
{ method: MethodName, params: MethodSpecificObject }
```

Success response:

```text
{ ok: true, result: MethodSpecificObject }
```

Error response:

```text
{ ok: false, error: { code: ErrorCode, message: ErrorMessage } }
```

`ErrorCode` is one of `busy`, `draining`, `incompatible`, `rollback`, `security_fault`, `invalid_state`, `native_failure`, `indeterminate`, or `unavailable`. `ErrorMessage` is fixed by code, ASCII, at most 96 bytes, and contains no peer/input/auth data. Schema/framing/MAC/sequence/unknown-method faults close without a semantic response where trust/correlation is unavailable.

Event payload:

```text
{ event: EventName, params: EventSpecificObject }
```

## 9. Methods and exact bounds

Every method below is an exact allowlist member; all params deny unknown fields.

### Transport-sequenced acquire/read-only

- `lease.acquire`: `{}`; result `{captureLeaseId:Bytes32,captureLeaseEpoch:U64,state:"disabled"}`.
- `maintenance.acquire` is an exact tagged union: update/rollback params are `{operation:"update"|"rollback",transactionId:Bytes32,sourceBuildDigest:Bytes32,targetBuildDigest:Bytes32,targetOwnerSha256:Bytes32}`; uninstall params are `{operation:"uninstall",transactionId:Bytes32,sourceBuildDigest:Bytes32}` and target fields are forbidden. Result only after durable staged seal is `{maintenanceCapabilityId:Bytes32,maintenanceCapabilityEpoch:U64,state:"sealed"|"draining"}`.
- `health.get`: `{}`; bounded aggregate result, maximum 1,024 JSON bytes.
- `permissions.get`: `{}`; exact booleans/known-enum permission states, maximum 512 bytes.
- `observability.get`: `{}`; strict aggregate counters, maximum 2,048 bytes.
- `front_app.get`: `{}`; strict predecessor token result, maximum 1,024 bytes; unavailable to maintenance.
- `front_app.metadata.get`: `{}`; accepted only when `FRONT_APP_METADATA_V1` was selected; strict display-metadata result, maximum 1,024 escaped JSON bytes; unavailable to maintenance.

### Capture-command sequenced

Each params object additionally has `captureLeaseId:Bytes32`, `captureLeaseEpoch:U64`, `commandSequence:U64`.

- `lease.renew`: no additional fields; response after reducer acceptance.
- `session.reconcile_off`: no additional fields; response after native off confirmation.
- `session.set_mode`: adds `mode:"off"|"recording"|"cancel-only"`; response after native confirmation; cannot enable session keys unless keyboard admission is enabled.
- `capture.replace_configuration`: adds `revision:U64,bindings:Binding[]`; `Binding` is exactly `{profileId:string,shortcut:BindingShortcut}`, `BindingShortcut` is exactly `{modifiers:{ctrl:boolean,alt:boolean,shift:boolean,meta:boolean},keys:Letter[]}`, and `Letter` is one uppercase ASCII `A` through `Z`. Snapshot has 0–13 bindings; profile ID is 1–36 UTF-8 bytes; keys contain 1–26 ordered unique letters. Duplicate profiles/shortcuts and reserved built-in ownership violations reject. Response follows native apply/fence confirmation.
- `capture.set_enabled`: adds `enabled:boolean`; enable responds after open confirmation; disable after close barrier.
- `paste.inject`: adds `operationId:Bytes32,ownerInstanceId:Bytes32,activationGeneration:U64,targetToken:string|null,fallbackTextSha256:Bytes32`; target token 1–64 UTF-8 bytes; response at the phase specified by mapping/lifecycle, never with clipboard text.
- `lease.release`: no additional fields; response after close barrier `{disposition:"neutral"|"draining"}`.
- `owner.exit_when_neutral`: carries the same capture capability fields and command sequence as `lease.release`. It seals new admission and returns the same disposition shape. A `draining` result remains connected until the final `lease.neutral` terminal event flushes; only then may planned process exit begin.
- `runtime.rollback`: no additional fields; priority command that latches before responding and cannot be blocked by pending native work.

Capture success results are exact: renew `{renewed:true}`; session reconciliation/set `{mode:SessionMode}`; configuration `{revision:U64}`; enable/disable `{enabled:boolean}`; release `{disposition:"neutral"|"draining"}`; rollback `{latched:true,disposition:"neutral"|"draining"}`. Paste result is exactly one tagged variant: `{state:"clipboard_only",reason:PasteRefusalReason}` where `PasteRefusalReason` is exactly `permission_denied|conflicting_modifiers|secure_input|target_unavailable|clipboard_changed|native_unavailable|native_rejected`, `{state:"waiting",operationId:Bytes32}`, `{state:"committed",operationId:Bytes32}`, or `{state:"indeterminate",operationId:Bytes32}`. A waiting response is not permission to retry after later claim.

### Maintenance-command sequenced

Each params object additionally has `maintenanceCapabilityId:Bytes32`, `maintenanceCapabilityEpoch:U64`, `commandSequence:U64`.

- `maintenance.renew`: no additional fields.
- `maintenance.prepare`: adds `transactionId:Bytes32,operation:"update"|"uninstall"|"rollback"`; final response `{readyToExit:true}` is produced only after authoritative adapter stop/unregister readiness, then flushed before exit.

Maintenance renew result is exactly `{renewed:true}`. Acquire/read-only result objects use exact schemas owned by their listed method: no result may add fields. `health.get` returns `{ownerInstanceId:Bytes32,reportedState:OwnerReportedState,processState:ProcessState,rollbackLatched:boolean,nativeStateUnknown:boolean,maintenanceSealed:boolean,keyboardBuildEligible:boolean,pasteReady:boolean,permissionsEligible:boolean,hookHealthy:boolean}`. `OwnerReportedState` is `starting|idle_neutral|lease_disabled|lease_enabled|lease_draining|orphan_cancelling|orphan_draining|maintenance_draining|degraded_draining|maintenance_ready|stopping`; `ProcessState` is `starting|healthy|rollback_latched|degraded|stopping_native|flushing_response|exiting`. `permissions.get` returns `{accessibility:PermissionState,inputMonitoring:PermissionState,eventPost:PermissionState}` where state is `granted|denied|unknown|not_required`. `front_app.get` remains exactly `{available:boolean,applicationToken:string|null}` with token 1–64 UTF-8 bytes and no name/title/PID. When negotiated, `front_app.metadata.get` returns exactly `{available:boolean,processName:string|null,windowTitle:string|null,windowBounds:{x:i32,y:i32,width:u32,height:u32}|null}`. Available metadata requires both strings; unavailable requires every nullable field to be null. Metadata is display-only and never an application token.

`observability.get` returns exactly the following object; every leaf is `Counter` and no map/extra field is allowed:

```text
{
  owner: {
    starts, cleanExits, abnormalExits, singletonCollisions,
    authAttempts,
    authFailures: {crossUser,wrongSession,codeIdentity,mac,protocol},
    leaseAcquired, leaseRenewed, leaseExpired, leaseDisconnected,
    leaseReleasedNeutral, leaseReleasedDraining,
    drainDurationMsTotal, drainDurationMsMax, maintenancePostponed,
    handoffSucceeded, handoffFailed, degraded, hookRecoveries
  },
  transactions: {
    started, committed, replayed, cancelled, journalHighWater,
    cancellationReasons: {
      invalidContinuation, modifierChanged, altGr, journalOverflow,
      configurationReplaced, revisionMismatch, gateClosed, shutdown,
      helperDisconnected, secureDesktop, timeout, activationDeliveryFailed,
      neutralizationFailed, replayFailed, effectProtocolViolation,
      physicalStateMismatch, targetChanged
    }
  },
  replay: {attempted,succeeded,partial,failed},
  dummy: {attempted,succeeded,partial,failed},
  nativePaste: {
    targetValidationFallbacks, modifierWaitDurationMsTotal,
    modifierWaitDurationMsMax, modifierTimeouts,
    shutdownOwnershipDeadlines
  }
}
```

Read-only methods/events do not consume capability command sequence or extend capability expiry. Only an accepted current `lease.renew` updates capture expiry; only an accepted current `maintenance.renew` updates maintenance expiry. Valid semantic errors consume command sequence but do not renew. Duplicate/stale/skipped/wrong-capability/wrapped active commands close/revoke. Wrong unrelated connections cannot disturb active authority. Reconnect never retries uncertain mutation.

The normative wire-to-state mapping and response points are in `keyboard-owner-wire-reducer-mapping.md`.

## 10. Events and predecessor terminal route

Ordinary events (kind 3) and exact params are:

- `activation`: `{captureLeaseEpoch:U64,ownerInstanceId:Bytes32,profileId:string,shortcut:BindingShortcut,activationGeneration:U64,targetToken:string|null,phase:"down"|"up",heldMs:U64|null}`; `heldMs` is non-null only for atomic completion/up semantics;
- `session_key`: `{captureLeaseEpoch:U64,key:"escape"|"enter",phase:"down"|"up"}`;
- `paste_committed`: `{captureLeaseEpoch:U64,operationId:Bytes32,state:"committed"|"indeterminate"}`;
- `audio_devices_changed`: `{captureLeaseEpoch:U64}`;
- `health_changed`: the exact `health.get` result;
- `terminal_degraded`: `{reason:"native_fault"|"ownership_unknown"|"callback_delivery"|"protocol"}`.

`BindingShortcut` is the same exact four booleans plus 1–26 ordered unique uppercase A–Z keys defined for configuration; profile/target bounds are identical. They are accepted only for the current owner instance/session/capability scope. No raw event stream or replay journal is sent.

After capture release/revocation, the owner may retain exactly one immutable predecessor route `{originalConnection,captureLeaseId,captureLeaseEpoch,terminalEventHighWater}`. Every kind-4 payload includes `{event,captureLeaseId:Bytes32,captureLeaseEpoch:U64,terminalSequence:U64}`. Sequence starts at 1, is exactly contiguous, never wraps, and updates `terminalEventHighWater` only after the frame is accepted by the local writer. Exact tagged payloads are:

- `{event:"lease.revoked",captureLeaseId,captureLeaseEpoch,terminalSequence,reason:"eof"|"heartbeat"|"maintenance"|"release"|"protocol"|"rollback"}`;
- `{event:"lease.draining",captureLeaseId,captureLeaseEpoch,terminalSequence,ownership:"candidate"|"activation"|"session"|"replay_cleanup"|"paste"|"multiple"}`;
- `{event:"lease.neutral",captureLeaseId,captureLeaseEpoch,terminalSequence,disposition:"neutral"}`;
- `{event:"lease.unavailable",captureLeaseId,captureLeaseEpoch,terminalSequence,reason:"native_fault"|"ownership_unknown"}`.

`lease.revoked` occurs at most once. Zero or more draining events may follow. Exactly one `lease.neutral` or `lease.unavailable` is final; no predecessor frame follows it. Local writer/connection failure discards the route without affecting native drain and does not retry on reconnect. The gateway accepts kind 4 despite the lease no longer being current only for the retained original connection/session/ID/epoch and next terminal sequence. Activation/session/paste mutations remain forbidden. The route is never transferred to a reconnect.

## 11. Capability matrix

| Operation | Observer | Capture | Maintenance |
| --- | ---: | ---: | ---: |
| bounded health/observability | yes | yes | yes |
| permissions/front app | policy subset | yes | no |
| capture/session configuration | no | yes | no |
| fresh keyboard admission | no | gated | no |
| owner paste admission | no | separately gated | no |
| planned neutral owner exit | no | yes | no |
| maintenance prepare | no | no | yes |
| arbitrary native operation | no | no | no |

Capture authority and keyboard availability differ. Feature-free exact test builds use the frozen `safe_disabled` wire value and may hold a disabled capture capability and separately authorized paste, but fresh keyboard admission remains impossible.

## 12. Privacy and fault behavior

Diagnostics/logs/errors/renderer-visible surfaces never expose configured keys/profile IDs, raw events, replay journals, clipboard text/hash, endpoint, PID/SID/UID/audit token, process creation marker, endpoint binding, ephemeral public/private key, capability/session/nonce/proof/MAC, target evidence, window/process names, certificate contents, or signing credentials. Authorized `paste.inject` necessarily carries a bounded text hash only inside authenticated owner IPC; it is never logged.

Parser/writer failure, EOF, queue failure, and protocol faults revoke authority and use state-machine close/cancel/drain. They do not terminate the native owner for convenience. No timeout authorizes fake neutrality, retransmit, balancing downs, or forced exit.
