import { net, protocol, type Protocol } from 'electron';
import { existsSync } from 'node:fs';
import { extname, resolve, sep } from 'node:path';
import { pathToFileURL } from 'node:url';
import { APP_PROTOCOL } from '../../shared/constants/app';
import { CAPTURE_PRODUCTION_CSP, PRODUCTION_CSP } from './csp';

const ACTIVE_PROTOCOL_HANDLERS = new WeakMap<Protocol, () => void>();

const MIME_TYPES: Readonly<Record<string, string>> = Object.freeze({
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.woff2': 'font/woff2',
});

export function registerPrivilegedScheme(): void {
  protocol.registerSchemesAsPrivileged([
    {
      scheme: APP_PROTOCOL,
      privileges: {
        standard: true,
        secure: true,
        supportFetchAPI: true,
        corsEnabled: false,
        stream: true,
      },
    },
  ]);
}

export function installApplicationProtocol(
  rendererRoot: string,
  target: Protocol = protocol,
  allowWorkers = false,
): () => void {
  ACTIVE_PROTOCOL_HANDLERS.get(target)?.();
  const normalizedRoot = resolve(rendererRoot);
  const contentSecurityPolicy = allowWorkers ? CAPTURE_PRODUCTION_CSP : PRODUCTION_CSP;
  target.handle(APP_PROTOCOL, async (request) => {
    try {
      const url = new URL(request.url);
      if (url.hostname !== 'app' || url.username !== '' || url.password !== '') return denied();
      const decoded = decodeURIComponent(url.pathname);
      if (decoded.includes('\\') || decoded.includes('\0')) return denied();
      const candidate = resolve(normalizedRoot, `.${decoded}`);
      if (!candidate.startsWith(`${normalizedRoot}${sep}`) || !existsSync(candidate))
        return denied();
      const source = await net.fetch(pathToFileURL(candidate).toString());
      const headers = new Headers(source.headers);
      headers.set('Content-Security-Policy', contentSecurityPolicy);
      headers.set('Cross-Origin-Opener-Policy', 'same-origin');
      headers.set('X-Content-Type-Options', 'nosniff');
      headers.set('Referrer-Policy', 'no-referrer');
      headers.set(
        'Content-Type',
        MIME_TYPES[extname(candidate).toLowerCase()] ?? 'application/octet-stream',
      );
      return new Response(source.body, { status: source.status, headers });
    } catch {
      return denied();
    }
  });
  const dispose = (): void => {
    if (ACTIVE_PROTOCOL_HANDLERS.get(target) !== dispose) return;
    ACTIVE_PROTOCOL_HANDLERS.delete(target);
    target.unhandle(APP_PROTOCOL);
  };
  ACTIVE_PROTOCOL_HANDLERS.set(target, dispose);
  return dispose;
}

function denied(): Response {
  return new Response('Not found', {
    status: 404,
    headers: { 'Content-Type': 'text/plain; charset=utf-8' },
  });
}
