# Verify a release

Use only assets from `https://github.com/gee666/talking-quill/releases`. `release-manifest.json` binds the source commit, tag, version, four platform provenance records, notices, and six installers/archives. `SHA256SUMS.txt` covers every uploaded asset—including provenance, notices, and the manifest—except itself.

Compare downloads with `SHA256SUMS.txt` (`Get-FileHash -Algorithm SHA256` on Windows or `shasum -a 256` on macOS). Reject missing, extra, duplicate, or mismatched assets.

On Windows, Properties → Digital Signatures must show the approved trusted publisher and timestamp. Independently run `Get-AuthenticodeSignature` or `signtool verify /pa /all /v` on the installer, installed `Talking Quill.exe`, and bundled `resources/helper/talking-quill-helper.exe`.

On macOS, verify before opening:

```text
codesign --verify --deep --strict --verbose=4 "/Applications/Talking Quill.app"
codesign -dv --verbose=4 "/Applications/Talking Quill.app"
spctl --assess --type execute --verbose=4 "/Applications/Talking Quill.app"
xcrun stapler validate "/Applications/Talking Quill.app"
xcrun stapler validate "Talking-Quill-1.0.2-mac-arm64.dmg"
```

Release CI additionally fails unless the Developer ID team, hardened-runtime flag, exact app/helper entitlements, bundle metadata, single target architecture, Gatekeeper assessment, and stapled app/DMG match approved configuration. A pinned per-file signing hook preserves Electron's reviewed JIT entitlement for Electron helpers while overriding the exact Rust helper path with an empty entitlement set; entitlement extraction errors fail closed.

Release candidate and publication workflows may be dispatched only from the canonical repository's protected default branch. Before candidate code or package installation runs, release control queries GitHub and fails closed unless that branch is protected and the `release-trust`, `release-signing`, and `release-publication` environments each require reviewers and protected branches. Every checkout disables credential persistence.

A real candidate requires strict semver, synchronized versions, a signed annotated tag contained by the protected default branch, and an approved public key and fingerprint supplied by the protected environment. The candidate's release-policy Git tree must match the independently configured `RELEASE_POLICY_TREE_SHA256` value. After the approved policy commit is on the default branch, calculate that value with `node scripts/verify-trusted-candidate.mjs --print-protected-hashes .`; review it independently before configuring the protected variable. `RELEASE_CONTROL_TOKEN` must be a read-only control-plane token able to read repository branch and environment configuration. Candidate generation additionally requires `RELEASE_TAG_SIGNING_PUBLIC_KEY` and `RELEASE_TAG_SIGNER_FINGERPRINTS`.

Fake-media widget evidence uses a separately named, directory-only `Talking Quill Packaged Test` composition under `tmp/`. It adds only the test activation composition needed to start deterministic capture and uses an isolated test profile. It is never an installer or release candidate. After that evidence, CI rebuilds the production package from production mode; package inspection rejects the test-composition chunk. Therefore this evidence proves that real Chromium fake media traverses the packaged production capture/worklet/session/widget implementation, but it does **not** claim that a production installer exposes a test activation driver.

Provider inventory contains 38 adapters. Pi evidence checks executable discovery, bounded capability probes, direct/application `--list-models` parity, fixed argv, stdin-only prompts, cancellation, timeout, and process-tree cleanup. No credentialed or billable provider call belongs in an uncredentialed gate.

Manual dispatch defaults to a deterministic, credential-free `dry_run`. It validates and packages unsigned artifacts without entering a protected signing environment or mutating GitHub Releases. A real run must first pass the same unsigned matrix; a separate `release-signing` job then rebuilds, signs, verifies, assembles, and attests the exact candidate assets before creating an authenticated draft.

Publication is a separate `publish-release.yml` dispatch through `release-publication`. Set `RELEASE_EXTERNAL_BLOCKERS_APPROVAL` to the independently approved exact `vMAJOR.MINOR.PATCH@40-character-commit` only after all manual, provider, legal, packet-capture, removal, and performance blockers are cleared. Publication re-verifies the protected key, signer, and policy hash, downloads the draft twice, checks every artifact attestation and byte, publishes it, and retries the public `/releases/latest` endpoint until that endpoint exposes the exact commit and asset set. A draft is intentionally absent from `/releases/latest`.
