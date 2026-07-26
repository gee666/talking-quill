import { isAbsolute, relative, resolve, sep } from 'node:path';

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
