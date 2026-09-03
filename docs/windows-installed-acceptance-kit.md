# Windows installed-acceptance kit

The installed-acceptance kit is nonpromotable test material for Windows 0.0.69. It does not replace the canonical installer and must never be published as a release asset.

## Inputs

`tmp/build-windows-installed-acceptance-kit.mjs` requires:

- the canonical `RELEASE.json` and its independently recorded SHA-256;
- the directory containing the exact canonical installer named by `RELEASE.json`;
- a Git object database containing the descriptor's full source commit and tree;
- a kit input JSON with the frozen candidate update, canonical predecessor and reinstall, acceptance repair, trusted launcher, synthetic sender, and ten fault packages;
- a P-256 request-signing key in PKCS8 DER form.

The builder accepts only version 0.0.69, a canonical fresh RELEASE, and x64 or ARM64. It verifies the descriptor hash, installer size and hash, full TQPKG2 identity, source commit/tree, and exact canonical installer reuse. The predecessor and fresh entries must name those same canonical bytes. The builder does not install or launch any package.

Run it from the repository root:

```powershell
node tmp/build-windows-installed-acceptance-kit.mjs `
  --release C:\release\RELEASE.json `
  --release-sha256 <sha256> `
  --source C:\source\talking-quill `
  --config tmp\windows-installed-acceptance\kit-input.json `
  --request-private-key C:\keys\acceptance-request.pkcs8 `
  --output tmp\windows-installed-acceptance\kit
```

The output stays below `tmp`. `evidence-input.json`, `signed-requests.json`, imported canonical files, and `bundle-manifest.json` use canonical JSON and sorted inventory paths. The manifest records each file's byte count and SHA-256. Recheck it before moving the kit to an acceptance machine.

## Signing and authorization

Build, Electron Builder, Cargo, installer packaging, and runner children do not receive private signing environment variables. The kit builder passes one payload and one private key over stdin to `scripts/windows-installed-acceptance-signer.mjs`. That subprocess has a minimal environment and returns only the public key and signature envelope.

Each run request lasts at most five minutes. Its issue and expiry times bind its scheduled invocation. A dry run verifies the build signature and every request signature but reserves no nonce. Dry-run output replaces signed requests, nonces, pipes, correlations, paths, and authorization values with hashes.

## Package classes

The candidate is an authenticated predecessor-bound update. The repair and all fault packages use the nondefault `installed-acceptance-repair` installer feature and carry `promotable: false` evidence. The ten fault packages cover, in order, `staged`, `prepared`, `predecessorMoved`, `publishing`, `publishedBeforePersist`, `published`, `registered`, `committed`, `legacyRetiring`, and `legacyRetired`. Normal TQPKG2 parsing rejects repair packages unless acceptance-repair inspection is explicitly enabled. Canonical setup builds do not enable this feature.

Before execution, plan freezing compares every TQPKG2 path and byte with the unpacked tree, rejects links and extra files, binds inner metadata to the outer release identity, verifies update authorization, and checks source-bound PE identities for the trusted launcher and synthetic sender.

## Matrix order

All acceptance-only candidate probes run before canonical removal and reinstall. The sequence performs update, package and runtime checks, repair, all ten fault recoveries, diagnostics, physical observation, and supplemental synthetic observation first. It then uninstalls while preserving user data, reinstalls the exact canonical RELEASE bytes, performs external normal-readiness observation, and checks final residue.
