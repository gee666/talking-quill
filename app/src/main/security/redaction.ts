import { isIP } from 'node:net';

const REDACTED = '[REDACTED]';
const SENSITIVE_KEY =
  /(?:authorization|proxy-authorization|cookie|set-cookie|api[-_]?key|token|secret|password|credential|body|transcript|prompt|input|output|response|endpoint|url|uri|host|hostname|address|\bip\b)/i;
const URL_PATTERN = /\b(?:https?|wss?):\/\/[^\s"'<>]+/gi;
const BEARER_PATTERN = /\b(?:bearer|basic)\s+[a-z0-9._~+/=-]+/gi;
const IPV4_PATTERN = /\b(?:\d{1,3}\.){3}\d{1,3}\b/g;
const BRACKETED_IPV6_PATTERN = /\[[a-z0-9:.%_-]*:[a-z0-9:.%_-]*\]/gi;
const BARE_IPV6_PATTERN = /(?<![a-z0-9])(?:[a-f0-9]{0,4}:){2,}[a-z0-9:.%_-]*(?![a-z0-9])/gi;
const HOST_PATTERN =
  /\b(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?)\b/gi;
const NETWORK_ERROR_HOST_PATTERN =
  /\b((?:getaddrinfo\s+)?(?:ENOTFOUND|EAI_AGAIN|ECONNREFUSED|EHOSTUNREACH|ENETUNREACH))\s+([a-z0-9._-]+)(?::\d+)?/gi;

export function redactText(value: string, secrets: readonly string[] = []): string {
  let redacted = value;
  const orderedSecrets = [...new Set(secrets.filter((secret) => secret.length > 0))].sort(
    (left, right) => right.length - left.length,
  );
  for (const secret of orderedSecrets) redacted = redacted.split(secret).join(REDACTED);
  return redacted
    .replace(NETWORK_ERROR_HOST_PATTERN, '$1 [REDACTED]')
    .replace(URL_PATTERN, REDACTED)
    .replace(BEARER_PATTERN, REDACTED)
    .replace(IPV4_PATTERN, REDACTED)
    .replace(BRACKETED_IPV6_PATTERN, (candidate) =>
      isIpv6Token(candidate.slice(1, -1)) ? REDACTED : candidate,
    )
    .replace(BARE_IPV6_PATTERN, (candidate) => (isIpv6Token(candidate) ? REDACTED : candidate))
    .replace(HOST_PATTERN, REDACTED);
}

export function redactSensitive(value: unknown, secrets: readonly string[] = []): unknown {
  return redactValue(value, secrets, new WeakSet());
}

function redactValue(value: unknown, secrets: readonly string[], seen: WeakSet<object>): unknown {
  if (typeof value === 'string') return redactText(value, secrets);
  if (value === null || typeof value !== 'object') return value;
  if (seen.has(value)) return REDACTED;
  seen.add(value);
  if (Array.isArray(value)) return value.map((item) => redactValue(item, secrets, seen));
  if (value instanceof Error) {
    return Object.freeze({ name: 'Error', message: redactText(value.message, secrets) });
  }
  const result: Record<string, unknown> = {};
  for (const [key, item] of Object.entries(value)) {
    result[key] = SENSITIVE_KEY.test(key) ? REDACTED : redactValue(item, secrets, seen);
  }
  return result;
}

function isIpv6Token(value: string): boolean {
  const withoutZone = value.split('%', 1)[0] ?? value;
  return isIP(withoutZone) === 6;
}
