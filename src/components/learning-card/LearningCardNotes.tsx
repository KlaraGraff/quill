import { Check, Loader2, Pencil, Trash2, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { LearningCardNote } from "./types";

interface LearningCardNotesProps {
  notes?: LearningCardNote[];
  editorOpen?: boolean;
  draft?: string;
  saving?: boolean;
  onDraftChange?: (value: string) => void;
  onSave?: () => void;
  onCancel?: () => void;
  onEdit?: (note: LearningCardNote) => void;
  onDelete?: (note: LearningCardNote) => void;
  onViewAll?: () => void;
  showScope?: boolean;
  scope?: "book" | "global";
  onScopeChange?: (scope: "book" | "global") => void;
}

export default function LearningCardNotes({
  notes = [],
  editorOpen = false,
  draft = "",
  saving = false,
  onDraftChange,
  onSave,
  onCancel,
  onEdit,
  onDelete,
  onViewAll,
  showScope = false,
  scope = "book",
  onScopeChange,
}: LearningCardNotesProps) {
  const { t, i18n } = useTranslation();
  if (notes.length === 0 && !editorOpen) return null;

  const formatter = new Intl.DateTimeFormat(i18n.language, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });

  return (
    <section className="border-t border-border/70 px-4 py-3" aria-labelledby="learning-card-notes-title">
      <div className="mb-2 flex items-center justify-between gap-3">
        <h3 id="learning-card-notes-title" className="text-[12px] font-semibold text-text-primary">
          {t("learningCard.notes.title")}
        </h3>
        {notes.length > 2 && onViewAll && (
          <button type="button" onClick={onViewAll} className="text-[11px] font-medium text-accent-text hover:opacity-70">
            {t("learningCard.notes.viewAll")}
          </button>
        )}
      </div>

      {notes.slice(0, 2).map((note) => (
        <article key={note.id} className="group border-t border-border-light py-2 first:border-t-0 first:pt-0">
          <p className="whitespace-pre-wrap break-words text-[12px] leading-[1.55] text-text-secondary">
            {note.content}
          </p>
          <div className="mt-1.5 flex min-h-6 items-center gap-2 text-[10px] text-text-muted">
            {note.scope && <span>{t(`learningCard.notes.scope.${note.scope}`)}</span>}
            {note.updatedAt && <span>{formatter.format(note.updatedAt)}</span>}
            <div className="ml-auto flex items-center gap-1 opacity-0 transition-opacity group-focus-within:opacity-100 group-hover:opacity-100">
              {onEdit && (
                <button
                  type="button"
                  onClick={() => onEdit(note)}
                  title={t("common.edit")}
                  aria-label={t("common.edit")}
                  className="flex size-6 items-center justify-center rounded-md hover:bg-bg-input"
                >
                  <Pencil size={12} />
                </button>
              )}
              {onDelete && (
                <button
                  type="button"
                  onClick={() => onDelete(note)}
                  title={t("common.delete")}
                  aria-label={t("common.delete")}
                  className="flex size-6 items-center justify-center rounded-md text-danger-text hover:bg-danger-bg"
                >
                  <Trash2 size={12} />
                </button>
              )}
            </div>
          </div>
        </article>
      ))}

      {editorOpen && (
        <div className="mt-2 border-t border-border-light pt-2">
          <label htmlFor="learning-card-note-draft" className="sr-only">
            {t("learningCard.notes.editorLabel")}
          </label>
          <textarea
            id="learning-card-note-draft"
            rows={3}
            autoFocus
            value={draft}
            onChange={(event) => onDraftChange?.(event.target.value)}
            placeholder={t("learningCard.notes.placeholder")}
            className="w-full resize-y rounded-md border border-border bg-bg-input px-3 py-2 text-[12px] leading-[1.5] text-text-primary outline-none placeholder:text-text-placeholder focus:border-accent"
          />
          {showScope ? (
            <div className="mt-2 flex w-fit rounded-md border border-border p-0.5" role="group" aria-label={t("learningCard.notes.scopeLabel")}>
              {(["book", "global"] as const).map((value) => (
                <button
                  key={value}
                  type="button"
                  aria-pressed={scope === value}
                  onClick={() => onScopeChange?.(value)}
                  className={`h-7 rounded-sm px-2 text-[11px] font-medium ${scope === value ? "bg-accent-bg text-accent-text" : "text-text-muted hover:text-text-primary"}`}
                >
                  {t(`learningCard.notes.scope.${value}`)}
                </button>
              ))}
            </div>
          ) : null}
          <div className="mt-2 flex justify-end gap-2">
            {onCancel && (
              <button
                type="button"
                onClick={onCancel}
                className="flex h-8 items-center gap-1.5 px-2 text-[12px] font-medium text-text-muted hover:text-text-primary"
              >
                <X size={13} />
                {t("common.cancel")}
              </button>
            )}
            {onSave && (
              <button
                type="button"
                disabled={saving || draft.trim().length === 0}
                onClick={onSave}
                className="flex h-8 items-center gap-1.5 rounded-md bg-accent px-3 text-[12px] font-medium text-white disabled:cursor-default disabled:opacity-40"
              >
                {saving ? <Loader2 size={13} className="animate-spin" /> : <Check size={13} />}
                {t("common.save")}
              </button>
            )}
          </div>
        </div>
      )}
    </section>
  );
}
