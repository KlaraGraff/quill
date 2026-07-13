import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Bookmark, Copy, Highlighter, Languages, Loader2, MessageSquareMore, RotateCcw, WandSparkles } from "lucide-react";
import { useTranslation } from "react-i18next";
import {
  getLearningCardFixture,
  LearningCardView,
  MENU_ACTION_DEFINITIONS,
  type CardDesignConfigV1,
  type LearningCardResult,
  type LearningCardKind,
  type SelectionMenuActionId,
  type SelectionMenuKind,
} from "../learning-card";

interface CardPreviewProps {
  kind: LearningCardKind;
  config: CardDesignConfigV1;
  explanationLanguage: string;
  targetLanguage: string;
  learnerLevel?: string;
  explanationMode?: string;
  showMenu?: boolean;
}

const actionIcons: Record<SelectionMenuActionId, typeof WandSparkles> = {
  define: WandSparkles,
  explain: WandSparkles,
  ask_ai: MessageSquareMore,
  collect: Bookmark,
  highlight: Highlighter,
  translate: Languages,
  copy: Copy,
};

export default function CardPreview({
  kind,
  config,
  explanationLanguage,
  targetLanguage,
  learnerLevel = "B1",
  explanationMode = "adaptive_bilingual",
  showMenu = false,
}: CardPreviewProps) {
  const { t } = useTranslation();
  const frameRef = useRef<HTMLDivElement>(null);
  const previewRequestRef = useRef<string | null>(null);
  const [availableWidth, setAvailableWidth] = useState(704);
  const [realResult, setRealResult] = useState<LearningCardResult | null>(null);
  const [realLoading, setRealLoading] = useState(false);
  const [realError, setRealError] = useState<string | null>(null);
  const localResult = useMemo(() => {
    const language = explanationLanguage === "zh" ? "zh" : "en";
    const fixture = getLearningCardFixture(kind, language);
    const contextMeaning = fixture.modules.context_meaning;
    if (contextMeaning) {
      fixture.modules = {
        ...fixture.modules,
        context_meaning: {
          ...contextMeaning,
          meta: [`CEFR ${learnerLevel}`, ...(contextMeaning.meta ?? [])],
          details: explanationMode === "adaptive_bilingual" && ["A1", "A2"].includes(learnerLevel)
            ? [
                ...(contextMeaning.details ?? []).slice(0, 1),
                language === "zh"
                  ? "英文部分会使用短句和基础词汇，中文用于保证释义准确。"
                  : "中文会补充容易误解的细节，英文保持短而清楚。",
              ]
            : contextMeaning.details,
        },
      };
    }
    if (targetLanguage && targetLanguage === explanationLanguage) {
      const modules = { ...fixture.modules };
      delete modules.target_translation;
      return { ...fixture, modules };
    }
    return fixture;
  }, [explanationLanguage, explanationMode, kind, learnerLevel, targetLanguage]);

  useEffect(() => {
    if (previewRequestRef.current) {
      invoke("ai_cancel", { requestId: previewRequestRef.current }).catch(() => {});
      previewRequestRef.current = null;
    }
    setRealResult(null);
    setRealLoading(false);
    setRealError(null);
  }, [config, explanationLanguage, explanationMode, kind, learnerLevel, targetLanguage]);

  useEffect(() => () => {
    if (previewRequestRef.current) {
      invoke("ai_cancel", { requestId: previewRequestRef.current }).catch(() => {});
    }
  }, []);

  useEffect(() => {
    const element = frameRef.current;
    if (!element) return;
    const update = () => setAvailableWidth(Math.max(0, Math.round(element.getBoundingClientRect().width)));
    update();
    const observer = new ResizeObserver(update);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const menuKind: SelectionMenuKind = kind === "passage" ? "passage" : "phrase";
  const definitions = new Map(MENU_ACTION_DEFINITIONS[menuKind].map((item) => [item.id, item]));
  const menuItems = config.selectionMenus[menuKind].filter((item) => item.enabled);
  const result = realResult ?? localResult;

  const generateRealPreview = async () => {
    const requestId = crypto.randomUUID();
    if (previewRequestRef.current) {
      await invoke("ai_cancel", { requestId: previewRequestRef.current }).catch(() => {});
    }
    previewRequestRef.current = requestId;
    setRealLoading(true);
    setRealError(null);
    try {
      const response = await invoke<LearningCardResult>("ai_learning_card", {
        text: localResult.sourceText,
        context: localResult.modules.source_excerpt?.quote ?? localResult.sourceText,
        kind,
        bookTitle: null,
        chapter: null,
        cardConfig: JSON.stringify(config),
        requestId,
      });
      if (previewRequestRef.current === requestId) setRealResult(response);
    } catch (error) {
      if (previewRequestRef.current === requestId) {
        setRealError(error instanceof Error ? error.message : String(error));
      }
    } finally {
      if (previewRequestRef.current === requestId) {
        previewRequestRef.current = null;
        setRealLoading(false);
      }
    }
  };

  return (
    <div className="min-w-0 lg:sticky lg:top-0">
      <div className="mb-2 flex min-h-8 items-center justify-between gap-3">
        <div>
          <p className="text-[10px] font-semibold uppercase tracking-[0.3px] text-text-muted">
            {t("settings.tools.preview")}
          </p>
          <p className="text-[10px] text-text-placeholder">
            {realResult
              ? t("settings.tools.realPreviewActive", { defaultValue: "当前显示真实 AI 结果" })
              : t("settings.tools.localPreviewHint", { defaultValue: "本地样例，不消耗 API" })}
          </p>
        </div>
        <div className="flex items-center gap-1">
          {realResult && (
            <button
              type="button"
              onClick={() => setRealResult(null)}
              title={t("settings.tools.restoreLocalPreview", { defaultValue: "恢复本地预览" })}
              aria-label={t("settings.tools.restoreLocalPreview", { defaultValue: "恢复本地预览" })}
              className="flex size-8 items-center justify-center rounded-md text-text-muted hover:bg-bg-input"
            >
              <RotateCcw size={14} />
            </button>
          )}
          <button
            type="button"
            onClick={generateRealPreview}
            disabled={realLoading}
            title={t("settings.tools.generateRealPreviewHint", { defaultValue: "调用当前 AI 服务并产生一次网络请求" })}
            className="flex h-8 items-center gap-1.5 rounded-md border border-border bg-bg-surface px-2.5 text-[11px] font-medium text-text-secondary hover:border-accent disabled:opacity-50"
          >
            {realLoading ? <Loader2 size={13} className="animate-spin" /> : <WandSparkles size={13} />}
            {t("settings.tools.generateRealPreview", { defaultValue: "生成真实预览" })}
          </button>
        </div>
      </div>
      {realError && <p role="alert" className="mb-2 break-words text-[11px] text-danger-text">{realError}</p>}
      <div
        ref={frameRef}
        className="flex min-h-[460px] min-w-0 flex-col items-center gap-3 overflow-hidden rounded-md border border-border bg-bg-muted p-3"
      >
        {showMenu && (
          <div role="toolbar" aria-label={t("settings.tools.menu.previewLabel")} className="flex max-w-full flex-wrap items-center justify-center gap-1 rounded-md border border-border bg-bg-surface p-1 shadow-popover">
            {menuItems.map((item) => {
              const definition = definitions.get(item.id);
              const Icon = actionIcons[item.id];
              if (!definition) return null;
              return (
                <button
                  key={item.id}
                  type="button"
                  tabIndex={-1}
                  className="flex h-8 items-center gap-1.5 rounded-sm px-2 text-[11px] font-medium text-text-secondary"
                >
                  <Icon size={13} className="text-text-muted" />
                  {t(definition.labelKey)}
                </button>
              );
            })}
          </div>
        )}
        <LearningCardView
          result={result}
          config={config}
          availableWidth={availableWidth}
          maxHeight={showMenu ? 400 : 430}
          presentationMode
        />
      </div>
    </div>
  );
}
