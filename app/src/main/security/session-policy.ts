import type { Session, WebContents } from 'electron';
import { APP_PROTOCOL } from '../../shared/constants/app';
import { developmentCsp } from './csp';
import type { WindowRoleRegistry } from '../app/window-role-registry';
import type { MicrophonePermissionController } from './microphone-permission';

const ACTIVE_SESSION_POLICY = new WeakMap<Session, () => void>();

export interface TrustedCaptureDocument {
  readonly url: string;
  readonly origin: string;
}

export interface SecureSessionOptions {
  readonly allowWorkers?: boolean;
  readonly microphone?: {
    readonly controller: MicrophonePermissionController;
    readonly getTrustedCaptureDocument: (
      webContents: WebContents | null,
    ) => TrustedCaptureDocument | null;
  };
}

export function secureSession(
  target: Session,
  developmentOrigin: string | null,
  options: SecureSessionOptions = {},
): () => void {
  ACTIVE_SESSION_POLICY.get(target)?.();
  target.setPermissionRequestHandler((webContents, permission, callback, details) => {
    const microphone = options.microphone;
    if (microphone === undefined) {
      callback(false);
      return;
    }
    const document = microphone.getTrustedCaptureDocument(webContents);
    const securityOrigin = readDetailString(details, 'securityOrigin');
    const request = {
      webContentsId: webContents.id,
      permission,
      mediaTypes: readPermissionRequestMediaTypes(details),
      isMainFrame: readDetailBoolean(details, 'isMainFrame'),
      requestingUrl: readDetailString(details, 'requestingUrl'),
      requestingOrigin: securityOrigin,
      securityOrigin,
      embeddingOrigin: readDetailString(details, 'embeddingOrigin'),
      expectedUrl: document?.url ?? null,
      expectedOrigin: document?.origin ?? null,
    };
    const allowed = microphone.controller.allowsRequest(request);
    if (!allowed) microphone.controller.notePolicyDenied(request);
    callback(allowed);
  });
  target.setPermissionCheckHandler((webContents, permission, requestingOrigin, details) => {
    const microphone = options.microphone;
    if (microphone === undefined) return false;
    const document = microphone.getTrustedCaptureDocument(webContents);
    const request = {
      webContentsId: webContents?.id ?? -1,
      permission,
      mediaTypes: readPermissionCheckMediaTypes(details),
      isMainFrame: readDetailBoolean(details, 'isMainFrame'),
      requestingUrl: readDetailString(details, 'requestingUrl'),
      requestingOrigin,
      securityOrigin: readDetailString(details, 'securityOrigin'),
      embeddingOrigin: readDetailString(details, 'embeddingOrigin'),
      expectedUrl: document?.url ?? null,
      expectedOrigin: document?.origin ?? null,
    };
    const allowed = microphone.controller.allowsCheck(request);
    if (!allowed) microphone.controller.notePolicyDenied(request);
    return allowed;
  });
  const preventDownload = (event: Electron.Event): void => event.preventDefault();
  target.on('will-download', preventDownload);

  target.webRequest.onBeforeRequest((details, callback) => {
    const url = new URL(details.url);
    if (
      url.protocol === `${APP_PROTOCOL}:` ||
      (developmentOrigin !== null && url.protocol === 'devtools:')
    ) {
      callback({ cancel: false });
      return;
    }
    if (developmentOrigin !== null && isAllowedDevelopmentUrl(url, developmentOrigin)) {
      callback({ cancel: false });
      return;
    }
    callback({ cancel: true });
  });

  target.webRequest.onHeadersReceived((details, callback) => {
    if (developmentOrigin === null || !details.url.startsWith(developmentOrigin)) {
      callback({ responseHeaders: details.responseHeaders ?? {} });
      return;
    }
    callback({
      responseHeaders: {
        ...details.responseHeaders,
        'Content-Security-Policy': [
          developmentCsp(developmentOrigin, options.allowWorkers === true),
        ],
        'X-Content-Type-Options': ['nosn'],
        'Referrer-Policy': ['no-referrer'],
      },
    });
  });

  const dispose = (): void => {
    if (ACTIVE_SESSION_POLICY.get(target) !== dispose) return;
    ACTIVE_SESSION_POLICY.delete(target);
    target.setPermissionRequestHandler(null);
    target.setPermissionCheckHandler(null);
    target.removeListener('will-download', preventDownload);
    target.webRequest.onBeforeRequest(null);
    target.webRequest.onHeadersReceived(null);
  };
  ACTIVE_SESSION_POLICY.set(target, dispose);
  return dispose;
}

export function getTrustedCaptureDocument(
  webContents: WebContents | null,
  roles: WindowRoleRegistry,
): TrustedCaptureDocument | null {
  if (webContents === null || webContents.isDestroyed()) return null;
  const registration = roles.get(webContents.id);
  if (registration?.role !== 'capture' || webContents.mainFrame.url !== registration.expectedUrl) {
    return null;
  }
  return {
    url: registration.expectedUrl,
    origin: securityOrigin(registration.expectedUrl),
  };
}

export function readPermissionRequestMediaTypes(details: unknown): readonly string[] {
  if (typeof details !== 'object' || details === null) return [];
  const mediaTypes = (details as Readonly<Record<string, unknown>>).mediaTypes;
  if (!Array.isArray(mediaTypes) || !mediaTypes.every((value) => typeof value === 'string')) {
    return [];
  }
  return mediaTypes;
}

export function readPermissionCheckMediaTypes(details: unknown): readonly string[] {
  if (typeof details !== 'object' || details === null) return [];
  const mediaType = (details as Readonly<Record<string, unknown>>).mediaType;
  return typeof mediaType === 'string' ? [mediaType] : [];
}

function readDetailString(details: unknown, key: string): string | null {
  if (typeof details !== 'object' || details === null) return null;
  const value: unknown = (details as Readonly<Record<string, unknown>>)[key];
  return typeof value === 'string' ? value : null;
}

function readDetailBoolean(details: unknown, key: string): boolean {
  return (
    typeof details === 'object' &&
    details !== null &&
    (details as Readonly<Record<string, unknown>>)[key] === true
  );
}

function securityOrigin(input: string): string {
  const url = new URL(input);
  return url.origin === 'null' ? `${url.protocol}//${url.host}` : url.origin;
}

function isAllowedDevelopmentUrl(url: URL, origin: string): boolean {
  const expected = new URL(origin);
  if (!['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname)) return false;
  if (url.protocol === 'ws:' || url.protocol === 'wss:') {
    return url.host === expected.host;
  }
  return url.origin === expected.origin;
}
