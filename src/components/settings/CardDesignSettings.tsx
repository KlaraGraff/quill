import { CircleHelp, Plus } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import Select from "../ui/Select";
import SortableList from "../ui/SortableList";
import {
  MODULE_DEFINITIONS,
  MAX_CUSTOM_CARD_MODULES,
  reorderArray,
  type CardKindConfig,
  type CardModuleConfig,
  type CardWidthMode,
  type ContentDensity,
  type LearningCardKind,
  type CustomLearningDefinition,
  type CustomLearningId,
} from "../learning-card";
import CardModuleRow from "./CardModuleRow";
import CustomActionEditor, {
  type CustomImportSource,
  type UnsavedEditorController,
} from "./CustomActionEditor";
import { ROW_CONTROL_WIDTH, ROW_CONTROL_WIDTH_COMPACT } from "./types";

interface CardDesignSettingsProps {
  kind: LearningCardKind;
  value: CardKindConfig;
  onChange: (value: CardKindConfig) => void;
  onOpenDensityHelp: () => void;
  onTouched?: (id: string) => void;
  importSources: CustomImportSource[];
  onTest: (text: string, customId: CustomLearningId, draft: CustomLearningDefinition, card: CardKindConfig) => void;
  requestNavigation: (action: () => void) => void;
  onEditorGuardChange: (controller: UnsavedEditorController | null) => void;
}

interface NewModuleDraft {
  module: CardModuleConfig;
  definition: CustomLearningDefinition;
}

export default function CardDesignSettings({
  kind,
  value,
  onChange,
  onOpenDensityHelp,
  onTouched,
  importSources,
  onTest,
  requestNavigation,
  onEditorGuardChange,
}: CardDesignSettingsProps) {
  const { t } = useTranslation();
  const [openId, setOpenId] = useState<string | null>(null);
  const [newModule, setNewModule] = useState<NewModuleDraft | null>(null);
  const definitions = new Map(MODULE_DEFINITIONS[kind].map((item) => [item.id, item]));

  const updateModule = (index: number, module: CardModuleConfig) => {
    const modules = value.modules.map((item, itemIndex) => itemIndex === index ? module : item);
    onChange({ ...value, modules });
    onTouched?.(module.id);
  };
  const move = (from: number, to: number) => {
    const modules = reorderArray(value.modules, from, to);
    if (modules !== value.modules) onChange({ ...value, modules });
    if (modules !== value.modules && value.modules[from]) onTouched?.(value.modules[from].id);
  };
  const toggleOpen = (id: string) => {
    const nextId = openId === id ? null : id;
    requestNavigation(() => {
      if (newModule && openId === newModule.module.id && nextId !== newModule.module.id) {
        setNewModule(null);
      }
      setOpenId(nextId);
    });
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
          className={ROW_CONTROL_WIDTH}
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
          className={ROW_CONTROL_WIDTH_COMPACT}
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
            className={ROW_CONTROL_WIDTH_COMPACT}
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
        <SortableList
          items={value.modules}
          getId={(module) => module.id}
          onReorder={(modules) => {
            const moved = modules.find((module, index) => value.modules[index]?.id !== module.id);
            onChange({ ...value, modules });
            if (moved) onTouched?.(moved.id);
          }}
          renderItem={(module, index) => {
            const custom = module.id.startsWith("custom_")
              ? value.customModules[module.id as CustomLearningId]
              : undefined;
            const definition = definitions.get(module.id) ?? (custom ? {
              id: module.id,
              labelKey: custom.name,
              descriptionKey: "settings.tools.custom.moduleHint",
              required: false,
              custom: true,
            } : undefined);
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
                open={openId === module.id}
                onToggleOpen={() => toggleOpen(module.id)}
                editor={custom ? (
                  <CustomActionEditor
                    value={custom}
                    importSources={importSources}
                    testPlaceholder={kind === "word" ? "serendipity" : kind === "phrase" ? "by and large" : "New ideas often emerge where established fields meet."}
                    onSave={(draft) => onChange({
                      ...value,
                      customModules: { ...value.customModules, [module.id as CustomLearningId]: draft },
                    })}
                    onDelete={() => {
                      const customModules = { ...value.customModules };
                      delete customModules[module.id as CustomLearningId];
                      onChange({ ...value, modules: value.modules.filter((item) => item.id !== module.id), customModules });
                    }}
                    onTest={(text, draft) => onTest(text, module.id as CustomLearningId, draft, value)}
                    onGuardChange={onEditorGuardChange}
                  />
                ) : undefined}
              />
            );
          }}
        />
        {newModule && (
          <CardModuleRow
            definition={{
              id: newModule.module.id,
              labelKey: t("settings.tools.custom.newModule"),
              descriptionKey: "settings.tools.custom.moduleHint",
              custom: true,
            }}
            value={newModule.module}
            index={value.modules.length}
            total={value.modules.length + 1}
            onChange={(module) => setNewModule((current) => current ? { ...current, module } : current)}
            onMove={() => {}}
            open={openId === newModule.module.id}
            onToggleOpen={() => toggleOpen(newModule.module.id)}
            unsaved
            editor={(
              <CustomActionEditor
                value={newModule.definition}
                importSources={importSources}
                testPlaceholder={kind === "word" ? "serendipity" : kind === "phrase" ? "by and large" : "New ideas often emerge where established fields meet."}
                namePlaceholder={t("settings.tools.custom.namePlaceholder")}
                promptPlaceholder={t("settings.tools.custom.promptPlaceholder")}
                onSave={(definition) => {
                  onChange({
                    ...value,
                    modules: [...value.modules, newModule.module],
                    customModules: {
                      ...value.customModules,
                      [newModule.module.id as CustomLearningId]: definition,
                    },
                  });
                  setNewModule(null);
                  onTouched?.(newModule.module.id);
                }}
                onDelete={() => {
                  setNewModule(null);
                  setOpenId(null);
                }}
                onDiscard={() => {
                  setNewModule(null);
                  setOpenId(null);
                }}
                onTest={(text, definition) => onTest(
                  text,
                  newModule.module.id as CustomLearningId,
                  definition,
                  {
                    ...value,
                    modules: [...value.modules, newModule.module],
                  },
                )}
                onGuardChange={onEditorGuardChange}
              />
            )}
          />
        )}
        {!newModule && Object.keys(value.customModules).length < MAX_CUSTOM_CARD_MODULES && (
          <button
            type="button"
            onClick={() => {
              const id = `custom_${crypto.randomUUID().replace(/-/g, "")}` as CustomLearningId;
              const now = Date.now();
              requestNavigation(() => {
                setNewModule({
                  module: { id, enabled: true, defaultExpanded: true, density: "inherit" },
                  definition: { name: "", prompt: "", createdAt: now, updatedAt: now },
                });
                setOpenId(id);
              });
            }}
            className="mt-2 flex h-8 items-center gap-1.5 rounded-md px-2 text-[11px] font-medium text-accent-text hover:bg-accent-bg"
          ><Plus size={13} />{t("settings.tools.custom.addModule")}</button>
        )}
      </div>
    </div>
  );
}
