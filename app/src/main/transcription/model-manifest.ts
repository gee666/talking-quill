import rawManifest from '../../../../scripts/model-manifest.json';
import {
  ModelManifestSchema,
  type ModelManifestEntry,
  type WhisperModelId,
} from '../../shared/schemas/model-manifest';

export const MODEL_MANIFEST = ModelManifestSchema.parse(rawManifest);

export function getModelManifest(modelId: WhisperModelId): ModelManifestEntry {
  const entry = MODEL_MANIFEST.models.find((candidate) => candidate.id === modelId);
  if (entry === undefined) throw new Error(`Unsupported Whisper model: ${modelId}`);
  return entry;
}
