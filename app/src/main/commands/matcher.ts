import type { VoiceCommand, VoiceCommandMatch } from '../../shared/schemas/commands';
import { normalizeCommandText } from '../../shared/text/command-normalization';

const MATCH_THRESHOLD_NUMERATOR = 85;
const MATCH_THRESHOLD_DENOMINATOR = 100;
const MAX_DISTANCE_NUMERATOR = MATCH_THRESHOLD_DENOMINATOR - MATCH_THRESHOLD_NUMERATOR;

export function matchVoiceCommand(
  transcript: string,
  commands: readonly VoiceCommand[],
): VoiceCommandMatch | null {
  const normalizedTranscript = normalizeCommandText(transcript);
  if (normalizedTranscript.length === 0) return null;
  const transcriptPoints = Array.from(normalizedTranscript);
  let best: VoiceCommandMatch | null = null;
  for (const command of commands) {
    const normalizedTrigger = normalizeCommandText(command.trigger);
    const score = thresholdMatchScore(normalizedTranscript, transcriptPoints, normalizedTrigger);
    if (score === null) continue;
    const candidate: VoiceCommandMatch = {
      command,
      score,
      kind: score === 1 ? 'exact' : 'fuzzy',
    };
    if (
      best === null ||
      candidate.score > best.score ||
      (candidate.score === best.score &&
        compareCodePoints(candidate.command.id, best.command.id) < 0)
    ) {
      best = candidate;
    }
  }
  return best;
}

export function findTriggerConflict(
  trigger: string,
  commands: readonly VoiceCommand[],
  excludedId?: string,
): VoiceCommandMatch | null {
  return matchVoiceCommand(
    trigger,
    commands.filter((command) => command.id !== excludedId),
  );
}

function thresholdMatchScore(
  normalizedTranscript: string,
  transcriptPoints: readonly string[],
  normalizedTrigger: string,
): number | null {
  if (normalizedTranscript === normalizedTrigger) return 1;
  const triggerPoints = Array.from(normalizedTrigger);
  const maximumLength = Math.max(transcriptPoints.length, triggerPoints.length);
  const minimumLength = Math.min(transcriptPoints.length, triggerPoints.length);
  // Levenshtein distance is at least maxLength - minLength. Therefore a candidate whose
  // min/max code-point length ratio is below 85/100 cannot reach the threshold. Integer
  // cross-multiplication keeps the inclusive boundary exact and cannot reject a valid match.
  if (minimumLength * MATCH_THRESHOLD_DENOMINATOR < maximumLength * MATCH_THRESHOLD_NUMERATOR) {
    return null;
  }
  // ratio >= 0.85 iff distance <= floor(maxLength * 15/100). This exact integer budget lets
  // the banded calculation stop as soon as every reachable edit path has exceeded the budget.
  const maximumDistance = Math.floor(
    (maximumLength * MAX_DISTANCE_NUMERATOR) / MATCH_THRESHOLD_DENOMINATOR,
  );
  const distance = boundedCodePointDistance(transcriptPoints, triggerPoints, maximumDistance);
  return distance > maximumDistance ? null : 1 - distance / maximumLength;
}

function boundedCodePointDistance(
  left: readonly string[],
  right: readonly string[],
  maximumDistance: number,
): number {
  const limit = Math.max(0, Math.trunc(maximumDistance));
  if (Math.abs(left.length - right.length) > limit) return limit + 1;
  const columns = left.length <= right.length ? left : right;
  const rows = left.length <= right.length ? right : left;
  const unreachable = limit + 1;
  let previous = Array.from({ length: columns.length + 1 }, (_, index) =>
    index <= limit ? index : unreachable,
  );
  for (let row = 1; row <= rows.length; row += 1) {
    const current = new Array<number>(columns.length + 1).fill(unreachable);
    if (row <= limit) current[0] = row;
    const firstColumn = Math.max(1, row - limit);
    const lastColumn = Math.min(columns.length, row + limit);
    let rowMinimum = current[0] ?? unreachable;
    for (let column = firstColumn; column <= lastColumn; column += 1) {
      const value = Math.min(
        (current[column - 1] ?? unreachable) + 1,
        (previous[column] ?? unreachable) + 1,
        (previous[column - 1] ?? unreachable) + (columns[column - 1] === rows[row - 1] ? 0 : 1),
      );
      current[column] = Math.min(value, unreachable);
      rowMinimum = Math.min(rowMinimum, value);
    }
    if (rowMinimum > limit) return unreachable;
    previous = current;
  }
  return previous[columns.length] ?? unreachable;
}

function compareCodePoints(left: string, right: string): number {
  if (left === right) return 0;
  return left < right ? -1 : 1;
}
