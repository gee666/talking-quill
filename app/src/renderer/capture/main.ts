import workletModuleUrl from './capture.worklet.ts?worker&url';
import { CaptureEngine, createBrowserCaptureEnvironment } from './capture-engine';
import { CapturePortController, readCapturePortEvent } from './capture-port';

let controller: CapturePortController | null = null;
let engine: CaptureEngine | null = null;

const onPort = (event: MessageEvent<unknown>) => {
  const port = readCapturePortEvent(event);
  if (port === null) return;
  controller?.close();
  engine?.disposeImmediately();
  controller = null;
  engine = null;

  let nextController: CapturePortController | null = null;
  const nextEngine = new CaptureEngine(createBrowserCaptureEnvironment(workletModuleUrl), {
    onDevicesChanged: (devices) => nextController?.notifyDevicesChanged(devices),
    onFrame: (samples, rms) => nextController?.notifyFrame(samples, rms),
    onUnexpectedStop: (reason) => nextController?.notifyUnexpectedStop(reason),
  });
  nextController = new CapturePortController(port, nextEngine);
  engine = nextEngine;
  controller = nextController;
};

window.addEventListener('message', onPort);
window.addEventListener('pagehide', () => {
  window.removeEventListener('message', onPort);
  controller?.close();
  controller = null;
  engine?.disposeImmediately();
  engine = null;
});

void window.talkingQuillCapture.ready().then(() => {
  document.documentElement.dataset.ready = 'true';
});
