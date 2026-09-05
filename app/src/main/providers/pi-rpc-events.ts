import { isDeepStrictEqual } from 'node:util';
import { ProviderError } from './errors';
import { PiRpcTimingStage, type PiRpcExpectedState } from './pi-rpc-types';
import {
  assertAssistantIdentity,
  userMessageText,
  validateAssistantBlocks,
} from './pi-rpc-messages';
import {
  exactKeys,
  requireRecord,
  isRecord,
  validIndex,
  validPositiveInteger,
  validText,
} from './pi-rpc-validation';

interface AssistantCandidate {
  readonly text: string;
}

/** Validates one prompt's agent/turn/message sequence, including retries and continuations. */
export class PiRpcEvents {
  #agentActive = false;
  #agentRuns = 0;
  #turnActive = false;
  #openMessageRole: 'user' | 'assistant' | null = null;
  #openUserMessageWire: string | null = null;
  #lastAssistantMessage: Readonly<Record<string, unknown>> | null = null;
  #assistantEndedInTurn = false;
  #lastAgentEnded = false;
  #sawRetry = false;
  #candidate: AssistantCandidate | null = null;
  promptText: string | null = null;
  get candidate(): AssistantCandidate | null {
    return this.#candidate;
  }
  constructor(
    private readonly expected: PiRpcExpectedState,
    private readonly timing: (stage: PiRpcTimingStage) => void,
    private readonly onSettled: () => void,
    private readonly onFailure: (error: ProviderError) => void,
  ) {}

  accept(type: string, record: Readonly<Record<string, unknown>>): void {
    switch (type) {
      case 'agent_start':
        this.#acceptAgentStart(record);
        return;
      case 'turn_start':
        this.#acceptTurnStart(record);
        return;
      case 'message_start':
        this.#acceptMessageStart(record);
        return;
      case 'message_update':
        this.#acceptMessageUpdate(record);
        return;
      case 'message_end':
        this.#acceptMessageEnd(record);
        return;
      case 'turn_end':
        this.#acceptTurnEnd(record);
        return;
      case 'agent_end':
        this.#acceptAgentEnd(record);
        return;
      case 'auto_retry_start':
        this.#acceptAutoRetryStart(record);
        return;
      case 'auto_retry_end':
        this.#acceptAutoRetryEnd(record);
        return;
      case 'agent_settled':
        this.#acceptAgentSettled(record);
        return;
      default:
        throw new ProviderError('INVALID_RESPONSE');
    }
  }

  #acceptAgentStart(record: Readonly<Record<string, unknown>>): void {
    if (
      !exactKeys(record, ['type']) ||
      this.#agentActive ||
      this.#turnActive ||
      this.#openMessageRole !== null ||
      (this.#agentRuns > 0 && !this.#lastAgentEnded)
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    this.#agentActive = true;
    this.#agentRuns += 1;
    this.#lastAgentEnded = false;
    this.#candidate = null;
  }

  #acceptTurnStart(record: Readonly<Record<string, unknown>>): void {
    if (!exactKeys(record, ['type']) || !this.#agentActive || this.#turnActive) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    this.#turnActive = true;
    this.#assistantEndedInTurn = false;
  }

  #acceptMessageStart(record: Readonly<Record<string, unknown>>): void {
    if (
      !exactKeys(record, ['message', 'type']) ||
      !this.#agentActive ||
      !this.#turnActive ||
      this.#openMessageRole !== null
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    const message = requireRecord(record.message);
    if (message.role !== 'user' && message.role !== 'assistant') {
      throw new ProviderError('INVALID_RESPONSE');
    }
    if (message.role === 'assistant') assertAssistantIdentity(this.expected, message, true);
    else {
      if (userMessageText(message) !== this.promptText) throw new ProviderError('INVALID_RESPONSE');
      this.#openUserMessageWire = JSON.stringify(message);
    }
    this.#openMessageRole = message.role;
  }

  #acceptMessageUpdate(record: Readonly<Record<string, unknown>>): void {
    if (
      !exactKeys(record, ['assistantMessageEvent', 'type', 'usage']) ||
      !this.#agentActive ||
      !this.#turnActive ||
      this.#openMessageRole !== 'assistant' ||
      !isRecord(record.usage)
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    const event = requireRecord(record.assistantMessageEvent);
    const eventType = event.type;
    if (
      ![
        'text_start',
        'text_delta',
        'text_end',
        'thinking_start',
        'thinking_delta',
        'thinking_end',
      ].includes(String(eventType)) ||
      !validIndex(event.contentIndex)
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    if (eventType === 'text_start' || eventType === 'thinking_start') {
      if (!exactKeys(event, ['contentIndex', 'type'])) throw new ProviderError('INVALID_RESPONSE');
      return;
    }
    if (eventType === 'text_delta' || eventType === 'thinking_delta') {
      if (!exactKeys(event, ['contentIndex', 'delta', 'type']) || typeof event.delta !== 'string') {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return;
    }
    if (
      !exactKeys(event, ['content', 'contentIndex', 'type']) ||
      typeof event.content !== 'string'
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
  }

  #acceptMessageEnd(record: Readonly<Record<string, unknown>>): void {
    if (
      !exactKeys(record, ['message', 'type']) ||
      !this.#agentActive ||
      !this.#turnActive ||
      this.#openMessageRole === null
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    const message = requireRecord(record.message);
    if (message.role !== this.#openMessageRole) throw new ProviderError('INVALID_RESPONSE');
    this.#openMessageRole = null;
    if (message.role !== 'assistant') {
      if (userMessageText(message) !== this.promptText) throw new ProviderError('INVALID_RESPONSE');
      if (JSON.stringify(message) !== this.#openUserMessageWire) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      this.#openUserMessageWire = null;
      return;
    }
    if (this.#assistantEndedInTurn) throw new ProviderError('INVALID_RESPONSE');
    this.#assistantEndedInTurn = true;
    assertAssistantIdentity(this.expected, message, false);
    const stopReason = message.stopReason;
    if (
      !['stop', 'length', 'toolUse', 'error', 'aborted', 'deferred'].includes(String(stopReason))
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    if (stopReason !== 'stop') validateAssistantBlocks(message, false);
    this.#candidate =
      stopReason === 'stop' ? { text: validateAssistantBlocks(message, true) } : null;
    this.#lastAssistantMessage = message;
    this.timing(PiRpcTimingStage.AssistantMessageEnded);
  }

  #acceptTurnEnd(record: Readonly<Record<string, unknown>>): void {
    if (
      !exactKeys(record, ['message', 'toolResults', 'type']) ||
      !this.#agentActive ||
      !this.#turnActive ||
      this.#openMessageRole !== null ||
      !this.#assistantEndedInTurn ||
      !Array.isArray(record.toolResults) ||
      record.toolResults.length !== 0
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    const message = requireRecord(record.message);
    assertAssistantIdentity(this.expected, message, false);
    // Pi repeats the authoritative message as a fresh JSON object. Object key order is not part of
    // the 0.84.2 event schema, but all values and array order must remain identical.
    if (!isDeepStrictEqual(message, this.#lastAssistantMessage)) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    this.#turnActive = false;
  }

  #acceptAgentEnd(record: Readonly<Record<string, unknown>>): void {
    // Pi 0.84.2's AgentEvent shape contains only type and messages. Some session surfaces may
    // append willRetry, so accept a boolean when present without treating it as authoritative.
    const validKeys =
      exactKeys(record, ['messages', 'type']) ||
      (exactKeys(record, ['messages', 'type', 'willRetry']) &&
        typeof record.willRetry === 'boolean');
    if (
      !validKeys ||
      !this.#agentActive ||
      this.#turnActive ||
      this.#openMessageRole !== null ||
      !Array.isArray(record.messages) ||
      record.messages.length === 0
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    const messages = record.messages as readonly unknown[];
    let lastAssistant: unknown;
    for (let index = messages.length - 1; index >= 0; index -= 1) {
      const message = messages[index];
      if (isRecord(message) && message.role === 'assistant') {
        lastAssistant = message;
        break;
      }
    }
    if (!isRecord(lastAssistant) || !isDeepStrictEqual(lastAssistant, this.#lastAssistantMessage)) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    this.#agentActive = false;
    this.#lastAgentEnded = true;
  }

  #acceptAutoRetryStart(record: Readonly<Record<string, unknown>>): void {
    if (
      !exactKeys(record, ['attempt', 'delayMs', 'errorMessage', 'maxAttempts', 'type']) ||
      !this.#lastAgentEnded ||
      !validPositiveInteger(record.attempt, 100) ||
      !validPositiveInteger(record.maxAttempts, 100) ||
      !validPositiveInteger(record.delayMs, 10 * 60_000) ||
      !validText(record.errorMessage, 16_384, true)
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    this.#sawRetry = true;
    this.#candidate = null;
  }

  #acceptAutoRetryEnd(record: Readonly<Record<string, unknown>>): void {
    if (!this.#sawRetry || typeof record.success !== 'boolean') {
      throw new ProviderError('INVALID_RESPONSE');
    }
    if (record.success) {
      if (
        !exactKeys(record, ['attempt', 'success', 'type']) ||
        !validPositiveInteger(record.attempt, 100)
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return;
    }
    if (
      !exactKeys(record, ['attempt', 'finalError', 'success', 'type']) ||
      !validPositiveInteger(record.attempt, 100) ||
      !validText(record.finalError, 16_384, true)
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
  }

  #acceptAgentSettled(record: Readonly<Record<string, unknown>>): void {
    if (
      !exactKeys(record, ['type']) ||
      this.#agentActive ||
      this.#turnActive ||
      this.#openMessageRole !== null ||
      !this.#lastAgentEnded ||
      this.#agentRuns === 0
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    if (this.#candidate === null) {
      this.onFailure(new ProviderError('REMOTE_FAILURE'));
      return;
    }
    this.onSettled();
    this.timing(PiRpcTimingStage.AgentSettled);
  }
}
