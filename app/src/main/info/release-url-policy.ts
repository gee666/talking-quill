import releaseConfig from '../../../../release.config.json';
import { PublicAppError } from '../security/public-error';

export const RELEASE_REPOSITORY = releaseConfig.repository;
export const RELEASES_API_URL = `https://api.github.com/repos/${RELEASE_REPOSITORY}/releases/latest`;

/** Canonical privileged-boundary policy for every release URL entering or leaving the app. */
export function validateReleaseUrl(value: string, expectedTag?: string): string {
  // URL canonicalization removes an explicit default port, so reject it from the raw authority.
  if (/^https:\/\/[^/]+:\d+(?:\/|$)/iu.test(value)) throw rejected();
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw rejected();
  }
  if (value !== url.toString()) throw rejected();
  const rawPath = url.pathname.toLowerCase();
  if (
    url.protocol !== 'https:' ||
    url.hostname !== 'github.com' ||
    url.port !== '' ||
    url.username !== '' ||
    url.password !== '' ||
    url.search !== '' ||
    url.hash !== '' ||
    /%2f|%5c|\\/iu.test(rawPath) ||
    !new RegExp(`^/${escapeRegExp(RELEASE_REPOSITORY)}/releases/tag/[A-Za-z0-9._-]+$`, 'u').test(
      url.pathname,
    ) ||
    (expectedTag !== undefined &&
      url.pathname !== `/${RELEASE_REPOSITORY}/releases/tag/${expectedTag}`)
  ) {
    throw rejected();
  }
  return url.toString();
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/gu, '\\$&');
}

function rejected(): PublicAppError {
  return new PublicAppError({ code: 'FORBIDDEN', message: 'The release link was rejected.' });
}
