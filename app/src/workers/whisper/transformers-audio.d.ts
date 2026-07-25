declare module '@huggingface/transformers' {
  export const env: {
    allowRemoteModels: boolean;
    allowLocalModels: boolean;
    useFSCache: boolean;
    cacheDir: string;
  };

  export function pipeline(
    task: 'automatic-speech-recognition',
    modelId: string,
    options: Readonly<Record<string, unknown>>,
  ): Promise<unknown>;
}
