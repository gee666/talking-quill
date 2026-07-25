import { z } from 'zod';
import { RunnableProviderIdSchema } from './providers';

export const CredentialIdSchema = z
  .string()
  .min(1)
  .max(64)
  .regex(/^[a-zA-Z0-9][a-zA-Z0-9._-]*$/);
export const CredentialSecretSchema = z
  .string()
  .min(8, 'Credentials must contain at least 8 characters.')
  .max(16_384)
  .refine((secret) => secret.trim() === secret, 'Credentials must not contain outer whitespace.')
  .refine(
    (secret) => !hasControlCharacters(secret),
    'Credentials must not contain control characters.',
  );

function hasControlCharacters(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code <= 31 || code === 127) return true;
  }
  return false;
}

export const AwsCredentialsSchema = z
  .object({
    accessKeyId: z
      .string()
      .min(16)
      .max(128)
      .regex(/^[A-Z0-9]+$/),
    secretAccessKey: z
      .string()
      .min(16)
      .max(256)
      .refine((value) => value.trim() === value && !hasControlCharacters(value)),
    sessionToken: z
      .string()
      .min(16)
      .max(4_096)
      .refine((value) => value.trim() === value && !hasControlCharacters(value))
      .optional(),
  })
  .strict();
export type AwsCredentials = z.infer<typeof AwsCredentialsSchema>;

export function serializeAwsCredentials(input: AwsCredentials): string {
  return JSON.stringify(AwsCredentialsSchema.parse(input));
}

export const CredentialStatusSchema = z
  .object({
    id: CredentialIdSchema,
    configured: z.boolean(),
    updatedAt: z.number().int().nonnegative().nullable(),
  })
  .strict();

export const ProviderCredentialStatusSchema = z
  .object({
    providerId: RunnableProviderIdSchema,
    configured: z.boolean(),
    updatedAt: z.number().int().nonnegative().nullable(),
  })
  .strict();

export const ProviderCredentialBindingTokenSchema = z.uuid();
export const ProviderCredentialStateSchema = ProviderCredentialStatusSchema.extend({
  bindingToken: ProviderCredentialBindingTokenSchema,
}).strict();

export type CredentialStatus = z.infer<typeof CredentialStatusSchema>;
export type ProviderCredentialStatus = z.infer<typeof ProviderCredentialStatusSchema>;
export type ProviderCredentialState = z.infer<typeof ProviderCredentialStateSchema>;
export type ProviderCredentialBindingToken = z.infer<typeof ProviderCredentialBindingTokenSchema>;
