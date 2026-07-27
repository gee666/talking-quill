const { writeFileSync } = require('node:fs');

const generationArgument = process.argv.find((value) => value.startsWith('--test-generation='));
const markerArgument = process.argv.find((value) => value.startsWith('--test-marker='));
const crashMarkerArgument = process.argv.find((value) => value.startsWith('--test-crash-marker='));
const generation = Number(generationArgument?.slice('--test-generation='.length));
const marker = markerArgument?.slice('--test-marker='.length);
const crashMarker = crashMarkerArgument?.slice('--test-crash-marker='.length);

process.parentPort.postMessage({
  version: 2,
  requestId: 'worker-ready',
  ok: true,
  result: { type: 'ready', networkGuarded: true, networkProbeCompleted: false },
});

process.parentPort.on('message', (event) => {
  const request = event.data;
  if (request?.type === 'transcribe') {
    if (generation === 1) {
      if (marker) writeFileSync(marker, 'active\n');
      setInterval(() => undefined, 1_000);
      return;
    }
    const firstSample = new Float32Array(request.pcm)[0] ?? 0;
    if (generation === 2 && firstSample > 0.25) {
      if (crashMarker) writeFileSync(crashMarker, 'active\n');
      setInterval(() => undefined, 1_000);
      return;
    }
    respond(request.requestId, {
      type: 'transcription',
      value: {
        text: 'healthy replacement',
        modelId: request.options.modelId,
        durationMs: 1,
        pipeline: { loadCount: 1, reused: generation > 1, loadDurationMs: 1 },
      },
    });
    return;
  }
  if (request?.type === 'shutdown' || request?.type === 'unload') {
    respond(request.requestId, { type: 'acknowledged', operation: request.type });
    if (request.type === 'shutdown') setImmediate(() => process.exit(0));
  }
});

function respond(requestId, result) {
  process.parentPort.postMessage({ version: 2, requestId, ok: true, result });
}
