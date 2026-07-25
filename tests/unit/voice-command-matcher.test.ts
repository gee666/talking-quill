import { describe, expect, it } from 'vitest';
import type { VoiceCommand } from '../../app/src/shared/schemas/commands';
import {
  boundedLevenshteinDistance,
  findTriggerConflict,
  levenshteinDistance,
  levenshteinRatio,
  matchVoiceCommand,
  normalizeCommandText,
} from '../../app/src/main/commands/matcher';

function command(id: string, trigger: string, snippet = 'result'): VoiceCommand {
  return { id, trigger, snippet, createdAt: 1, updatedAt: 1 };
}

const A = '11111111-1111-4111-8111-111111111111';
const B = '22222222-2222-4222-8222-222222222222';

describe('voice command matching', () => {
  it.each([
    ['  RÉSUMÉ, PLEASE! ', 'resume please'],
    ["Don't—forget", 'dontforget'],
    ['co-op', 'coop'],
    ['ŁÓDŹ', 'lodz'],
    ['turn---off', 'turnoff'],
  ])('normalizes punctuation, whitespace, case and diacritics', (input, expected) => {
    expect(normalizeCommandText(input)).toBe(expected);
  });

  it.each([
    ["don't", 'dont'],
    ['co-op', 'coop'],
    ['Łódź', 'lodz'],
  ])('treats compatibility spelling %s and %s as exact', (transcript, trigger) => {
    expect(matchVoiceCommand(transcript, [command(A, trigger)])).toMatchObject({
      kind: 'exact',
      score: 1,
    });
  });

  it('uses Unicode code points for Levenshtein distance', () => {
    expect(levenshteinDistance('😀a', '😀b')).toBe(1);
    expect(levenshteinRatio('12345678901234567890', '12345678901234567abc')).toBe(0.85);
  });

  it('matches exact first, fuzzy inclusively at .85, and rejects immediately below threshold', () => {
    expect(matchVoiceCommand('resume, please!', [command(A, 'résumé please')])).toMatchObject({
      kind: 'exact',
      score: 1,
    });
    expect(
      matchVoiceCommand('12345678901234567abc', [command(A, '12345678901234567890')])?.score,
    ).toBe(0.85);
    expect(
      matchVoiceCommand('1234567890123456abcd', [command(A, '12345678901234567890')]),
    ).toBeNull();
  });

  it('rejects a 10k transcript against 100 commands by the safe length bound', () => {
    const commands = Array.from({ length: 100 }, (_, index) =>
      command(`command-${String(index).padStart(3, '0')}`, `short trigger ${String(index)}`),
    );
    expect(matchVoiceCommand('a'.repeat(10_000), commands)).toBeNull();
  });

  it('rejects many large length mismatches without changing deterministic selection', () => {
    const commands = [
      ...Array.from({ length: 99 }, (_, index) =>
        command(`long-${String(index).padStart(3, '0')}`, `z${'x'.repeat(5_000 + index)}`),
      ),
      command(A, 'target phrase'),
    ];
    expect(matchVoiceCommand('target phrase', commands)?.command.id).toBe(A);
  });

  it('keeps inclusive threshold arithmetic based on Unicode code-point lengths', () => {
    const exactThreshold = `${'😀'.repeat(17)}abc`;
    const belowThreshold = `${'😀'.repeat(16)}abcd`;
    expect(matchVoiceCommand(exactThreshold, [command(A, '😀'.repeat(20))])?.score).toBe(0.85);
    expect(matchVoiceCommand(belowThreshold, [command(A, '😀'.repeat(20))])).toBeNull();
  });

  it('matches bounded distance to the unbounded reference across generated cases', () => {
    const alphabet = ['a', 'b', 'é', '😀'];
    const values = generatedStrings(alphabet, 3);
    for (const left of values) {
      for (const right of values) {
        const reference = levenshteinDistance(left, right);
        for (let limit = 0; limit <= 4; limit += 1) {
          expect(
            boundedLevenshteinDistance(left, right, limit),
            `${left}/${right}/${String(limit)}`,
          ).toBe(reference <= limit ? reference : limit + 1);
        }
      }
    }
  });

  it('preserves optimized matcher equivalence with the unbounded full-transcript reference', () => {
    const pairs = [
      ['hello world', 'hello worle'],
      ['co-op', 'coop'],
      ['Łódź', 'lodz'],
      ['12345678901234567abc', '12345678901234567890'],
      ['1234567890123456abcd', '12345678901234567890'],
      ['short', 'a much longer phrase'],
      ['😀😀😀😀😀😀😀😀😀😀', '😀😀😀😀😀😀😀😀😀x'],
    ] as const;
    for (const [transcript, trigger] of pairs) {
      const normalizedTranscript = normalizeCommandText(transcript);
      const normalizedTrigger = normalizeCommandText(trigger);
      const reference = levenshteinRatio(normalizedTranscript, normalizedTrigger);
      const match = matchVoiceCommand(transcript, [command(A, trigger)]);
      expect(match !== null, `${transcript}/${trigger}`).toBe(reference >= 0.85);
      if (match !== null) expect(match.score).toBe(reference);
    }
  });

  it('compares the full transcript rather than searching token windows', () => {
    expect(
      matchVoiceCommand('please now send weekly report to the whole team', [
        command(A, 'send weekly report'),
      ]),
    ).toBeNull();
  });

  it('selects the best match and resolves equal scores by stable id', () => {
    expect(
      matchVoiceCommand('hello world', [command(B, 'hello worle'), command(A, 'hello worlf')])
        ?.command.id,
    ).toBe(A);
  });

  it('uses the same threshold for save conflicts and excludes the edited command', () => {
    const commands = [command(A, 'launch dashboard')];
    expect(findTriggerConflict('launch dashboart', commands)?.command.id).toBe(A);
    expect(findTriggerConflict('launch dashboard', commands, A)).toBeNull();
  });

  it('does not match punctuation-only text', () => {
    expect(matchVoiceCommand('... —', [command(A, 'anything')])).toBeNull();
  });
});

function generatedStrings(alphabet: readonly string[], maximumLength: number): readonly string[] {
  const values = [''];
  let frontier = [''];
  for (let length = 1; length <= maximumLength; length += 1) {
    frontier = frontier.flatMap((prefix) => alphabet.map((character) => `${prefix}${character}`));
    values.push(...frontier);
  }
  return values;
}
