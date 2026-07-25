const COMPATIBILITY_LETTERS: Readonly<Record<string, string>> = Object.freeze({
  Ł: 'L',
  ł: 'l',
  Đ: 'D',
  đ: 'd',
  Ø: 'O',
  ø: 'o',
  Æ: 'AE',
  æ: 'ae',
  Œ: 'OE',
  œ: 'oe',
  Ð: 'D',
  ð: 'd',
  Þ: 'TH',
  þ: 'th',
});

export function normalizeCommandText(value: string): string {
  return Array.from(value)
    .map((character) => COMPATIBILITY_LETTERS[character] ?? character)
    .join('')
    .normalize('NFKD')
    .replace(/\p{M}+/gu, '')
    .toLocaleLowerCase('en-US')
    .replace(/[\p{Pd}'’ʼ]+/gu, '')
    .replace(/\p{P}+/gu, ' ')
    .replace(/\s+/gu, ' ')
    .trim();
}

export function isMeaningfulCommandText(value: string): boolean {
  return /[\p{L}\p{N}]/u.test(normalizeCommandText(value));
}
