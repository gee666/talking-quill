import { describe, expect, it } from 'vitest';
import {
  InfoLocationSchema,
  InfoPermissionSchema,
  UpdateCheckResultSchema,
} from '../../app/src/shared/schemas/info';

describe('Info IPC schemas', () => {
  it('allowlists only fixed locations and permission panes', () => {
    expect(InfoLocationSchema.safeParse('data').success).toBe(true);
    expect(InfoLocationSchema.safeParse('C:/secret').success).toBe(false);
    expect(InfoPermissionSchema.safeParse('screen-recording').success).toBe(true);
    expect(InfoPermissionSchema.safeParse('camera').success).toBe(false);
  });
  it('requires a bounded HTTPS release result', () => {
    expect(
      UpdateCheckResultSchema.safeParse({
        status: 'current',
        currentVersion: '1.0.0',
        latestVersion: '1.0.0',
        releaseUrl: 'https://github.com/release',
      }).success,
    ).toBe(true);
    expect(
      UpdateCheckResultSchema.safeParse({
        status: 'current',
        currentVersion: '1',
        latestVersion: '1',
        releaseUrl: 'not-url',
      }).success,
    ).toBe(false);
  });
});
