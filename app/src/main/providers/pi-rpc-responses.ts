import { ProviderError } from './errors';
import type { PiRpcExpectedState } from './pi-rpc-types';
import { exactKeys, onlyKeys, hasOwn, requireRecord, validText } from './pi-rpc-validation';

export const READY_REQUEST_ID = 'talking-quill-state-1';
export const PROMPT_REQUEST_ID = 'talking-quill-prompt-1';
export const ABORT_REQUEST_ID = 'talking-quill-abort-1';

/** False is an accepted remote failure, not a transport/protocol fault. */
export function validatePromptResponse(record: Readonly<Record<string, unknown>>): boolean {
  if (record.id !== PROMPT_REQUEST_ID || record.command !== 'prompt') {
    throw new ProviderError('INVALID_RESPONSE');
  }
  if (record.success === true && exactKeys(record, ['command', 'id', 'success', 'type'])) {
    return true;
  }
  if (
    record.success === false &&
    exactKeys(record, ['command', 'error', 'id', 'success', 'type']) &&
    validText(record.error, 16_384, true)
  ) {
    return false;
  }
  throw new ProviderError('INVALID_RESPONSE');
}

export function validateReadinessResponse(
  expected: PiRpcExpectedState,
  record: Readonly<Record<string, unknown>>,
): void {
  if (
    record.id !== READY_REQUEST_ID ||
    record.command !== 'get_state' ||
    record.success !== true ||
    !exactKeys(record, ['command', 'data', 'id', 'success', 'type'])
  ) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  const data = requireRecord(record.data);
  if (
    !onlyKeys(data, [
      'autoCompactionEnabled',
      'followUpMode',
      'isCompacting',
      'isStreaming',
      'messageCount',
      'model',
      'pendingMessageCount',
      'sessionFile',
      'sessionId',
      'sessionName',
      'steeringMode',
      'thinkingLevel',
    ]) ||
    hasOwn(data, 'sessionFile') ||
    hasOwn(data, 'sessionName') ||
    data.isStreaming !== false ||
    data.isCompacting !== false ||
    data.messageCount !== 0 ||
    data.pendingMessageCount !== 0 ||
    data.thinkingLevel !== expected.thinking ||
    !['all', 'one-at-a-time'].includes(String(data.steeringMode)) ||
    !['all', 'one-at-a-time'].includes(String(data.followUpMode)) ||
    typeof data.autoCompactionEnabled !== 'boolean' ||
    !validText(data.sessionId, 256, false)
  ) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  const model = requireRecord(data.model);
  if (model.provider !== expected.provider || model.id !== expected.model) {
    throw new ProviderError('INVALID_RESPONSE');
  }
}
