import { basename, dirname, isAbsolute, relative, resolve, sep } from 'node:path';

export interface PathContainmentOperations {
  readonly isAbsolute: (path: string) => boolean;
  readonly relative: (from: string, to: string) => string;
  readonly sep: string;
}

export interface AbsolutePathOperations {
  readonly isAbsolute: (path: string) => boolean;
  readonly resolve: (path: string) => string;
}

const DEFAULT_CONTAINMENT_OPERATIONS: PathContainmentOperations = {
  isAbsolute,
  relative,
  sep,
};

const DEFAULT_ABSOLUTE_PATH_OPERATIONS: AbsolutePathOperations = {
  isAbsolute,
  resolve,
};

export function isStrictPathChild(
  parent: string,
  candidate: string,
  operations: PathContainmentOperations = DEFAULT_CONTAINMENT_OPERATIONS,
): boolean {
  const relativePath = operations.relative(parent, candidate);
  return (
    relativePath !== '' &&
    relativePath !== '..' &&
    !relativePath.startsWith(`..${operations.sep}`) &&
    !operations.isAbsolute(relativePath)
  );
}

export interface UninstallResetTargetPolicy {
  readonly expectedTarget?: string;
  readonly isolatedTestBase?: string;
}

export function validateUninstallResetTarget(
  value: string,
  policy: UninstallResetTargetPolicy = {},
): string {
  const target = selectAbsolutePathOverride(undefined, value);
  if (
    target === null ||
    basename(target) !== 'Talking Quill' ||
    basename(dirname(target)) !== 'Roaming' ||
    basename(dirname(dirname(target))) !== 'AppData'
  ) {
    throw new Error('Uninstall reset target is not the exact Talking Quill roaming-data folder');
  }
  const { expectedTarget, isolatedTestBase } = policy;
  if (expectedTarget !== undefined) {
    const expected = selectAbsolutePathOverride(undefined, expectedTarget);
    if (expected?.toLocaleLowerCase('en-US') !== target.toLocaleLowerCase('en-US')) {
      throw new Error('Uninstall reset target does not belong to the signed-in Windows user');
    }
  }
  if (isolatedTestBase !== undefined) {
    const base = selectAbsolutePathOverride(undefined, isolatedTestBase);
    const profileRoot = dirname(dirname(dirname(target)));
    const testRoot = dirname(profileRoot);
    if (
      base?.toLocaleLowerCase('en-US') !== dirname(testRoot).toLocaleLowerCase('en-US') ||
      basename(profileRoot) !== 'profile'
    ) {
      throw new Error('Uninstall test reset target is outside its fixed temporary root');
    }
  }
  return target;
}

export function selectAbsolutePathOverride(
  environmentValue: string | undefined,
  argumentValue: string | null,
  operations: AbsolutePathOperations = DEFAULT_ABSOLUTE_PATH_OPERATIONS,
): string | null {
  const requested =
    environmentValue === undefined || environmentValue.length === 0
      ? argumentValue
      : environmentValue;
  if (requested === null) return null;
  if (!operations.isAbsolute(requested)) {
    throw new Error('Runtime path override must be absolute');
  }
  return operations.resolve(requested);
}
