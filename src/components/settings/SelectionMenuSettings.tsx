import { ArrowDown, ArrowUp, ChevronDown, ChevronRight, GripVertical, Plus } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import Toggle from "../ui/Toggle";
import SortableList from "../ui/SortableList";
import {
  MENU_ACTION_DEFINITIONS,
  MAX_CUSTOM_MENU_ACTIONS,
  reorderArray,
  type SelectionMenuItemConfig,
  type SelectionMenuKind,
  type CustomLearningDefinition,
  type CustomLearningId,
} from "../learning-card";
import CustomActionEditor, {
  type CustomImportSource,
  type UnsavedEditorController,
} from "./CustomActionEditor";

interface SelectionMenuSettingsProps {
  kind: SelectionMenuKind;
  value: SelectionMenuItemConfig[];
  onChange: (value: SelectionMenuItemConfig[]) => void;
  onTouched?: (id: string) => void;
  importSources: CustomImportSource[];
  onTest: (text: string, draft: CustomLearningDefinition, id: CustomLearningId) => void;
  requestNavigation: (action: () => void) => void;
  onEditorGuardChange: (controller: UnsavedEditorController | null) => void;
}

interface NewActionDraft {
  item: SelectionMenuItemConfig;
  definition: CustomLearningDefinition;
}

export default function SelectionMenuSettings({
  kind,
  value,
  onChange,
  onTouched,
  importSources,
  onTest,
  requestNavigation,
  onEditorGuardChange,
}: SelectionMenuSettingsProps) {
  const { t } = useTranslation();
  const [openId, setOpenId] = useState<string | null>(null);
  const [newAction, setNewAction] = useState<NewActionDraft | null>(null);
  const definitions = new Map(MENU_ACTION_DEFINITIONS[kind].map((item) => [item.id, item]));
  const move = (from: number, to: number) => {
    const next = reorderArray(value, from, to);
    if (next !== value) onChange(next);
    if (next !== value && value[from]) onTouched?.(value[from].id);
  };
  const toggleOpen = (id: string) => {
    const nextId = openId === id ? null : id;
    requestNavigation(() => {
      if (newAction && openId === newAction.item.id && nextId !== newAction.item.id) {
        setNewAction(null);
      }
      setOpenId(nextId);
    });
  };

  const renderAction = (
    item: SelectionMenuItemConfig,
    index: number,
    draftDefinition?: CustomLearningDefinition,
  ) => {
    const isDraft = Boolean(draftDefinition);
    const custom = draftDefinition ?? (item.id.startsWith("custom_") && item.name && item.prompt ? {
      name: item.name,
      prompt: item.prompt,
      sourceRef: item.sourceRef,
      follow: item.follow,
      dirtySinceImport: item.dirtySinceImport,
      createdAt: item.createdAt ?? 0,
      updatedAt: item.updatedAt ?? item.createdAt ?? 0,
    } satisfies CustomLearningDefinition : null);
    const definition = definitions.get(item.id);
    if (!definition && !custom) return null;
    const label = isDraft
      ? t("settings.tools.custom.newAction")
      : custom?.name ?? t(definition!.labelKey);
    const total = value.length + (newAction ? 1 : 0);
    return (
      <div className="border-t border-border-light first:border-t-0">
        <div className="flex min-h-12 items-center gap-1 py-1">
          <span
            title={t("settings.tools.reorder")}
            aria-label={t("settings.tools.reorderMenuAction", { name: label })}
            className={`flex size-8 shrink-0 items-center justify-center text-text-muted ${item.enabled ? "" : "opacity-50"}`}
          >
            <GripVertical size={14} />
          </span>
          {custom && (
            <button
              type="button"
              aria-expanded={openId === item.id}
              onClick={() => toggleOpen(item.id)}
              className={`flex size-7 items-center justify-center rounded-md text-text-muted hover:bg-bg-input ${item.enabled ? "" : "opacity-50"}`}
            >
              {openId === item.id ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
            </button>
          )}
          <span className={`min-w-0 flex-1 break-words text-[12px] font-medium text-text-primary ${item.enabled ? "" : "opacity-50"}`}>
            {label}
          </span>
          {isDraft && (
            <span
              className="size-1.5 shrink-0 rounded-full bg-accent"
              title={t("settings.tools.custom.unsaved")}
              aria-label={t("settings.tools.custom.unsaved")}
            />
          )}
          <button
            type="button"
            disabled={isDraft || index === 0}
            onClick={() => move(index, index - 1)}
            title={t("settings.tools.moveUp")}
            aria-label={t("settings.tools.moveMenuActionUp", { name: label })}
            className="flex size-7 items-center justify-center rounded-md text-text-muted hover:bg-bg-input disabled:opacity-25"
          >
            <ArrowUp size={12} />
          </button>
          <button
            type="button"
            disabled={isDraft || index === total - 1}
            onClick={() => move(index, index + 1)}
            title={t("settings.tools.moveDown")}
            aria-label={t("settings.tools.moveMenuActionDown", { name: label })}
            className="flex size-7 items-center justify-center rounded-md text-text-muted hover:bg-bg-input disabled:opacity-25"
          >
            <ArrowDown size={12} />
          </button>
          <Toggle
            checked={item.enabled}
            label={t("settings.tools.toggleMenuAction", { name: label })}
            onChange={(enabled) => {
              if (isDraft) {
                setNewAction((current) => current ? { ...current, item: { ...current.item, enabled } } : current);
              } else {
                onChange(value.map((current, itemIndex) => itemIndex === index ? { ...current, enabled } : current));
                onTouched?.(item.id);
              }
            }}
          />
        </div>
        {custom && openId === item.id && (
          <div className="pb-3 pl-9">
            <CustomActionEditor
              value={custom}
              importSources={importSources}
              testPlaceholder={kind === "word" ? "serendipity" : kind === "phrase" ? "by and large" : "New ideas often emerge where established fields meet."}
              namePlaceholder={isDraft ? t("settings.tools.custom.namePlaceholder") : undefined}
              promptPlaceholder={isDraft ? t("settings.tools.custom.promptPlaceholder") : undefined}
              onSave={(saved) => {
                if (isDraft) {
                  onChange([...value, {
                    ...item,
                    ...saved,
                  }]);
                  setNewAction(null);
                  onTouched?.(item.id);
                } else {
                  onChange(value.map((current) => current.id === item.id ? { ...current, ...saved } : current));
                }
              }}
              onDelete={() => {
                if (isDraft) {
                  setNewAction(null);
                  setOpenId(null);
                } else {
                  onChange(value.filter((current) => current.id !== item.id));
                }
              }}
              onDiscard={isDraft ? () => {
                setNewAction(null);
                setOpenId(null);
              } : undefined}
              onTest={(text, saved) => onTest(text, saved, item.id as CustomLearningId)}
              onGuardChange={onEditorGuardChange}
            />
          </div>
        )}
      </div>
    );
  };

  return (
    <div className="min-w-0">
      <div className="pb-2">
        <h4 className="text-[12px] font-medium text-text-primary">{t(`settings.tools.menu.${kind}`)}</h4>
        <p className="text-[10px] leading-[1.5] text-text-muted">{t("settings.tools.menu.hint")}</p>
      </div>
      <SortableList
        items={value}
        getId={(item) => item.id}
        onReorder={(items) => {
          const moved = items.find((item, index) => value[index]?.id !== item.id);
          onChange(items);
          if (moved) onTouched?.(moved.id);
        }}
        className="border-y border-border-light"
        renderItem={(item, index) => renderAction(item, index)}
      />
      {newAction && renderAction(newAction.item, value.length, newAction.definition)}
      {!newAction && value.filter((item) => item.id.startsWith("custom_")).length < MAX_CUSTOM_MENU_ACTIONS && (
        <button
          type="button"
          onClick={() => {
            const id = `custom_${crypto.randomUUID().replace(/-/g, "")}` as CustomLearningId;
            const timestamp = Date.now();
            requestNavigation(() => {
              setNewAction({
                item: { id, enabled: true, createdAt: timestamp, updatedAt: timestamp },
                definition: { name: "", prompt: "", createdAt: timestamp, updatedAt: timestamp },
              });
              setOpenId(id);
            });
          }}
          className="mt-2 flex h-8 items-center gap-1.5 rounded-md px-2 text-[11px] font-medium text-accent-text hover:bg-accent-bg"
        ><Plus size={13} />{t("settings.tools.custom.addAction")}</button>
      )}
    </div>
  );
}
