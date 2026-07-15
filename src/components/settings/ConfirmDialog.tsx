import { useEffect, useId, useRef } from "react";
import { createPortal } from "react-dom";
import Button from "../ui/Button";

interface ConfirmDialogProps {
  title: string;
  description?: string;
  primaryLabel: string;
  primaryDisabled?: boolean;
  onPrimary: () => void;
  secondaryLabel: string;
  onSecondary: () => void;
  tertiaryLabel?: string;
  onTertiary?: () => void;
}

const FOCUSABLE_SELECTOR = [
  "button:not([disabled])",
  "[href]",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "[tabindex]:not([tabindex='-1'])",
].join(",");

export default function ConfirmDialog({
  title,
  description,
  primaryLabel,
  primaryDisabled = false,
  onPrimary,
  secondaryLabel,
  onSecondary,
  tertiaryLabel,
  onTertiary,
}: ConfirmDialogProps) {
  const titleId = useId();
  const descriptionId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const continueEditing = onTertiary ?? onSecondary;

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    dialog.querySelector<HTMLElement>(FOCUSABLE_SELECTOR)?.focus();
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        continueEditing();
        return;
      }
      if (event.key !== "Tab") return;
      event.stopPropagation();
      const focusable = Array.from(dialog.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR));
      if (focusable.length === 0) return;
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener("keydown", handleKeyDown, true);
    return () => document.removeEventListener("keydown", handleKeyDown, true);
  }, [continueEditing]);

  return createPortal(
    <div
      className="fixed inset-0 z-[100] flex items-center justify-center bg-overlay p-4"
      onMouseDown={(event) => event.target === event.currentTarget && continueEditing()}
    >
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={description ? descriptionId : undefined}
        className="w-[min(420px,calc(100vw-32px))] rounded-md border border-border bg-bg-surface p-5 shadow-context"
      >
        <h2 id={titleId} className="break-words text-[15px] font-semibold text-text-primary">{title}</h2>
        {description && (
          <p id={descriptionId} className="mt-1.5 break-words text-[12px] leading-5 text-text-muted">
            {description}
          </p>
        )}
        <div className="mt-5 flex flex-wrap justify-end gap-2">
          {tertiaryLabel && onTertiary && (
            <Button variant="ghost" size="sm" onClick={onTertiary}>{tertiaryLabel}</Button>
          )}
          <Button variant="secondary" size="sm" onClick={onSecondary}>{secondaryLabel}</Button>
          <Button size="sm" disabled={primaryDisabled} onClick={onPrimary}>{primaryLabel}</Button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
