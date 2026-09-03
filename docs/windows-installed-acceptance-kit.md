# Windows installed-acceptance kit

The installed-acceptance kit is nonpromotable test material for Windows 0.0.69. It does not replace the canonical installer and must never be published as a release asset.

## Inputs

`scripts/build-windows-installed-acceptance-inputs.mjs` produces the complete x64 input set. It imports and extracts the canonical package, runs the acceptance candidate, repair, fault, native sender, launcher, and isolated-validation build stages, creates the fixed run window and ordered request nonces, then calls the kit assembler. Use `--help` to list its required protected-key paths and output options.

`scripts/build-windows-installed-acceptance-kit.mjs` requires:

- the canonical `RELEASE.json` and its independently recorded SHA-256;
- the canonical `artifact-provenance.json` and its independently recorded SHA-256;
- the directory containing the exact canonical installer named by `RELEASE.json`;
- a Git object database containing the descriptor's full source commit and tree;
- a kit input JSON with the frozen candidate update, canonical predecessor and reinstall, acceptance repair, trusted launcher, synthetic sender, and ten fault packages;
- protected P-256 request, manifest, update, and validation signing keys in PKCS8 DER form, each with one link and no linked or reparse-point ancestors;
- the native signer, acceptance broker, and source-bound verified-child bootstrap.

The builder accepts only version 0.0.69, a canonical fresh RELEASE, and x64 or ARM64. It verifies the descriptor hash, installer size and hash, full TQPKG2 identity, source commit/tree, builder provenance inventory, and exact canonical installer reuse. The predecessor and fresh entries must name those same canonical bytes. The builder does not install or launch any package.

Run it from the repository root:

```powershell
pnpm acceptance:win:installed:build-kit -- `
  --release C:\release\RELEASE.json `
  --release-sha256 <sha256> `
  --provenance C:\release\artifact-provenance.json `
  --provenance-sha256 <sha256> `
  --source C:\source\talking-quill `
  --config tmp\windows-installed-acceptance\kit-input.json `
  --request-private-key C:\keys\acceptance-request.pkcs8 `
  --signer helper\target\release\talking-quill-acceptance-signer.exe `
  --signer-sha256 <sha256> `
  --output tmp\windows-installed-acceptance\kit `
  --bundle tmp\windows-installed-acceptance\kit.zip
```

The output stays below `tmp`. `evidence-input.json`, `signed-requests.json`, imported canonical files, and `bundle-manifest.json` use canonical JSON and sorted inventory paths. The manifest records each file's byte count and SHA-256. It also records the domain-separated `producerArtifactSetIdentity`, which covers source commit/tree, build ID, candidate/update/repair identities, every fault installer and layout, bootstrap/broker/signer/sender hashes, the embedded manifest and validation key, the fault-chain head, and the complete payload inventory. Only `producer-result.json` and `bundle-manifest.json` are excluded from that inventory to avoid a digest cycle. The builder writes a stored ZIP with sorted slash-separated paths, fixed regular-file mode, a 1980-01-01 timestamp, and no optional ZIP fields. It verifies the archive, extracts it into a separate directory, verifies that tree, then removes the check directory. Fixed inputs produce the same identity and ZIP bytes on repeated builds.

## Signing and authorization

Build, Electron Builder, Cargo, installer packaging, and runner children do not receive private signing environment variables. Installed-acceptance artifacts keep their bundle-bound signer identities because the bundle authorization authenticates them. Public updater release signing is separate: it accepts only a protected key path and resolves its native chain from the clean reviewed checkout. The producer publishes the broker and bootstrap below an identity-named ProgramData directory. SYSTEM and Administrators have full control, while the run user has read and execute access. Inheritance and reparse points are rejected. JavaScript passes pinned identities to the bootstrap instead of hashing and then spawning a mutable broker path. The bootstrap retains the broker file and parent directory with replacement-denying sharing, creates the broker suspended with an explicit standard-handle list, verifies the process image, and keeps the retained handles until exit. The broker applies the same checks to its signer snapshot. Before it makes the key handle inheritable, native shared key admission retains every ancestor and checks the file owner, protected DACL, exact SYSTEM, Administrators, and current-user read-only ACEs, link count, and reparse attributes. The signer repeats handle-level admission after inheritance.

The broker creates the readiness, armed, and startup pipes before it starts Electron. Electron receives NUL standard handles and only the random startup pipe name in argv. It connects outbound after the acceptance entry starts. The broker obtains the client PID from the kernel, requires the exact Electron creation identity, and writes the signed request only after that check. No startup capability handle reaches Electron or an early child. Readiness clients must be the exact child or a creation-time-valid descendant with matching user SID, logon SID, WTS session, integrity level, canonical image path, file ID, SHA-256, architecture, and source provenance. Rejected clients do not consume the endpoint or extend the absolute deadline.

Each run request lasts at most five minutes. Its issue and expiry times bind its scheduled invocation. The request authority signs `producer-result.json`, binding the artifact-set identity, source, and build ID. The outer bundle authorization must bind both that identity and the ZIP hash. Before execution, the workflow requires equality among the authorization, bundle manifest, signed producer result, and a fresh digest computed from the downloaded bundle. Installed evidence records that same digest plus the exact bundle, manifest, authorization, and producer-result hashes. A dry run verifies the build signature and every request signature but reserves no nonce. Dry-run output replaces signed requests, nonces, pipes, correlations, paths, and authorization values with hashes.

## Package classes

The candidate is an authenticated predecessor-bound update. Its manifest-signed acceptance payload contains the validation public key and exact fault-validator policy before packaging. Fault evidence verification uses that embedded key, never the artifact set's untrusted copy. The repair and all fault packages use the nondefault `installed-acceptance-repair` installer feature and carry `promotable: false` evidence. The ten fault packages cover, in order, `staged`, `prepared`, `predecessorMoved`, `publishing`, `publishedBeforePersist`, `published`, `registered`, `committed`, `legacyRetiring`, and `legacyRetired`. Normal TQPKG2 parsing rejects repair packages unless acceptance-repair inspection is explicitly enabled. Canonical setup builds do not enable this feature.

Before extraction, the checked-in verifier rejects unlisted ZIP entries, path and case collisions, links, non-normal metadata, compression, trailing data, and size, CRC, or SHA-256 mismatches. It extracts with exclusive file creation and verifies the exact manifest inventory. The workflow verifies the extracted tree again next to execution. The planner repeats that verification immediately before every artifact freeze. Plan freezing then compares every TQPKG2 path and byte with the unpacked tree, binds inner metadata to the outer release identity, verifies update authorization, and checks source-bound PE identities for the trusted launcher, acceptance broker, and synthetic sender. `outputPath` is fixed to `../evidence.json`; runtime accepts it only as `tmp/windows-installed-acceptance/evidence.json` through a canonical, no-link parent.

## Matrix order

Each faulting worker writes a nonce-bound phase audit before exit 197. The validator signs the before, faulted, and recovered observations. Those observations inventory the full test registry namespace, product processes and image hashes, V1/V2 mutexes, services, scheduled tasks, Run entries, App Paths, uninstall entries, files, journals, and recovery state. The verifier derives clean recovery and unchanged production state from those measurements; it does not accept asserted booleans.

All acceptance-only candidate probes run before canonical removal and reinstall. The sequence performs update, package and runtime checks, repair, all ten fault recoveries, diagnostics, physical observation, and supplemental synthetic observation first. It then uninstalls while preserving user data, reinstalls the exact canonical RELEASE bytes, performs external normal-readiness observation, and checks final residue.

The separate protected x64 producer workflow runs under the `release-signing` environment. It authenticates the exact artifact digest and successful x64 package job from a fresh release run without depending on that run's later acceptance-gated conclusion, imports the updater key only through stdin, removes the secret environment variable before invoking Node, and runs the non-mocked producer. Import emits a protected cleanup descriptor binding the key identity to the already-built reviewed native-chain snapshot. The descriptor also retains an identity-checked copy of the minimal native deleter, so the unconditional cleanup step neither invokes Cargo nor depends on a successful source build. Generated request, manifest, and validation keys use the same descriptor cleanup. The workflow's forced-failure input exercises cleanup after producer build failure.

The protected x64 workflow verifies the exact source, authorized bundle, candidate, native broker, validation key, phase inventory, physical observation, and residue result. It consumes the signed producer result carried by the downloaded and executed bundle; it does not perform an unrelated post-execution rebuild. The canonical gate record retains the artifact-set, bundle, manifest, outer authorization, and producer-result identities. Release assembly authenticates that workflow run and exact record before including it in signed Windows promotion evidence.
