const ALLOWED_DEPENDENCY_LICENSES = new Set([
  '(BSD-2-Clause OR MIT OR Apache-2.0)',
  '(MIT OR Apache-2.0) AND Unicode-3.0',
  '(MIT OR CC0-1.0)',
  '(MIT OR WTFPL)',
  'Apache-2.0',
  'Apache-2.0 OR MIT',
  'BSD-3-Clause',
  'BlueOak-1.0.0',
  'ISC',
  'MIT',
  'MIT OR Apache-2.0',
  'MIT/Apache-2.0',
  'Python-2.0',
  'Unlicense OR MIT',
]);

export function assertAllowedDependencyLicenses(records, kind) {
  if (records.length === 0) throw new Error(`${kind} production inventory is empty.`);
  for (const record of records) {
    if (!ALLOWED_DEPENDENCY_LICENSES.has(record.license)) {
      throw new Error(
        `${kind} dependency has an unapproved license: ${record.name}@${record.version} (${record.license || 'missing'})`,
      );
    }
  }
}
