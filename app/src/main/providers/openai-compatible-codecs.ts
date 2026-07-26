import { ProviderCompletionRequestSchema } from '../../shared/schemas/providers';
import { ProviderError } from './errors';
import type { OpenAICompatiblePreset } from './presets';

const MAX_RESPONSE_OUTPUT_ITEMS = 256;
const MAX_RESPONSE_CONTENT_ITEMS = 512;
const MAX_COMPLETION_CHARACTERS = 200_000;

type ParsedCompletionRequest = ReturnType<typeof ProviderCompletionRequestSchema.parse>;

export function parseCompletionRequest(input: unknown): ParsedCompletionRequest {
  try {
    return ProviderCompletionRequestSchema.parse(input);
  } catch {
    throw new ProviderError('INVALID_CONFIG');
  }
}

export function createCompletionBody(
  preset: OpenAICompatiblePreset,
  model: string | null | undefined,
  request: ParsedCompletionRequest,
  maxOutputTokens: number,
): Readonly<Record<string, unknown>> {
  return preset.protocol === 'responses'
    ? createResponsesBody(preset, model, request, maxOutputTokens)
    : createChatCompletionsBody(preset, model, request, maxOutputTokens);
}

export function parseCompletionOutput(
  protocol: OpenAICompatiblePreset['protocol'],
  input: unknown,
  maxOutputTokens: number,
  allowTokenExhaustion = false,
): string {
  const maximumCharacters = Math.min(MAX_COMPLETION_CHARACTERS, Math.max(16, maxOutputTokens * 16));
  const text =
    protocol === 'responses'
      ? parseResponsesOutput(input, maximumCharacters, allowTokenExhaustion)
      : parseChatCompletionOutput(input, maximumCharacters);
  if (text.trim().length === 0 && !allowTokenExhaustion) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  return text;
}

function createResponsesBody(
  preset: OpenAICompatiblePreset,
  model: string | null | undefined,
  request: ParsedCompletionRequest,
  maxOutputTokens: number,
): Readonly<Record<string, unknown>> {
  const temperature =
    preset.temperatureMode === 'openai-reasoning-one' &&
    model !== null &&
    model !== undefined &&
    (/^o\d(?:[.-]|$)/i.test(model) || /^gpt-5(?:[.-]|$)/i.test(model))
      ? 1
      : request.temperature;
  return Object.freeze({
    model,
    input:
      request.image === undefined
        ? request.input
        : Object.freeze([
            Object.freeze({
              role: 'user',
              content: Object.freeze([
                Object.freeze({ type: 'input_text', text: request.input }),
                Object.freeze({ type: 'input_image', image_url: imageDataUrl(request.image) }),
              ]),
            }),
          ]),
    store: false,
    max_output_tokens: maxOutputTokens,
    temperature,
  });
}

function createChatCompletionsBody(
  preset: OpenAICompatiblePreset,
  model: string | null | undefined,
  request: ParsedCompletionRequest,
  maxOutputTokens: number,
): Readonly<Record<string, unknown>> {
  const maxTokensField = preset.maxTokensField ?? 'max_tokens';
  return Object.freeze({
    ...(model === null || model === undefined ? {} : { model }),
    messages: Object.freeze([
      Object.freeze({
        role: 'user',
        content:
          request.image === undefined
            ? request.input
            : Object.freeze([
                Object.freeze({ type: 'text', text: request.input }),
                Object.freeze({
                  type: 'image_url',
                  image_url: Object.freeze({ url: imageDataUrl(request.image) }),
                }),
              ]),
      }),
    ]),
    stream: false,
    temperature: request.temperature,
    [maxTokensField]: maxOutputTokens,
  });
}

function imageDataUrl(image: { readonly mimeType: 'image/jpeg'; readonly base64: string }): string {
  return `data:${image.mimeType};base64,${image.base64}`;
}

function parseResponsesOutput(
  input: unknown,
  maximumCharacters: number,
  allowTokenExhaustion = false,
): string {
  const record = asRecord(input);
  if (record === null) throw new ProviderError('INVALID_RESPONSE');
  if (record.status !== 'completed') {
    const incompleteDetails = asRecord(record.incomplete_details);
    if (
      !allowTokenExhaustion ||
      record.status !== 'incomplete' ||
      (record.error !== undefined && record.error !== null) ||
      incompleteDetails?.reason !== 'max_output_tokens'
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    // Validation only needs to prove authenticated model reachability. Cleanup never enables this.
    return '';
  }
  if (record.error !== undefined && record.error !== null) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  if (!Array.isArray(record.output) || record.output.length > MAX_RESPONSE_OUTPUT_ITEMS) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  const parts: string[] = [];
  let characters = 0;
  for (const item of record.output) {
    const output = asRecord(item);
    if (output === null) throw new ProviderError('INVALID_RESPONSE');
    if (output.type !== 'message') continue;
    if (output.role !== 'assistant') throw new ProviderError('INVALID_RESPONSE');
    if (!Array.isArray(output.content) || output.content.length > MAX_RESPONSE_CONTENT_ITEMS) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    for (const contentItem of output.content) {
      const content = asRecord(contentItem);
      if (content === null || typeof content.type !== 'string') {
        throw new ProviderError('INVALID_RESPONSE');
      }
      if (content.type === 'output_text') {
        if (typeof content.text !== 'string') throw new ProviderError('INVALID_RESPONSE');
        characters += content.text.length;
        if (characters > maximumCharacters) throw new ProviderError('INVALID_RESPONSE');
        parts.push(content.text);
      } else {
        // Refusals and other message content must not be mixed into transcript output.
        throw new ProviderError('INVALID_RESPONSE');
      }
    }
  }
  if (parts.length === 0) throw new ProviderError('INVALID_RESPONSE');
  return parts.join('');
}

function parseChatCompletionOutput(input: unknown, maximumCharacters: number): string {
  const record = asRecord(input);
  const choices = record?.choices;
  if (!Array.isArray(choices) || choices.length === 0) throw new ProviderError('INVALID_RESPONSE');
  const first = asRecord(choices[0]);
  if (
    first === null ||
    (first.finish_reason !== undefined &&
      first.finish_reason !== null &&
      first.finish_reason !== 'stop')
  ) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  const message = asRecord(first.message);
  if (typeof message?.content !== 'string') throw new ProviderError('INVALID_RESPONSE');
  return boundedOutput(message.content, maximumCharacters);
}

function boundedOutput(value: string, maximumCharacters: number): string {
  if (value.length > maximumCharacters) throw new ProviderError('INVALID_RESPONSE');
  return value;
}

function asRecord(input: unknown): Readonly<Record<string, unknown>> | null {
  return typeof input === 'object' && input !== null && !Array.isArray(input)
    ? (input as Readonly<Record<string, unknown>>)
    : null;
}
