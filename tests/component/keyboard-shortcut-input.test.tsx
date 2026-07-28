// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { KeyboardShortcutInput } from '../../app/src/renderer/main/settings/KeyboardShortcutInput';
import { shortcutFromLegacyActivation, type Shortcut } from '../../app/src/shared/schemas/shortcut';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function installCaptureApi(start: () => Promise<void>, stop: () => Promise<void>) {
  Object.defineProperty(window, 'talkingQuill', {
    configurable: true,
    value: { shortcutCapture: { start, stop } },
  });
}

function Harness({
  disabled,
  onChange,
  onValidity,
}: {
  readonly disabled: boolean;
  readonly onChange: (shortcut: Shortcut) => void;
  readonly onValidity: (valid: boolean) => void;
}) {
  const [shortcut, setShortcut] = useState(shortcutFromLegacyActivation('Z', false));
  return (
    <KeyboardShortcutInput
      label="Shortcut chord"
      shortcut={shortcut}
      platform="win32"
      disabled={disabled}
      onChange={(next) => {
        setShortcut(next);
        onChange(next);
      }}
      onCaptureValidityChange={onValidity}
    />
  );
}

describe('KeyboardShortcutInput capture lifecycle', () => {
  it('invites capture with a single short hint instead of a paragraph of instructions', () => {
    installCaptureApi(
      vi.fn(() => Promise.resolve()),
      vi.fn(() => Promise.resolve()),
    );
    render(<Harness disabled={false} onChange={vi.fn()} onValidity={vi.fn()} />);

    // The full explanation lives once in the section lead (see
    // dictation-profiles-section.test.tsx), never repeated under every field.
    expect(screen.getByText('Click here and press the shortcut you want.')).toBeVisible();
    expect(screen.queryByText(/keep every letter held down/i)).toBeNull();
    expect(
      screen.queryByText(/keys before the last one still reach whatever app you are in/i),
    ).toBeNull();
  });

  it('requires one nonempty exact modifier mask and fences mismatches until all letters release', async () => {
    const start = vi.fn(() => Promise.resolve());
    const stop = vi.fn(() => Promise.resolve());
    installCaptureApi(start, stop);
    const onChange = vi.fn();
    render(<Harness disabled={false} onChange={onChange} onValidity={vi.fn()} />);
    const input = screen.getByRole('textbox', { name: 'Shortcut chord' });
    fireEvent.focus(input);
    await waitFor(() => expect(input).not.toHaveAttribute('aria-busy'));

    fireEvent.keyDown(input, { key: 'x', code: 'KeyX' });
    fireEvent.keyDown(input, { key: 'p', code: 'KeyP', altKey: true });
    expect(input).toHaveValue('Alt + Z');
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByRole('status')).toHaveTextContent(/Let go of the letters and start again/i);

    fireEvent.keyUp(input, { key: 'p', code: 'KeyP', altKey: true });
    fireEvent.keyUp(input, { key: 'x', code: 'KeyX' });
    fireEvent.keyDown(input, { key: 'x', code: 'KeyX', altKey: true });
    expect(input).toHaveValue('Alt + X');
    fireEvent.keyDown(input, { key: 'p', code: 'KeyP', altKey: true, ctrlKey: true });
    expect(input).toHaveValue('Alt + X');
    expect(screen.getByRole('status')).toHaveTextContent(/same modifier keys the whole time/i);

    fireEvent.keyUp(input, { key: 'p', code: 'KeyP', altKey: true, ctrlKey: true });
    fireEvent.keyUp(input, { key: 'x', code: 'KeyX', altKey: true });
    fireEvent.keyDown(input, { key: 'x', code: 'KeyX', altKey: true });
    fireEvent.keyDown(input, { key: 'p', code: 'KeyP', altKey: true });
    expect(input).toHaveValue('Alt + X + P');
    expect(onChange).toHaveBeenLastCalledWith({
      modifiers: { ctrl: false, alt: true, shift: false, meta: false },
      keys: ['X', 'P'],
    });
  });

  it('fences release and repress of an established modifier until every letter is released', async () => {
    const start = vi.fn(() => Promise.resolve());
    const stop = vi.fn(() => Promise.resolve());
    installCaptureApi(start, stop);
    const onChange = vi.fn();
    render(<Harness disabled={false} onChange={onChange} onValidity={vi.fn()} />);
    const input = screen.getByRole('textbox', { name: 'Shortcut chord' });
    fireEvent.focus(input);
    await waitFor(() => expect(input).not.toHaveAttribute('aria-busy'));

    fireEvent.keyDown(input, { key: 'Alt', code: 'AltLeft', altKey: true });
    fireEvent.keyDown(input, { key: 'x', code: 'KeyX', altKey: true });
    fireEvent.keyUp(input, { key: 'Alt', code: 'AltLeft' });
    fireEvent.keyDown(input, { key: 'Alt', code: 'AltLeft', altKey: true });
    fireEvent.keyDown(input, { key: 'p', code: 'KeyP', altKey: true });

    expect(input).toHaveValue('Alt + X');
    expect(onChange).toHaveBeenCalledOnce();
    expect(screen.getByRole('status')).toHaveTextContent(/same modifier keys the whole time/i);

    fireEvent.keyUp(input, { key: 'p', code: 'KeyP', altKey: true });
    fireEvent.keyUp(input, { key: 'x', code: 'KeyX', altKey: true });
    fireEvent.keyDown(input, { key: 'x', code: 'KeyX', altKey: true });
    fireEvent.keyDown(input, { key: 'p', code: 'KeyP', altKey: true });
    expect(input).toHaveValue('Alt + X + P');
  });

  it('fences a temporary extra modifier even when it is released before the next letter', async () => {
    const start = vi.fn(() => Promise.resolve());
    const stop = vi.fn(() => Promise.resolve());
    installCaptureApi(start, stop);
    const onChange = vi.fn();
    render(<Harness disabled={false} onChange={onChange} onValidity={vi.fn()} />);
    const input = screen.getByRole('textbox', { name: 'Shortcut chord' });
    fireEvent.focus(input);
    await waitFor(() => expect(input).not.toHaveAttribute('aria-busy'));

    fireEvent.keyDown(input, { key: 'x', code: 'KeyX', altKey: true });
    fireEvent.keyDown(input, {
      key: 'Control',
      code: 'ControlLeft',
      altKey: true,
      ctrlKey: true,
    });
    fireEvent.keyUp(input, { key: 'Control', code: 'ControlLeft', altKey: true });
    fireEvent.keyDown(input, { key: 'p', code: 'KeyP', altKey: true });

    expect(input).toHaveValue('Alt + X');
    expect(onChange).toHaveBeenCalledOnce();
    expect(screen.getByRole('status')).toHaveTextContent(/same modifier keys the whole time/i);
  });

  it('releases and resets capture when the browser window loses focus', async () => {
    const start = vi.fn(() => Promise.resolve());
    const stop = vi.fn(() => Promise.resolve());
    installCaptureApi(start, stop);
    render(<Harness disabled={false} onChange={vi.fn()} onValidity={vi.fn()} />);
    const input = screen.getByRole('textbox', { name: 'Shortcut chord' });
    fireEvent.focus(input);
    await waitFor(() => expect(input).not.toHaveAttribute('aria-busy'));
    fireEvent.keyDown(input, { key: 'x', code: 'KeyX', altKey: true });

    fireEvent(window, new Event('blur'));
    await waitFor(() => expect(stop).toHaveBeenCalledOnce());

    fireEvent.focus(input);
    await waitFor(() => expect(start).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(input).not.toHaveAttribute('aria-busy'));
    fireEvent.keyDown(input, { key: 'p', code: 'KeyP', altKey: true });
    expect(input).toHaveValue('Alt + P');
  });

  it('releases capture when disabled and resets held letters across disable, blur, and unmount', async () => {
    const start = vi.fn(() => Promise.resolve());
    const stop = vi.fn(() => Promise.resolve());
    installCaptureApi(start, stop);
    const onChange = vi.fn();
    const onValidity = vi.fn();
    const view = render(<Harness disabled={false} onChange={onChange} onValidity={onValidity} />);

    let input = screen.getByRole('textbox', { name: 'Shortcut chord' });
    fireEvent.focus(input);
    await waitFor(() => expect(input).not.toHaveAttribute('aria-busy'));
    fireEvent.keyDown(input, { key: 'x', code: 'KeyX', altKey: true });
    expect(input).toHaveValue('Alt + X');

    view.rerender(<Harness disabled onChange={onChange} onValidity={onValidity} />);
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));

    view.rerender(<Harness disabled={false} onChange={onChange} onValidity={onValidity} />);
    input = screen.getByRole('textbox', { name: 'Shortcut chord' });
    fireEvent.focus(input);
    await waitFor(() => expect(start).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(input).not.toHaveAttribute('aria-busy'));
    fireEvent.keyDown(input, { key: 'p', code: 'KeyP', altKey: true });
    expect(input).toHaveValue('Alt + P');

    fireEvent.blur(input);
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(2));
    fireEvent.focus(input);
    await waitFor(() => expect(start).toHaveBeenCalledTimes(3));
    view.unmount();
    expect(stop).toHaveBeenCalledTimes(3);
  });

  it('retains and releases a failed capture owner, and announces restoration failures', async () => {
    const start = vi
      .fn<() => Promise<void>>()
      .mockRejectedValueOnce(new Error('activation suspend failed'))
      .mockResolvedValue(undefined);
    const stop = vi
      .fn<() => Promise<void>>()
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error('activation restore failed'))
      .mockResolvedValue(undefined);
    installCaptureApi(start, stop);
    const onValidity = vi.fn();
    render(<Harness disabled={false} onChange={vi.fn()} onValidity={onValidity} />);

    const input = screen.getByRole('textbox', { name: 'Shortcut chord' });
    fireEvent.focus(input);
    await waitFor(() =>
      expect(screen.getByRole('status')).toHaveTextContent(
        'Talking Quill can’t read your keys right now. Try again.',
      ),
    );
    expect(
      screen.getByText('Talking Quill can’t read your keys right now. Try again.', {
        selector: '.me-field__error',
      }),
    ).toBeVisible();
    expect(input).toHaveAttribute('aria-invalid', 'true');
    expect(onValidity).toHaveBeenLastCalledWith(false);

    fireEvent.blur(input);
    await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
    expect(onValidity).toHaveBeenLastCalledWith(true);

    fireEvent.focus(input);
    await waitFor(() => expect(start).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(input).not.toHaveAttribute('aria-busy'));
    fireEvent.blur(input);
    await waitFor(() =>
      expect(screen.getByRole('status')).toHaveTextContent(
        'Your shortcuts couldn’t be switched back on.',
      ),
    );
    expect(
      screen.getByText(
        'Your shortcuts couldn’t be switched back on. Click this field again, then press Tab to leave it.',
        { selector: '.me-field__error' },
      ),
    ).toBeVisible();
    expect(onValidity).toHaveBeenLastCalledWith(false);
  });

  it('stops a pending capture lease when disable races with start completion', async () => {
    let resolveStart!: () => void;
    const start = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveStart = resolve;
        }),
    );
    const stop = vi.fn(() => Promise.resolve());
    installCaptureApi(start, stop);
    const onChange = vi.fn();
    const onValidity = vi.fn();
    const view = render(<Harness disabled={false} onChange={onChange} onValidity={onValidity} />);
    const input = screen.getByRole('textbox', { name: 'Shortcut chord' });
    fireEvent.focus(input);
    await waitFor(() => expect(input).toHaveAttribute('aria-busy', 'true'));

    view.rerender(<Harness disabled onChange={onChange} onValidity={onValidity} />);
    await waitFor(() => expect(stop).toHaveBeenCalledOnce());
    await act(async () => {
      resolveStart();
      await Promise.resolve();
    });
    expect(stop).toHaveBeenCalledOnce();
  });
});
