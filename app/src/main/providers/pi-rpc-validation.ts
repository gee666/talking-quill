import { ProviderError } from './errors';

export function exactKeys(
  record: Readonly<Record<string, unknown>>,
  keys: readonly string[],
): boolean {
  const actual = Object.keys(record).sort();
  const expected = [...keys].sort();
  return actual.length === expected.length && actual.every((key, index) => key === expected[index]);
}

export function optionalKeys(
  record: Readonly<Record<string, unknown>>,
  required: readonly string[],
  optional: readonly string[],
): boolean {
  const keys = Object.keys(record);
  return (
    required.every((key) => keys.includes(key)) &&
    keys.every((key) => [...required, ...optional].includes(key))
  );
}

export function onlyKeys(
  record: Readonly<Record<string, unknown>>,
  allowed: readonly string[],
): boolean {
  return Object.keys(record).every((key) => allowed.includes(key));
}

export function validStringArray(
  value: unknown,
  maximumItems: number,
  maximumLength: number,
): boolean {
  return (
    Array.isArray(value) &&
    value.length <= maximumItems &&
    value.every((item) => typeof item === 'string' && item.length <= maximumLength)
  );
}

export function validOptionalText(value: unknown, maximum: number): boolean {
  return value === undefined || (typeof value === 'string' && value.length <= maximum);
}

export function validOptionalTimeout(value: unknown): boolean {
  return value === undefined || validPositiveInteger(value, 10 * 60_000);
}

export function validPositiveInteger(value: unknown, maximum: number): value is number {
  return Number.isInteger(value) && Number(value) >= 0 && Number(value) <= maximum;
}

export function validIndex(value: unknown): value is number {
  return Number.isInteger(value) && Number(value) >= 0 && Number(value) <= 255;
}

export function validText(value: unknown, maximum: number, allowEmpty: boolean): value is string {
  return (
    typeof value === 'string' &&
    value.length <= maximum &&
    (allowEmpty || value.length > 0) &&
    noControlsExceptWhitespace(value)
  );
}

export function validId(value: unknown): value is string {
  return typeof value === 'string' && value.length > 0 && value.length <= 128 && noControls(value);
}

export function noControls(value: string): boolean {
  for (const character of value) {
    const code = character.codePointAt(0) ?? 0;
    if (code < 0x20 || code === 0x7f) return false;
  }
  return true;
}

export function noControlsExceptWhitespace(value: string): boolean {
  for (const character of value) {
    const code = character.codePointAt(0) ?? 0;
    if (
      (code < 0x20 && character !== '\n' && character !== '\r' && character !== '\t') ||
      code === 0x7f
    ) {
      return false;
    }
  }
  return true;
}

export function requireRecord(value: unknown): Readonly<Record<string, unknown>> {
  if (!isRecord(value)) throw new ProviderError('INVALID_RESPONSE');
  return value;
}

export function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

export function hasOwn(record: Readonly<Record<string, unknown>>, key: string): boolean {
  return Object.prototype.hasOwnProperty.call(record, key);
}
