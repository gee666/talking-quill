import type { link, rename } from 'node:fs/promises';
import type {
  ModelManifest,
  ModelManifestEntry,
  ModelManifestFile,
  WhisperModelId,
} from '../../shared/schemas/model-manifest';
import type { ModelState, ModelStatus } from '../../shared/schemas/transcription';
import type { EgressObserver } from '../security/egress-audit';
import type { ModelAccessCoordinator } from './model-access-coordinator';
import type { inspectFile } from './model-integrity';
import type { RevisionBackupRemover } from './model-publication';

export type DownloadIntent = 'running' | 'paused' | 'cancelled' | 'external' | 'shutdown';

export interface ActiveDownload {
  readonly modelId: WhisperModelId;
  readonly controller: AbortController;
  readonly settled: Promise<ModelStatus>;
  intent: DownloadIntent;
  state: Extract<ModelState, 'downloading' | 'verifying' | 'installing'>;
}

export interface ModelUseGrant {
  readonly status: ModelStatus;
  release(): void;
}

export interface ModelManagerOptions {
  readonly modelsDirectory: string;
  readonly temporaryDirectory: string;
  readonly fetch?: typeof fetch;
  readonly availableBytes?: (path: string) => Promise<number>;
  readonly urlFor?: (model: ModelManifestEntry, file: ModelManifestFile) => string;
  readonly validateRequestUrl?: (url: string) => boolean;
  readonly requestTimeoutMs?: number;
  readonly manifest?: ModelManifest;
  readonly accessCoordinator?: ModelAccessCoordinator;
  readonly inspectFile?: typeof inspectFile;
  readonly observeEgress?: EgressObserver;
  /** Test seams for platform filesystem behavior; production uses node:fs/promises. */
  readonly rename?: typeof rename;
  readonly link?: typeof link;
  readonly removeRevisionBackup?: RevisionBackupRemover;
}
