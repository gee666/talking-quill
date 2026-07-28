import { nativeImage, type NativeImage } from 'electron';

// Electron's nativeImage decodes PNG/JPEG only - it cannot decode SVG, and an
// SVG data URL silently yields an EMPTY image (an invisible but still clickable
// tray icon). The artwork is therefore embedded as real PNG bytes.
//
// Regenerate with: node scripts/generate-tray-icon.mjs

// Windows/Linux: opaque navy tile with a cream quill, legible on light and dark taskbars.
const TRAY_ICON_16_PNG_BASE64 =
  'iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAYAAAAf8/9hAAAATUlEQVR42mNgoBbQMi35TwqmSDOGIcRq+Pb6GhiTZQBMM1kGYNNMtAG4NBNlAD7NBA0gpJloA8iKRmI0EzSA7IRErGayUiJV8wPVcjEAb/aJDK9w1sEAAAAASUVORK5CYII=';
const TRAY_ICON_32_PNG_BASE64 =
  'iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAYAAABzenr0AAAAiElEQVR42u3Xuw2AMAxFUeahZzNmZQl6WqhBQUrs6+fwsZT6nlRxhuGfwozTvEectHAVRBW/RbwOsK3L6UgB17gUUIrLALXxEEBLHAe0xlGAJY4BrHEE4Im7Ad64C0DEUYD0LaDiCED+GpLx5wHouBmQshFF3N4ESNsJI+J9bsXpH5MuvmafnANuJiJnmmYOCwAAAABJRU5ErkJggg==';
// macOS: pure black pixels plus an alpha mask, used as a template image so the
// menu bar tints it for the current appearance.
const TRAY_TEMPLATE_16_PNG_BASE64 =
  'iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAYAAAAf8/9hAAAAH0lEQVR42mNgGHHgPxRTpPn/qGY6hzhF8T0wmocgAAAPwxbqWZ3/YwAAAABJRU5ErkJggg==';
const TRAY_TEMPLATE_32_PNG_BASE64 =
  'iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAYAAABzenr0AAAAO0lEQVR42u3VMQoAMAgEwfv/py+9vSzEXbAetEgSM9urY1C8NF5xcXHxb59Y9IO5hbv93dPj24fGzdZ75alapqlJBsoAAAAASUVORK5CYII=';
// Detail-free opaque squares, only used if a decode ever fails, so the tray is
// never blank. The template variants are fully opaque black so macOS tints them
// instead of rendering a solid navy block over the menu bar.
const TRAY_FALLBACK_16_PNG_BASE64 =
  'iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAYAAAAf8/9hAAAAGUlEQVR42mPQMi35TwlmGDVg1IBRA4aLAQBsb9IQJOdAXAAAAABJRU5ErkJggg==';
const TRAY_FALLBACK_32_PNG_BASE64 =
  'iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAYAAABzenr0AAAAL0lEQVR42u3OIQEAAAgDMPLgaUZ/iHEzMb/q2UsqAQEBAQEBAQEBAQEBAQGBdOABhI5Iai5sKdEAAAAASUVORK5CYII=';
const TRAY_FALLBACK_TEMPLATE_16_PNG_BASE64 =
  'iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAYAAAAf8/9hAAAAGElEQVR42mNgYGD4TyEeNWDUgFEDhocBAJvM/wFK6ATsAAAAAElFTkSuQmCC';
const TRAY_FALLBACK_TEMPLATE_32_PNG_BASE64 =
  'iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAYAAABzenr0AAAAK0lEQVR42u3OIQEAAAwEoetfeovxBoGn6sYEBAQEBAQEBAQEBAQEBAS2gQe3tfwuWQd6sAAAAABJRU5ErkJggg==';

function pngDataUrl(base64: string): string {
  return `data:image/png;base64,${base64}`;
}

export const TRAY_ICON_DATA_URLS = {
  colour16: pngDataUrl(TRAY_ICON_16_PNG_BASE64),
  colour32: pngDataUrl(TRAY_ICON_32_PNG_BASE64),
  template16: pngDataUrl(TRAY_TEMPLATE_16_PNG_BASE64),
  template32: pngDataUrl(TRAY_TEMPLATE_32_PNG_BASE64),
  fallback16: pngDataUrl(TRAY_FALLBACK_16_PNG_BASE64),
  fallback32: pngDataUrl(TRAY_FALLBACK_32_PNG_BASE64),
  fallbackTemplate16: pngDataUrl(TRAY_FALLBACK_TEMPLATE_16_PNG_BASE64),
  fallbackTemplate32: pngDataUrl(TRAY_FALLBACK_TEMPLATE_32_PNG_BASE64),
} as const;

export function createTrayImage(): NativeImage {
  const template = process.platform === 'darwin';
  const base = template ? TRAY_ICON_DATA_URLS.template16 : TRAY_ICON_DATA_URLS.colour16;
  const retina = template ? TRAY_ICON_DATA_URLS.template32 : TRAY_ICON_DATA_URLS.colour32;
  const fallback = template
    ? TRAY_ICON_DATA_URLS.fallbackTemplate16
    : TRAY_ICON_DATA_URLS.fallback16;
  const fallbackRetina = template
    ? TRAY_ICON_DATA_URLS.fallbackTemplate32
    : TRAY_ICON_DATA_URLS.fallback32;

  let image = nativeImage.createFromDataURL(base);
  let hiDpi = retina;
  let asTemplate = template;
  if (image.isEmpty()) {
    image = nativeImage.createFromDataURL(fallback);
    hiDpi = fallbackRetina;
  }
  if (asTemplate && image.isEmpty()) {
    // The template fallback did not decode either. An opaque tile is visible in
    // every appearance, so it beats an invisible-but-clickable tray icon.
    image = nativeImage.createFromDataURL(TRAY_ICON_DATA_URLS.fallback16);
    hiDpi = TRAY_ICON_DATA_URLS.fallback32;
    asTemplate = false;
  }
  if (!image.isEmpty()) {
    // 32x32 as the @2x representation keeps HiDPI tray slots sharp.
    image.addRepresentation({ scaleFactor: 2, dataURL: hiDpi });
  }
  image.setTemplateImage(asTemplate);
  return image;
}
