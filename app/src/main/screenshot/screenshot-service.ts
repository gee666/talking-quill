import { desktopCapturer, screen, systemPreferences, type Rectangle } from 'electron';
import type { HelperFrontApp } from '../../shared/helper/protocol';
import { MAX_PROVIDER_IMAGE_BYTES, type ProviderImage } from '../../shared/schemas/providers';
import { ProviderError } from '../providers/errors';
import { physicalBoundsToDip } from '../app/display-bounds';

export const SCREENSHOT_MAX_EDGE = 1_568;
export const SCREENSHOT_JPEG_QUALITY = 80;
export const SCREENSHOT_NATIVE_CAPTURE_TIMEOUT_MS = 5_000;
export const SCREENSHOT_NATIVE_RECOVERY_COOLDOWN_MS = 30_000;

type NativeCaptureCircuitState = 'closed' | 'open' | 'probing' | 'degraded';

export interface CapturedScreenshot {
  readonly image: ProviderImage;
}

export class ScreenshotService {
  readonly #setWidgetExcluded: (excluded: boolean) => void | Promise<void>;
  readonly #nativeCaptureTimeoutMs: number;
  readonly #nativeRecoveryCooldownMs: number;
  #captureTail: Promise<void> = Promise.resolve();
  #nativeCaptureSettled: Promise<void> | null = null;
  #nativeCircuitState: NativeCaptureCircuitState = 'closed';
  #nativeCircuitOpenedAt = 0;
  #nativeRecoveryProbeUsed = false;

  constructor(
    options: {
      readonly setWidgetExcluded?: (excluded: boolean) => void | Promise<void>;
      readonly nativeCaptureTimeoutMs?: number;
      readonly nativeRecoveryCooldownMs?: number;
    } = {},
  ) {
    this.#setWidgetExcluded = options.setWidgetExcluded ?? (() => undefined);
    this.#nativeCaptureTimeoutMs = Math.max(
      1,
      options.nativeCaptureTimeoutMs ?? SCREENSHOT_NATIVE_CAPTURE_TIMEOUT_MS,
    );
    this.#nativeRecoveryCooldownMs = Math.max(
      1,
      options.nativeRecoveryCooldownMs ?? SCREENSHOT_NATIVE_RECOVERY_COOLDOWN_MS,
    );
  }

  permissionStatus(): 'granted' | 'denied' | 'unknown' {
    if (process.platform !== 'darwin') return 'granted';
    const status = systemPreferences.getMediaAccessStatus('screen');
    return status === 'granted'
      ? 'granted'
      : status === 'denied' || status === 'restricted'
        ? 'denied'
        : 'unknown';
  }

  async capture(
    targetBounds: HelperFrontApp['windowBounds'],
    signal: AbortSignal,
  ): Promise<CapturedScreenshot> {
    const precedingCapture = this.#captureTail;
    let releaseTurn!: () => void;
    const turn = new Promise<void>((resolve) => {
      releaseTurn = resolve;
    });
    this.#captureTail = precedingCapture.then(() => turn);
    try {
      await waitForAbort(precedingCapture, signal);
      return await this.#captureExclusive(targetBounds, signal);
    } finally {
      releaseTurn();
    }
  }

  async #captureExclusive(
    targetBounds: HelperFrontApp['windowBounds'],
    signal: AbortSignal,
  ): Promise<CapturedScreenshot> {
    assertNotAborted(signal);
    if (this.permissionStatus() === 'denied') throw new ProviderError('UNAVAILABLE');
    // Electron does not expose cancellation for getSources. Keep the circuit open while a native
    // orphan is young, then permit one half-open probe. If that probe also wedges, the service
    // remains degraded rather than piling up native calls; settlement closes the circuit again.
    let recoveryProbe = this.#recoveryProbeAllowed();
    const bounds = this.#targetBounds(targetBounds);
    if (bounds === null) throw new ProviderError('UNAVAILABLE');
    const display = screen.getDisplayMatching(bounds);
    const pixelWidth = Math.max(1, Math.round(display.bounds.width * display.scaleFactor));
    const pixelHeight = Math.max(1, Math.round(display.bounds.height * display.scaleFactor));
    const scale = Math.min(1, SCREENSHOT_MAX_EDGE / Math.max(pixelWidth, pixelHeight));
    const thumbnailSize = {
      width: Math.max(1, Math.round(pixelWidth * scale)),
      height: Math.max(1, Math.round(pixelHeight * scale)),
    };
    await this.#setWidgetExcluded(true);
    try {
      await abortableDelay(34, signal);
      if (
        recoveryProbe &&
        (this.#nativeCircuitState !== 'open' || this.#nativeCaptureSettled === null)
      ) {
        // The orphan recovered while the widget was being excluded. Continue as an ordinary
        // capture so a failure cannot strand a stale half-open state without an observer.
        recoveryProbe = false;
      }
      if (recoveryProbe) {
        this.#nativeCircuitState = 'probing';
        this.#nativeRecoveryProbeUsed = true;
      }
      let nativeCapture: ReturnType<typeof desktopCapturer.getSources>;
      let nativeCaptureSettled: Promise<void>;
      try {
        nativeCapture = desktopCapturer.getSources({ types: ['screen'], thumbnailSize });
        nativeCaptureSettled = nativeCapture.then(
          () => undefined,
          () => undefined,
        );
      } catch (error: unknown) {
        if (recoveryProbe) this.#nativeCircuitState = 'degraded';
        throw error;
      }
      // The original orphan remains owned by Electron. Replacing only our observer after the
      // probe starts permits one service-lifetime liveness check without accumulating calls. A
      // synchronous probe failure leaves the original observer able to recover the circuit.
      this.#nativeCaptureSettled = nativeCaptureSettled;
      void nativeCaptureSettled.then(() => {
        if (this.#nativeCaptureSettled === nativeCaptureSettled) {
          this.#nativeCaptureSettled = null;
          this.#nativeCircuitState = 'closed';
        }
      });
      let sources: Awaited<typeof nativeCapture>;
      try {
        sources = await waitForAbort(nativeCapture, signal, this.#nativeCaptureTimeoutMs);
      } catch (error: unknown) {
        if (this.#nativeCaptureSettled === nativeCaptureSettled) {
          if (recoveryProbe || this.#nativeRecoveryProbeUsed) {
            this.#nativeCircuitState = 'degraded';
          } else {
            this.#nativeCircuitState = 'open';
            this.#nativeCircuitOpenedAt = Date.now();
          }
        }
        throw error;
      }
      assertNotAborted(signal);
      const source = sources.find((candidate) => candidate.display_id === String(display.id));
      if (source === undefined) throw new ProviderError('UNAVAILABLE');
      const size = source.thumbnail.getSize();
      const resizeScale = Math.min(1, SCREENSHOT_MAX_EDGE / Math.max(size.width, size.height));
      let image =
        resizeScale < 1
          ? source.thumbnail.resize({
              width: Math.max(1, Math.round(size.width * resizeScale)),
              height: Math.max(1, Math.round(size.height * resizeScale)),
              quality: 'best',
            })
          : source.thumbnail;
      let jpeg = image.toJPEG(SCREENSHOT_JPEG_QUALITY);
      for (let attempt = 0; jpeg.length > MAX_PROVIDER_IMAGE_BYTES && attempt < 6; attempt += 1) {
        const current = image.getSize();
        const reduction = Math.min(0.9, Math.sqrt(MAX_PROVIDER_IMAGE_BYTES / jpeg.length) * 0.95);
        image = image.resize({
          width: Math.max(1, Math.floor(current.width * reduction)),
          height: Math.max(1, Math.floor(current.height * reduction)),
          quality: 'best',
        });
        jpeg = image.toJPEG(SCREENSHOT_JPEG_QUALITY);
      }
      if (jpeg.length === 0 || jpeg.length > MAX_PROVIDER_IMAGE_BYTES) {
        throw new ProviderError('REQUEST_TOO_LARGE');
      }
      return Object.freeze({
        image: Object.freeze({ mimeType: 'image/jpeg' as const, base64: jpeg.toString('base64') }),
      });
    } finally {
      await this.#setWidgetExcluded(false);
    }
  }

  #recoveryProbeAllowed(): boolean {
    if (this.#nativeCircuitState === 'closed') {
      if (this.#nativeCaptureSettled === null) return false;
      throw new ProviderError('UNAVAILABLE');
    }
    if (
      this.#nativeCircuitState === 'open' &&
      !this.#nativeRecoveryProbeUsed &&
      Date.now() - this.#nativeCircuitOpenedAt >= this.#nativeRecoveryCooldownMs
    ) {
      return true;
    }
    throw new ProviderError('UNAVAILABLE');
  }

  #targetBounds(bounds: HelperFrontApp['windowBounds']): Rectangle | null {
    if (bounds === null) return null;
    return process.platform === 'win32'
      ? physicalBoundsToDip(bounds, (point) => screen.screenToDipPoint(point))
      : bounds;
  }
}

function assertNotAborted(signal: AbortSignal): void {
  if (signal.aborted) throw new ProviderError('CANCELLED');
}

function waitForAbort<Value>(
  operation: Promise<Value>,
  signal: AbortSignal,
  timeoutMs?: number,
): Promise<Value> {
  assertNotAborted(signal);
  return new Promise<Value>((resolve, reject) => {
    let finished = false;
    const timer =
      timeoutMs === undefined
        ? null
        : setTimeout(() => finish(() => reject(new ProviderError('TIMEOUT'))), timeoutMs);
    timer?.unref();
    const finish = (callback: () => void): void => {
      if (finished) return;
      finished = true;
      if (timer !== null) clearTimeout(timer);
      signal.removeEventListener('abort', abort);
      callback();
    };
    const abort = (): void => finish(() => reject(new ProviderError('CANCELLED')));
    signal.addEventListener('abort', abort, { once: true });
    void operation.then(
      (value) => finish(() => resolve(value)),
      (error: unknown) =>
        finish(() => reject(error instanceof Error ? error : new ProviderError('UNAVAILABLE'))),
    );
  });
}

function abortableDelay(milliseconds: number, signal: AbortSignal): Promise<void> {
  if (signal.aborted) return Promise.reject(new ProviderError('CANCELLED'));
  return new Promise((resolve, reject) => {
    const finish = (): void => {
      signal.removeEventListener('abort', abort);
      resolve();
    };
    const timer = setTimeout(finish, milliseconds);
    const abort = (): void => {
      clearTimeout(timer);
      signal.removeEventListener('abort', abort);
      reject(new ProviderError('CANCELLED'));
    };
    signal.addEventListener('abort', abort, { once: true });
  });
}
