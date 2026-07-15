import { Keyboard, Plus, Trash2 } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  bindingFromKeyboardEvent,
  formatReaderBinding,
  isReservedReaderBinding,
  type ReaderActionBinding,
  type ReaderActionId,
} from "../reader-bindings";
import type { CardDesignConfigV1 } from "../learning-card";
import Button from "../ui/Button";
import Select from "../ui/Select";

interface ReaderBindingsSettingsProps {
  value: ReaderActionBinding[];
  config: CardDesignConfigV1;
  doubleClickEnabled: boolean;
  previousPageBinding: string;
  nextPageBinding: string;
  onChange: (value: ReaderActionBinding[]) => void;
}

export default function ReaderBindingsSettings({
  value,
  config,
  doubleClickEnabled,
  previousPageBinding,
  nextPageBinding,
  onChange,
}: ReaderBindingsSettingsProps) {
  const { t, i18n } = useTranslation();
  const [recording, setRecording] = useState<number | null>(null);
  const [errors, setErrors] = useState<Record<number, string>>({});
  const [draftAction, setDraftAction] = useState<ReaderActionId>("translate");
  const actions = useMemo(() => {
    const builtIns: Array<{ value: ReaderActionId; label: string }> = [
      "lookup", "translate", "collect", "highlight", "copy", "ask_ai", "explain",
    ].map((id) => ({ value: id as ReaderActionId, label: t(`settings.tools.bindings.actions.${id}`) }));
    const custom = Object.values(config.selectionMenus)
      .flat()
      .filter((item) => item.id.startsWith("custom_") && item.name)
      .filter((item, index, items) => items.findIndex((candidate) => candidate.id === item.id) === index)
      .map((item) => ({ value: item.id as ReaderActionId, label: item.name! }));
    return [...builtIns, ...custom];
  }, [config.selectionMenus, t]);

  useEffect(() => {
    if (recording === null) return;
    const handler = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        setRecording(null);
        return;
      }
      const binding = bindingFromKeyboardEvent(event);
      if (!binding) return;
      event.preventDefault();
      event.stopPropagation();
      const conflict = isReservedReaderBinding(binding)
        ? t("settings.tools.bindings.reserved")
        : binding === previousPageBinding || binding === nextPageBinding
          ? t("settings.tools.bindings.pageConflict")
          : value.some((item, index) => index !== recording && item.trigger === binding)
            ? t("settings.tools.bindings.duplicate")
            : null;
      if (conflict) {
        setErrors((current) => ({ ...current, [recording]: conflict }));
        return;
      }
      setErrors((current) => {
        const next = { ...current };
        delete next[recording];
        return next;
      });
      onChange(value.map((item, index) => index === recording ? { ...item, trigger: binding } : item));
      setRecording(null);
    };
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [nextPageBinding, onChange, previousPageBinding, recording, t, value]);

  const setDoubleClick = (index: number) => {
    const conflict = doubleClickEnabled
      ? t("settings.tools.bindings.doubleClickConflict")
      : value.some((item, itemIndex) => itemIndex !== index && item.trigger === "mouse:double")
        ? t("settings.tools.bindings.duplicate")
        : null;
    if (conflict) {
      setErrors((current) => ({ ...current, [index]: conflict }));
      return;
    }
    onChange(value.map((item, itemIndex) => itemIndex === index ? { ...item, trigger: "mouse:double" } : item));
  };
  const nextTrigger = Array.from({ length: 23 }, (_, index) => `key:F${index + 2}`)
    .find((trigger) => trigger !== previousPageBinding
      && trigger !== nextPageBinding
      && !value.some((binding) => binding.trigger === trigger));

  return (
    <div className="mt-4 border-t border-border-light pt-4">
      <p className="text-[12px] font-medium text-text-primary">{t("settings.tools.bindings.title")}</p>
      <p className="mt-1 text-[11px] leading-[18px] text-text-muted">{t("settings.tools.bindings.hint")}</p>
      <div className="mt-3 space-y-2">
        {value.map((binding, index) => (
          <div key={`${binding.actionId}:${index}`} className="rounded-md border border-border-light px-3 py-2">
            <div className="flex items-center gap-2">
              <Select
                className="min-w-0 flex-1"
                value={binding.actionId}
                onChange={(actionId) => onChange(value.map((item, itemIndex) => itemIndex === index ? { ...item, actionId: actionId as ReaderActionId } : item))}
                options={actions.filter((action) => action.value === binding.actionId || !value.some((item) => item.actionId === action.value))}
              />
              <button
                type="button"
                onClick={() => setRecording(index)}
                className={`flex h-8 min-w-[120px] items-center justify-center gap-1.5 rounded-md border px-2 text-[11px] ${recording === index ? "border-accent bg-accent-bg text-accent-text" : "border-border text-text-secondary"}`}
              >
                <Keyboard size={13} />
                {recording === index ? t("settings.tools.bindings.recording") : formatReaderBinding(binding.trigger, i18n.language)}
              </button>
              <button
                type="button"
                onClick={() => setDoubleClick(index)}
                className="h-8 rounded-md border border-border px-2 text-[11px] text-text-secondary hover:bg-bg-input"
              >{t("settings.tools.bindings.doubleClick")}</button>
              <button
                type="button"
                title={t("common.delete")}
                onClick={() => onChange(value.filter((_, itemIndex) => itemIndex !== index))}
                className="flex size-8 items-center justify-center rounded-md text-text-muted hover:bg-danger-bg hover:text-danger-text"
              ><Trash2 size={13} /></button>
            </div>
            {errors[index] && <p className="mt-1 text-[10px] text-danger-text">{errors[index]}</p>}
          </div>
        ))}
      </div>
      <div className="mt-3 flex items-center gap-2">
        <Select className="min-w-0 flex-1" value={draftAction} onChange={(value) => setDraftAction(value as ReaderActionId)} options={actions} />
        <Button
          variant="secondary"
          size="sm"
          disabled={!nextTrigger || value.some((item) => item.actionId === draftAction)}
          onClick={() => {
            if (nextTrigger) onChange([...value, { actionId: draftAction, trigger: nextTrigger }]);
          }}
        ><Plus size={13} />{t("settings.tools.bindings.add")}</Button>
      </div>
    </div>
  );
}
