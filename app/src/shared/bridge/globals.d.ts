import type { CaptureApi, MainApi, WidgetApi } from './api';

declare global {
  interface Window {
    readonly talkingQuill: MainApi;
    readonly talkingQuillWidget: WidgetApi;
    readonly talkingQuillCapture: CaptureApi;
  }
}

export {};
