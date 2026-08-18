import type { HelperFrontApp } from '../../shared/helper/protocol';
import type { Settings } from '../../shared/schemas/settings';

export interface WidgetVisibilityLease {
  readonly generation: number;
}

interface WidgetWindowTarget {
  acquireWidgetVisibilityLease(): WidgetVisibilityLease | null;
  excludeWidgetFromCapture(): void;
  restoreWidgetVisibility(
    lease: WidgetVisibilityLease | null,
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
        this.#windows.excludeWidgetFromCapture();
      }
      this.#exclusions += 1;
      return;
    }
    if (this.#exclusions === 0) return;
    this.#exclusions -= 1;
    const lease = this.#restoreLease;
    if (this.#exclusions > 0) return;
    this.#restoreLease = null;
    // A widget may have been created while a capture that began without one was in flight.
    // Releasing a null lease lets WindowManager show that newly desired widget without moving it.
    const front = lease === null ? null : await this.#getFrontApp().catch(() => null);
    this.#windows.restoreWidgetVisibility(
      lease,
      this.#getWidgetSize(),
      front?.windowBounds ?? null,
    );
  };
}
