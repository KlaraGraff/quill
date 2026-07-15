import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { Highlighter, LayoutPanelTop, MousePointer2, MousePointerClick, PanelRightOpen } from "lucide-react";
import { useTranslation } from "react-i18next";
import {
  LEARNING_CARD_CONFIG_SETTING_KEY,
  createDefaultCardDesignConfig,
  parseCardDesignConfig,
  serializeCardDesignConfig,
  type CardDesignConfigV1,
  type LearningCardKind,
  type SelectionMenuKind,
  type CustomLearningId,
} from "../learning-card";
import type { CustomImportSource, UnsavedEditorController } from "./CustomActionEditor";
import Toggle from "../ui/Toggle";
import CardDesignSettings from "./CardDesignSettings";
import DensityHelpDialog from "./DensityHelpDialog";
import SelectionMenuSettings from "./SelectionMenuSettings";
import MarkerStyleSettings from "./MarkerStyleSettings";
import ReaderBindingsSettings from "./ReaderBindingsSettings";
import ConfirmDialog from "./ConfirmDialog";
import type { SettingsProps } from "./types";
import {
  MARKER_STYLE_SETTING_KEY,
  createDefaultMarkerStyleConfig,
  parseMarkerStyleConfig,
  serializeMarkerStyleConfig,
  type MarkerStyleConfigV1,
} from "../marker-style";
import { notifyReadingAssistanceSettingsChanged } from "../reading-assistance-events";
import { parseReaderBindings, READER_BINDINGS_SETTING_KEY, type ReaderActionBinding } from "../reader-bindings";

type ToolsView = "interaction" | "cards" | "menu" | "markers";

export interface ToolsPreviewState {
  kind: LearningCardKind;
  config: CardDesignConfigV1;
  explanationLanguage: string;
  targetLanguage: string;
  learnerLevel: string;
  explanationMode: string;
  showMenu: boolean;
  lastTouched: { id: string; nonce: number } | null;
  testText?: string;
  testNonce?: number;
  customActionTest?: { name: string; prompt: string; text: string; nonce: number };
  onDismiss: () => void;
}

interface ToolsSettingsProps extends SettingsProps {
  onPreviewChange?: (preview: ToolsPreviewState | null) => void;
  onNavigationGuardChange?: (guard: ((action: () => void) => void) | null) => void;
}

function SettingsRow({ title, subtitle, children }: { title: string; subtitle: string; children: ReactNode }) {
  return (
    <div className="flex min-h-[52px] w-full items-center justify-between gap-4 px-1 py-1.5">
      <div className="min-w-0 flex-1">
        <p className="text-[13px] font-medium text-text-primary">{title}</p>
        <p className="break-words text-[11px] leading-[17px] text-text-placeholder">{subtitle}</p>
      </div>
      {children}
    </div>
  );
}

function setWordTranslationModule(config: CardDesignConfigV1, enabled: boolean): CardDesignConfigV1 {
  return {
    ...config,
    cards: {
      ...config.cards,
      word: {
        ...config.cards.word,
        modules: config.cards.word.modules.map((module) =>
          module.id === "target_translation" ? { ...module, enabled } : module,
        ),
      },
    },
  };
}

function wordTranslationEnabled(config: CardDesignConfigV1) {
  return config.cards.word.modules.find((module) => module.id === "target_translation")?.enabled ?? true;
}

function resolveFollowingSources(config: CardDesignConfigV1): CardDesignConfigV1 {
  const cards = { ...config.cards };
  for (const kind of ["word", "phrase", "passage"] as LearningCardKind[]) {
    const card = cards[kind];
    const customModules = { ...card.customModules };
    for (const [id, definition] of Object.entries(customModules)) {
      if (!definition?.sourceRef || !definition.follow) continue;
      const source = config.cards[definition.sourceRef.kind].customModules[definition.sourceRef.id];
      customModules[id as CustomLearningId] = source && !source.sourceRef
        ? { ...definition, name: source.name, prompt: source.prompt, dirtySinceImport: false, updatedAt: source.updatedAt }
        : { ...definition, follow: false };
    }
    cards[kind] = { ...card, customModules };
  }
  const selectionMenus = { ...config.selectionMenus };
  for (const kind of ["word", "phrase", "passage"] as LearningCardKind[]) {
    selectionMenus[kind] = selectionMenus[kind].map((item) => {
      if (!item.sourceRef || !item.follow) return item;
      const source = config.selectionMenus[item.sourceRef.kind]
        .find((candidate) => candidate.id === item.sourceRef?.id && !candidate.sourceRef);
      return source?.name && source.prompt
        ? { ...item, name: source.name, prompt: source.prompt, dirtySinceImport: false, updatedAt: source.updatedAt }
        : { ...item, follow: false };
    });
  }
  return { ...config, cards, selectionMenus };
}

export default function ToolsSettings({
  settings,
  loading,
  save,
  saveBulk,
  showSavedToast,
  onPreviewChange,
  onNavigationGuardChange,
}: ToolsSettingsProps) {
  const { t } = useTranslation();
  const [view, setView] = useState<ToolsView>("interaction");
  const [previewOpen, setPreviewOpen] = useState(false);
  const [cardKind, setCardKind] = useState<LearningCardKind>("word");
  const [menuKind, setMenuKind] = useState<SelectionMenuKind>("word");
  const [densityHelpOpen, setDensityHelpOpen] = useState(false);
  const [config, setConfig] = useState<CardDesignConfigV1>(createDefaultCardDesignConfig);
  const [autoHighlightLookupWords, setAutoHighlightLookupWords] = useState(true);
  const [markerStyle, setMarkerStyle] = useState<MarkerStyleConfigV1>(createDefaultMarkerStyleConfig);
  const [doubleClickQuickLookup, setDoubleClickQuickLookup] = useState(true);
  const [readerBindings, setReaderBindings] = useState<ReaderActionBinding[]>([]);
  const [lastTouched, setLastTouched] = useState<{ id: string; nonce: number } | null>(null);
  const [testPreview, setTestPreview] = useState<{ config: CardDesignConfigV1; text: string; id: string; nonce: number } | null>(null);
  const [customActionTest, setCustomActionTest] = useState<ToolsPreviewState["customActionTest"]>();
  const saveQueue = useRef<Promise<void>>(Promise.resolve());
  const hydratedRef = useRef(false);
  const editorControllerRef = useRef<UnsavedEditorController | null>(null);
  const pendingNavigationRef = useRef<(() => void) | null>(null);
  const [editorController, setEditorController] = useState<UnsavedEditorController | null>(null);
  const [guardOpen, setGuardOpen] = useState(false);
  const touch = (id: string) => {
    setLastTouched((current) => ({ id, nonce: (current?.nonce ?? 0) + 1 }));
  };
  const handleEditorGuardChange = useCallback((controller: UnsavedEditorController | null) => {
    editorControllerRef.current = controller;
    setEditorController(controller);
  }, []);
  const requestNavigation = useCallback((action: () => void) => {
    const controller = editorControllerRef.current;
    if (!controller?.dirty) {
      action();
      return;
    }
    pendingNavigationRef.current = action;
    setGuardOpen(true);
  }, []);
  const finishPendingNavigation = () => {
    const action = pendingNavigationRef.current;
    pendingNavigationRef.current = null;
    setGuardOpen(false);
    action?.();
  };
  const continueEditing = () => {
    pendingNavigationRef.current = null;
    setGuardOpen(false);
  };

  useEffect(() => {
    onNavigationGuardChange?.(requestNavigation);
    return () => onNavigationGuardChange?.(null);
  }, [onNavigationGuardChange, requestNavigation]);

  useEffect(() => {
    if (loading || hydratedRef.current) return;
    let parsed = parseCardDesignConfig(settings[LEARNING_CARD_CONFIG_SETTING_KEY]);
    if (!settings[LEARNING_CARD_CONFIG_SETTING_KEY] && settings.show_translation !== undefined) {
      parsed = setWordTranslationModule(parsed, settings.show_translation === "true");
    }
    setConfig(parsed);
    setAutoHighlightLookupWords(settings.auto_highlight_lookup_words !== "false");
    setMarkerStyle(parseMarkerStyleConfig(settings[MARKER_STYLE_SETTING_KEY]));
    setDoubleClickQuickLookup(settings.double_click_quick_lookup !== "false");
    setReaderBindings(parseReaderBindings(settings[READER_BINDINGS_SETTING_KEY]).bindings);
    hydratedRef.current = true;
  }, [settings, loading]);

  const previewExplanationMode = settings.explanation_mode || "adaptive_bilingual";
  const resolvedExplanationLanguage = previewExplanationMode === "chinese"
    || (previewExplanationMode === "adaptive_bilingual" && ["A1", "A2"].includes(settings.cefr_level || "B1"))
    ? "zh"
    : "en";
  const targetLanguage = settings.translation_language
    || settings.lookup_translation_language
    || "zh";

  useEffect(() => {
    if (loading || !previewOpen || (view !== "cards" && view !== "menu")) {
      onPreviewChange?.(null);
      return;
    }

    const isMenuPreview = view === "menu";
    const kind = isMenuPreview ? menuKind : cardKind;
    onPreviewChange?.({
      kind,
      config: testPreview?.config ?? config,
      explanationLanguage: resolvedExplanationLanguage,
      targetLanguage,
      learnerLevel: settings.cefr_level || "B1",
      explanationMode: previewExplanationMode,
      showMenu: isMenuPreview,
      lastTouched: testPreview
        ? { id: testPreview.id, nonce: testPreview.nonce }
        : lastTouched,
      testText: testPreview?.text,
      testNonce: testPreview?.nonce,
      customActionTest,
      onDismiss: () => setPreviewOpen(false),
    });
  }, [
    cardKind,
    config,
    customActionTest,
    lastTouched,
    loading,
    menuKind,
    onPreviewChange,
    previewOpen,
    previewExplanationMode,
    resolvedExplanationLanguage,
    settings.cefr_level,
    settings.explanation_mode,
    settings.translation_language,
    targetLanguage,
    testPreview,
    view,
  ]);

  useEffect(() => () => onPreviewChange?.(null), [onPreviewChange]);

  if (loading) return null;

  const queueSave = (entries: Record<string, string>, toastMessage?: string) => {
    const keys = Object.keys(entries);
    saveQueue.current = saveQueue.current
      .catch(() => {})
      .then(() => saveBulk(entries))
      .then(() => notifyReadingAssistanceSettingsChanged(keys))
      .then(() => showSavedToast(toastMessage))
      .catch((error) => console.error("Failed to save learning tool settings:", error));
  };
  const persistConfig = (next: CardDesignConfigV1) => {
    const normalized = parseCardDesignConfig(resolveFollowingSources(next));
    const translationEnabled = wordTranslationEnabled(normalized);
    setConfig(normalized);
    setTestPreview(null);
    setCustomActionTest(undefined);
    queueSave({
      [LEARNING_CARD_CONFIG_SETTING_KEY]: serializeCardDesignConfig(normalized),
      show_translation: String(translationEnabled),
    });
  };
  const persistLegacy = (key: string, value: string) => {
    save(key, value)
      .then(() => {
        showSavedToast();
        return notifyReadingAssistanceSettingsChanged([key]);
      })
      .catch((error) => {
        console.error(`Failed to save ${key}:`, error);
      });
  };
  const persistMarkerStyle = (next: MarkerStyleConfigV1) => {
    const normalized = parseMarkerStyleConfig(next);
    setMarkerStyle(normalized);
    const serialized = serializeMarkerStyleConfig(normalized);
    queueSave({ [MARKER_STYLE_SETTING_KEY]: serialized });
  };
  const updateCard = (kind: LearningCardKind, card: CardDesignConfigV1["cards"][LearningCardKind]) => {
    persistConfig({ ...config, cards: { ...config.cards, [kind]: card } });
  };
  const importSources = (targetKind: LearningCardKind): CustomImportSource[] => (
    (Object.keys(config.cards) as LearningCardKind[])
      .filter((kind) => kind !== targetKind)
      .flatMap((kind) => Object.entries(config.cards[kind].customModules)
        .filter(([, definition]) => definition && !definition.sourceRef)
        .map(([id, definition]) => ({
          kind,
          id: id as CustomLearningId,
          name: definition!.name,
          prompt: definition!.prompt,
        })))
  );
  const menuImportSources = (targetKind: LearningCardKind): CustomImportSource[] => (
    (Object.keys(config.selectionMenus) as LearningCardKind[])
      .filter((kind) => kind !== targetKind)
      .flatMap((kind) => config.selectionMenus[kind]
        .filter((item) => item.id.startsWith("custom_") && item.name && item.prompt && !item.sourceRef)
        .map((item) => ({
          kind,
          id: item.id as CustomLearningId,
          name: item.name!,
          prompt: item.prompt!,
        })))
  );
  const views: { id: ToolsView; icon: typeof Highlighter; label: string }[] = [
    { id: "interaction", icon: MousePointerClick, label: t("settings.tools.views.interaction") },
    { id: "cards", icon: LayoutPanelTop, label: t("settings.tools.views.cards") },
    { id: "menu", icon: MousePointer2, label: t("settings.tools.views.menu") },
    {
      id: "markers",
      icon: Highlighter,
      label: t("settings.tools.views.markers", { defaultValue: "正文标记" }),
    },
  ];

  return (
    <div className="w-full min-w-0 pb-10">
      <div role="tablist" className="mb-4 flex min-w-0 gap-1 border-b border-border-light">
        {views.map((item) => {
          const Icon = item.icon;
          return (
            <button
              key={item.id}
              type="button"
              role="tab"
              aria-selected={view === item.id}
              onClick={() => requestNavigation(() => {
                setView(item.id);
                setPreviewOpen(item.id === "cards" || item.id === "menu");
              })}
              className={`flex h-10 min-w-0 items-center gap-1.5 border-b-2 px-3 text-[12px] font-medium ${view === item.id ? "border-accent text-accent-text" : "border-transparent text-text-muted hover:text-text-primary"}`}
            >
              <Icon size={14} className="shrink-0" />
              <span className="truncate">{item.label}</span>
            </button>
          );
        })}
      </div>

      {view === "interaction" && (
        <div className="mx-auto w-full max-w-[620px]">
          <SettingsRow
            title={t("settings.tools.interaction.doubleClick")}
            subtitle={t("settings.tools.interaction.doubleClickHint")}
          >
            <Toggle
              label={t("settings.tools.interaction.doubleClick")}
              checked={doubleClickQuickLookup}
              onChange={(enabled) => {
                if (enabled && readerBindings.some((binding) => binding.trigger === "mouse:double")) {
                  showSavedToast(t("settings.tools.bindings.doubleClickConflictReverse"));
                  return;
                }
                setDoubleClickQuickLookup(enabled);
                persistLegacy("double_click_quick_lookup", String(enabled));
              }}
            />
          </SettingsRow>
          <ReaderBindingsSettings
            value={readerBindings}
            config={config}
            doubleClickEnabled={doubleClickQuickLookup}
            previousPageBinding={settings.previous_page_binding || "key:ArrowLeft"}
            nextPageBinding={settings.next_page_binding || "key:ArrowRight"}
            onChange={(bindings) => {
              setReaderBindings(bindings);
              queueSave(
                { [READER_BINDINGS_SETTING_KEY]: JSON.stringify({ version: 1, bindings }) },
                t("settings.tools.bindings.savedToast"),
              );
            }}
          />
        </div>
      )}

      {view === "cards" && (
        <div>
          <div className="mb-4 flex items-center justify-between gap-2 border-b border-border-light">
            <div className="flex gap-1" role="tablist">
              {(["word", "phrase", "passage"] as LearningCardKind[]).map((kind) => (
                <button
                  key={kind}
                  type="button"
                  role="tab"
                  aria-selected={cardKind === kind}
                  onClick={() => requestNavigation(() => setCardKind(kind))}
                  className={`h-9 border-b-2 px-3 text-[12px] font-medium ${cardKind === kind ? "border-accent text-accent-text" : "border-transparent text-text-muted"}`}
                >
                  {t(`settings.tools.cardKind.${kind}`)}
                </button>
              ))}
            </div>
            {!previewOpen && (
              <button
                type="button"
                onClick={() => setPreviewOpen(true)}
                title={t("settings.tools.showPreview")}
                aria-label={t("settings.tools.showPreview")}
                className="flex size-8 shrink-0 items-center justify-center rounded-md text-text-muted hover:bg-bg-input hover:text-text-primary"
              >
                <PanelRightOpen size={15} />
              </button>
            )}
          </div>
          <div className="mx-auto w-full max-w-[620px]">
            <CardDesignSettings
              kind={cardKind}
              value={config.cards[cardKind]}
              onChange={(card) => updateCard(cardKind, card)}
              onOpenDensityHelp={() => setDensityHelpOpen(true)}
              onTouched={touch}
              importSources={importSources(cardKind)}
              requestNavigation={requestNavigation}
              onEditorGuardChange={handleEditorGuardChange}
              onTest={(text, customId, draft, card) => {
                const testCard = {
                  ...card,
                  customModules: { ...card.customModules, [customId]: draft },
                };
                setTestPreview({
                  text,
                  id: customId,
                  nonce: Date.now(),
                  config: { ...config, cards: { ...config.cards, [cardKind]: testCard } },
                });
                setPreviewOpen(true);
              }}
            />
          </div>
        </div>
      )}

      {view === "menu" && (
        <div>
          <div className="mb-4 flex items-center justify-between gap-2 border-b border-border-light">
            <div className="flex gap-1" role="tablist">
              {(["word", "phrase", "passage"] as SelectionMenuKind[]).map((kind) => (
                <button
                  key={kind}
                  type="button"
                  role="tab"
                  aria-selected={menuKind === kind}
                  onClick={() => requestNavigation(() => setMenuKind(kind))}
                  className={`h-9 border-b-2 px-3 text-[12px] font-medium ${menuKind === kind ? "border-accent text-accent-text" : "border-transparent text-text-muted"}`}
                >
                  {t(`settings.tools.cardKind.${kind}`)}
                </button>
              ))}
            </div>
            {!previewOpen && (
              <button
                type="button"
                onClick={() => setPreviewOpen(true)}
                title={t("settings.tools.showPreview")}
                aria-label={t("settings.tools.showPreview")}
                className="flex size-8 shrink-0 items-center justify-center rounded-md text-text-muted hover:bg-bg-input hover:text-text-primary"
              >
                <PanelRightOpen size={15} />
              </button>
            )}
          </div>
          <div className="mx-auto w-full max-w-[620px]">
            <SelectionMenuSettings
              kind={menuKind}
              value={config.selectionMenus[menuKind]}
              onChange={(menu) => persistConfig({
                ...config,
                selectionMenus: { ...config.selectionMenus, [menuKind]: menu },
              })}
              onTouched={touch}
              importSources={menuImportSources(menuKind)}
              requestNavigation={requestNavigation}
              onEditorGuardChange={handleEditorGuardChange}
              onTest={(text, draft) => {
                setCustomActionTest({ name: draft.name, prompt: draft.prompt, text, nonce: Date.now() });
                setPreviewOpen(true);
              }}
            />
          </div>
        </div>
      )}

      {view === "markers" && (
        <div>
          <MarkerStyleSettings value={markerStyle} onChange={persistMarkerStyle} />
          <div className="mx-auto mt-4 w-full max-w-[620px] border-t border-border-light pt-2">
            <SettingsRow
              title={t("settings.tools.autoHighlightLookupWords", { defaultValue: "查词后自动标记" })}
              subtitle={t("settings.tools.autoHighlightLookupWordsHint", {
                defaultValue: "查词成功后创建单词标记；手动标记始终保持独立。",
              })}
            >
              <Toggle
                label={t("settings.tools.autoHighlightLookupWords", { defaultValue: "查词后自动标记" })}
                checked={autoHighlightLookupWords}
                onChange={(enabled) => {
                  setAutoHighlightLookupWords(enabled);
                  persistLegacy("auto_highlight_lookup_words", String(enabled));
                }}
              />
            </SettingsRow>
          </div>
        </div>
      )}

      {densityHelpOpen && <DensityHelpDialog initialKind={cardKind} onClose={() => setDensityHelpOpen(false)} />}
      {guardOpen && editorController && (
        <ConfirmDialog
          title={t("settings.tools.custom.unsavedTitle", {
            name: editorController.name || t("settings.tools.custom.untitled"),
          })}
          description={t("settings.tools.custom.unsavedDescription")}
          primaryLabel={t("common.save")}
          primaryDisabled={!editorController.canSave}
          onPrimary={() => {
            if (editorControllerRef.current?.save()) finishPendingNavigation();
          }}
          secondaryLabel={t("settings.tools.custom.discard")}
          onSecondary={() => {
            editorControllerRef.current?.discard();
            finishPendingNavigation();
          }}
          tertiaryLabel={t("settings.tools.custom.continueEditing")}
          onTertiary={continueEditing}
        />
      )}
    </div>
  );
}
