import '@testing-library/jest-dom/vitest';
import { configure } from '@testing-library/dom';

// Lazy renderer chunks can take longer than Testing Library's one-second default on shared CI.
configure({ asyncUtilTimeout: 5_000 });

if (typeof HTMLDialogElement !== 'undefined') {
  Object.defineProperty(HTMLDialogElement.prototype, 'showModal', {
    configurable: true,
    value(this: HTMLDialogElement) {
      this.setAttribute('open', '');
    },
  });
  Object.defineProperty(HTMLDialogElement.prototype, 'close', {
    configurable: true,
    value(this: HTMLDialogElement) {
      this.removeAttribute('open');
      this.dispatchEvent(new Event('close'));
    },
  });
}
