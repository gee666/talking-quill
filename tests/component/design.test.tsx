// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  Button,
  Card,
  Dialog,
  EmptyState,
  Input,
  Progress,
  Select,
  Status,
  TextArea,
  Toast,
  Toggle,
} from '../../app/src/renderer/design';

afterEach(cleanup);

describe('design primitives', () => {
  it('renders semantic labelled controls and non-color status text', async () => {
    const user = userEvent.setup();
    const onToggle = vi.fn();
    render(
      <div>
        <Button disabled>Save</Button>
        <Input label="Name" error="Required" />
        <TextArea label="Notes" required />
        <Select label="Mode" defaultValue="raw">
          <option value="raw">Raw</option>
        </Select>
        <Toggle label="Enabled" onChange={onToggle} />
        <Card title="Card">Body</Card>
        <Progress label="Download" value={30} />
        <Status tone="warning">Needs Setup</Status>
        <Toast tone="success" message="Saved" />
        <EmptyState title="Nothing here" description="Add an item." />
      </div>,
    );
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
    expect(screen.getByLabelText('Name')).toHaveAccessibleDescription('Required');
    expect(screen.getByRole('textbox', { name: 'Notes' })).toBeRequired();
    expect(document.querySelector('.me-field__required')).toHaveTextContent('*');
    expect(screen.getByRole('progressbar', { name: 'Download' })).toHaveAttribute('value', '30');
    expect(screen.getByText('Needs Setup')).toBeVisible();
    expect(screen.getByRole('status')).toHaveTextContent('Saved');
    await user.click(screen.getByRole('checkbox', { name: 'Enabled' }));
    expect(onToggle).toHaveBeenCalledOnce();
  });

  it('keeps labels and descriptions intact in both field layouts', () => {
    render(
      <div>
        <Input label="Row input" layout="row" hint="Row hint" />
        <Input label="Stacked input" layout="stacked" hint="Stacked hint" />
        <Select label="Row select" layout="row" hint="Row select hint" defaultValue="raw">
          <option value="raw">Raw</option>
        </Select>
        <Select label="Stacked select" layout="stacked" hint="Stacked select hint">
          <option value="raw">Raw</option>
        </Select>
        <TextArea label="Row notes" layout="row" hint="Row notes hint" />
        <TextArea label="Stacked notes" hint="Stacked notes hint" />
      </div>,
    );
    for (const [name, description, layout] of [
      ['Row input', 'Row hint', 'row'],
      ['Stacked input', 'Stacked hint', 'stacked'],
      ['Row select', 'Row select hint', 'row'],
      ['Stacked select', 'Stacked select hint', 'stacked'],
      ['Row notes', 'Row notes hint', 'row'],
      ['Stacked notes', 'Stacked notes hint', 'stacked'],
    ] as const) {
      const control = screen.getByLabelText(name);
      expect(control).toHaveAccessibleDescription(description);
      expect(control.closest('.me-field')).toHaveClass(`me-field--${layout}`);
    }
  });

  it('activates interactive cards from Enter and Space while respecting disabled state', async () => {
    const user = userEvent.setup();
    const activate = vi.fn();
    const disabledActivate = vi.fn();
    render(
      <div>
        <Card title="Action card" interactive onClick={activate}>
          Run action
        </Card>
        <Card title="Disabled card" interactive disabled onClick={disabledActivate}>
          Unavailable action
        </Card>
      </div>,
    );

    const card = screen.getByRole('button', { name: /Action card/ });
    card.focus();
    await user.keyboard('{Enter}');
    await user.keyboard(' ');
    expect(activate).toHaveBeenCalledTimes(2);
    expect(screen.getByText('Disabled card').closest('[role="button"]')).toHaveAttribute(
      'aria-disabled',
      'true',
    );
    expect(disabledActivate).not.toHaveBeenCalled();
  });

  it('supports modal Escape close and returns focus', async () => {
    const user = userEvent.setup();
    function Harness() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <Button onClick={() => setOpen(true)}>Open dialog</Button>
          <Dialog
            open={open}
            title="Confirm"
            onClose={() => setOpen(false)}
            actions={<Button onClick={() => setOpen(false)}>Done</Button>}
          >
            Secure action
          </Dialog>
        </>
      );
    }
    render(<Harness />);
    const trigger = screen.getByRole('button', { name: 'Open dialog' });
    await user.click(trigger);
    expect(screen.getByRole('dialog')).toHaveAttribute('open');
    fireEvent(screen.getByRole('dialog'), new Event('cancel', { cancelable: true }));
    expect(screen.getByRole('dialog', { hidden: true })).not.toHaveAttribute('open');
    expect(trigger).toHaveFocus();
  });
});
