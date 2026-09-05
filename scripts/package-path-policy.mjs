const FORBIDDEN_PARTS = [
  'reference',
  'anythingllm',
  'prisma',
  'vectordb',
  '__tests__',
  'test_extension',
  'task6-test-composition',
  'node_modules/.cache',
];
const FORBIDDEN_EXTENSIONS = new Set([
  '.c',
  '.cc',
  '.cpp',
  '.h',
  '.hpp',
  '.gyp',
  '.iobj',
  '.ipdb',
  '.lib',
  '.map',
  '.obj',
  '.pdb',
  '.ts',
  '.tsx',
  '.vcxproj',
]);

export function normalizePackagePath(path) {
  return path.replaceAll('\\', '/').replace(/^\/+/, '').replace(/\/$/, '');
}

export function assertSafePaths(entries) {
  const forbidden = entries.filter((entry) => {
    const lower = entry.toLowerCase();
    if (FORBIDDEN_PARTS.some((part) => lower.includes(part))) {
      return true;
    }
    const dot = lower.lastIndexOf('.');
    return dot >= 0 && FORBIDDEN_EXTENSIONS.has(lower.slice(dot));
  });
  if (forbidden.length > 0) {
    throw new Error(`Forbidden packaged files: ${forbidden.join(', ')}`);
  }
}
