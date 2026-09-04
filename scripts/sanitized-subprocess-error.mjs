const MAX_DIAGNOSTIC_BYTES = 1024;

export function subprocessFailure(label, result) {
  const status = Number.isInteger(result?.status) ? String(result.status) : 'none';
  const signal = /^[A-Z0-9]+$/u.test(result?.signal ?? '') ? result.signal : 'none';
  const spawnCode = /^[A-Z0-9_]+$/u.test(result?.error?.code ?? '')
    ? result.error.code
    : result?.error === undefined
      ? 'none'
      : 'unknown';
  const stderr = redactDiagnostic(result?.stderr);
  return new Error(
    `${label} failed (status=${status}, signal=${signal}, spawn=${spawnCode}${stderr === '' ? '' : `, stderr=${stderr}`})`,
  );
}

export function redactDiagnostic(value) {
  if (typeof value !== 'string' || value === '') return '';
  const diagnostic = Array.from(value.slice(0, MAX_DIAGNOSTIC_BYTES), (character) => {
    const code = character.codePointAt(0);
    return code <= 0x1f || code === 0x7f ? ' ' : character;
  })
    .join('')
    .replace(/\s+/gu, ' ')
    .trim();
  return diagnostic.includes('\\') || diagnostic.includes('/') ? '<redacted>' : diagnostic;
}
