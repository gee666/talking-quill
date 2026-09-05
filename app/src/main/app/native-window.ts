import { app as electronApp, BrowserWindow } from 'electron';
import { join } from 'node:path';
import { CAPTURE_PARTITION, UI_PARTITION, type WindowRole } from '../../shared/constants/app';
import { WIDGET_DIMENSIONS } from '../../shared/constants/echo-session';

export function createNativeWindow(role: WindowRole, allowsDevTools: boolean): BrowserWindow {
  const common: Electron.BrowserWindowConstructorOptions = {
    show: false,
    frame: false,
    backgroundColor: '#161B23',
    icon: electronApp.isPackaged
      ? join(process.resourcesPath, 'app-icon.png')
      : join(electronApp.getAppPath(), 'assets', 'app-icon.png'),
    webPreferences: {
      preload: join(__dirname, '..', 'preload', `${role}.js`),
      sandbox: true,
      contextIsolation: true,
      nodeIntegration: false,
      nodeIntegrationInWorker: false,
      nodeIntegrationInSubFrames: false,
      webSecurity: true,
      allowRunningInsecureContent: false,
      webviewTag: false,
      devTools: allowsDevTools,
      partition: role === 'capture' ? CAPTURE_PARTITION : UI_PARTITION,
      // Keep the active widget responsive while the user's foreground app has focus.
      backgroundThrottling: role === 'main',
    },
  };

  if (role === 'main') {
    return new BrowserWindow({
      ...common,
      title: 'Talking Quill',
      width: 1100,
      height: 720,
      minWidth: 960,
      minHeight: 600,
    });
  }
  if (role === 'widget') {
    return new BrowserWindow({
      ...common,
      title: 'Talking Quill Widget',
      // An opaque background colour defeats `transparent`, so the widget window
      // must clear it for the floating pill to sit directly on the desktop.
      backgroundColor: '#00000000',
      width: WIDGET_DIMENSIONS.default.width,
      height: WIDGET_DIMENSIONS.default.height,
      resizable: false,
      hasShadow: false,
      transparent: true,
      alwaysOnTop: true,
      focusable: false,
      skipTaskbar: true,
    });
  }

  return new BrowserWindow({
    ...common,
    title: 'Talking Quill Capture',
    width: 1,
    height: 1,
    resizable: false,
    focusable: false,
    skipTaskbar: true,
  });
}
