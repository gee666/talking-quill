import { describe, expect, it } from 'vitest';
import {
  ApplicationUpdateStateSchema,
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
  it('requires a bounded update state with no installer path or arbitrary payload', () => {
    expect(
      ApplicationUpdateStateSchema.safeParse({
        phase: 'available',
        currentVersion: '1.0.0',
        availableVersion: '1.1.0',
        releaseUrl: 'https://github.com/gee666/talking-quill/releases/tag/v1.1.0',
        percent: null,
        message: null,
        revision: 1,
      }).success,
    ).toBe(true);
    expect(
      ApplicationUpdateStateSchema.safeParse({
        phase: 'available',
        currentVersion: '1.0.0',
        availableVersion: '1.1.0',
        releaseUrl: null,
        percent: null,
        message: null,
        revision: 1,
        installerPath: 'C:/untrusted.exe',
      }).success,
    ).toBe(false);
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
