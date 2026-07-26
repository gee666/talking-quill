import { useEffect, useState } from 'react';
import type { RunnableProviderId } from '../../../shared/schemas/providers';

export type ProviderOperationSlot = 'models' | 'connection' | 'destination';

export interface ProviderLease {
  readonly providerId: RunnableProviderId;
  readonly generation: number;
}

export interface ProviderOperationTicket extends ProviderLease {
  readonly slot: ProviderOperationSlot;
  readonly operationId: string;
}

export interface ProviderSelectionTicket extends ProviderLease {
  readonly targetProviderId: RunnableProviderId;
  readonly sequence: number;
}

export class ProviderUiCoordinator {
  private providerId: RunnableProviderId;
  private generation = 0;
  private operationSequence = 0;
  private selectionSequence = 0;
  private disposed = false;
  private pendingSelection: ProviderSelectionTicket | null = null;
  private readonly operations: Record<ProviderOperationSlot, ProviderOperationTicket | null> = {
    models: null,
    connection: null,
    destination: null,
  };

  public constructor(initialProviderId: RunnableProviderId) {
    this.providerId = initialProviderId;
  }

  public current(): ProviderLease {
    return { providerId: this.providerId, generation: this.generation };
  }

  public isCurrent(lease: ProviderLease): boolean {
    return (
      !this.disposed && lease.providerId === this.providerId && lease.generation === this.generation
    );
  }

  public isActiveProvider(providerId: RunnableProviderId): boolean {
    return !this.disposed && this.providerId === providerId;
  }

  public createOperationId(providerId: RunnableProviderId, kind: string): string {
    this.operationSequence += 1;
    return `${providerId}-${kind}-${Date.now().toString(36)}-${this.operationSequence.toString(36)}`;
  }

  public invalidate(): ProviderLease {
    if (!this.disposed) this.generation += 1;
    this.cancelAllOperations();
    return this.current();
  }

  public beginSelection(targetProviderId: RunnableProviderId): ProviderSelectionTicket | null {
    if (this.disposed || this.pendingSelection !== null) return null;
    const lease = this.invalidate();
    this.selectionSequence += 1;
    const ticket = {
      ...lease,
      targetProviderId,
      sequence: this.selectionSequence,
    };
    this.pendingSelection = ticket;
    return ticket;
  }

  public hasPendingSelection(): boolean {
    return this.pendingSelection !== null;
  }

  public isCurrentSelection(ticket: ProviderSelectionTicket): boolean {
    return (
      this.pendingSelection?.sequence === ticket.sequence &&
      this.pendingSelection.targetProviderId === ticket.targetProviderId &&
      this.isCurrent(ticket)
    );
  }

  public commitSelection(ticket: ProviderSelectionTicket): ProviderLease | null {
    if (!this.isCurrentSelection(ticket)) return null;
    this.providerId = ticket.targetProviderId;
    this.pendingSelection = null;
    return this.current();
  }

  public rejectSelection(ticket: ProviderSelectionTicket): ProviderLease | null {
    if (!this.isCurrentSelection(ticket)) return null;
    this.pendingSelection = null;
    return this.current();
  }

  public adoptProvider(providerId: RunnableProviderId): ProviderLease | null {
    if (this.disposed) return null;
    this.generation += 1;
    this.pendingSelection = null;
    this.cancelAllOperations();
    this.providerId = providerId;
    return this.current();
  }

  public beginOperation(
    slot: ProviderOperationSlot,
    providerId: RunnableProviderId,
    kind: string,
  ): ProviderOperationTicket | null {
    if (this.disposed) return null;
    const lease = this.current();
    if (lease.providerId !== providerId) return null;
    this.cancelOperation(slot);
    const ticket = {
      ...lease,
      slot,
      operationId: this.createOperationId(providerId, kind),
    };
    this.operations[slot] = ticket;
    return ticket;
  }

  public isCurrentOperation(ticket: ProviderOperationTicket): boolean {
    return (
      this.operations[ticket.slot]?.operationId === ticket.operationId && this.isCurrent(ticket)
    );
  }

  public finishOperation(ticket: ProviderOperationTicket): void {
    if (this.operations[ticket.slot]?.operationId === ticket.operationId) {
      this.operations[ticket.slot] = null;
    }
  }

  public cancelOperation(slot: ProviderOperationSlot): string | null {
    const ticket = this.operations[slot];
    this.operations[slot] = null;
    if (ticket !== null) {
      void window.talkingQuill.providers.cancel(ticket.operationId).catch(() => undefined);
    }
    return ticket?.operationId ?? null;
  }

  public dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.generation += 1;
    this.pendingSelection = null;
    this.cancelAllOperations();
  }

  private cancelAllOperations(): void {
    for (const slot of ['models', 'connection', 'destination'] as const) {
      this.cancelOperation(slot);
    }
  }
}

export function useProviderUiCoordinator(
  initialProviderId: RunnableProviderId,
): ProviderUiCoordinator {
  const [coordinator] = useState(() => new ProviderUiCoordinator(initialProviderId));

  useEffect(() => () => coordinator.dispose(), [coordinator]);

  return coordinator;
}
