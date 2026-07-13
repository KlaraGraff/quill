import { useState, type DragEvent } from "react";
import { ArrowDown, ArrowUp, GripVertical } from "lucide-react";
import { useTranslation } from "react-i18next";
import {
  MENU_ACTION_DEFINITIONS,
  reorderArray,
  type SelectionMenuItemConfig,
  type SelectionMenuKind,
} from "../learning-card";

interface SelectionMenuSettingsProps {
  kind: SelectionMenuKind;
  value: SelectionMenuItemConfig[];
  onChange: (value: SelectionMenuItemConfig[]) => void;
}

function Switch({ checked, label, onChange }: { checked: boolean; label: string; onChange: (value: boolean) => void }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      onClick={() => onChange(!checked)}
      className={`relative h-6 w-11 shrink-0 rounded-full transition-colors ${checked ? "bg-accent" : "bg-border"}`}
    >
      <span className={`absolute top-0.5 size-5 rounded-full bg-white shadow-sm transition-transform ${checked ? "translate-x-[22px]" : "translate-x-0.5"}`} />
    </button>
  );
}

export default function SelectionMenuSettings({ kind, value, onChange }: SelectionMenuSettingsProps) {
  const { t } = useTranslation();
  const [dragIndex, setDragIndex] = useState<number | null>(null);
  const definitions = new Map(MENU_ACTION_DEFINITIONS[kind].map((item) => [item.id, item]));
  const move = (from: number, to: number) => {
    const next = reorderArray(value, from, to);
    if (next !== value) onChange(next);
  };
  const handleDragStart = (index: number, event: DragEvent<HTMLButtonElement>) => {
    setDragIndex(index);
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", String(index));
  };
  const handleDrop = (index: number, event: DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    const from = dragIndex ?? Number(event.dataTransfer.getData("text/plain"));
    setDragIndex(null);
    if (Number.isSafeInteger(from)) move(from, index);
  };

  return (
    <div className="min-w-0">
      <div className="pb-2">
        <h4 className="text-[12px] font-medium text-text-primary">{t(`settings.tools.menu.${kind}`)}</h4>
        <p className="text-[10px] leading-[1.5] text-text-muted">{t("settings.tools.menu.hint")}</p>
      </div>
      <div className="border-y border-border-light">
        {value.map((item, index) => {
          const definition = definitions.get(item.id);
          if (!definition) return null;
          const label = t(definition.labelKey);
          return (
            <div
              key={item.id}
              onDragOver={(event) => event.preventDefault()}
              onDrop={(event) => handleDrop(index, event)}
              className="flex min-h-12 items-center gap-1 border-t border-border-light py-1 first:border-t-0"
            >
              <button
                type="button"
                draggable
                onDragStart={(event) => handleDragStart(index, event)}
                title={t("settings.tools.reorder")}
                aria-label={t("settings.tools.reorderMenuAction", { name: label })}
                className="flex size-8 shrink-0 cursor-grab items-center justify-center rounded-md text-text-muted hover:bg-bg-input active:cursor-grabbing"
              >
                <GripVertical size={14} />
              </button>
              <span className="min-w-0 flex-1 break-words text-[12px] font-medium text-text-primary">{label}</span>
              <button
                type="button"
                disabled={index === 0}
                onClick={() => move(index, index - 1)}
                title={t("settings.tools.moveUp")}
                aria-label={t("settings.tools.moveMenuActionUp", { name: label })}
                className="flex size-7 items-center justify-center rounded-md text-text-muted hover:bg-bg-input disabled:opacity-25"
              >
                <ArrowUp size={12} />
              </button>
              <button
                type="button"
                disabled={index === value.length - 1}
                onClick={() => move(index, index + 1)}
                title={t("settings.tools.moveDown")}
                aria-label={t("settings.tools.moveMenuActionDown", { name: label })}
                className="flex size-7 items-center justify-center rounded-md text-text-muted hover:bg-bg-input disabled:opacity-25"
              >
                <ArrowDown size={12} />
              </button>
              <Switch
                checked={item.enabled}
                label={t("settings.tools.toggleMenuAction", { name: label })}
                onChange={(enabled) => onChange(value.map((current, itemIndex) => itemIndex === index ? { ...current, enabled } : current))}
              />
            </div>
          );
        })}
      </div>
    </div>
  );
}
