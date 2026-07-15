import { invoke } from "@tauri-apps/api/core";
import {
  Import,
  Loader2,
  Redo2,
  RotateCw,
  Save,
  Trash2,
  Undo2,
  Wand2,
  X,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  MAX_CUSTOM_NAME_LENGTH,
  MAX_CUSTOM_PROMPT_LENGTH,
  type CustomLearningDefinition,
  type CustomLearningId,
  type LearningCardKind,
} from "../learning-card";
import Button from "../ui/Button";
import Select from "../ui/Select";
import Toggle from "../ui/Toggle";
import ConfirmDialog from "./ConfirmDialog";

export interface CustomImportSource {
  kind: LearningCardKind;
  id: CustomLearningId;
  name: string;
  prompt: string;
}

export interface UnsavedEditorController {
  dirty: boolean;
  canSave: boolean;
  name: string;
  save: () => boolean;
  discard: () => void;
}

interface CustomActionEditorProps {
  value: CustomLearningDefinition;
  importSources: CustomImportSource[];
  testPlaceholder: string;
  onSave: (value: CustomLearningDefinition) => void;
  onDelete: () => void;
  onTest: (text: string, draft: CustomLearningDefinition) => void;
  onDiscard?: () => void;
  onGuardChange?: (controller: UnsavedEditorController | null) => void;
  namePlaceholder?: string;
  promptPlaceholder?: string;
}

function comparableDefinition(value: CustomLearningDefinition) {
  return {
    name: value.name.trim(),
    prompt: value.prompt.trim(),
    sourceRef: value.sourceRef ?? null,
    follow: value.follow === true,
    dirtySinceImport: value.dirtySinceImport === true,
  };
}

export default function CustomActionEditor({
  value,
  importSources,
  testPlaceholder,
  onSave,
  onDelete,
  onTest,
  onDiscard,
  onGuardChange,
  namePlaceholder,
  promptPlaceholder,
}: CustomActionEditorProps) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState(value);
  const [testText, setTestText] = useState("");
  const [sourceKey, setSourceKey] = useState("");
  const [optimizing, setOptimizing] = useState(false);
  const [optimizeError, setOptimizeError] = useState<string | null>(null);
  const [history, setHistory] = useState<[string, string] | null>(null);
  const [historyIndex, setHistoryIndex] = useState<0 | 1>(1);
  const [optimizeLocked, setOptimizeLocked] = useState(false);
  const [overwriteSource, setOverwriteSource] = useState<CustomImportSource | null>(null);
  const requestRef = useRef<string | null>(null);
  const onSaveRef = useRef(onSave);
  const onDiscardRef = useRef(onDiscard);

  useEffect(() => {
    setDraft(value);
    setOptimizeLocked(false);
  }, [value]);
  useEffect(() => {
    onSaveRef.current = onSave;
    onDiscardRef.current = onDiscard;
  }, [onDiscard, onSave]);
  useEffect(() => () => {
    if (requestRef.current) void invoke("ai_cancel", { requestId: requestRef.current });
  }, []);

  const dirty = JSON.stringify(comparableDefinition(draft)) !== JSON.stringify(comparableDefinition(value));
  const canSave = Boolean(draft.name.trim() && draft.prompt.trim());
  useEffect(() => {
    onGuardChange?.({
      dirty,
      canSave,
      name: draft.name.trim(),
      save: () => {
        if (!canSave) return false;
        onSaveRef.current({
          ...draft,
          name: draft.name.trim(),
          prompt: draft.prompt.trim(),
          updatedAt: Date.now(),
        });
        return true;
      },
      discard: () => {
        setDraft(value);
        onDiscardRef.current?.();
      },
    });
  }, [canSave, dirty, draft, onGuardChange, value]);
  useEffect(() => () => onGuardChange?.(null), [onGuardChange]);

  const edit = (patch: Partial<CustomLearningDefinition>) => {
    setDraft((current) => ({
      ...current,
      ...patch,
      follow: current.sourceRef ? false : current.follow,
      dirtySinceImport: current.sourceRef ? true : current.dirtySinceImport,
      updatedAt: Date.now(),
    }));
    setHistory(null);
    setOptimizeLocked(false);
    setOptimizeError(null);
  };

  const optimize = async () => {
    if (optimizing) {
      if (requestRef.current) await invoke("ai_cancel", { requestId: requestRef.current }).catch(() => {});
      requestRef.current = null;
      setOptimizing(false);
      return;
    }
    if (!draft.name.trim() || !draft.prompt.trim()) return;
    const requestId = crypto.randomUUID();
    requestRef.current = requestId;
    setOptimizing(true);
    setOptimizeError(null);
    try {
      const optimized = await invoke<string>("ai_optimize_prompt", {
        name: draft.name.trim(),
        prompt: draft.prompt.trim(),
        requestId,
      });
      if (requestRef.current !== requestId) return;
      setHistory([draft.prompt, optimized]);
      setHistoryIndex(1);
      setOptimizeLocked(true);
      setDraft((current) => ({
        ...current,
        prompt: optimized,
        follow: current.sourceRef ? false : current.follow,
        dirtySinceImport: current.sourceRef ? true : current.dirtySinceImport,
        updatedAt: Date.now(),
      }));
    } catch (error) {
      if (requestRef.current === requestId) {
        setOptimizeError(error instanceof Error ? error.message : String(error));
      }
    } finally {
      if (requestRef.current === requestId) requestRef.current = null;
      setOptimizing(false);
    }
  };

  const applyImport = () => {
    const source = importSources.find((item) => `${item.kind}:${item.id}` === sourceKey);
    if (!source) return;
    setDraft((current) => ({
      ...current,
      name: source.name,
      prompt: source.prompt,
      sourceRef: { kind: source.kind, id: source.id },
      follow: true,
      dirtySinceImport: false,
      updatedAt: Date.now(),
    }));
    setHistory(null);
  };

  const source = draft.sourceRef
    ? importSources.find((item) => item.kind === draft.sourceRef?.kind && item.id === draft.sourceRef?.id)
    : undefined;
  const followSource = (nextSource: CustomImportSource) => {
    setDraft((current) => ({
      ...current,
      name: nextSource.name,
      prompt: nextSource.prompt,
      follow: true,
      dirtySinceImport: false,
      updatedAt: Date.now(),
    }));
    setOverwriteSource(null);
  };

  return (
    <div className="space-y-3 rounded-md bg-bg-muted p-3" data-no-drag>
      <label className="block">
        <span className="mb-1 block text-[11px] text-text-muted">{t("settings.tools.custom.name")}</span>
        <input
          value={draft.name}
          placeholder={namePlaceholder}
          maxLength={MAX_CUSTOM_NAME_LENGTH}
          onChange={(event) => edit({ name: event.target.value })}
          className="h-9 w-full rounded-md border border-border bg-bg-surface px-3 text-[12px] text-text-primary outline-none focus:border-accent"
        />
      </label>
      <label className="block">
        <span className="mb-1 block text-[11px] text-text-muted">{t("settings.tools.custom.prompt")}</span>
        <span className="relative block">
          <textarea
            value={draft.prompt}
            placeholder={promptPlaceholder}
            maxLength={MAX_CUSTOM_PROMPT_LENGTH}
            rows={5}
            onChange={(event) => edit({ prompt: event.target.value })}
            className="w-full resize-y rounded-md border border-border bg-bg-surface px-3 py-2 pr-10 text-[12px] leading-5 text-text-primary outline-none focus:border-accent"
          />
          <button
            type="button"
            onClick={() => void optimize()}
            disabled={!draft.name.trim() || !draft.prompt.trim() || optimizeLocked}
            title={optimizing ? t("common.cancel") : t("settings.tools.custom.optimize")}
            aria-label={optimizing ? t("common.cancel") : t("settings.tools.custom.optimize")}
            className="absolute right-2 top-2 flex size-7 items-center justify-center rounded-md text-text-muted hover:bg-bg-input hover:text-accent-text disabled:opacity-30"
          >
            {optimizing ? <Loader2 size={14} className="animate-spin" /> : <Wand2 size={14} />}
          </button>
        </span>
        <span className="mt-1 block text-right text-[10px] text-text-placeholder">{draft.prompt.length}/{MAX_CUSTOM_PROMPT_LENGTH}</span>
      </label>
      {history && (
        <div className="flex justify-end gap-1">
          <button
            type="button"
            disabled={historyIndex === 0}
            title={t("common.undo")}
            onClick={() => {
              setHistoryIndex(0);
              setDraft((current) => ({ ...current, prompt: history[0] }));
            }}
            className="flex size-7 items-center justify-center rounded-md text-text-muted hover:bg-bg-input disabled:opacity-25"
          ><Undo2 size={13} /></button>
          <button
            type="button"
            disabled={historyIndex === 1}
            title={t("common.redo")}
            onClick={() => {
              setHistoryIndex(1);
              setDraft((current) => ({ ...current, prompt: history[1] }));
            }}
            className="flex size-7 items-center justify-center rounded-md text-text-muted hover:bg-bg-input disabled:opacity-25"
          ><Redo2 size={13} /></button>
        </div>
      )}
      {optimizeError && <p role="alert" className="break-words text-[10px] text-danger-text">{optimizeError}</p>}

      {importSources.length > 0 && (
        <div className="flex items-center gap-2">
          <Select
            className="min-w-0 flex-1"
            value={sourceKey}
            onChange={setSourceKey}
            options={[
              { value: "", label: t("settings.tools.custom.importPlaceholder") },
              ...importSources.map((item) => ({
                value: `${item.kind}:${item.id}`,
                label: `${item.name} (${t(`settings.tools.cardKind.${item.kind}`)})`,
              })),
            ]}
          />
          <Button variant="secondary" size="sm" disabled={!sourceKey} onClick={applyImport}>
            <Import size={13} />{t("settings.tools.custom.import")}
          </Button>
        </div>
      )}

      {draft.sourceRef && (
        <div className="flex min-h-9 items-center justify-between gap-3 border-y border-border-light py-1">
          <div className="min-w-0">
            <p className="truncate text-[11px] text-text-primary">
              {source ? t("settings.tools.custom.source", { name: source.name }) : t("settings.tools.custom.sourceDeleted")}
            </p>
          </div>
          <Toggle
            checked={Boolean(source && draft.follow)}
            disabled={!source}
            label={t("settings.tools.custom.follow")}
            onChange={(follow) => {
              if (!follow) {
                setDraft((current) => ({ ...current, follow: false }));
                return;
              }
              if (!source) return;
              if (draft.dirtySinceImport) {
                setOverwriteSource(source);
                return;
              }
              followSource(source);
            }}
          />
        </div>
      )}

      <div className="flex gap-2">
        <input
          value={testText}
          onChange={(event) => setTestText(event.target.value)}
          placeholder={testPlaceholder}
          className="h-8 min-w-0 flex-1 rounded-md border border-border bg-bg-surface px-2 text-[11px] text-text-primary outline-none focus:border-accent"
        />
        <Button variant="secondary" size="sm" onClick={() => onTest(testText.trim() || testPlaceholder, draft)}>
          <RotateCw size={13} />{t("settings.tools.custom.test")}
        </Button>
      </div>

      <div className="flex items-center justify-between gap-2 border-t border-border-light pt-3">
        <button
          type="button"
          onClick={onDelete}
          className="flex h-8 items-center gap-1.5 rounded-md px-2 text-[11px] text-danger-text hover:bg-danger-bg"
        ><Trash2 size={13} />{t("common.delete")}</button>
        <div className="flex gap-2">
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              setDraft(value);
              onDiscardRef.current?.();
            }}
          ><X size={13} />{t("common.cancel")}</Button>
          <Button
            size="sm"
            disabled={!canSave}
            onClick={() => {
              onSaveRef.current({
                ...draft,
                name: draft.name.trim(),
                prompt: draft.prompt.trim(),
                updatedAt: Date.now(),
              });
            }}
          ><Save size={13} />{t("common.save")}</Button>
        </div>
      </div>
      {overwriteSource && (
        <ConfirmDialog
          title={t("settings.tools.custom.overwriteTitle", { name: overwriteSource.name })}
          description={t("settings.tools.custom.overwriteConfirm", { name: overwriteSource.name })}
          primaryLabel={t("settings.tools.custom.overwrite")}
          onPrimary={() => followSource(overwriteSource)}
          secondaryLabel={t("common.cancel")}
          onSecondary={() => setOverwriteSource(null)}
        />
      )}
    </div>
  );
}
