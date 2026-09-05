import type { ChildProcessWithoutNullStreams } from './pi-rpc-operation';
import type { PiThinkingLevel } from '../../shared/schemas/providers';
import type { PiCliIdentity } from './pi-executable';
import type { SpawnPi } from './pi-process-runtime';
import type { PiRpcLimits } from './pi-rpc-transport';

export enum PiRpcTimingStage {
  ProcessSpawned = 'process-spawned',
  ReadinessProbeWritten = 'readiness-probe-written',
  Ready = 'ready',
  PromptWritten = 'prompt-written',
  PromptAccepted = 'prompt-accepted',
  AssistantMessageEnded = 'assistant-message-ended',
  AgentSettled = 'agent-settled',
  RetirementStarted = 'retirement-started',
  Retired = 'retired',
}

export interface PiRpcExpectedState {
  readonly provider: string;
  readonly model: string;
  readonly thinking: PiThinkingLevel;
}

export interface PiRpcPromptResult {
  readonly text: string;
  /** Resolves after graceful exit or confirmed process-tree termination. */
  readonly retirement: Promise<void>;
}

export type TerminatePiRpcTree = (
  child: ChildProcessWithoutNullStreams,
  platform: NodeJS.Platform,
  environment: NodeJS.ProcessEnv,
) => Promise<void>;

export interface PiRpcPrewarmOptions {
  readonly identity: PiCliIdentity;
  readonly expected: PiRpcExpectedState;
  /** Canonical extension files or package roots already approved by the later argv layer. */
  readonly explicitExtensions?: readonly string[];
  readonly signal?: AbortSignal;
  readonly environment?: NodeJS.ProcessEnv;
  readonly platform?: NodeJS.Platform;
  readonly workingDirectory?: string;
  readonly spawnPi?: SpawnPi;
  readonly terminateTree?: TerminatePiRpcTree;
  readonly limits?: Partial<PiRpcLimits>;
  readonly timeoutMs?: number;
  readonly abortGraceMs?: number;
  readonly retirementGraceMs?: number;
  readonly treeTerminationTimeoutMs?: number;
  /** Receives enum values only. The caller owns timestamps and any aggregation. */
  readonly onTiming?: (stage: PiRpcTimingStage) => void;
}
