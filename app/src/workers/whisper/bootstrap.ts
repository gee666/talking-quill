import { createRequire } from 'node:module';
import { installWorkerNetworkGuard, verifyWorkerNetworkGuardForTest } from './network-guard';

installWorkerNetworkGuard();

void startPayload();

async function startPayload(): Promise<void> {
  try {
    if (process.argv.includes('--network-guard-probe')) {
      await verifyWorkerNetworkGuardForTest();
    }
    createRequire(__filename)('./whisper-payload.cjs');
  } catch {
    process.stderr.write('Whisper worker bootstrap failed.\n');
    process.exit(1);
  }
}
