import { mkdir, open, rm } from 'node:fs/promises';
import { resolve } from 'node:path';
import { canonicalAcceptanceJson } from './windows-installed-acceptance-probe.mjs';

export async function reserveAcceptanceRequestNonces(stableTempRoot, requests) {
  const reserved = [];
  try {
    for (const request of requests) {
      const record = acceptanceReservationRecord(request.payload);
      const paths = acceptanceReplayLedgerPaths(
        stableTempRoot,
        record.buildId,
        record.requestNonce,
      );
      await mkdir(paths.buildRoot, { recursive: true, mode: 0o700 });
      if (await exists(paths.consumed)) {
        throw new Error(`Acceptance request nonce was already consumed: ${request.invocationId}`);
      }
      let handle;
      try {
        handle = await open(paths.reserved, 'wx', 0o600);
      } catch (error) {
        if (error?.code === 'EEXIST') {
          throw new Error(`Acceptance request nonce was already reserved: ${request.invocationId}`);
        }
        throw error;
      }
      reserved.push(paths.reserved);
      try {
        await handle.writeFile(`${canonicalAcceptanceJson(record)}\n`, 'utf8');
        await handle.sync();
      } finally {
        await handle.close();
      }
      if (await exists(paths.consumed)) {
        throw new Error(
          `Acceptance request nonce was concurrently consumed: ${request.invocationId}`,
        );
      }
    }
    return Object.freeze({ reservedCount: reserved.length });
  } catch (error) {
    await Promise.all(reserved.map((path) => rm(path, { force: true })));
    throw error;
  }
}

export function acceptanceReservationRecord(payload) {
  return Object.freeze({
    version: 1,
    buildId: payload.buildId,
    requestNonce: payload.requestNonce,
    invocationId: payload.invocationId,
    runWindow: payload.runWindow,
    latestStartOffsetMs: payload.latestStartOffsetMs,
    deadlineOffsetMs: payload.deadlineOffsetMs,
    requestExpiresAtMs: payload.expiresAtMs,
  });
}

export function acceptanceReplayLedgerPaths(stableTempRoot, buildId, requestNonce) {
  const buildRoot = resolve(
    stableTempRoot,
    'TalkingQuillInstalledAcceptanceNonceLedger',
    'v1',
    buildId,
  );
  return Object.freeze({
    buildRoot,
    reserved: resolve(buildRoot, `${requestNonce}.reserved.json`),
    consumed: resolve(buildRoot, `${requestNonce}.consumed.json`),
  });
}

async function exists(path) {
  try {
    const handle = await open(path, 'r');
    await handle.close();
    return true;
  } catch (error) {
    if (error?.code === 'ENOENT') return false;
    throw error;
  }
}
