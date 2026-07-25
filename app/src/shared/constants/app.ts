export const APP_ID = 'com.talkingquill.app';
export const APP_NAME = 'Talking Quill';
export const APP_PROTOCOL = 'talking-quill';
export const UI_PARTITION = 'persist:talking-quill-ui';
export const CAPTURE_PARTITION = 'persist:talking-quill-capture';

export const WINDOW_ROLES = ['main', 'widget', 'capture'] as const;
export type WindowRole = (typeof WINDOW_ROLES)[number];

export const RENDERER_PATHS: Readonly<Record<WindowRole, string>> = Object.freeze({
  main: '/main/index.html',
  widget: '/widget/index.html',
  capture: '/capture/index.html',
});
