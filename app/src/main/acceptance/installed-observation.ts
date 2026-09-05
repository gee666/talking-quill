import net from 'node:net';
import { runPhysicalObservation } from './installed-physical-observation';
import { runReadinessObservation } from './installed-readiness-observation';
import type {
  InstalledAcceptanceHelper,
  InstalledObservationRequest,
  InstalledObservationContext,
} from './installed-observation-types';

export {
  EndpointObservabilitySchema,
  PauseLeaseRenewalSchema,
} from './installed-observation-schema';
export type {
  InstalledAcceptanceHelper,
  InstalledObservationRequest,
  InstalledObservationContext,
} from './installed-observation-types';

export async function runInstalledObservation(
  helper: InstalledAcceptanceHelper,
  request: InstalledObservationRequest,
  context: InstalledObservationContext,
): Promise<void> {
  if (request.physicalObservation || request.automationValidation) {
    await runPhysicalObservation(helper, request, context, writePipe);
    return;
  }
  await runReadinessObservation(helper, request, context, writePipe);
}

function writePipe(pipeName: string, value: unknown): Promise<void> {
  const evidence =
    value !== null && typeof value === 'object'
      ? { ...value, runtimeLifecycleAuthoritative: false }
      : value;
  return new Promise((resolveWrite, reject) => {
    const socket = net.connect(pipeName);
    socket.once('error', reject);
    socket.once('connect', () => socket.end(`${JSON.stringify(evidence)}\n`, resolveWrite));
  });
}
