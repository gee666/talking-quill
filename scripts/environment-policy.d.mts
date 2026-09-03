export function normalizeEnvironment(environment: NodeJS.ProcessEnv): NodeJS.ProcessEnv;
export function sanitizedSubprocessEnvironment(
  source?: NodeJS.ProcessEnv,
  overrides?: NodeJS.ProcessEnv,
): NodeJS.ProcessEnv;
