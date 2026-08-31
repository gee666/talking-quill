export function verifyPublicationHistory(options: {
  readonly candidatePath: string;
  readonly historyIndexPath: string;
  readonly repository: string;
  readonly publicKeyPath: string;
  readonly expectedCandidateTag: string;
  readonly expectedCandidateSequence: number | string;
}): Promise<{ readonly highestSequence: number; readonly accepted: number }>;
