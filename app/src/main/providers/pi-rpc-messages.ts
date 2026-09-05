import { ProviderError } from './errors';
import { MAX_NATIVE_OUTPUT_CHARACTERS } from './native-common';
import type { PiRpcExpectedState } from './pi-rpc-types';
import { exactKeys, optionalKeys, requireRecord, validText, isRecord } from './pi-rpc-validation';

export function validateAssistantBlocks(
  message: Readonly<Record<string, unknown>>,
  requireText: boolean,
): string {
  const content = message.content;
  if (!Array.isArray(content) || content.length > 256 || (requireText && content.length === 0)) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  let text = '';
  for (const rawBlock of content) {
    const block = requireRecord(rawBlock);
    if (block.type === 'thinking') {
      if (
        typeof block.thinking !== 'string' ||
        !optionalKeys(block, ['thinking', 'type'], ['redacted', 'thinkingSignature']) ||
        (block.redacted !== undefined && typeof block.redacted !== 'boolean') ||
        (block.thinkingSignature !== undefined && typeof block.thinkingSignature !== 'string')
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      continue;
    }
    if (
      block.type !== 'text' ||
      typeof block.text !== 'string' ||
      !optionalKeys(block, ['text', 'type'], ['textSignature']) ||
      (block.textSignature !== undefined && typeof block.textSignature !== 'string')
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    text += block.text;
    if (text.length > MAX_NATIVE_OUTPUT_CHARACTERS) {
      throw new ProviderError('RESPONSE_TOO_LARGE');
    }
  }
  const result = text.trim();
  if (requireText && result.length === 0) throw new ProviderError('INVALID_RESPONSE');
  return result;
}

export function userMessageText(message: Readonly<Record<string, unknown>>): string {
  if (
    message.role !== 'user' ||
    !Number.isSafeInteger(message.timestamp) ||
    Number(message.timestamp) < 0
  ) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  if (typeof message.content === 'string') return message.content;
  if (!Array.isArray(message.content) || message.content.length !== 1) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  const block = requireRecord(message.content[0]);
  if (
    !exactKeys(block, ['text', 'type']) ||
    block.type !== 'text' ||
    typeof block.text !== 'string'
  ) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  return block.text;
}

export function assertAssistantIdentity(
  expected: PiRpcExpectedState,
  message: Readonly<Record<string, unknown>>,
  allowPending: boolean,
): void {
  if (
    message.role !== 'assistant' ||
    message.provider !== expected.provider ||
    message.model !== expected.model ||
    !Array.isArray(message.content) ||
    message.content.length > 256 ||
    !validText(message.api, 128, false) ||
    !isRecord(message.usage) ||
    !Number.isSafeInteger(message.timestamp) ||
    Number(message.timestamp) < 0 ||
    (allowPending && message.stopReason !== 'pending') ||
    (!allowPending && message.stopReason === 'pending')
  ) {
    throw new ProviderError('INVALID_RESPONSE');
  }
}
