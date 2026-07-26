import rawManifest from '../../../../scripts/model-manifest.json';
import { ModelManifestSchema } from '../../shared/schemas/model-manifest';

export const MODEL_MANIFEST = ModelManifestSchema.parse(rawManifest);
