import type { BrowserWindow } from 'electron';
import { APP_PROTOCOL, RENDERER_PATHS, type WindowRole } from '../../shared/constants/app';

export function selectDevelopmentRendererUrl(
  isPackaged: boolean,
  environmentUrl: string | undefined,
): string | undefined {
  return isPackaged ? undefined : environmentUrl;
}

export class RendererLoader {
  readonly #developmentOrigin: string | null;

  constructor(developmentOrigin: string | undefined) {
    this.#developmentOrigin =
      developmentOrigin === undefined ? null : normalizeOrigin(developmentOrigin);
  }

  get developmentOrigin(): string | null {
    return this.#developmentOrigin;
  }

  get allowsDevTools(): boolean {
    return this.#developmentOrigin !== null;
  }

  urlFor(role: WindowRole): string {
    const path = RENDERER_PATHS[role];
    return this.#developmentOrigin === null
      ? `${APP_PROTOCOL}://app${path}`
      : `${this.#developmentOrigin}${path}`;
  }

  async load(window: BrowserWindow, role: WindowRole): Promise<void> {
    await window.loadURL(this.urlFor(role));
  }
}

function normalizeOrigin(input: string): string {
  const url = new URL(input);
  if (!['http:', 'https:'].includes(url.protocol)) throw new Error('Invalid renderer origin');
  return url.origin;
}
