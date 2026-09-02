import { spawnSync } from 'node:child_process';

const namespaceId = process.env.TQ_MACHINE_LOCK_TEST_NAMESPACE_ID;
if (!/^[0-9a-f]{32}$/u.test(namespaceId ?? '')) process.exit(64);
const namespace = `HKCU\\Software\\Talking Quill Tests\\${namespaceId}`;
for (const [key, name] of [
  [namespace, 'NamespaceValue'],
  [`${namespace}\\RecoveryStateLockV1`, 'RecoveryValue'],
]) {
  const result = spawnSync(
    'reg.exe',
    ['add', key, '/v', name, '/t', 'REG_SZ', '/d', 'value', '/f'],
    {
      windowsHide: true,
    },
  );
  if (result.status !== 0) process.exit(1);
}
