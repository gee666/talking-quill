import { describe, expect, it } from 'vitest';
import { validateReleaseUrl } from '../../app/src/main/info/release-url-policy';

describe('release URL policy', () => {
  it('accepts only the canonical HTTPS repository release tag URL', () => {
    expect(validateReleaseUrl('https://github.com/gee666/talking-quill/releases/tag/v1.2.3')).toBe(
      'https://github.com/gee666/talking-quill/releases/tag/v1.2.3',
    );
  });

  it.each([
    'http://github.com/gee666/talking-quill/releases/tag/v1.2.3',
    'https://user@github.com/gee666/talking-quill/releases/tag/v1.2.3',
    'https://github.com:443/gee666/talking-quill/releases/tag/v1.2.3',
    'https://github.com.evil.invalid/gee666/talking-quill/releases/tag/v1.2.3',
    'https://github.com/gee666/talking-quill-lookalike/releases/tag/v1.2.3',
    'https://github.com/gee666/talking-quill/releases/tag/v1.2.3?next=evil',
    'https://github.com/gee666/talking-quill/releases/tag/v1.2.3#fragment',
    'https://github.com/gee666/talking-quill/releases%2ftag%2fv1.2.3',
    'https://github.com/gee666/talking-quill/releases/latest',
  ])('rejects %s', (url) => {
    expect(() => validateReleaseUrl(url)).toThrow('rejected');
  });
});
