# Real q8 Whisper integration

Run from the repository root:

```text
pnpm test:whisper-real
```

The harness uses the production `ModelManager`, production `WhisperWorkerClient`, bundled utility
worker, pinned `Xenova/whisper-small` revision, and committed realistic speech fixtures.

On the first run, the production manager downloads all seven pinned manifest artifacts into the
ignored seed cache:

```text
tmp/whisper-real-cache/models/
```

Every run then starts a file-backed local HTTP Range server, preseeds a partial artifact, and uses a
second production `ModelManager` to resume and publish a fresh revision under
`tmp/whisper-real-run/`. The actual worker/client stack transcribes from that fresh publication.
The run directory is removed afterward; the seed remains reusable by CI caching. No weight is ever
written to a tracked path or packaged resource.

The test checks:

- a real Range-resumed download, complete seven-file verification, and atomic publication;
- the executable bootstrap network probe before loading the Transformers/ONNX payload;
- worker-side authoritative SHA-256 verification and offline-only pipeline load;
- short-fixture key content;
- one cold run and three warm runs;
- stable pipeline metadata (`loadCount === 1`, warm `reused === true`);
- strictly faster median warm runtime;
- production streaming through 30-second windows, 5-second left/right overlap and 20-second hops;
- each boundary anchor (`alpha`, `bravo`, `charlie`, `delta` boundary marker) exactly once.

This is an explicitly network-dependent real-model lane only when the seed cache is absent. A
missing seed triggers the real pinned model download, and any download or inference failure fails
the command; it never silently skips.
