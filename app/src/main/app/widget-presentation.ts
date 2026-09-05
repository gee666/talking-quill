import { screen, type BrowserWindow } from 'electron';
import type { HelperFrontApp } from '../../shared/helper/protocol';
import type { Settings } from '../../shared/schemas/settings';
import type { WidgetVisibilityLease } from './widget-capture-exclusion';
import { physicalBoundsToDip } from './display-bounds';
import { widgetContentBounds } from './widget-geometry';

interface DesiredWidgetVisibility {
  readonly size: Settings['app']['widgetSize'];
  readonly targetBounds: HelperFrontApp['windowBounds'];
}

// Desired visibility survives renderer replacement; capture leases cannot revive ended sessions.
export class WidgetPresentation {
  #desiredWidgetVisibility: DesiredWidgetVisibility | null = null;
  #widgetVisibilityGeneration = 0;
  #widgetExcludedFromCapture = false;
  readonly #getWindow: () => BrowserWindow | undefined;
  constructor(getWindow: () => BrowserWindow | undefined) {
    this.#getWindow = getWindow;
  }
  get excludedFromCapture(): boolean {
    return this.#widgetExcludedFromCapture;
  }

  showWidget(
    size: Settings['app']['widgetSize'],
    targetBounds: HelperFrontApp['windowBounds'] = null,
  ): boolean {
    this.#widgetVisibilityGeneration += 1;
    this.#setDesiredWidgetVisibility(size, targetBounds);
    return this.showDesiredWidget();
  }

  isWidgetVisible(): boolean {
    const widget = this.#getWindow();
    const visible = widget !== undefined && !widget.isDestroyed() && widget.isVisible();
    // A renderer recovery gap, including a not-yet-loaded replacement, must not make screenshot
    // exclusion forget that an active session expects the widget to become visible.
    return visible || (this.#desiredWidgetVisibility !== null && !this.#widgetExcludedFromCapture);
  }

  acquireWidgetVisibilityLease(): WidgetVisibilityLease | null {
    return this.isWidgetVisible()
      ? Object.freeze({ generation: this.#widgetVisibilityGeneration })
      : null;
  }

  restoreWidgetVisibility(
    lease: WidgetVisibilityLease | null,
    size: Settings['app']['widgetSize'],
    targetBounds: HelperFrontApp['windowBounds'],
  ): boolean {
    if (
      (lease !== null && lease.generation !== this.#widgetVisibilityGeneration) ||
      this.#desiredWidgetVisibility === null
    ) {
      return false;
    }
    if (lease !== null) {
      this.#desiredWidgetVisibility = {
        size,
        targetBounds: targetBounds === null ? null : { ...targetBounds },
      };
    }
    this.#widgetExcludedFromCapture = false;
    return this.showDesiredWidget();
  }

  excludeWidgetFromCapture(): void {
    this.#widgetExcludedFromCapture = true;
    const widget = this.#getWindow();
    if (widget !== undefined && !widget.isDestroyed()) {
      widget.hide();
    }
  }

  removeWidget(): void {
    this.#widgetVisibilityGeneration += 1;
    this.#desiredWidgetVisibility = null;
    this.#widgetExcludedFromCapture = false;
    const widget = this.#getWindow();
    if (widget !== undefined && !widget.isDestroyed()) {
      widget.hide();
    }
  }

  setWidgetInteractive(webContentsId: number, interactive: boolean): void {
    const widget = this.#getWindow();
    if (widget?.webContents.id !== webContentsId || widget.isDestroyed()) return;
    // focusable:false is fixed at construction. Reapplying it to a visible Windows widget
    // calls Electron's native Deactivate(), which can move focus away from the dictation target.
    widget.setIgnoreMouseEvents(!interactive, { forward: !interactive });
  }

  #setDesiredWidgetVisibility(
    size: Settings['app']['widgetSize'],
    targetBounds: HelperFrontApp['windowBounds'],
  ): void {
    this.#desiredWidgetVisibility = {
      size,
      targetBounds: targetBounds === null ? null : { ...targetBounds },
    };
  }

  showDesiredWidget(): boolean {
    const desired = this.#desiredWidgetVisibility;
    const widget = this.#getWindow();
    if (desired === null || widget === undefined || widget.isDestroyed()) return false;
    if (this.#widgetExcludedFromCapture) return true;
    const displayBounds =
      desired.targetBounds !== null && process.platform === 'win32'
        ? physicalBoundsToDip(desired.targetBounds, (point) => screen.screenToDipPoint(point))
        : desired.targetBounds;
    const display =
      displayBounds === null
        ? screen.getDisplayNearestPoint(screen.getCursorScreenPoint())
        : screen.getDisplayMatching(displayBounds);
    widget.setContentBounds(widgetContentBounds(desired.size, display.workArea), false);
    // Electron's default Windows level places the widget behind the taskbar. If the taskbar
    // is temporarily not topmost, that also demotes the widget behind ordinary app windows.
    // Keep it in the topmost band without touching the foreground window.
    widget.setAlwaysOnTop(true, process.platform === 'win32' ? 'screen-saver' : 'floating');
    widget.showInactive();
    widget.webContents.invalidate();
    // Preserve renderer-selected hit testing across screenshot-only hide/show cycles so a
    // stationary pointer can still click Stop or Cancel.
    return widget.isVisible();
  }
}
