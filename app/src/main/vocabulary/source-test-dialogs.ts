import { join } from 'node:path';
import type { VocabularyDialogPort } from './file-service';

export const SOURCE_TEST_VOCABULARY_FILENAME = 'vocabulary-ui-roundtrip.txt';

export function createSourceTestVocabularyDialogs(userDataDirectory: string): VocabularyDialogPort {
  const filePath = join(userDataDirectory, SOURCE_TEST_VOCABULARY_FILENAME);
  return Object.freeze({
    showOpenDialog: () =>
      Promise.resolve({ canceled: false as const, filePaths: [filePath] as const }),
    showSaveDialog: () => Promise.resolve({ canceled: false as const, filePath }),
  });
}
