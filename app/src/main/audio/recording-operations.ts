import type { RecordingContext } from './recording-context';

export function withPermissionOperation<Result>(
  context: RecordingContext,
  operation: () => Promise<Result>,
): Promise<Result> {
  const result = context.permissionOperation.then(operation, operation);
  context.permissionOperation = result.then(
    () => undefined,
    () => undefined,
  );
  return result;
}

export async function enqueue(
  context: RecordingContext,
  operation: () => Promise<void>,
): Promise<void> {
  const next = context.operation.then(operation, operation);
  context.operation = next.catch(() => undefined);
  await next;
}
