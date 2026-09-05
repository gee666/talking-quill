import type { Dirent, Stats } from 'node:fs';
import type { EgressObserver } from '../security/egress-audit';
import type { PiCliIdentity } from './pi-executable';
import type { SpawnPi } from './pi-process-runtime';
import type {
  prewarmPiRpcOperation,
  PiRpcTimingStage,
  TerminatePiRpcTree,
} from './pi-rpc-operation';

export interface PiProviderOptions {
  readonly spawnPi?: SpawnPi;
  readonly environment?: NodeJS.ProcessEnv;
  readonly platform?: NodeJS.Platform;
  readonly workingDirectory?: string;
  readonly now?: () => number;
  readonly observeEgress?: EgressObserver;
  readonly configuredPath?: () => string | null;
  readonly interactiveAppData?: string;
  readonly interactiveHome?: string;
  readonly resolveCli?: (signal?: AbortSignal) => Promise<PiCliIdentity>;
  readonly revalidateCli?: (identity: PiCliIdentity, signal?: AbortSignal) => Promise<void>;
  readonly canonicalizeExtensionPath?: (path: string) => Promise<string>;
  readonly statExtensionPath?: (path: string) => Promise<Stats>;
  readonly accessExtensionPath?: (path: string, mode?: number) => Promise<void>;
  readonly readExtensionFile?: (path: string) => Promise<string>;
  readonly readExtensionDirectory?: (path: string) => Promise<readonly Dirent[]>;
  readonly extensionResolutionTimeoutMs?: number;
  readonly rpcReadyTtlMs?: number;
  readonly prewarmRpcOperation?: typeof prewarmPiRpcOperation;
  readonly terminateRpcTree?: TerminatePiRpcTree;
  /** Receives timing stage enums only; no content, paths, model IDs, or extension IDs are exposed. */
  readonly onRpcTiming?: (stage: PiRpcTimingStage) => void;
}
