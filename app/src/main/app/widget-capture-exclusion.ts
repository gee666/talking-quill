import type { HelperFrontApp } from '../../shared/helper/protocol';
import type { Settings } from '../../shared/schemas/settings';

export interface WidgetVisibilityLease {
  readonly generation: number;
}

interface WidgetWindowTarget {
  acquireWidgetVisibilityLease(): WidgetVisibilityLease | null;
  hideWidget(preserveInteraction?: boolean): void;
  restoreWidgetVisibility(
    lease: WidgetVisibilityLease,
    size: Settings['app']['widgetSize'],
    targetBounds: HelperFrontApp['windowBounds'],
  ): boolean;
}

/** Keeps the widget out of nested screen captures and restores its prior visibility. */
export class WidgetCaptureExclusion {
  readonly #windows: WidgetWindowTarget;
  readonly #getWidgetSize: () => Settings['app']['widgetSize'];
  readonly #getFrontApp: () => Promise<HelperFrontApp>;
  #exclusions = 0;
  #restoreLease: WidgetVisibilityLease | null = null;

  constructor(options: {
    readonly windows: WidgetWindowTarget;
    readonly getWidgetSize: () => Settings['app']['widgetSize'];
    readonly getFrontApp: () => Promise<HelperFrontApp>;
  }) {
    this.#windows = options.windows;
    this.#getWidgetSize = options.getWidgetSize;
    this.#getFrontApp = options.getFrontApp;
  }

  readonly setExcluded = async (excluded: boolean): Promise<void> => {
    if (excluded) {
      if (this.#exclusions === 0) {
        this.#restoreLease = this.#windows.acquireWidgetVisibilityLease();
        this.#windows.hideWidget(true);
      }
      this.#exclusions += 1;
      return;
    }
    this.#exclusions = Math.max(0, this.#exclusions - 1);
    const lease = this.#restoreLease;
    if (this.#exclusions > 0 || lease === null) return;
    this.#restoreLease = null;
    const front = await this.#getFrontApp().catch(() => null);
    this.#windows.restoreWidgetVisibility(
      lease,
      this.#getWidgetSize(),
      front?.windowBounds ?? null,
    );
  };
}
