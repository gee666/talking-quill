// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { KeyboardShortcutInput } from '../../app/src/renderer/main/settings/KeyboardShortcutInput';
import {
  ShortcutCaptureLeaseIdSchema,
  type ShortcutCaptureLeaseId,
} from '../../app/src/shared/schemas/shortcut-capture';
import { shortcutFromLegacyActivation, type Shortcut } from '../../app/src/shared/schemas/shortcut';

const CAPTURE_LEASE = ShortcutCaptureLeaseIdSchema.parse('33333333-3333-4333-8333-333333333333');
const SECOND_CAPTURE_LEASE = ShortcutCaptureLeaseIdSchema.parse(
  '44444444-4444-4444-8444-444444444444',
);

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function installLeaseApi(
  start: () => Promise<ShortcutCaptureLeaseId>,
  stop: (leaseId: ShortcutCaptureLeaseId) => Promise<void>,
) {
  Object.defineProperty(window, 'talkingQuill', {
    configurable: true,
    value: { shortcutCapture: { start, stop } },
  });
}

function installCaptureApi(
  start: () => Promise<void>,
  stop: (leaseId: ShortcutCaptureLeaseId) => Promise<void>,
) {
  installLeaseApi(() => start().then(() => CAPTURE_LEASE), stop);
}

function Harness({
  disabled = false,
  platform = 'win32',
  onChange = vi.fn(),
  onValidity = vi.fn(),
}: {
  readonly disabled?: boolean;
  readonly platform?: string;
  readonly onChange?: (shortcut: Shortcut) => void;
  readonly onValidity?: (valid: boolean) => void;
}) {
  const [shortcut, setShortcut] = useState(shortcutFromLegacyActivation('Z', false));
  return (
    <KeyboardShortcutInput
      label="Shortcut chord"
      shortcut={shortcut}
      platform={platform}
      disabled={disabled}
      onChange={(next) => {
        setShortcut(next);
        onChange(next);
      }}
      onCaptureValidityChange={onValidity}
    />
  );
}

function DualHarness() {
  const [first, setFirst] = useState(shortcutFromLegacyActivation('A', false));
  const [second, setSecond] = useState(shortcutFromLegacyActivation('B', false));
  return (
    <>
      <KeyboardShortcutInput
        label="First shortcut"
        shortcut={first}
        platform="win32"
        disabled={false}
        onChange={setFirst}
        onCaptureValidityChange={vi.fn()}
      />
      <KeyboardShortcutInput
        label="Second shortcut"
        shortcut={second}
        platform="win32"
        disabled={false}
        onChange={setSecond}
        onCaptureValidityChange={vi.fn()}
      />
    </>
  );
}

async function beginCapture(): Promise<HTMLElement> {
  const input = screen.getByRole('textbox', { name: 'Shortcut chord' });
  fireEvent.focus(input);
  await waitFor(() => expect(input).not.toHaveAttribute('aria-busy'));
  return input;
}

describe('KeyboardShortcutInput', () => {
  it('renders no generated guidance or shared-prefix instructions beneath the field', () => {
    installCaptureApi(
      vi.fn(() => Promise.resolve()),
      vi.fn(() => Promise.resolve()),
    );
    render(<Harness />);

    expect(screen.getByRole('status')).toBeEmptyDOMElement();
    expect(screen.queryByText(/shared prefix/i)).toBeNull();
    expect(screen.queryByText(/release the shorter/i)).toBeNull();
  });

  it('starts exactly one lease for an ordinary mouse click and keeps it until blur', async () => {
    const start = vi.fn(() => Promise.resolve());
    const stop = vi.fn(() => Promise.resolve());
    installCaptureApi(start, stop);
    const user = userEvent.setup();
    render(<Harness />);

    await user.click(screen.getByRole('textbox', { name: 'Shortcut chord' }));
    await waitFor(() => expect(start).toHaveBeenCalledOnce());
    expect(stop).not.toHaveBeenCalled();
  });

  it('captures arbitrary modifier aggregates and unique letters in physical down order', async () => {
    installCaptureApi(
      vi.fn(() => Promise.resolve()),
      vi.fn(() => Promise.resolve()),
    );
    const onChange = vi.fn();
    const onValidity = vi.fn();
    render(<Harness onChange={onChange} onValidity={onValidity} />);
    const input = await beginCapture();
    const modifiers = { ctrlKey: true, altKey: true, shiftKey: true, metaKey: true };

    fireEvent.keyDown(input, { key: 'q', code: 'KeyQ', ...modifiers });
    fireEvent.keyDown(input, { key: 'a', code: 'KeyA', ...modifiers });

    expect(input).toHaveValue('Ctrl + Alt + Shift + Win + Q + A');
    expect(onChange).toHaveBeenLastCalledWith({
      modifiers: { ctrl: true, alt: true, shift: true, meta: true },
      keys: ['Q', 'A'],
    });
    expect(screen.getByRole('status')).toBeEmptyDOMElement();
    expect(onValidity).toHaveBeenLastCalledWith(true);
  });

  it('keeps the previous shortcut after unsupported, duplicate, and released-prefix input', async () => {
    installCaptureApi(
      vi.fn(() => Promise.resolve()),
      vi.fn(() => Promise.resolve()),
    );
    const onChange = vi.fn();
    render(<Harness onChange={onChange} />);
    const input = await beginCapture();

    fireEvent.keyDown(input, { key: 'q', code: 'KeyQ', altKey: true });
    expect(input).toHaveValue('Alt + Q');
    fireEvent.keyDown(input, { key: 'F1', code: 'F1', altKey: true });
    expect(input).toHaveValue('Alt + Z');
    expect(screen.getByRole('status')).toHaveTextContent('Use only the letters A to Z.');

    fireEvent.keyUp(input, { key: 'q', code: 'KeyQ', altKey: true });
    fireEvent.keyDown(input, { key: 'q', code: 'KeyQ', altKey: true });
    fireEvent.keyDown(input, { key: 'a', code: 'KeyA', altKey: true });
    fireEvent.keyUp(input, { key: 'q', code: 'KeyQ', altKey: true });
    fireEvent.keyDown(input, { key: 'q', code: 'KeyQ', altKey: true });
    expect(input).toHaveValue('Alt + Z');
    expect(screen.getByRole('status')).toHaveTextContent('Use each letter only once.');

    fireEvent.keyUp(input, { key: 'q', code: 'KeyQ', altKey: true });
    fireEvent.keyUp(input, { key: 'a', code: 'KeyA', altKey: true });
    fireEvent.keyDown(input, { key: 'q', code: 'KeyQ', altKey: true });
    fireEvent.keyDown(input, { key: 'a', code: 'KeyA', altKey: true });
    fireEvent.keyUp(input, { key: 'q', code: 'KeyQ', altKey: true });
    fireEvent.keyDown(input, { key: 'b', code: 'KeyB', altKey: true });
    expect(input).toHaveValue('Alt + Z');
    expect(screen.getByRole('status')).toHaveTextContent(
      'Hold each earlier letter while pressing the next.',
    );
    expect(onChange).toHaveBeenLastCalledWith(shortcutFromLegacyActivation('Z', false));
  });

  it('requires a stable nonempty modifier mask and restores the previous value on mismatch', async () => {
    installCaptureApi(
      vi.fn(() => Promise.resolve()),
      vi.fn(() => Promise.resolve()),
    );
    const onChange = vi.fn();
    render(<Harness onChange={onChange} />);
    const input = await beginCapture();

    fireEvent.keyDown(input, { key: 'q', code: 'KeyQ' });
    expect(input).toHaveValue('Alt + Z');
    expect(screen.getByRole('status')).toHaveTextContent(
      'Add Ctrl, Alt, Shift, or the Windows key.',
    );
    fireEvent.keyUp(input, { key: 'q', code: 'KeyQ' });

    fireEvent.keyDown(input, { key: 'q', code: 'KeyQ', altKey: true });
    expect(input).toHaveValue('Alt + Q');
    fireEvent.keyDown(input, {
      key: 'a',
      code: 'KeyA',
      altKey: true,
      ctrlKey: true,
    });
    expect(input).toHaveValue('Alt + Z');
    expect(screen.getByRole('status')).toHaveTextContent(
      'Hold the same modifiers for the whole shortcut.',
    );
    expect(onChange).toHaveBeenLastCalledWith(shortcutFromLegacyActivation('Z', false));
  });

  it('rejects Windows-reserved and AltGr input, then accepts risky Shift-only typing with a warning', async () => {
    installCaptureApi(
      vi.fn(() => Promise.resolve()),
      vi.fn(() => Promise.resolve()),
    );
    const onChange = vi.fn();
    render(<Harness onChange={onChange} />);
    const input = await beginCapture();

    fireEvent.keyDown(input, { key: 'l', code: 'KeyL', metaKey: true });
    expect(input).toHaveValue('Alt + Z');
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByRole('status')).toHaveTextContent(/Windows reserves Win \+ L/i);

    fireEvent.keyUp(input, { key: 'l', code: 'KeyL', metaKey: true });
    const altGraph = new KeyboardEvent('keydown', {
      key: 'q',
      code: 'KeyQ',
      ctrlKey: true,
      altKey: true,
      bubbles: true,
      cancelable: true,
    });
    Object.defineProperty(altGraph, 'getModifierState', {
      value: (modifier: string) => modifier === 'AltGraph',
    });
    fireEvent(input, altGraph);
    expect(input).toHaveValue('Alt + Z');
    expect(screen.getByRole('status')).toHaveTextContent(
      'Use physical Ctrl and Alt instead of AltGr.',
    );
    fireEvent.keyUp(input, { key: 'q', code: 'KeyQ', ctrlKey: true, altKey: true });

    fireEvent.keyDown(input, { key: 'a', code: 'KeyA', shiftKey: true });
    expect(input).toHaveValue('Shift + A');
    expect(screen.getByRole('status')).toBeEmptyDOMElement();
  });

  it('keeps the previous value when capture startup or restoration fails', async () => {
    const start = vi
      .fn<() => Promise<void>>()
      .mockRejectedValueOnce(new Error('suspend failed'))
      .mockResolvedValue(undefined);
    const stop = vi
      .fn<(leaseId: ShortcutCaptureLeaseId) => Promise<void>>()
      .mockRejectedValueOnce(new Error('restore failed'))
      .mockResolvedValue(undefined);
    installCaptureApi(start, stop);
    const onChange = vi.fn();
    const onValidity = vi.fn();
    render(<Harness onChange={onChange} onValidity={onValidity} />);

    const input = screen.getByRole('textbox', { name: 'Shortcut chord' });
    fireEvent.focus(input);
    await waitFor(() =>
      expect(screen.getByRole('status')).toHaveTextContent(/previous shortcut was kept/i),
    );
    expect(input).toHaveValue('Alt + Z');
    fireEvent.click(input);
    await waitFor(() => expect(input).not.toHaveAttribute('aria-busy'));
    expect(start).toHaveBeenCalledTimes(2);
    expect(stop).not.toHaveBeenCalled();

    fireEvent.keyDown(input, { key: 'q', code: 'KeyQ', altKey: true });
    expect(input).toHaveValue('Alt + Q');
    fireEvent.blur(input);
    await waitFor(() => expect(input).toHaveValue('Alt + Z'));
    expect(screen.getByRole('status')).toHaveTextContent(/couldn’t be switched back on/i);
    expect(onChange).toHaveBeenLastCalledWith(shortcutFromLegacyActivation('Z', false));
    expect(onValidity).toHaveBeenLastCalledWith(false);

    fireEvent.click(input);
    fireEvent.blur(input);
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(2));
    expect(stop.mock.calls).toEqual([[CAPTURE_LEASE], [CAPTURE_LEASE]]);
    expect(start).toHaveBeenCalledTimes(2);
    expect(onValidity).toHaveBeenLastCalledWith(true);
  });

  it('restores capture on window blur and resets ordered state before the next focus', async () => {
    const start = vi.fn(() => Promise.resolve());
    const stop = vi.fn(() => Promise.resolve());
    installCaptureApi(start, stop);
    render(<Harness />);
    let input = await beginCapture();
    fireEvent.keyDown(input, { key: 'q', code: 'KeyQ', altKey: true });

    fireEvent(window, new Event('blur'));
    await waitFor(() => expect(stop).toHaveBeenCalledOnce());
    input = await beginCapture();
    fireEvent.keyDown(input, { key: 'a', code: 'KeyA', altKey: true });
    expect(input).toHaveValue('Alt + A');
    expect(start).toHaveBeenCalledTimes(2);
  });

  it('fences another editor acquisition behind an unresolved acquisition and restoration', async () => {
    let resolveFirstStart!: (leaseId: ShortcutCaptureLeaseId) => void;
    let resolveFirstStop!: () => void;
    const start = vi
      .fn<() => Promise<ShortcutCaptureLeaseId>>()
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveFirstStart = resolve;
          }),
      )
      .mockResolvedValue(SECOND_CAPTURE_LEASE);
    const stop = vi
      .fn<(leaseId: ShortcutCaptureLeaseId) => Promise<void>>()
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveFirstStop = resolve;
          }),
      )
      .mockResolvedValue(undefined);
    installLeaseApi(start, stop);
    render(<DualHarness />);
    const first = screen.getByRole('textbox', { name: 'First shortcut' });
    const second = screen.getByRole('textbox', { name: 'Second shortcut' });

    fireEvent.focus(first);
    await waitFor(() => expect(start).toHaveBeenCalledOnce());
    fireEvent.blur(first);
    fireEvent.focus(second);
    expect(start).toHaveBeenCalledOnce();

    resolveFirstStart(CAPTURE_LEASE);
    await waitFor(() => expect(stop).toHaveBeenCalledWith(CAPTURE_LEASE));
    expect(start).toHaveBeenCalledOnce();

    resolveFirstStop();
    await waitFor(() => expect(start).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(second).not.toHaveAttribute('aria-busy'));
  });

  it('does not let delayed disabled cleanup release a newer refocused lease', async () => {
    const start = vi
      .fn<() => Promise<ShortcutCaptureLeaseId>>()
      .mockResolvedValueOnce(CAPTURE_LEASE)
      .mockResolvedValue(SECOND_CAPTURE_LEASE);
    const stop = vi.fn<(leaseId: ShortcutCaptureLeaseId) => Promise<void>>(() => Promise.resolve());
    installLeaseApi(start, stop);
    const view = render(<Harness />);
    let input = await beginCapture();

    view.rerender(<Harness disabled />);
    fireEvent.blur(input);
    view.rerender(<Harness />);
    input = screen.getByRole('textbox', { name: 'Shortcut chord' });
    fireEvent.focus(input);
    await waitFor(() => expect(start).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(input).not.toHaveAttribute('aria-busy'));

    expect(stop.mock.calls.every(([leaseId]) => leaseId !== SECOND_CAPTURE_LEASE)).toBe(true);
    fireEvent.blur(input);
    await waitFor(() => expect(stop).toHaveBeenCalledWith(SECOND_CAPTURE_LEASE));
  });

  it('restores a stale pending lease before acquiring a newer focused lease', async () => {
    let resolveFirst!: (leaseId: ShortcutCaptureLeaseId) => void;
    let resolveSecond!: (leaseId: ShortcutCaptureLeaseId) => void;
    const start = vi
      .fn<() => Promise<ShortcutCaptureLeaseId>>()
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveFirst = resolve;
          }),
      )
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveSecond = resolve;
          }),
      );
    const stop = vi.fn(() => Promise.resolve());
    installLeaseApi(start, stop);
    render(<Harness />);
    const input = screen.getByRole('textbox', { name: 'Shortcut chord' });

    fireEvent.focus(input);
    fireEvent.blur(input);
    fireEvent.focus(input);
    await waitFor(() => expect(start).toHaveBeenCalledOnce());

    await act(async () => {
      resolveFirst(CAPTURE_LEASE);
      await Promise.resolve();
    });
    await waitFor(() => expect(stop).toHaveBeenCalledWith(CAPTURE_LEASE));
    await waitFor(() => expect(start).toHaveBeenCalledTimes(2));
    await act(async () => {
      resolveSecond(SECOND_CAPTURE_LEASE);
      await Promise.resolve();
    });
    expect(input).not.toHaveAttribute('aria-busy');

    fireEvent.blur(input);
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(2));
    expect(stop.mock.calls).toEqual([[CAPTURE_LEASE], [SECOND_CAPTURE_LEASE]]);
  });

  it('retains a failed unmount restoration and retries the same lease on window focus', async () => {
    const start = vi.fn(() => Promise.resolve());
    const stop = vi
      .fn<(leaseId: ShortcutCaptureLeaseId) => Promise<void>>()
      .mockRejectedValueOnce(new Error('temporary restoration failure'))
      .mockResolvedValue(undefined);
    installCaptureApi(start, stop);
    const view = render(<Harness />);
    await beginCapture();

    view.unmount();
    await waitFor(() => expect(stop).toHaveBeenCalledOnce());
    fireEvent(window, new Event('focus'));
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(2));
    expect(stop.mock.calls).toEqual([[CAPTURE_LEASE], [CAPTURE_LEASE]]);
  });

  it('waits for a pending start and releases its exact lease once after unmount', async () => {
    let resolveStart!: () => void;
    const start = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveStart = resolve;
        }),
    );
    const stop = vi.fn(() => Promise.resolve());
    installCaptureApi(start, stop);
    const view = render(<Harness />);
    const input = screen.getByRole('textbox', { name: 'Shortcut chord' });
    fireEvent.focus(input);
    await waitFor(() => expect(input).toHaveAttribute('aria-busy', 'true'));

    view.unmount();
    expect(stop).not.toHaveBeenCalled();
    await act(async () => {
      resolveStart();
      await Promise.resolve();
    });
    await waitFor(() => expect(stop).toHaveBeenCalledOnce());
    expect(stop).toHaveBeenCalledWith(CAPTURE_LEASE);
  });
});
