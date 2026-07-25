import { z } from 'zod';

export const WhisperModelIdSchema = z.enum([
  'onnx-community/whisper-large-v3-turbo',
  'Xenova/whisper-small',
]);
export type WhisperModelId = z.infer<typeof WhisperModelIdSchema>;

export const ModelManifestFileSchema = z
  .object({
    path: z
      .string()
      .min(1)
      .max(160)
      .refine((value) => !value.startsWith('/') && !value.includes('..') && !value.includes('\\')),
    size: z.number().int().positive(),
    sha256: z.string().regex(/^[a-f0-9]{64}$/),
  })
  .strict();

export const ModelManifestEntrySchema = z
  .object({
    id: WhisperModelIdSchema,
    revision: z.string().regex(/^[a-f0-9]{40}$/),
    dtype: z.literal('q8'),
    totalBytes: z.number().int().positive(),
    files: z.array(ModelManifestFileSchema).length(7),
  })
  .strict()
  .superRefine((value, context) => {
    const total = value.files.reduce((sum, file) => sum + file.size, 0);
    if (total !== value.totalBytes) {
      context.addIssue({ code: 'custom', message: 'Model file sizes do not match totalBytes' });
    }
    if (new Set(value.files.map((file) => file.path)).size !== value.files.length) {
      context.addIssue({ code: 'custom', message: 'Model file paths must be unique' });
    }
  });

export const VerifiedModelFileIdentitySchema = z
  .object({
    path: ModelManifestFileSchema.shape.path,
    size: z.number().int().positive(),
    mtimeMs: z.number().nonnegative(),
    ctimeMs: z.number().nonnegative(),
    birthtimeMs: z.number().nonnegative(),
    device: z.string().regex(/^\d+$/),
    inode: z.string().regex(/^\d+$/),
  })
  .strict();

export const ModelManifestSchema = z
  .object({
    schemaVersion: z.literal(1),
    transformersVersion: z.literal('3.8.1'),
    models: z.array(ModelManifestEntrySchema).length(2),
  })
  .strict()
  .superRefine((value, context) => {
    if (new Set(value.models.map((model) => model.id)).size !== value.models.length) {
      context.addIssue({ code: 'custom', message: 'Model ids must be unique' });
    }
  });

export type ModelManifest = z.infer<typeof ModelManifestSchema>;
export type ModelManifestEntry = z.infer<typeof ModelManifestEntrySchema>;
export type ModelManifestFile = z.infer<typeof ModelManifestFileSchema>;
export type VerifiedModelFileIdentity = z.infer<typeof VerifiedModelFileIdentitySchema>;
