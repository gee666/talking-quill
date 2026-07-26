import { describe, expect, it } from 'vitest';
import type { VoiceCommand } from '../../app/src/shared/schemas/commands';
import { normalizeCommandText } from '../../app/src/shared/text/command-normalization';
import { findTriggerConflict, matchVoiceCommand } from '../../app/src/main/commands/matcher';

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

  it('preserves meaning-bearing marks in non-Latin scripts', () => {
    const words = ['कल', 'काल', 'कुल', 'केल'];
    expect(new Set(words.map(normalizeCommandText)).size).toBe(words.length);
    expect(matchVoiceCommand('काल', [command(A, 'कल'), command(B, 'काल')])?.command.id).toBe(B);
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

  it('matches the unbounded reference across generated positions around the threshold', () => {
    const transcript = 'abcdefghijklmnopqrst';
    for (let start = 0; start < transcript.length; start += 1) {
      for (let edits = 0; edits <= 4; edits += 1) {
        const points = Array.from(transcript);
        for (let offset = 0; offset < edits; offset += 1) {
          points[(start + offset) % points.length] = '0';
        }
        const trigger = points.join('');
        const reference = levenshteinRatio(transcript, trigger);
        const match = matchVoiceCommand(transcript, [command(A, trigger)]);
        expect(match !== null, `${String(start)}/${String(edits)}`).toBe(reference >= 0.85);
        if (match !== null) expect(match.score).toBe(reference);
      }
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

function levenshteinDistance(left: string, right: string): number {
  const leftPoints = Array.from(left);
  const rightPoints = Array.from(right);
  let previous = Array.from({ length: leftPoints.length + 1 }, (_, index) => index);
  for (let row = 1; row <= rightPoints.length; row += 1) {
    const current = [row];
    for (let column = 1; column <= leftPoints.length; column += 1) {
      current[column] = Math.min(
        (current[column - 1] ?? 0) + 1,
        (previous[column] ?? 0) + 1,
        (previous[column - 1] ?? 0) + (leftPoints[column - 1] === rightPoints[row - 1] ? 0 : 1),
      );
    }
    previous = current;
  }
  return previous[leftPoints.length] ?? 0;
}

function levenshteinRatio(left: string, right: string): number {
  const maximum = Math.max(Array.from(left).length, Array.from(right).length);
  return maximum === 0 ? 1 : 1 - levenshteinDistance(left, right) / maximum;
}
