import { z } from 'zod';
import { UpdateCheckResultSchema, type UpdateCheckResult } from '../../shared/schemas/info';
import {
  PinnedJsonTransport,
  type JsonTransport,
  type JsonTransportResponse,
} from '../providers/json-transport';
import { PublicAppError } from '../security/public-error';
import { ProviderError } from '../providers/errors';
import { RELEASE_REPOSITORY, RELEASES_API_URL, validateReleaseUrl } from './release-url-policy';

const ReleaseSchema = z.looseObject({
  tag_name: z.string().min(1).max(64),
  html_url: z.url().max(2_048),
  draft: z.boolean(),
  prerelease: z.boolean(),
});

export class UpdateService {
  readonly #transport: JsonTransport;

  constructor(transport: JsonTransport = new PinnedJsonTransport()) {
    this.#transport = transport;
  }

  async check(currentVersion: string, signal: AbortSignal): Promise<UpdateCheckResult> {
    const normalizedCurrent = normalizeVersion(currentVersion);
    let response: JsonTransportResponse;
    try {
      response = await this.#transport.request({
        url: RELEASES_API_URL,
        method: 'GET',
        headers: {
          accept: 'application/vnd.github+json',
          'user-agent': `Talking-Quill/${normalizedCurrent}`,
          'x-github-api-version': '2022-11-28',
        },
        credentialed: false,
        fixedCloud: true,
        allowedOrigins: ['https://api.github.com'],
        signal,
        timeoutMs: 10_000,
        maxResponseBytes: 256 * 1024,
      });
    } catch (error: unknown) {
      if (error instanceof ProviderError && error.code === 'MODEL_NOT_FOUND') {
        throw new PublicAppError({
          code: 'UNAVAILABLE',
          message: `No public GitHub release exists for ${RELEASE_REPOSITORY}.`,
        });
      }
      throw error;
    }
    const release = ReleaseSchema.parse(response.body);
    if (release.draft || release.prerelease) {
      throw new PublicAppError({ code: 'UNAVAILABLE', message: 'No stable release is available.' });
    }
    const latestVersion = normalizeVersion(release.tag_name);
    let releaseUrl: string;
    try {
      releaseUrl = validateReleaseUrl(release.html_url, release.tag_name);
    } catch {
      throw new PublicAppError({ code: 'UNAVAILABLE', message: 'The release link was invalid.' });
    }
    return UpdateCheckResultSchema.parse({
      status: compareVersions(latestVersion, normalizedCurrent) > 0 ? 'available' : 'current',
      currentVersion: normalizedCurrent,
      latestVersion,
      releaseUrl,
    });
  }
}

export function normalizeVersion(value: string): string {
  const match = /^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/u.exec(value.trim());
  if (match === null)
    throw new PublicAppError({ code: 'UNAVAILABLE', message: 'The release version was invalid.' });
  return match.slice(1).join('.');
}

export function compareVersions(left: string, right: string): number {
  const a = normalizeVersion(left).split('.').map(BigInt);
  const b = normalizeVersion(right).split('.').map(BigInt);
  for (let index = 0; index < 3; index += 1) {
    const leftPart = a[index] ?? 0n;
    const rightPart = b[index] ?? 0n;
    if (leftPart !== rightPart) return leftPart > rightPart ? 1 : -1;
  }
  return 0;
}
