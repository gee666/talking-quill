import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { isAbsolute, relative, resolve } from 'node:path';
import { z } from 'zod';
import type { InstalledWindowsUpdateIdentity } from './publication-selection';

const Hex = z.string().regex(/^[0-9a-f]{64}$/u);
const Role = z.strictObject({
  role: z.enum(['gateway', 'owner']),
  path: z.string().min(1).max(512),
  sha256: Hex,
  suppressionCapable: z.boolean(),
});
const Manifest = z.strictObject({
  schemaVersion: z.literal(1),
  kind: z.literal('talking-quill-local-owner-release'),
  version: z.string().regex(/^\d+\.\d+\.\d+$/u),
  platform: z.literal('win'),
  architecture: z.enum(['x64', 'arm64']),
  ownerMode: z.literal('local-unsigned-enabled'),
  packageMode: z.enum(['fresh', 'update', 'repair']),
  sourceCommit: z.string().regex(/^[0-9a-f]{40}$/u),
  sourceTree: z.string().regex(/^[0-9a-f]{40}$/u),
  roles: z.array(Role).length(2),
  predecessor: z.unknown().nullable(),
  releaseBuildDigest: Hex,
  packageLayoutDigest: Hex,
  update: z.unknown(),
});

export async function readInstalledWindowsUpdateIdentity(
  resourcesPath: string,
  architecture: 'x64' | 'arm64',
): Promise<InstalledWindowsUpdateIdentity> {
  const root = resolve(resourcesPath, '..');
  const bytes = await readFile(resolve(resourcesPath, 'keyboard-owner-release-v1.json'));
  const manifest = Manifest.parse(JSON.parse(bytes.toString('utf8')) as unknown);
  if (manifest.architecture !== architecture)
    throw new Error('Installed Windows update architecture is invalid');
  const role = (name: 'gateway' | 'owner') => {
    const matches = manifest.roles.filter((candidate) => candidate.role === name);
    if (matches.length !== 1) throw new Error('Installed Windows update roles are ambiguous');
    const value = matches.at(0);
    if (value === undefined) throw new Error('Installed Windows update role is missing');
    return value;
  };
  const gateway = role('gateway');
  const owner = role('owner');
  for (const value of [gateway, owner]) {
    const path = resolve(root, value.path);
    const relation = relative(root, path);
    if (relation === '' || relation.startsWith('..') || isAbsolute(relation))
      throw new Error('Installed Windows update role path escapes its root');
    const actual = createHash('sha256')
      .update(await readFile(path))
      .digest('hex');
    if (actual !== value.sha256) throw new Error('Installed Windows update role identity changed');
  }
  return {
    version: manifest.version,
    architecture,
    releaseBuildDigest: manifest.releaseBuildDigest,
    gatewaySha256: gateway.sha256,
    ownerSha256: owner.sha256,
  };
}
