const { app, utilityProcess } = require('electron');
const { mkdirSync } = require('node:fs');
const { resolve } = require('node:path');

const cache = resolve('tmp', 'tests', 'worker-empty-cache');
const workerPath = resolve('app', 'out', 'workers', 'whisper-bootstrap.cjs');
mkdirSync(cache, { recursive: true });

app.whenReady().then(async () => {
  try {
    await terminateAfterHandshake();
    const first = await probeMissingModel('offline-proof-1');
    const second = await probeMissingModel('offline-proof-2');
    if (first !== 'MODEL_MISSING' || second !== 'MODEL_MISSING') {
      throw new Error(`Unexpected probe results: ${first}, ${second}`);
    }
    console.log(
      'Real utility worker blocked every probed network API before importing ONNX, then rejected missing local models offline across fresh generations',
    );
    app.exit(0);
  } catch (error) {
    console.error(error);
    app.exit(1);
  }
});

function terminateAfterHandshake() {
  return withWorker((worker, complete) => {
    worker.on('message', (response) => {
      if (response?.requestId !== 'worker-ready') return;
      if (
        response.result?.networkGuarded !== true ||
        response.result?.networkProbeCompleted !== true
      ) {
        complete(new Error('Worker became ready without completing the bootstrap network probe.'));
        return;
      }
      complete(undefined);
    });
  }, true);
}

function probeMissingModel(requestId) {
  return withWorker((worker, complete) => {
    worker.on('message', (response) => {
      if (response?.requestId === 'worker-ready') {
        if (response.result?.networkGuarded !== true) {
          complete(new Error('Worker became ready without the pre-import network guard.'));
          return;
        }
        worker.postMessage({
          version: 1,
          requestId,
          type: 'model-check',
          modelId: 'Xenova/whisper-small',
        });
        return;
      }
      if (response?.requestId !== requestId) return;
      if (
        response.ok !== false ||
        !['MODEL_MISSING', 'MODEL_CORRUPT'].includes(response.error?.code)
      ) {
        complete(new Error(`Unexpected worker response: ${JSON.stringify(response)}`));
        return;
      }
      const code = response.error.code;
      worker.kill();
      complete(undefined, code);
    });
  });
}

function withWorker(run, networkProbe = false) {
  return new Promise((resolvePromise, reject) => {
    const worker = utilityProcess.fork(
      workerPath,
      [`--model-cache=${cache}`, ...(networkProbe ? ['--network-guard-probe'] : [])],
      {
        stdio: ['ignore', 'pipe', 'pipe'],
      },
    );
    let diagnostics = '';
    let settled = false;
    for (const stream of [worker.stdout, worker.stderr]) {
      stream?.on('data', (chunk) => {
        diagnostics = `${diagnostics}${chunk.toString()}`.slice(-8_192);
      });
    }
    const timer = setTimeout(() => complete(new Error(`Worker timed out\n${diagnostics}`)), 20_000);
    worker.on('exit', (code) => {
      if (!settled && code !== 0)
        complete(new Error(`Worker exited (${String(code)})\n${diagnostics}`));
    });
    const complete = (error, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      worker.kill();
      if (error) reject(error);
      else resolvePromise(value);
    };
    run(worker, complete);
  });
}
