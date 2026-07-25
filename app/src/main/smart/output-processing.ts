import { TranscriptTextSchema } from '../../shared/schemas/transcription';
import { ProviderError } from '../providers/errors';

const ENCLOSING_FENCE = /^```[^\n\r`]*\r?\n([\s\S]*?)\r?\n?```$/;

export function normalizeSmartOutput(input: string): string {
  let output = input.replace(/\r\n?/g, '\n').trim();
  const fence = ENCLOSING_FENCE.exec(output);
  if (fence !== null) output = (fence[1] ?? '').trim();
  if (output.length === 0) throw new ProviderError('INVALID_RESPONSE');
  const parsed = TranscriptTextSchema.safeParse(output);
  if (!parsed.success) throw new ProviderError('INVALID_RESPONSE');
  return parsed.data;
}
