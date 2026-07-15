import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { Bookmark, Copy, Highlighter, Languages, Loader2, MessageSquareMore, RotateCcw, WandSparkles } from "lucide-react";
import { useTranslation } from "react-i18next";
import {
  getLearningCardFixture,
  LearningCardView,
  LearningCardStreamParser,
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
  lastTouched?: { id: string; nonce: number } | null;
  testText?: string;
  testNonce?: number;
  customActionTest?: { name: string; prompt: string; text: string; nonce: number };
}

type PreviewMarkState = "unmarked" | "current" | "book";

const actionIcons: Record<SelectionMenuActionId, typeof WandSparkles> = {
  define: WandSparkles,
  explain: WandSparkles,
  ask_ai: MessageSquareMore,
  collect: Bookmark,
  highlight: Highlighter,
  translate: Languages,
  copy: Copy,
};

const TARGET_TRANSLATION_PREVIEWS: Record<LearningCardKind, Record<string, string>> = {
  word: {
    en: "interface; point where systems meet",
    zh: "界面；交界处；接口",
  },
  phrase: {
    en: "It turned out to be a hidden benefit.",
    zh: "结果证明这是因祸得福。",
  },
  passage: {
    en: "New ideas often emerge where established fields meet.",
    zh: "新的想法往往产生于成熟领域之间的交界处。",
  },
};

const ADAPTIVE_EXPLANATION_PREVIEWS: Record<
  LearningCardKind,
  { beginnerEnglish: string; intermediateChinese: string }
> = {
  word: {
    beginnerEnglish: "Simple English: Here, interface means a place where two things meet and work together.",
    intermediateChinese: "中文补充：这里强调两个领域发生互动的交界处，而不只是静态边界。",
  },
  phrase: {
    beginnerEnglish: "Simple English: It looked bad at first, but it brought something good later.",
    intermediateChinese: "中文补充：这个短语表示一件起初不利的事最终带来了好结果。",
  },
  passage: {
    beginnerEnglish: "Simple English: New ideas can come when different fields work together.",
    intermediateChinese: "中文补充：作者强调跨领域合作更容易产生新想法。",
  },
};

const SCROLL_DURATION_MS = 350;
const TARGET_WAIT_MS = 1_200;

function easeInOutCubic(progress: number) {
  return progress < 0.5
    ? 4 * progress * progress * progress
    : 1 - Math.pow(-2 * progress + 2, 3) / 2;
}

function scrollToModule(
  container: HTMLElement,
  target: HTMLElement,
  onComplete: () => void,
): () => void {
  const containerRect = container.getBoundingClientRect();
  const targetRect = target.getBoundingClientRect();
  const start = container.scrollTop;
  const desired = start
    + targetRect.top
    - containerRect.top
    - (container.clientHeight - targetRect.height) / 2;
  const maxScroll = Math.max(0, container.scrollHeight - container.clientHeight);
  const destination = Math.max(0, Math.min(maxScroll, desired));
  const distance = destination - start;
  let animationFrame = 0;
  let cancelled = false;
  let startedAt: number | null = null;

  const cancel = () => {
    cancelled = true;
    window.cancelAnimationFrame(animationFrame);
    container.removeEventListener("wheel", cancel);
    container.removeEventListener("touchstart", cancel);
    container.removeEventListener("pointerdown", cancel);
  };
  const finish = () => {
    cancel();
    onComplete();
  };
  const step = (timestamp: number) => {
    if (cancelled) return;
    startedAt ??= timestamp;
    const progress = Math.min(1, (timestamp - startedAt) / SCROLL_DURATION_MS);
    container.scrollTop = start + distance * easeInOutCubic(progress);
    if (progress < 1) {
      animationFrame = window.requestAnimationFrame(step);
    } else {
      finish();
    }
  };

  container.addEventListener("wheel", cancel, { passive: true });
  container.addEventListener("touchstart", cancel, { passive: true });
  container.addEventListener("pointerdown", cancel, { passive: true });
  if (Math.abs(distance) < 1) {
    finish();
  } else {
    animationFrame = window.requestAnimationFrame(step);
  }
  return cancel;
}

export default function CardPreview({
  kind,
  config,
  explanationLanguage,
  targetLanguage,
  learnerLevel = "B1",
  explanationMode = "adaptive_bilingual",
  showMenu = false,
  lastTouched = null,
  testText,
  testNonce,
  customActionTest,
}: CardPreviewProps) {
  const { t } = useTranslation();
  const frameRef = useRef<HTMLDivElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const previewRequestRef = useRef<string | null>(null);
  const scrollCancelRef = useRef<(() => void) | null>(null);
  const targetFrameRef = useRef(0);
  const highlightTimerRef = useRef(0);
  const scrollContextRef = useRef({ config, kind, lastTouched });
  const [availableWidth, setAvailableWidth] = useState(704);
  const [availableHeight, setAvailableHeight] = useState(430);
  const [menuHeight, setMenuHeight] = useState(0);
  const [realResult, setRealResult] = useState<LearningCardResult | null>(null);
  const [realLoading, setRealLoading] = useState(false);
  const [realError, setRealError] = useState<string | null>(null);
  const [previewMarkState, setPreviewMarkState] = useState<PreviewMarkState>("unmarked");
  const [highlightedId, setHighlightedId] = useState<string | null>(null);
  const [customActionResult, setCustomActionResult] = useState("");
  const [customActionLoading, setCustomActionLoading] = useState(false);
  const localResult = useMemo(() => {
    const level = learnerLevel.trim().toUpperCase();
    const beginnerAdaptive = explanationMode === "adaptive_bilingual" && ["A1", "A2"].includes(level);
    const language = explanationMode === "chinese" || beginnerAdaptive
      ? "zh"
      : explanationMode === "english_by_level" || explanationMode === "adaptive_bilingual"
        ? "en"
        : explanationLanguage === "zh"
          ? "zh"
          : "en";
    const fixture = getLearningCardFixture(kind, language);
    const contextMeaning = fixture.modules.context_meaning;
    if (contextMeaning) {
      const adaptivePreview = ADAPTIVE_EXPLANATION_PREVIEWS[kind];
      const bilingualDetails = explanationMode === "adaptive_bilingual"
        ? beginnerAdaptive
          ? adaptivePreview.beginnerEnglish
          : level === "B1"
            ? adaptivePreview.intermediateChinese
            : null
        : null;
      fixture.modules = {
        ...fixture.modules,
        context_meaning: {
          ...contextMeaning,
          meta: [`CEFR ${level}`, ...(contextMeaning.meta ?? [])],
          details: bilingualDetails
            ? [
                ...(contextMeaning.details ?? []).slice(0, beginnerAdaptive ? 1 : undefined),
                bilingualDetails,
              ]
            : contextMeaning.details,
        },
      };
    }
    const sameAsPureExplanation = (explanationMode === "chinese" && targetLanguage === "zh")
      || (explanationMode === "english_by_level" && targetLanguage === "en")
      || (explanationMode === "adaptive_bilingual"
        && ["B2", "C1", "C2"].includes(level)
        && targetLanguage === "en");
    const targetTranslation = TARGET_TRANSLATION_PREVIEWS[kind][targetLanguage]
      ?? `${targetLanguage.toUpperCase()}: ${TARGET_TRANSLATION_PREVIEWS[kind].en}`;
    if (sameAsPureExplanation) {
      const modules = { ...fixture.modules };
      delete modules.target_translation;
      fixture.modules = modules;
    } else if (fixture.modules.target_translation) {
      fixture.modules = {
        ...fixture.modules,
        target_translation: {
          ...fixture.modules.target_translation,
          summary: targetTranslation,
        },
      };
    }
    for (const module of config.cards[kind].modules) {
      if (!module.enabled || !module.id.startsWith("custom_")) continue;
      const definition = config.cards[kind].customModules[module.id as `custom_${string}`];
      if (!definition) continue;
      fixture.modules[module.id] = {
        summary: t("settings.tools.custom.previewPlaceholder", { name: definition.name }),
      };
    }
    return fixture;
  }, [config, explanationLanguage, explanationMode, kind, learnerLevel, t, targetLanguage]);

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
    if (kind !== "word" && previewMarkState === "book") {
      setPreviewMarkState("current");
    }
  }, [kind, previewMarkState]);

  useEffect(() => {
    const element = frameRef.current;
    if (!element) return;
    const update = () => {
      const rect = element.getBoundingClientRect();
      setAvailableWidth(Math.max(0, Math.round(rect.width)));
      setAvailableHeight(Math.max(0, Math.round(rect.height)));
      setMenuHeight(Math.max(0, Math.round(menuRef.current?.getBoundingClientRect().height ?? 0)));
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(element);
    if (menuRef.current) observer.observe(menuRef.current);
    return () => observer.disconnect();
  }, [showMenu]);

  useEffect(() => {
    scrollContextRef.current = { config, kind, lastTouched };
  }, [config, kind, lastTouched]);

  useEffect(() => {
    const { config: currentConfig, kind: currentKind, lastTouched: currentTouch } = scrollContextRef.current;
    if (!currentTouch) return;
    const { id } = currentTouch;
    const cardModule = currentConfig.cards[currentKind].modules.find((module) => module.id === id);
    const menuItem = currentConfig.selectionMenus[currentKind].find((item) => item.id === id);
    if (cardModule && !cardModule.enabled) return;
    if (menuItem && !menuItem.enabled) return;

    scrollCancelRef.current?.();
    window.cancelAnimationFrame(targetFrameRef.current);
    window.clearTimeout(highlightTimerRef.current);
    const startedAt = performance.now();
    let disposed = false;
    const highlight = () => {
      if (disposed) return;
      setHighlightedId(id);
      highlightTimerRef.current = window.setTimeout(() => {
        setHighlightedId((current) => current === id ? null : current);
      }, 800);
    };
    const findTarget = () => {
      if (disposed) return;
      const escaped = CSS.escape(id);
      const target = frameRef.current?.querySelector<HTMLElement>(
        `[data-module-id="${escaped}"],[data-menu-id="${escaped}"]`,
      );
      if (target && !target.closest('[data-module-exiting="true"]')) {
        const container = target.closest<HTMLElement>("[data-card-scroll]");
        scrollCancelRef.current = container
          ? scrollToModule(container, target, highlight)
          : null;
        if (!container) highlight();
        return;
      }
      if (performance.now() - startedAt < TARGET_WAIT_MS) {
        targetFrameRef.current = window.requestAnimationFrame(findTarget);
      }
    };
    targetFrameRef.current = window.requestAnimationFrame(findTarget);
    return () => {
      disposed = true;
      window.cancelAnimationFrame(targetFrameRef.current);
      scrollCancelRef.current?.();
      scrollCancelRef.current = null;
    };
  }, [lastTouched?.nonce]);

  useEffect(() => () => {
    window.cancelAnimationFrame(targetFrameRef.current);
    window.clearTimeout(highlightTimerRef.current);
    scrollCancelRef.current?.();
  }, []);

  const menuKind: SelectionMenuKind = kind;
  const previewMarkStates: PreviewMarkState[] = kind === "word"
    ? ["unmarked", "current", "book"]
    : ["unmarked", "current"];
  const previewMarked = previewMarkState !== "unmarked";
  const definitions = new Map(MENU_ACTION_DEFINITIONS[menuKind].map((item) => [item.id, item]));
  const menuItems = config.selectionMenus[menuKind].filter((item) => item.enabled);
  const result = realResult ?? localResult;

  const generateRealPreview = async (sourceText = localResult.sourceText) => {
    const requestId = crypto.randomUUID();
    if (previewRequestRef.current) {
      await invoke("ai_cancel", { requestId: previewRequestRef.current }).catch(() => {});
    }
    previewRequestRef.current = requestId;
    setRealLoading(true);
    setRealError(null);
    setRealResult({ version: 1, kind, sourceText, modules: {} });
    const allowedIds = new Set(config.cards[kind].modules.filter((module) => module.enabled).map((module) => module.id));
    const parser = new LearningCardStreamParser(allowedIds);
    let unlisten: UnlistenFn | undefined;
    try {
      unlisten = await listen<{ delta: string; done: boolean }>(`ai-learning-card-chunk-${requestId}`, (event) => {
        if (previewRequestRef.current !== requestId || event.payload.done || !event.payload.delta) return;
        const modules = parser.push(event.payload.delta);
        if (Object.keys(modules).length === 0) return;
        setRealResult((current) => current ? { ...current, modules: { ...current.modules, ...modules } } : current);
      });
      const response = await invoke<LearningCardResult>("ai_learning_card", {
        text: sourceText,
        context: sourceText,
        kind,
        bookTitle: null,
        bookAuthor: null,
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
      unlisten?.();
      if (previewRequestRef.current === requestId) {
        previewRequestRef.current = null;
        setRealLoading(false);
      }
    }
  };

  useEffect(() => {
    if (!testText || !testNonce) return;
    void generateRealPreview(testText);
    // A nonce deliberately retriggers the same test input.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [testNonce]);

  useEffect(() => {
    if (!customActionTest) return;
    let active = true;
    let unlisten: UnlistenFn | undefined;
    const requestId = crypto.randomUUID();
    setCustomActionResult("");
    setCustomActionLoading(true);
    const run = async () => {
      unlisten = await listen<{ delta: string; done: boolean }>(`ai-custom-action-chunk-${requestId}`, (event) => {
        if (!active) return;
        if (event.payload.done) {
          setCustomActionLoading(false);
          return;
        }
        setCustomActionResult((current) => current + event.payload.delta);
      });
      await invoke("ai_custom_action", {
        name: customActionTest.name,
        prompt: customActionTest.prompt,
        text: customActionTest.text,
        context: customActionTest.text,
        bookTitle: null,
        chapter: null,
        requestId,
      }).catch((error) => {
        if (active) setCustomActionResult(error instanceof Error ? error.message : String(error));
      });
      if (active) setCustomActionLoading(false);
    };
    void run().catch((error) => {
      if (active) {
        setCustomActionResult(error instanceof Error ? error.message : String(error));
        setCustomActionLoading(false);
      }
    });
    return () => {
      active = false;
      unlisten?.();
      void invoke("ai_cancel", { requestId });
    };
  }, [customActionTest]);

  return (
    <div className="flex h-full min-h-0 min-w-0 flex-col lg:sticky lg:top-0">
      <div className="mb-2 flex min-h-8 items-center justify-between gap-3">
        <div>
          <p className="text-[10px] font-semibold uppercase tracking-[0.3px] text-text-muted">
            {t("settings.tools.preview")}
          </p>
          <p className="text-[10px] text-text-placeholder">
            {realResult
              ? t("settings.tools.realPreviewActive")
              : t("settings.tools.localPreviewHint")}
          </p>
        </div>
        <div className="flex items-center gap-1">
          {realResult && (
            <button
              type="button"
              onClick={() => setRealResult(null)}
              title={t("settings.tools.restoreLocalPreview")}
              aria-label={t("settings.tools.restoreLocalPreview")}
              className="flex size-8 items-center justify-center rounded-md text-text-muted hover:bg-bg-input"
            >
              <RotateCcw size={14} />
            </button>
          )}
          <button
            type="button"
            onClick={() => void generateRealPreview()}
            disabled={realLoading}
            title={t("settings.tools.generateRealPreviewHint")}
            className="flex h-8 items-center gap-1.5 rounded-md border border-border bg-bg-surface px-2.5 text-[11px] font-medium text-text-secondary hover:border-accent disabled:opacity-50"
          >
            {realLoading ? <Loader2 size={13} className="animate-spin" /> : <WandSparkles size={13} />}
            {t("settings.tools.generateRealPreview")}
          </button>
        </div>
      </div>
      {realError && <p role="alert" className="mb-2 break-words text-[11px] text-danger-text">{realError}</p>}
      <div
        ref={frameRef}
        className="flex min-h-[460px] min-w-0 flex-1 flex-col items-center gap-3 overflow-hidden rounded-md border border-border bg-bg-muted p-3"
      >
        {showMenu && (
          <div ref={menuRef} className="flex max-w-full shrink-0 flex-col items-center gap-2">
            <div className="flex items-center gap-2 text-[10px] text-text-muted">
              <span>{t("settings.tools.menu.previewState")}</span>
              <div className="flex rounded-md bg-bg-input p-0.5">
                {previewMarkStates.map((state) => (
                  <button
                    key={state}
                    type="button"
                    aria-pressed={previewMarkState === state}
                    onClick={() => setPreviewMarkState(state)}
                    className={`h-7 rounded-sm px-2 text-[10px] font-medium transition-colors ${
                      previewMarkState === state
                        ? "bg-bg-surface text-text-primary shadow-sm"
                        : "text-text-muted hover:text-text-secondary"
                    }`}
                  >
                    {t(`settings.tools.menu.previewState.${state}`)}
                  </button>
                ))}
              </div>
            </div>
            <div role="toolbar" aria-label={t("settings.tools.menu.previewLabel")} className="flex max-w-full flex-wrap items-center justify-center gap-1 rounded-md border border-border bg-bg-surface p-1 shadow-popover">
              {menuItems.map((item) => {
                const definition = definitions.get(item.id);
                const custom = item.id.startsWith("custom_") && item.name;
                const Icon = custom ? WandSparkles : actionIcons[item.id];
                if (!definition && !custom) return null;
                const label = item.id === "highlight"
                  ? !previewMarked
                    ? t("contextMenu.mark")
                    : kind === "word"
                      ? t("contextMenu.removeCurrentMark")
                      : t("contextMenu.removeHighlight")
                  : custom ? item.name! : t(definition!.labelKey);
                return (
                  <Fragment key={item.id}>
                    <button
                      type="button"
                      tabIndex={-1}
                      data-menu-id={item.id}
                      className="flex h-8 items-center gap-1.5 rounded-sm px-2 text-[11px] font-medium text-text-secondary"
                      style={highlightedId === item.id ? { outline: "2px solid var(--color-accent)", outlineOffset: "1px" } : undefined}
                    >
                      <Icon size={13} className="text-text-muted" />
                      {label}
                    </button>
                    {item.id === "highlight" && previewMarkState === "book" && kind === "word" && (
                      <button
                        type="button"
                        tabIndex={-1}
                        className="flex h-8 items-center gap-1.5 rounded-sm px-2 text-[11px] font-medium text-text-secondary"
                      >
                        <Highlighter size={13} className="text-text-muted" />
                        {t("contextMenu.removeBookWordMark")}
                      </button>
                    )}
                  </Fragment>
                );
              })}
            </div>
          </div>
        )}
        {showMenu && customActionTest && (
          <div className="w-full max-w-[520px] rounded-md border border-border bg-bg-surface shadow-context">
            <div className="flex h-10 items-center gap-2 border-b border-border-light bg-accent-bg px-3 text-[12px] font-medium text-accent-text">
              <WandSparkles size={13} />{customActionTest.name}
              {customActionLoading && <Loader2 size={12} className="ml-auto animate-spin" />}
            </div>
            <p className="max-h-[180px] overflow-y-auto whitespace-pre-wrap px-3 py-3 text-[12px] leading-5 text-text-primary">
              {customActionResult || t("explain.thinking")}
            </p>
          </div>
        )}
        <LearningCardView
          result={result}
          config={config}
          availableWidth={availableWidth}
          maxHeight={Math.max(360, availableHeight - 24 - (showMenu ? menuHeight + 12 : 0))}
          presentationMode
          highlightedModuleId={highlightedId}
          animateModuleChanges
        />
      </div>
    </div>
  );
}
