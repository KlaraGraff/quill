import { useState, type DragEvent } from "react";
import { CircleHelp } from "lucide-react";
import { useTranslation } from "react-i18next";
import Select from "../ui/Select";
import {
  MODULE_DEFINITIONS,
  reorderArray,
  type CardKindConfig,
  type CardModuleConfig,
  type CardWidthMode,
  type ContentDensity,
  type LearningCardKind,
} from "../learning-card";
import CardModuleRow from "./CardModuleRow";

interface CardDesignSettingsProps {
  kind: LearningCardKind;
  value: CardKindConfig;
  onChange: (value: CardKindConfig) => void;
  onOpenDensityHelp: () => void;
}

export default function CardDesignSettings({ kind, value, onChange, onOpenDensityHelp }: CardDesignSettingsProps) {
  const { t } = useTranslation();
  const [dragIndex, setDragIndex] = useState<number | null>(null);
  const definitions = new Map(MODULE_DEFINITIONS[kind].map((item) => [item.id, item]));

  const updateModule = (index: number, module: CardModuleConfig) => {
    const modules = value.modules.map((item, itemIndex) => itemIndex === index ? module : item);
    onChange({ ...value, modules });
  };
  const move = (from: number, to: number) => {
    const modules = reorderArray(value.modules, from, to);
    if (modules !== value.modules) onChange({ ...value, modules });
  };
  const handleDragStart = (index: number, event: DragEvent<HTMLElement>) => {
    setDragIndex(index);
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", String(index));
  };
  const handleDrop = (index: number, event: DragEvent<HTMLElement>) => {
    event.preventDefault();
    const from = dragIndex ?? Number(event.dataTransfer.getData("text/plain"));
    setDragIndex(null);
    if (Number.isSafeInteger(from)) move(from, index);
  };

  return (
    <div className="min-w-0">
      <div className="flex min-h-12 items-center justify-between gap-4 border-b border-border-light py-2">
        <div className="min-w-0">
          <div className="flex items-center gap-1.5">
            <span className="text-[12px] font-medium text-text-primary">{t("settings.tools.defaultDensity")}</span>
            <button
              type="button"
              onClick={onOpenDensityHelp}
              title={t("settings.tools.densityHelp.open")}
              aria-label={t("settings.tools.densityHelp.open")}
              className="flex size-6 items-center justify-center rounded-md text-text-muted hover:bg-bg-input"
            >
              <CircleHelp size={13} />
            </button>
          </div>
          <p className="text-[10px] text-text-muted">{t(`settings.tools.densitySummary.${value.defaultDensity}`)}</p>
        </div>
        <div className="flex shrink-0 rounded-md bg-bg-input p-0.5">
          {(["compact", "standard", "detailed"] as ContentDensity[]).map((density) => (
            <button
              key={density}
              type="button"
              aria-pressed={value.defaultDensity === density}
              onClick={() => onChange({ ...value, defaultDensity: density })}
              className={`h-7 px-2 text-[11px] font-medium ${value.defaultDensity === density ? "rounded-sm bg-bg-surface text-accent-text shadow-sm" : "text-text-muted"}`}
            >
              {t(`settings.tools.density.${density}`)}
            </button>
          ))}
        </div>
      </div>

      <div className="flex min-h-12 items-center justify-between gap-4 border-b border-border-light py-2">
        <div>
          <p className="text-[12px] font-medium text-text-primary">{t("settings.tools.cardWidth")}</p>
          <p className="text-[10px] text-text-muted">{t("settings.tools.cardWidthHint")}</p>
        </div>
        <Select
          className="w-[120px] shrink-0"
          value={value.widthMode}
          onChange={(widthMode) => onChange({ ...value, widthMode: widthMode as CardWidthMode })}
          options={[
            { value: "auto", label: t("settings.tools.width.auto") },
            { value: "compact", label: t("settings.tools.width.compact") },
            { value: "wide", label: t("settings.tools.width.wide") },
          ]}
        />
      </div>

      <div className="flex min-h-12 items-center justify-between gap-4 border-b border-border-light py-2">
        <div>
          <p className="text-[12px] font-medium text-text-primary">{t("settings.tools.exampleCount")}</p>
          <p className="text-[10px] text-text-muted">{t("settings.tools.exampleCountHint")}</p>
        </div>
        <Select
          className="w-[96px] shrink-0"
          value={String(value.exampleCount)}
          onChange={(count) => onChange({ ...value, exampleCount: Number(count) })}
          options={[0, 1, 2, 3].map((count) => ({ value: String(count), label: String(count) }))}
        />
      </div>

      {kind === "passage" && (
        <div className="flex min-h-12 items-center justify-between gap-4 border-b border-border-light py-2">
          <div>
            <p className="text-[12px] font-medium text-text-primary">{t("settings.tools.keyTermCount")}</p>
            <p className="text-[10px] text-text-muted">{t("settings.tools.keyTermCountHint")}</p>
          </div>
          <Select
            className="w-[96px] shrink-0"
            value={String(value.keyTermCount)}
            onChange={(count) => onChange({ ...value, keyTermCount: Number(count) })}
            options={Array.from({ length: 8 }, (_, index) => index + 1).map((count) => ({ value: String(count), label: String(count) }))}
          />
        </div>
      )}

      <div className="pt-3">
        <div className="flex items-center justify-between pb-1">
          <h4 className="text-[11px] font-semibold uppercase tracking-[0.3px] text-text-muted">{t("settings.tools.modulesTitle")}</h4>
          <span className="text-[10px] text-text-muted">{t("settings.tools.modulesCount", { count: value.modules.filter((module) => module.enabled).length })}</span>
        </div>
        <div>
          {value.modules.map((module, index) => {
            const definition = definitions.get(module.id);
            if (!definition) return null;
            return (
              <CardModuleRow
                key={module.id}
                definition={definition}
                value={module}
                index={index}
                total={value.modules.length}
                onChange={(next) => updateModule(index, next)}
                onMove={move}
                onDragStart={handleDragStart}
                onDrop={handleDrop}
              />
            );
          })}
        </div>
      </div>
    </div>
  );
}
