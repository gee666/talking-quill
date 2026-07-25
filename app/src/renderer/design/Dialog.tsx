import { useEffect, useId, useRef, type ReactNode } from 'react';

export interface DialogProps {
  readonly open: boolean;
  readonly title: string;
  readonly description?: string;
  readonly children: ReactNode;
  readonly actions?: ReactNode;
  readonly onClose: () => void;
}

export function Dialog({ open, title, description, children, actions, onClose }: DialogProps) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const returnFocusRef = useRef<HTMLElement | null>(null);
  const titleId = useId();
  const descriptionId = useId();

  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog === null) return;
    if (open && !dialog.open) {
      returnFocusRef.current =
        document.activeElement instanceof HTMLElement ? document.activeElement : null;
      dialog.showModal();
      const focusTarget = dialog.querySelector<HTMLElement>(
        '[data-autofocus], button, input, select',
      );
      focusTarget?.focus();
    } else if (!open && dialog.open) {
      dialog.close();
      returnFocusRef.current?.focus();
    }
  }, [open]);

  return (
    <dialog
      ref={dialogRef}
      className="me-dialog"
      aria-labelledby={titleId}
      aria-describedby={description === undefined ? undefined : descriptionId}
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
      onClose={() => {
        if (open) onClose();
      }}
    >
      <div className="me-dialog__body">
        <h2 id={titleId} className="me-dialog__title">
          {title}
        </h2>
        {description === undefined ? null : (
          <p id={descriptionId} className="me-dialog__description">
            {description}
          </p>
        )}
        <div>{children}</div>
        {actions === undefined ? null : <div className="me-dialog__actions">{actions}</div>}
      </div>
    </dialog>
  );
}
