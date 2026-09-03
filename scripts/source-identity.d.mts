export interface SourceIdentity {
  readonly sourceCommit: string;
  readonly sourceTree: string;
}

export function currentSourceIdentity(
  options?: Readonly<{
    repositoryRoot?: string;
    environment?: NodeJS.ProcessEnv;
    subprocessEnvironment?: NodeJS.ProcessEnv;
    gitCommand?: Readonly<{ executable: string; arguments: readonly string[] }>;
    requireClean?: boolean;
  }>,
): SourceIdentity;
