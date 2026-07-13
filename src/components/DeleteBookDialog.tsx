import { useEffect, useRef, useState } from "react";
import { BookX, FileText, Loader2, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import Button from "./ui/Button";

export type DeleteBookNotePolicy = "delete" | "preserve";

interface DeleteBookDialogProps {
  title: string;
  onCancel: () => void;
  onConfirm: (policy: DeleteBookNotePolicy) => Promise<void>;
}

export default function DeleteBookDialog({ title, onCancel, onConfirm }: DeleteBookDialogProps) {
  const { t } = useTranslation();
  const cancelRef = useRef<HTMLButtonElement>(null);
  const [busy, setBusy] = useState<DeleteBookNotePolicy | null>(null);
  const [error, setError] = useState(false);

  useEffect(() => {
    cancelRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !busy) onCancel();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [busy, onCancel]);

  const confirm = async (policy: DeleteBookNotePolicy) => {
    setBusy(policy);
    setError(false);
    try {
      await onConfirm(policy);
    } catch {
      setError(true);
      setBusy(null);
    }
  };

  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-overlay px-4" onMouseDown={(event) => {
      if (event.target === event.currentTarget && !busy) onCancel();
    }}>
      <div role="dialog" aria-modal="true" aria-labelledby="delete-book-title" className="w-[480px] max-w-full rounded-lg border border-border bg-bg-surface p-5 shadow-popover">
        <div className="flex items-start gap-3">
          <div className="flex size-9 shrink-0 items-center justify-center rounded-md bg-danger-bg text-danger-text">
            <BookX size={18} />
          </div>
          <div className="min-w-0">
            <h2 id="delete-book-title" className="text-[16px] font-semibold text-text-primary">{t("bookDelete.title")}</h2>
            <p className="mt-1 break-words text-[13px] leading-5 text-text-secondary">{t("bookDelete.message", { title })}</p>
          </div>
        </div>

        <div className="mt-5 grid gap-2 sm:grid-cols-2">
          <button type="button" disabled={busy !== null} onClick={() => confirm("preserve")} className="flex min-h-[88px] items-start gap-3 rounded-md border border-border p-3 text-left transition-colors hover:bg-bg-input disabled:opacity-50">
            {busy === "preserve" ? <Loader2 size={17} className="mt-0.5 shrink-0 animate-spin" /> : <FileText size={17} className="mt-0.5 shrink-0 text-accent-text" />}
            <span>
              <span className="block text-[13px] font-medium text-text-primary">{t("bookDelete.preserve")}</span>
              <span className="mt-1 block text-[11px] leading-4 text-text-muted">{t("bookDelete.preserveHint")}</span>
            </span>
          </button>
          <button type="button" disabled={busy !== null} onClick={() => confirm("delete")} className="flex min-h-[88px] items-start gap-3 rounded-md border border-danger-text/30 p-3 text-left transition-colors hover:bg-danger-bg disabled:opacity-50">
            {busy === "delete" ? <Loader2 size={17} className="mt-0.5 shrink-0 animate-spin" /> : <Trash2 size={17} className="mt-0.5 shrink-0 text-danger-text" />}
            <span>
              <span className="block text-[13px] font-medium text-danger-text">{t("bookDelete.deleteNotes")}</span>
              <span className="mt-1 block text-[11px] leading-4 text-text-muted">{t("bookDelete.deleteNotesHint")}</span>
            </span>
          </button>
        </div>

        {error && <p role="alert" className="mt-3 text-[12px] text-danger-text">{t("bookDelete.error")}</p>}
        <div className="mt-4 flex justify-end">
          <Button ref={cancelRef} type="button" variant="ghost" size="md" disabled={busy !== null} onClick={onCancel}>{t("common.cancel")}</Button>
        </div>
      </div>
    </div>
  );
}
