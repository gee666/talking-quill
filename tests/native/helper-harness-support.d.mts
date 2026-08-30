export const FAILURE_CLEANUP_REQUESTS: readonly (
  | readonly ['session.set_capture', Readonly<{ mode: 'off' }>]
  | readonly ['activation.configure', Readonly<{ enabled: false; bindings: readonly never[] }>]
  | readonly ['shutdown', Readonly<Record<string, never>>]
)[];

export function prepareHelperHarnessExecutable(options: {
  helper: string;
  repositoryRoot: string;
  platform?: NodeJS.Platform;
  architecture?: string;
  processId?: number;
}): Promise<{
  executable: string;
  staged: boolean;
  cleanup(): Promise<void>;
}>;
