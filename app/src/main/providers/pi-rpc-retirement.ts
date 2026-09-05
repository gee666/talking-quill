import type { ChildProcessWithoutNullStreams } from './pi-rpc-operation';
import { ProviderError } from './errors';
import { terminateProcessTree } from './pi-process-runtime';
import type { PiRpcTransport } from './pi-rpc-transport';
import type { PiRpcPrewarmOptions, TerminatePiRpcTree } from './pi-rpc-types';
import type { validatePrewarmOptions } from './pi-rpc-config';
import { ABORT_REQUEST_ID } from './pi-rpc-responses';
import { exactKeys } from './pi-rpc-validation';
import { deferred, delay } from './pi-rpc-async';

/** Owns cooperative abort, observed process exit, and bounded tree-termination escalation. */
export class PiRpcRetirement {
  readonly #terminateTree: TerminatePiRpcTree;
  readonly #closed = deferred<undefined>();
  readonly #abortResponse = deferred<undefined>();
  readonly #retirementEscalation = deferred<undefined>();
  #closedObserved = false;
  #abortWritten = false;
  #abortResponded = false;

  constructor(
    private readonly child: ChildProcessWithoutNullStreams,
    private readonly transport: PiRpcTransport,
    options: PiRpcPrewarmOptions,
    private readonly validated: ReturnType<typeof validatePrewarmOptions>,
  ) {
    this.#terminateTree = options.terminateTree ?? terminateProcessTree;
  }

  observeClose(): void {
    if (this.#closedObserved) return;
    this.#closedObserved = true;
    this.#closed.resolve(undefined);
  }

  escalate(): void {
    this.#retirementEscalation.resolve(undefined);
  }

  acceptResponse(record: Readonly<Record<string, unknown>>): void {
    if (
      record.type === 'response' &&
      record.id === ABORT_REQUEST_ID &&
      record.command === 'abort' &&
      record.success === true &&
      exactKeys(record, ['command', 'id', 'success', 'type']) &&
      !this.#abortResponded
    ) {
      this.#abortResponded = true;
      this.#abortResponse.resolve(undefined);
    }
  }

  async run(cooperativeAbort: boolean): Promise<void> {
    if (cooperativeAbort && !this.#closedObserved && !this.#abortWritten) {
      this.#abortWritten = true;
      const abortWrite = this.transport
        .write({ id: ABORT_REQUEST_ID, type: 'abort' })
        .catch(() => undefined);
      await Promise.race([
        Promise.all([abortWrite, this.#abortResponse.promise]).then(() => undefined),
        this.#closed.promise,
        delay(this.validated.abortGraceMs),
      ]);
    }
    if (!this.#closedObserved) {
      this.transport.end();
      await Promise.race([
        this.#closed.promise,
        this.#retirementEscalation.promise,
        delay(this.validated.retirementGraceMs),
      ]);
    }
    if (!this.#closedObserved) {
      await Promise.race([
        this.#terminateTree(this.child, this.validated.platform, this.validated.environment),
        delay(this.validated.treeTerminationTimeoutMs).then(() => {
          throw new ProviderError('PI_LAUNCH_FAILED');
        }),
      ]);
    }
  }
}
