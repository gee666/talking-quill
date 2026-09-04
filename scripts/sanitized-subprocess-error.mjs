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
  return typeof value === 'string' && value.length > 0 ? '<redacted>' : '';
}
