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
- a P-256 request-signing key in PKCS8 DER form, with one link and no linked/reparse ancestors;
- the native `talking-quill-acceptance-signer` executable and its adjacent `talking-quill-windows-acceptance-broker` executable.

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

The output stays below `tmp`. `evidence-input.json`, `signed-requests.json`, imported canonical files, and `bundle-manifest.json` use canonical JSON and sorted inventory paths. The manifest records each file's byte count and SHA-256. The builder writes a stored ZIP with sorted slash-separated paths, fixed regular-file mode, a 1980-01-01 timestamp, and no optional ZIP fields. It verifies the archive, extracts it into a separate directory, verifies that tree, then removes the check directory. Fixed build ID, run window, invocation payloads, and explicit ordered nonces produce the same ZIP bytes on repeated builds.

## Signing and authorization

Build, Electron Builder, Cargo, installer packaging, and runner children do not receive private signing environment variables. The JavaScript orchestrator never reads private key bytes and never launches the signer directly. It sends a typed request to the Rust acceptance broker. The broker opens the key and signer with delete sharing disabled, copies the signer by retained handle into an ACL-restricted no-reparse directory, and creates it suspended with an explicit handle list. It compares the child kernel image path, file identity, byte count, SHA-256, parent, and creation time before resuming it. The signer seals the inherited key handle against further inheritance before parsing PKCS8. RustCrypto P-256 produces RFC6979 deterministic ECDSA, and JavaScript accepts only the complete typed broker result.

The same broker owns packaged probes. It creates first-instance, local-only named pipes under a logon-SID DACL before starting the app. It passes the signed request through one anonymous startup handle and starts the expected image suspended with an explicit handle list and a kill-on-close job. Before reading any response bytes, it obtains the pipe client PID from the kernel. It admits only the exact child or a creation-time-valid descendant with matching user SID, logon SID, WTS session, integrity level, canonical image path, file ID, SHA-256, architecture, and source commit/tree. It disconnects rejected clients and keeps accepting until the absolute deadline.

Each run request lasts at most five minutes. Its issue and expiry times bind its scheduled invocation. A dry run verifies the build signature and every request signature but reserves no nonce. Dry-run output replaces signed requests, nonces, pipes, correlations, paths, and authorization values with hashes.

## Package classes

The candidate is an authenticated predecessor-bound update. The repair and all fault packages use the nondefault `installed-acceptance-repair` installer feature and carry `promotable: false` evidence. The ten fault packages cover, in order, `staged`, `prepared`, `predecessorMoved`, `publishing`, `publishedBeforePersist`, `published`, `registered`, `committed`, `legacyRetiring`, and `legacyRetired`. Normal TQPKG2 parsing rejects repair packages unless acceptance-repair inspection is explicitly enabled. Canonical setup builds do not enable this feature.

Before extraction, the checked-in verifier rejects unlisted ZIP entries, path and case collisions, links, non-normal metadata, compression, trailing data, and size, CRC, or SHA-256 mismatches. It extracts with exclusive file creation and verifies the exact manifest inventory. The workflow verifies the extracted tree again next to execution. The planner repeats that verification immediately before every artifact freeze. Plan freezing then compares every TQPKG2 path and byte with the unpacked tree, binds inner metadata to the outer release identity, verifies update authorization, and checks source-bound PE identities for the trusted launcher, acceptance broker, and synthetic sender. `outputPath` is fixed to `../evidence.json`; runtime accepts it only as `tmp/windows-installed-acceptance/evidence.json` through a canonical, no-link parent.

## Matrix order

All acceptance-only candidate probes run before canonical removal and reinstall. The sequence performs update, package and runtime checks, repair, all ten fault recoveries, diagnostics, physical observation, and supplemental synthetic observation first. It then uninstalls while preserving user data, reinstalls the exact canonical RELEASE bytes, performs external normal-readiness observation, and checks final residue.
