export const PRODUCTION_CSP = productionCsp(false);
export const CAPTURE_PRODUCTION_CSP = productionCsp(true);

export function productionCsp(allowWorkers: boolean): string {
  return [
    "default-src 'none'",
    "script-src 'self'",
    allowWorkers ? "worker-src 'self'" : "worker-src 'none'",
    "style-src 'self'",
    "img-src 'self' blob:",
    "font-src 'self'",
    "connect-src 'none'",
    "media-src 'none'",
    "object-src 'none'",
    "base-uri 'none'",
    "frame-src 'none'",
    "frame-ancestors 'none'",
    "form-action 'none'",
  ].join('; ');
}

export function developmentCsp(origin: string, allowWorkers = false): string {
  const webSocketOrigin = origin.replace(/^http/, 'ws');
  return [
    "default-src 'none'",
    "script-src 'self' 'unsafe-eval'",
    allowWorkers ? "worker-src 'self'" : "worker-src 'none'",
    "style-src 'self' 'unsafe-inline'",
    "img-src 'self' blob:",
    `connect-src 'self' ${origin} ${webSocketOrigin}`,
    "font-src 'self'",
    "object-src 'none'",
    "base-uri 'none'",
    "frame-src 'none'",
    "frame-ancestors 'none'",
    "form-action 'none'",
  ].join('; ');
}
