import { ProviderError } from './errors';
import type { PiRpcOutboundCommand } from './pi-rpc-transport';
import { READY_REQUEST_ID, PROMPT_REQUEST_ID, ABORT_REQUEST_ID } from './pi-rpc-responses';
import {
  exactKeys,
  optionalKeys,
  validId,
  validText,
  validStringArray,
  validOptionalTimeout,
  validOptionalText,
} from './pi-rpc-validation';

const MAX_EXTENSION_UI_REQUESTS = 128;
const MAX_EXTENSION_TEXT = 512 * 1024;

/** Cancels blocking UI and bounds IDs for the entire process lifetime. */
export class PiRpcExtensionUi {
  readonly #extensionUiIds = new Set<string>();
  constructor(private readonly respond: (command: PiRpcOutboundCommand) => void) {}

  accept(record: Readonly<Record<string, unknown>>): void {
    const method = validateExtensionUiRequest(record);
    const id = record.id as string;
    if (
      this.#extensionUiIds.size >= MAX_EXTENSION_UI_REQUESTS ||
      this.#extensionUiIds.has(id) ||
      id === READY_REQUEST_ID ||
      id === PROMPT_REQUEST_ID ||
      id === ABORT_REQUEST_ID
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    this.#extensionUiIds.add(id);
    if (!['select', 'confirm', 'input', 'editor'].includes(method)) return;
    this.respond({ type: 'extension_ui_response', id, cancelled: true });
  }
}

function validateExtensionUiRequest(record: Readonly<Record<string, unknown>>): string {
  if (!validId(record.id) || typeof record.method !== 'string') {
    throw new ProviderError('INVALID_RESPONSE');
  }
  switch (record.method) {
    case 'select':
      if (
        !optionalKeys(record, ['id', 'method', 'options', 'title', 'type'], ['timeout']) ||
        !validText(record.title, 4_096, true) ||
        !validStringArray(record.options, 128, 4_096) ||
        !validOptionalTimeout(record.timeout)
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return record.method;
    case 'confirm':
      if (
        !optionalKeys(record, ['id', 'message', 'method', 'title', 'type'], ['timeout']) ||
        !validText(record.title, 4_096, true) ||
        !validText(record.message, 16_384, true) ||
        !validOptionalTimeout(record.timeout)
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return record.method;
    case 'input':
      if (
        !optionalKeys(record, ['id', 'method', 'title', 'type'], ['placeholder', 'timeout']) ||
        !validText(record.title, 4_096, true) ||
        !validOptionalText(record.placeholder, 4_096) ||
        !validOptionalTimeout(record.timeout)
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return record.method;
    case 'editor':
      if (
        !optionalKeys(record, ['id', 'method', 'title', 'type'], ['prefill']) ||
        !validText(record.title, 4_096, true) ||
        !validOptionalText(record.prefill, MAX_EXTENSION_TEXT)
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return record.method;
    case 'notify':
      if (
        !optionalKeys(record, ['id', 'message', 'method', 'type'], ['notifyType']) ||
        !validText(record.message, 16_384, true) ||
        (record.notifyType !== undefined &&
          (typeof record.notifyType !== 'string' ||
            !['info', 'warning', 'error'].includes(record.notifyType)))
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return record.method;
    case 'setStatus':
      if (
        !optionalKeys(record, ['id', 'method', 'statusKey', 'type'], ['statusText']) ||
        !validText(record.statusKey, 512, false) ||
        !validOptionalText(record.statusText, 16_384)
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return record.method;
    case 'setWidget':
      if (
        !optionalKeys(
          record,
          ['id', 'method', 'type', 'widgetKey'],
          ['widgetLines', 'widgetPlacement'],
        ) ||
        !validText(record.widgetKey, 512, false) ||
        (record.widgetLines !== undefined && !validStringArray(record.widgetLines, 256, 16_384)) ||
        (record.widgetPlacement !== undefined &&
          (typeof record.widgetPlacement !== 'string' ||
            !['aboveEditor', 'belowEditor'].includes(record.widgetPlacement)))
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return record.method;
    case 'setTitle':
      if (
        !exactKeys(record, ['id', 'method', 'title', 'type']) ||
        !validText(record.title, 4_096, true)
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return record.method;
    case 'set_editor_text':
      if (
        !exactKeys(record, ['id', 'method', 'text', 'type']) ||
        !validText(record.text, MAX_EXTENSION_TEXT, true)
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return record.method;
    default:
      throw new ProviderError('INVALID_RESPONSE');
  }
}
