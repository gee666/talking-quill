export class HelperClientError extends Error {
  readonly code:
    'not-running' | 'request-capacity' | 'request-timeout' | 'rpc-error' | 'transport-error';
  readonly rpcCode: number | null;

  constructor(code: HelperClientError['code'], message: string, rpcCode: number | null = null) {
    super(message);
    this.name = 'HelperClientError';
    this.code = code;
    this.rpcCode = rpcCode;
  }
}
