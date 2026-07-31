import type { HelperFrontApp } from '../../shared/helper/protocol';
import type { Settings } from '../../shared/schemas/settings';

interface WidgetWindowTarget {
  isWidgetVisible(): boolean;
  hideWidget(preserveInteraction?: boolean): void;
  showWidget(
    size: Settings['app']['widgetSize'],
    targetBounds: HelperFrontApp['windowBounds'],
  ): void;
}

/** Keeps the widget out of nested screen captures and restores its prior visibility. */
export class WidgetCaptureExclusion {
  readonly #windows: WidgetWindowTarget;
  readonly #getWidgetSize: () => Settings['app']['widgetSize'];
  readonly #getFrontApp: () => Promise<HelperFrontApp>;
  #exclusions = 0;
  #restoreAfterCapture = false;

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
        this.#restoreAfterCapture = this.#windows.isWidgetVisible();
        this.#windows.hideWidget(true);
      }
      this.#exclusions += 1;
      return;
    }
    this.#exclusions = Math.max(0, this.#exclusions - 1);
    if (this.#exclusions > 0 || !this.#restoreAfterCapture) return;
    this.#restoreAfterCapture = false;
    const front = await this.#getFrontApp().catch(() => null);
    this.#windows.showWidget(this.#getWidgetSize(), front?.windowBounds ?? null);
  };
}
