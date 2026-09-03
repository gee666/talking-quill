export function normalizeEnvironment(environment) {
  const entries = new Map();
  for (const [name, value] of Object.entries(environment)) {
    const normalizedName = name.toUpperCase();
    entries.set(normalizedName, [
      normalizedName.startsWith('TALKING_QUILL_') ? normalizedName : name,
      value,
    ]);
  }
  return Object.fromEntries(entries.values());
}
