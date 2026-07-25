import type { WebContents } from 'electron';

export function hardenWebContents(webContents: WebContents, expectedUrl: string): void {
  webContents.setWindowOpenHandler(() => ({ action: 'deny' }));
  webContents.on('will-navigate', (event, targetUrl) => {
    if (targetUrl !== expectedUrl) event.preventDefault();
  });
  webContents.on('will-redirect', (event, targetUrl) => {
    if (targetUrl !== expectedUrl) event.preventDefault();
  });
  webContents.on('will-attach-webview', (event) => event.preventDefault());
}
