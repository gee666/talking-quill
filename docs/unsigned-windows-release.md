# Ordinary unsigned Windows release

Dispatch **Build Windows x64 and ARM64 native setup release candidates** on `master`. There are no inputs, repository secrets, protected environments or baseline-workflow prerequisites. Versions must agree across the source packages.

The workflow runs source validation and security audits, builds the current fresh Windows x64 and ARM64 native installers, inspects packages, preserves provenance and runs cancellation smoke tests on architecture-native hosted runners. Ordinary assembly consumes the stage script's four-file output, including `RELEASE.json`, without passing it through the incompatible signed/update assembler.

A successful run automatically starts **Publish Windows x64 and ARM64 unsigned owner installers**. Only that publisher has `contents: write`. It authenticates the canonical repository's successful default-branch producer, requires its commit to remain the current `master` tip, and downloads artifacts from that exact run. It binds the manifest to the producer, rejects an existing release/tag, uploads a draft, downloads and byte-verifies all assets, then publishes and verifies the public latest release and tag commit. The publisher rechecks the current tip before creating and publishing the draft.

Only the automatically supplied `GITHUB_TOKEN` is needed. GitHub Actions must be enabled and repository policy must permit the publisher's declared write permission. Both Windows hosted runner architectures must be available.

The release is unsigned. It includes hashes, provenance and automated native smoke results, not a signed publication envelope or update feed. It does not claim real-reboot, migration or protected installed-acceptance checks passed. The separate signed/manual tools remain available but are not prerequisites for this ordinary fresh-install release.

Use a new coordinated version for a new release. Failed publication may leave a draft for inspection; existing releases and tags are never overwritten.
