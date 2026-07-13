import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { X } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { LearningCardKind, LearningModuleId } from "../learning-card";

interface DensityHelpDialogProps {
  initialKind: LearningCardKind;
  onClose: () => void;
}

const rows: Record<LearningCardKind, LearningModuleId[]> = {
  word: ["word_info", "context_meaning", "common_senses", "morphology", "grammar_role", "tone"],
  phrase: ["context_meaning", "common_senses", "collocations", "grammar_analysis", "idioms", "tone"],
  passage: ["context_meaning", "grammar_analysis", "key_terms", "idioms", "references", "tone"],
};

export default function DensityHelpDialog({ initialKind, onClose }: DensityHelpDialogProps) {
  const { t } = useTranslation();
  const [kind, setKind] = useState(initialKind);
  const dialogRef = useRef<HTMLDivElement>(null);
  const titleId = "density-help-title";

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const focusable = dialog.querySelector<HTMLElement>("button:not([disabled])");
    focusable?.focus();

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onClose();
        return;
      }
      if (event.key !== "Tab") return;
      const elements = [...dialog.querySelectorAll<HTMLElement>(
        "button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex='-1'])",
      )];
      if (elements.length === 0) return;
      const first = elements[0];
      const last = elements[elements.length - 1];
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
  }, [onClose]);

  return createPortal(
    <div className="fixed inset-0 z-[90] flex items-center justify-center bg-overlay p-4" onMouseDown={(event) => event.target === event.currentTarget && onClose()}>
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="flex max-h-[calc(100dvh-32px)] w-[min(900px,calc(100vw-32px))] flex-col overflow-hidden rounded-md border border-border bg-bg-surface shadow-context"
      >
        <header className="flex shrink-0 items-center gap-3 border-b border-border px-5 py-3">
          <div className="min-w-0 flex-1">
            <h2 id={titleId} className="text-[15px] font-semibold text-text-primary">{t("settings.tools.densityHelp.title")}</h2>
            <p className="text-[11px] text-text-muted">{t("settings.tools.densityHelp.subtitle")}</p>
          </div>
          <button
            type="button"
            onClick={onClose}
            title={t("common.close")}
            aria-label={t("common.close")}
            className="flex size-8 items-center justify-center rounded-md text-text-muted hover:bg-bg-input"
          >
            <X size={15} />
          </button>
        </header>

        <div className="flex shrink-0 gap-1 border-b border-border-light px-5 pt-2" role="tablist">
          {(["word", "phrase", "passage"] as LearningCardKind[]).map((item) => (
            <button
              key={item}
              type="button"
              role="tab"
              aria-selected={kind === item}
              onClick={() => setKind(item)}
              className={`h-9 border-b-2 px-3 text-[12px] font-medium ${kind === item ? "border-accent text-accent-text" : "border-transparent text-text-muted"}`}
            >
              {t(`settings.tools.cardKind.${item}`)}
            </button>
          ))}
        </div>

        <div className="min-h-0 overflow-auto p-5">
          <table className="w-full min-w-[680px] table-fixed border-collapse text-left">
            <thead>
              <tr className="border-b border-border">
                <th scope="col" className="w-[18%] px-2 py-2 text-[11px] font-semibold text-text-muted">{t("settings.tools.densityHelp.module")}</th>
                {(["compact", "standard", "detailed"] as const).map((density) => (
                  <th key={density} scope="col" className="px-2 py-2 text-[11px] font-semibold text-text-muted">
                    {t(`settings.tools.density.${density}`)}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows[kind].map((moduleId) => (
                <tr key={moduleId} className="border-b border-border-light align-top last:border-b-0">
                  <th scope="row" className="break-words px-2 py-3 text-[11px] font-semibold text-text-primary">
                    {t(`settings.tools.modules.${moduleId}`)}
                  </th>
                  {(["compact", "standard", "detailed"] as const).map((density) => (
                    <td key={density} className="break-words px-2 py-3 text-[11px] leading-[1.5] text-text-secondary">
                      {t(`settings.tools.densityHelp.examples.${moduleId}.${density}`)}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </div>
    </div>,
    document.body,
  );
}
