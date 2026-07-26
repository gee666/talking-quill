import type { InvokeChannel } from '../../shared/ipc/registry';
import type { LifecycleStep } from './lifecycle';

interface IpcDrainTarget {
  drain(excludedChannels?: readonly InvokeChannel[]): Promise<void>;
}

interface AsyncDrainTarget {
  drain(): Promise<void>;
}

interface ShutdownTarget {
  shutdown(): void | Promise<void>;
}

interface CloseTarget {
  close(): void | Promise<void>;
}

interface StopTarget {
  stop(): void | Promise<void>;
}

interface FlushTarget {
  flush(): Promise<void>;
}

interface DisposeTarget {
  dispose(): void | Promise<void>;
}

export interface ApplicationDrainTargets {
  readonly ipc: IpcDrainTarget | null;
  readonly tray: AsyncDrainTarget | null;
  readonly providerMutations: AsyncDrainTarget | null;
  readonly echo: ShutdownTarget | null;
  readonly recording: ShutdownTarget | null;
  readonly models: ShutdownTarget | null;
  readonly whisper: CloseTarget | null;
  readonly helper: StopTarget | null;
  readonly history: CloseTarget | null;
  readonly settings: FlushTarget | null;
  readonly vault: FlushTarget | null;
  readonly diagnostics: DisposeTarget | null;
}

/** Ordered producer-to-store teardown shared by reset preparation and ordinary shutdown. */
export function createApplicationDrainSteps(
  targets: ApplicationDrainTargets,
  excludedIpcChannels: readonly InvokeChannel[] = [],
): readonly LifecycleStep[] {
  return [
    { name: 'ipc-drain', run: () => targets.ipc?.drain(excludedIpcChannels) },
    { name: 'tray-actions', run: () => targets.tray?.drain() },
    { name: 'provider-mutations', run: () => targets.providerMutations?.drain() },
    { name: 'echo-session', run: () => targets.echo?.shutdown() },
    { name: 'recording', run: () => targets.recording?.shutdown() },
    { name: 'models', run: () => targets.models?.shutdown() },
    { name: 'whisper-worker', run: () => targets.whisper?.close() },
    { name: 'helper', run: () => targets.helper?.stop() },
    { name: 'history', run: () => targets.history?.close() },
    { name: 'settings', run: () => targets.settings?.flush() },
    { name: 'vault', run: () => targets.vault?.flush() },
    { name: 'diagnostic-logger', run: () => targets.diagnostics?.dispose() },
  ];
}
