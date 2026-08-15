import type { ChildProcessWithoutNullStreams } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';

export interface ScriptedPiRpcOptions {
  readonly closeOnStdinEnd?: boolean;
  readonly onCommand?: (
    command: Readonly<Record<string, unknown>>,
    fixture: ScriptedPiRpcFixture,
  ) => void;
}

export class ScriptedPiRpcFixture {
  readonly child: ChildProcessWithoutNullStreams;
  readonly commands: Readonly<Record<string, unknown>>[] = [];
  readonly stdin = new PassThrough();
  readonly stdout = new PassThrough();
  readonly stderr = new PassThrough();
  #input = Buffer.alloc(0);
  #closed = false;

  constructor(options: ScriptedPiRpcOptions = {}) {
    const emitter = new EventEmitter();
    Object.assign(emitter, {
      stdin: this.stdin,
      stdout: this.stdout,
      stderr: this.stderr,
      pid: undefined,
      exitCode: null,
      signalCode: null,
      kill: () => {
        this.close(null, 'SIGKILL');
        return true;
      },
    });
    this.child = emitter as ChildProcessWithoutNullStreams;
    this.stdin.on('data', (chunk: Buffer) => {
      this.#input = Buffer.concat([this.#input, Buffer.from(chunk)]);
      for (;;) {
        const newline = this.#input.indexOf(0x0a);
        if (newline === -1) break;
        const line = this.#input.subarray(0, newline);
        this.#input = this.#input.subarray(newline + 1);
        const command: unknown = JSON.parse(line.toString('utf8'));
        if (typeof command !== 'object' || command === null || Array.isArray(command)) {
          throw new Error('Fixture received a non-object command');
        }
        const record = command as Readonly<Record<string, unknown>>;
        this.commands.push(record);
        options.onCommand?.(record, this);
      }
    });
    this.stdin.once('finish', () => {
      if (options.closeOnStdinEnd !== false) queueMicrotask(() => this.close(0, null));
    });
  }

  send(value: Readonly<Record<string, unknown>>, carriageReturn = false): void {
    this.stdout.write(`${JSON.stringify(value)}${carriageReturn ? '\r' : ''}\n`);
  }

  sendRecords(values: readonly Readonly<Record<string, unknown>>[]): void {
    this.stdout.write(values.map((value) => `${JSON.stringify(value)}\n`).join(''));
  }

  sendBytes(bytes: Uint8Array): void {
    this.stdout.write(Buffer.from(bytes));
  }

  endStdout(): void {
    this.stdout.end();
  }

  close(code: number | null = 0, signal: NodeJS.Signals | null = null): void {
    if (this.#closed) return;
    this.#closed = true;
    this.stdout.end();
    this.stderr.end();
    Object.assign(this.child, { exitCode: code, signalCode: signal });
    this.child.emit('close', code, signal);
  }
}
