# Unsigned Windows release CI

Run **Build Windows x64 and ARM64 native setup release candidates** on the protected default branch. The dispatch has no inputs. The version comes from the coordinated application source, not a fixed tag.

A successful run automatically starts **Publish Windows x64 and ARM64 unsigned owner installers** through `workflow_run`. The publisher checks the producing workflow, repository, default branch, commit and successful conclusion before downloading its artifacts. It creates a draft, verifies the remote bytes, then publishes an immutable stable GitHub release. The publisher's manual dispatch remains available for retrying publication of a successful candidate run with its exact tag.

Source validation, security audits, package inspection, provenance, architecture-native UI smoke tests, fresh-install lifecycle tests and the protected local migration baseline checks remain required. Signed publication inventories, attestations, rollback checks and protected publication controls also remain in place.

The release no longer requires x64/ARM64 real-reboot run IDs or an x64 protected installed-acceptance run ID. Those workflows remain separate diagnostics. CI does not manufacture their evidence or claim they passed. `windows-release-validation.json` inventories the automated lifecycle evidence and explicitly records real reboot and protected installed acceptance as `not-collected`. The signed publication envelope's existing `promotionEvidenceSha256` field binds this report for fresh releases, preserving the installed client's envelope format. Historical signed lifecycle evidence remains supported by the publication tooling.

## Repository prerequisites

Input-free dispatch does not bypass protected environments. The existing `release-trust`, `release-signing`, `release-publication` and `windows-local-migration-trust` configuration, required approvals, control-plane token, publication-manifest signing key, local 0.0.67 baseline variables and runner availability still apply. The promotion-evidence signing key is no longer required by release-control preflight. Other existing release-control checks were not relaxed. A missing secret, baseline, approval or immutable-release setting still fails closed.

Use a new coordinated version before publishing a new release. Rerunning publication does not overwrite an existing tag or release.
