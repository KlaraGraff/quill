import { Bookmark, Check, Copy, MessageSquareMore, StickyNote } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { LearningCardActionId, LearningCardActionState } from "./types";

interface LearningCardActionsProps {
  states?: Partial<Record<LearningCardActionId, LearningCardActionState>>;
  onAction?: (action: LearningCardActionId) => void;
}

const actionOrder: LearningCardActionId[] = ["collect", "ask_ai", "note", "copy"];

export default function LearningCardActions({ states, onAction }: LearningCardActionsProps) {
  const { t } = useTranslation();

  return (
    <div className="flex min-h-11 shrink-0 flex-wrap items-center gap-x-4 gap-y-1 border-t border-border/70 px-4 py-2">
      {actionOrder.map((action) => {
        const state = states?.[action];
        const completed = action === "collect" ? state?.collected : action === "copy" ? state?.copied : false;
        const Icon = completed
          ? Check
          : action === "collect"
            ? Bookmark
            : action === "ask_ai"
              ? MessageSquareMore
              : action === "note"
                ? StickyNote
                : Copy;
        const label = action === "collect" && state?.collected
          ? t("learningCard.actions.collected")
          : action === "copy" && state?.copied
            ? t("learningCard.actions.copied")
            : t(`learningCard.actions.${action}`);

        return (
          <button
            key={action}
            type="button"
            disabled={state?.disabled}
            onClick={() => onAction?.(action)}
            className={`flex min-h-8 items-center gap-1.5 text-[12px] font-medium transition-colors disabled:cursor-default disabled:opacity-40 ${
              action === "collect" ? "text-accent-text" : "text-text-secondary hover:text-accent-text"
            }`}
          >
            <Icon size={14} aria-hidden="true" />
            <span>{label}</span>
          </button>
        );
      })}
    </div>
  );
}
