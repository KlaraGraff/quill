import { useState, type DragEvent } from "react";
import { ArrowDown, ArrowUp, ChevronDown, ChevronRight, GripVertical } from "lucide-react";
import { useTranslation } from "react-i18next";
import Select from "../ui/Select";
import Toggle from "../ui/Toggle";
import type { CardModuleConfig, LearningModuleDefinition, ModuleDensity } from "../learning-card";
import { ROW_CONTROL_WIDTH } from "./types";

interface CardModuleRowProps {
  definition: LearningModuleDefinition;
  value: CardModuleConfig;
  index: number;
  total: number;
  onChange: (value: CardModuleConfig) => void;
  onMove: (from: number, to: number) => void;
  onDragStart: (index: number, event: DragEvent<HTMLElement>) => void;
  onDrop: (index: number, event: DragEvent<HTMLElement>) => void;
}

export default function CardModuleRow({
  definition,
  value,
  index,
  total,
  onChange,
  onMove,
  onDragStart,
  onDrop,
}: CardModuleRowProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);

  return (
    <div
      onDragOver={(event) => event.preventDefault()}
      onDrop={(event) => onDrop(index, event)}
      className="border-t border-border-light first:border-t-0"
    >
      <div className="flex min-h-12 items-center gap-1 py-1.5">
        {!open ? (
          <button
            type="button"
            draggable
            onDragStart={(event) => onDragStart(index, event)}
            title={t("settings.tools.reorder")}
            aria-label={t("settings.tools.reorderModule", { name: t(definition.labelKey) })}
            className="flex size-8 shrink-0 cursor-grab items-center justify-center rounded-md text-text-muted hover:bg-bg-input active:cursor-grabbing"
          >
            <GripVertical size={14} />
          </button>
        ) : <span className="size-8 shrink-0" />}
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen((value) => !value)}
          className="flex min-w-0 flex-1 items-center gap-2 text-left"
        >
          {open ? <ChevronDown size={14} className="shrink-0 text-text-muted" /> : <ChevronRight size={14} className="shrink-0 text-text-muted" />}
          <span className="min-w-0 flex-1 break-words text-[12px] font-medium text-text-primary">
            {t(definition.labelKey)}
          </span>
          {definition.required && <span className="shrink-0 text-[10px] text-text-muted">{t("settings.tools.required")}</span>}
        </button>
        <button
          type="button"
          disabled={index === 0}
          onClick={() => onMove(index, index - 1)}
          title={t("settings.tools.moveUp")}
          aria-label={t("settings.tools.moveModuleUp", { name: t(definition.labelKey) })}
          className="flex size-7 shrink-0 items-center justify-center rounded-md text-text-muted hover:bg-bg-input disabled:opacity-25"
        >
          <ArrowUp size={12} />
        </button>
        <button
          type="button"
          disabled={index === total - 1}
          onClick={() => onMove(index, index + 1)}
          title={t("settings.tools.moveDown")}
          aria-label={t("settings.tools.moveModuleDown", { name: t(definition.labelKey) })}
          className="flex size-7 shrink-0 items-center justify-center rounded-md text-text-muted hover:bg-bg-input disabled:opacity-25"
        >
          <ArrowDown size={12} />
        </button>
      </div>

      {open && (
        <div className="ml-8 space-y-2 pb-3 pl-5">
          <p className="break-words text-[11px] leading-[1.5] text-text-muted">{t(definition.descriptionKey)}</p>
          <div className="flex min-h-9 items-center justify-between gap-3">
            <span className="text-[12px] text-text-secondary">{t("settings.tools.showModule")}</span>
            <Toggle
              checked={value.enabled}
              disabled={definition.required}
              label={t("settings.tools.toggleModule", { name: t(definition.labelKey) })}
              onChange={(enabled) => onChange({ ...value, enabled })}
            />
          </div>
          <div className="flex min-h-9 items-center justify-between gap-3">
            <span className="text-[12px] text-text-secondary">{t("settings.tools.expandedByDefault")}</span>
            <Toggle
              checked={value.defaultExpanded}
              label={t("settings.tools.toggleExpanded", { name: t(definition.labelKey) })}
              onChange={(defaultExpanded) => onChange({ ...value, defaultExpanded })}
            />
          </div>
          <div className="flex min-h-9 items-center justify-between gap-3">
            <span className="text-[12px] text-text-secondary">{t("settings.tools.moduleDensity")}</span>
            <Select
              className={ROW_CONTROL_WIDTH}
              value={value.density}
              onChange={(density) => onChange({ ...value, density: density as ModuleDensity })}
              options={[
                { value: "inherit", label: t("settings.tools.density.inherit") },
                { value: "compact", label: t("settings.tools.density.compact") },
                { value: "standard", label: t("settings.tools.density.standard") },
                { value: "detailed", label: t("settings.tools.density.detailed") },
              ]}
            />
          </div>
        </div>
      )}
    </div>
  );
}
