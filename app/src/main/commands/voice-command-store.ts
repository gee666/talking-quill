import { randomUUID } from 'node:crypto';
import {
  VOICE_COMMAND_LIMIT,
  VoiceCommandInputSchema,
  VoiceCommandListSchema,
  VoiceCommandSchema,
  VoiceCommandUpdateSchema,
  type VoiceCommand,
  type VoiceCommandInput,
  type VoiceCommandMatch,
  type VoiceCommandUpdate,
} from '../../shared/schemas/commands';
import { PublicAppError } from '../security/public-error';
import type { SettingsStore } from '../persistence/settings-store';
import { findTriggerConflict, matchVoiceCommand } from './matcher';

export class VoiceCommandStore {
  readonly #settings: SettingsStore;
  #mutationTail: Promise<void> = Promise.resolve();

  constructor(settings: SettingsStore) {
    this.#settings = settings;
  }

  list(): readonly VoiceCommand[] {
    return VoiceCommandListSchema.parse(this.#settings.get().voiceCommands);
  }

  match(transcript: string): VoiceCommandMatch | null {
    return matchVoiceCommand(transcript, this.list());
  }

  create(input: VoiceCommandInput): Promise<VoiceCommand> {
    return this.#mutate(async () => {
      const parsed = VoiceCommandInputSchema.parse(normalizeInput(input));
      const commands = [...this.list()];
      if (commands.length >= VOICE_COMMAND_LIMIT)
        badRequest('The voice command limit has been reached.');
      this.#assertAvailable(parsed.trigger, commands);
      const now = Date.now();
      const command = VoiceCommandSchema.parse({
        ...parsed,
        id: randomUUID(),
        createdAt: now,
        updatedAt: now,
      });
      await this.#settings.update({ voiceCommands: [...commands, command] });
      return command;
    });
  }

  update(id: string, patch: VoiceCommandUpdate): Promise<VoiceCommand> {
    return this.#mutate(async () => {
      const parsed = VoiceCommandUpdateSchema.parse(normalizeInput(patch));
      const commands = [...this.list()];
      const index = commands.findIndex((command) => command.id === id);
      if (index < 0) notFound();
      const current = commands[index];
      if (current === undefined) notFound();
      const candidate = VoiceCommandSchema.parse({ ...current, ...parsed, updatedAt: Date.now() });
      this.#assertAvailable(candidate.trigger, commands, id);
      commands[index] = candidate;
      await this.#settings.update({ voiceCommands: commands });
      return candidate;
    });
  }

  delete(id: string): Promise<boolean> {
    return this.#mutate(async () => {
      const commands = [...this.list()];
      const remaining = commands.filter((command) => command.id !== id);
      if (remaining.length === commands.length) return false;
      await this.#settings.update({ voiceCommands: remaining });
      return true;
    });
  }

  #assertAvailable(trigger: string, commands: readonly VoiceCommand[], excludedId?: string): void {
    const conflict = findTriggerConflict(trigger, commands, excludedId);
    if (conflict !== null)
      badRequest(`This trigger is too similar to “${conflict.command.trigger}”.`);
  }

  #mutate<Value>(operation: () => Promise<Value>): Promise<Value> {
    const result = this.#mutationTail.then(operation, operation);
    this.#mutationTail = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }
}

function normalizeInput(input: {
  readonly trigger?: string | undefined;
  readonly snippet?: string | undefined;
}): { readonly trigger?: string; readonly snippet?: string } {
  return {
    ...(input.trigger === undefined ? {} : { trigger: input.trigger.trim() }),
    ...(input.snippet === undefined ? {} : { snippet: input.snippet.replace(/\r\n?/gu, '\n') }),
  };
}

function badRequest(message: string): never {
  throw new PublicAppError({ code: 'BAD_REQUEST', message });
}
function notFound(): never {
  throw new PublicAppError({ code: 'NOT_FOUND', message: 'The voice command no longer exists.' });
}
