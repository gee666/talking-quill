declare module '@huggingface/transformers' {
  export const env: {
    allowRemoteModels: boolean;
    allowLocalModels: boolean;
    useFSCache: boolean;
    cacheDir: string;
  };

  export class LogitsProcessorList {
    push(processor: (inputIds: unknown, logits: unknown) => unknown): void;
  }

  export function pipeline(
    task: 'automatic-speech-recognition',
    modelId: string,
    options: Readonly<Record<string, unknown>>,
  ): Promise<unknown>;
}
