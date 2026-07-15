import { useState, useRef, useCallback, useEffect, useMemo } from "react";
import { useParams, useNavigate, useLocation } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import {
  ArrowLeft,
  BookOpen,
  List,
  Bookmark,
  Bot,
  Languages,
  Loader2,
  Minus,
  Plus,
} from "lucide-react";
import Button from "../components/ui/Button";
import Toast from "../components/ui/Toast";
import AiPanel from "../components/AiPanel";
import BookmarksPanel from "../components/BookmarksPanel";
import ReaderSettings, { type ReaderSettingsState } from "../components/ReaderSettings";
import {
  getThemeStyles,
  getReaderCapabilities,
} from "../components/reader-settings";
import ReaderContextMenu, { type ReaderMenuAction } from "../components/ReaderContextMenu";
import DictionaryPanel from "../components/DictionaryPanel";
import TranslationPopover from "../components/TranslationPopover";
import TableOfContents from "../components/TableOfContents";
import TextBookReader from "../components/TextBookReader";
import { textLocation, type TextBookDocument } from "../components/text-book-location";
import type { CitedSource } from "../hooks/useAiChat";
import {
  classifySelection,
  normalizeInteractionText,
  serializableRect,
  type ReaderInteraction,
  type SerializableRect,
} from "../components/reader-interaction";
import {
  planCfiHighlightRemoval,
  planCfiHighlightMutation,
  planTextHighlightRemoval,
  planTextHighlightMutation,
  type HighlightMutationPlan,
} from "../components/highlight-ranges";
import { getBook, type Book } from "../hooks/useBooks";
import { getAllSettings } from "../hooks/useSettings";
import type { Highlight } from "../hooks/useBookmarks";
import {
  DEFAULT_CARD_DESIGN_CONFIG,
  LearningCardController,
  parseCardDesignConfig,
  type CardDesignConfigV1,
} from "../components/learning-card";
import {
  MARKER_STYLE_SETTING_KEY,
  createDefaultMarkerStyleConfig,
  parseMarkerStyleConfig,
  type MarkerStyleConfigV1,
} from "../components/marker-style";
import { loadCustomFonts } from "../components/custom-fonts";
import {
  listenForReadingAssistanceSettingsChanged,
  readingAssistanceSettingsChanged,
} from "../components/reading-assistance-events";
import {
  runPageTurnTransition,
} from "../components/page-turn-transition";
import {
  getPdfOverlays,
  getReaderThemeVars,
} from "./reader/reader-theme";
import { ReadingProgressWriter } from "./reader/reading-progress-writer";
import { useBookAvailability } from "./reader/useBookAvailability";
import { usePageTurnInput } from "./reader/usePageTurnInput";
import { useReaderInteractions } from "./reader/useReaderInteractions";
import {
  mergeStoredReaderSettings,
  useReaderSettingsSync,
} from "./reader/useReaderSettingsSync";
import { useWindowSizePersistence } from "./reader/useWindowSizePersistence";
import { useSidePanelResize } from "./reader/useSidePanelResize";
import {
  useFoliateAnnotations,
  type LookupOccurrenceMark,
  type WordMarkException,
  type WordMarkRule,
} from "./reader/useFoliateAnnotations";
import type {
  FoliateView,
  ReaderPageInfo,
  TocChapter,
} from "./reader/foliate-types";
import { useFoliateView } from "./reader/useFoliateView";
import { useReaderNavigation } from "./reader/useReaderNavigation";

type SidePanel = "ai" | "bookmarks" | "vocab" | null;

const readerMenuActionMap: Record<string, ReaderMenuAction> = {
  define: "primary",
  explain: "primary",
  ask_ai: "ask-ai",
  collect: "save",
  highlight: "highlight",
  translate: "translate",
  copy: "copy",
};

const appWindow = getCurrentWebviewWindow();
const isStandaloneWindow = appWindow.label.startsWith("reader-");

// One-time migration of `reader-zoom-${bookId}` keys written by PR #199.
// That PR saved "100" on every default open, so every pre-upgrade book has
// it even when the user never touched zoom. Under the new scheme, the
// default is "fit" and "100" should mean the user explicitly chose 100%.
// Rewrite legacy "100" → "fit" once, guarded by a global marker so new
// explicit 100% saves aren't clobbered on subsequent opens.
(() => {
  try {
    if (localStorage.getItem("reader-zoom-v2")) return;
    for (let i = localStorage.length - 1; i >= 0; i--) {
      const k = localStorage.key(i);
      if (k?.startsWith("reader-zoom-") && localStorage.getItem(k) === "100") {
        localStorage.setItem(k, "fit");
      }
    }
    localStorage.setItem("reader-zoom-v2", "1");
  } catch { /* private-mode storage failures are non-fatal */ }
})();

interface TextReaderProgressDetails {
  chapterProgress: number;
  page?: ReaderPageInfo;
}

export default function Reader() {
  const { bookId } = useParams();
  const navigate = useNavigate();
  const location = useLocation();
  const { t } = useTranslation();
  const [book, setBook] = useState<Book | null>(null);
  const isTextBook = book?.render_format === "text";
  const capabilities = useMemo(
    () => getReaderCapabilities(book?.render_format || book?.format),
    [book?.format, book?.render_format],
  );
  const supportsSelection = capabilities.supportsSelection;
  const supportsManualAnnotations = capabilities.supportsManualAnnotations;
  const supportsWordMarkers = capabilities.supportsWordMarkers;
  const supportsCfiNavigation = capabilities.supportsCfiNavigation;
  const [loading, setLoading] = useState(true);
  const [sidePanel, setSidePanel] = useState<SidePanel>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [zoom, setZoom] = useState<number | "fit">("fit");
  const [tocOpen, setTocOpen] = useState(false);
  const [chapters, setChapters] = useState<TocChapter[]>([]);
  const [currentChapterIndex, setCurrentChapterIndex] = useState(-1);
  const [progress, setProgress] = useState(0);
  const [chapterProgress, setChapterProgress] = useState(0);
  const [pageInfo, setPageInfo] = useState<ReaderPageInfo | null>(null);
  const currentCfiRef = useRef<string | null>(null);
  const [progressWriter] = useState(() => new ReadingProgressWriter());
  const [bookReady, setBookReady] = useState(false);
  const [canGoBack, setCanGoBack] = useState(false);
  const [readerError, setReaderError] = useState<string | null>(null);
  const [readerRetry, setReaderRetry] = useState(0);
  const [pdfTextLayerNotice, setPdfTextLayerNotice] = useState(false);
  const [textInitialLocation, setTextInitialLocation] = useState<string | null>(null);
  const [contextMenu, setContextMenu] = useState<ReaderInteraction | null>(null);
  const [contextSelectionFullyMarked, setContextSelectionFullyMarked] = useState(false);
  const [contextManualSelectionFullyMarked, setContextManualSelectionFullyMarked] = useState(false);
  const [contextHasManualSelectionMark, setContextHasManualSelectionMark] = useState(false);
  const [contextHasLookupOccurrenceMark, setContextHasLookupOccurrenceMark] = useState(false);
  const [contextHasBookWordMark, setContextHasBookWordMark] = useState(false);
  const [contextBookWordMarkExcluded, setContextBookWordMarkExcluded] = useState(false);
  const [contextMarkStateLoading, setContextMarkStateLoading] = useState(false);
  const [learningCardConfig, setLearningCardConfig] = useState<CardDesignConfigV1>(DEFAULT_CARD_DESIGN_CONFIG);
  const [learningInteraction, setLearningInteraction] = useState<ReaderInteraction | null>(null);
  const [readerToast, setReaderToast] = useState<string | null>(null);
  const [readerRect, setReaderRect] = useState<SerializableRect | null>(null);
  const [aiContext, setAiContext] = useState<{ text: string; cfi?: string; analysis?: string } | undefined>();
  const [initialChatId, setInitialChatId] = useState<string | undefined>();
  const [activeVocabCfi, setActiveVocabCfi] = useState<string | null>(null);
  const [translation, setTranslation] = useState<{
    x: number;
    y: number;
    text: string;
    context?: string;
    cfi?: string;
  } | null>(null);
  const {
    readerSettings,
    setReaderSettings,
    readerSettingsRef,
    settingsLoadedBookRef: dbSettingsLoadedRef,
    handleReaderSettingsChange,
  } = useReaderSettingsSync(bookId);
  const autoHighlightLookupsRef = useRef(true);
  const [markerStyle, setMarkerStyle] = useState<MarkerStyleConfigV1>(createDefaultMarkerStyleConfig);
  const markerStyleRef = useRef(markerStyle);
  const markMatchingWordsRef = useRef(markerStyle.markMatchingWords);
  const doubleClickQuickLookupRef = useRef(true);
  const [doubleClickQuickLookup, setDoubleClickQuickLookup] = useState(true);

  const applyReadingAssistanceSettings = useCallback((settings: Record<string, string>) => {
    const doubleClick = settings.double_click_quick_lookup !== "false";
    const nextMarkerStyle = parseMarkerStyleConfig(settings[MARKER_STYLE_SETTING_KEY]);
    doubleClickQuickLookupRef.current = doubleClick;
    autoHighlightLookupsRef.current = settings.auto_highlight_lookup_words !== "false";
    markerStyleRef.current = nextMarkerStyle;
    markMatchingWordsRef.current = nextMarkerStyle.markMatchingWords;
    setDoubleClickQuickLookup(doubleClick);
    setMarkerStyle(nextMarkerStyle);
    setLearningCardConfig(parseCardDesignConfig(settings.learning_card_config));
  }, []);

  const readingAssistanceSettingsRef = useRef<Record<string, string>>({});

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<{ id: string; title: string; author: string }>("book-metadata-changed", (event) => {
      if (event.payload.id !== bookId) return;
      setBook((current) => current ? {
        ...current,
        title: event.payload.title,
        author: event.payload.author,
      } : current);
      if (isStandaloneWindow) appWindow.setTitle(event.payload.title).catch(() => {});
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    }).catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [bookId]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const refresh = async () => {
      const settings = await getAllSettings().catch(() => null);
      if (!disposed && settings) {
        readingAssistanceSettingsRef.current = settings;
        applyReadingAssistanceSettings(settings);
      }
    };
    listenForReadingAssistanceSettingsChanged(refresh).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    }).catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [applyReadingAssistanceSettings]);

  useEffect(() => {
    const refreshOnFocus = async () => {
      const settings = await getAllSettings().catch(() => null);
      if (!settings || !readingAssistanceSettingsChanged(
        settings,
        readingAssistanceSettingsRef.current,
      )) return;
      readingAssistanceSettingsRef.current = settings;
      applyReadingAssistanceSettings(settings);
    };
    window.addEventListener("focus", refreshOnFocus);
    return () => window.removeEventListener("focus", refreshOnFocus);
  }, [applyReadingAssistanceSettings]);

  const settingsAnchorRef = useRef<HTMLButtonElement>(null);
  const viewerRef = useRef<HTMLDivElement>(null);
  const readerViewportRef = useRef<HTMLElement>(null);
  const viewRef = useRef<FoliateView | null>(null);
  const { handlePanelResizePointerDown, panelRef, panelWidth } = useSidePanelResize(viewRef);
  const zoomRef = useRef<number | "fit">(zoom);
  const fitPctRef = useRef(100);
  const textReaderNavigateRef = useRef<((location: string, flash?: boolean) => void) | null>(null);
  const textReaderPageNavigationRef = useRef<{ prev: () => void; next: () => void } | null>(null);
  const [textNavigationRegistration, setTextNavigationRegistration] = useState(0);
  const chaptersRef = useRef<TocChapter[]>([]);
  const pendingWordClickRef = useRef<number | null>(null);
  const pendingSelectionMenuRef = useRef<number | null>(null);
  const readerInteractionGenerationRef = useRef(0);
  const forceClickSuppressedUntilRef = useRef(0);
  const pdfTextLayerNoticeTimerRef = useRef<number | null>(null);
  const annotationClickDocumentRef = useRef<Document | null>(null);
  const contextMenuRequestRef = useRef(0);

  const handleTextBookReady = useCallback((document: TextBookDocument) => {
    const textChapters = document.toc.map((entry) => ({
      title: entry.title,
      href: textLocation(entry.source_offset),
      targetHref: textLocation(entry.source_offset),
      depth: entry.depth,
    }));
    chaptersRef.current = textChapters;
    setChapters(textChapters);
    setCurrentChapterIndex((current) => current < 0 ? 0 : current);
    setBookReady(true);
    setReaderError(null);
  }, []);

  const queueReadingProgress = useCallback((targetBookId: string, nextProgress: number, cfi: string) => {
    progressWriter.queue(targetBookId, nextProgress, cfi);
  }, [progressWriter]);

  useEffect(() => {
    const flush = () => { void progressWriter.flush(); };
    window.addEventListener("pagehide", flush);
    return () => {
      window.removeEventListener("pagehide", flush);
      flush();
    };
  }, [bookId, progressWriter]);

  const handleTextBookProgress = useCallback((
    nextProgress: number,
    textLocationValue: string,
    chapterIndex: number,
    details?: TextReaderProgressDetails,
  ) => {
    setProgress(nextProgress);
    setChapterProgress(details?.chapterProgress ?? nextProgress);
    setPageInfo(details?.page ?? null);
    currentCfiRef.current = textLocationValue;
    setCurrentChapterIndex(chapterIndex);
    if (bookId) queueReadingProgress(bookId, nextProgress, textLocationValue);
  }, [bookId, queueReadingProgress]);

  const cancelPendingWordClick = useCallback(() => {
    if (pendingWordClickRef.current !== null) {
      window.clearTimeout(pendingWordClickRef.current);
      pendingWordClickRef.current = null;
    }
  }, []);

  const cancelPendingSelectionMenu = useCallback(() => {
    if (pendingSelectionMenuRef.current !== null) {
      window.clearTimeout(pendingSelectionMenuRef.current);
      pendingSelectionMenuRef.current = null;
    }
  }, []);

  useEffect(() => {
    if (!readerToast) return;
    const timer = window.setTimeout(() => setReaderToast(null), 2500);
    return () => window.clearTimeout(timer);
  }, [readerToast]);

  const openLearningCard = useCallback((interaction: ReaderInteraction) => {
    const hasEnabledModule = learningCardConfig.cards[interaction.kind].modules
      .some((module) => module.enabled);
    if (!hasEnabledModule) {
      setReaderToast(t("learningCard.allModulesDisabled"));
      return;
    }
    setLearningInteraction(interaction);
  }, [learningCardConfig, t]);

  const getHighlightMutationPlan = useCallback(async (
    interaction: ReaderInteraction,
    highlights: Highlight[],
  ): Promise<HighlightMutationPlan | null> => (
    interaction.source === "text"
      ? planTextHighlightMutation(interaction.location, highlights, "yellow", interaction.text)
      : planCfiHighlightMutation(interaction.location, highlights, "yellow", interaction.text)
  ), []);

  const getHighlightRemovalPlan = useCallback(async (
    interaction: ReaderInteraction,
    highlights: Highlight[],
  ): Promise<HighlightMutationPlan | null> => (
    interaction.source === "text"
      ? planTextHighlightRemoval(interaction.location, highlights)
      : planCfiHighlightRemoval(interaction.location, highlights)
  ), []);

  const openLearningInteraction = useCallback((interaction: ReaderInteraction) => {
    cancelPendingWordClick();
    cancelPendingSelectionMenu();
    if (interaction.trigger !== "word-quick-lookup") {
      const requestToken = ++contextMenuRequestRef.current;
      setContextMenu(interaction);
      setContextMarkStateLoading(Boolean(bookId));
      setContextSelectionFullyMarked(false);
      setContextManualSelectionFullyMarked(false);
      setContextHasManualSelectionMark(false);
      setContextHasLookupOccurrenceMark(false);
      setContextHasBookWordMark(false);
      setContextBookWordMarkExcluded(false);
      if (bookId) {
        Promise.all([
          invoke<Highlight[]>("list_highlights", { bookId }),
          interaction.kind === "word"
            ? invoke<WordMarkRule[]>("list_word_marks", { bookId })
            : Promise.resolve([]),
          interaction.kind === "word"
            ? invoke<WordMarkException[]>("list_word_mark_exceptions", { bookId })
            : Promise.resolve([]),
          interaction.kind === "word"
            ? invoke<LookupOccurrenceMark[]>("list_lookup_occurrence_marks", { bookId })
            : Promise.resolve([]),
        ]).then(async ([highlights, wordMarks, exceptions, occurrences]) => {
          if (contextMenuRequestRef.current !== requestToken) return;
          const [plan, removalPlan] = await Promise.all([
            getHighlightMutationPlan(interaction, highlights),
            getHighlightRemovalPlan(interaction, highlights),
          ]);
          if (contextMenuRequestRef.current !== requestToken) return;
          const hasBookRule = wordMarks.some((rule) => (
            rule.enabled && rule.normalized_word === interaction.normalizedText
          ));
          const isExcluded = hasBookRule && exceptions.some((exception) => (
            exception.excluded
            && exception.normalized_word === interaction.normalizedText
            && exception.location === interaction.location
          ));
          const manualFullyMarked = Boolean(plan?.fullyHighlighted);
          const hasManualSelectionMark = Boolean(removalPlan?.removeIds.length);
          const hasLookupOccurrence = occurrences.some((mark) => (
            mark.enabled && mark.location === interaction.location
          ));
          setContextManualSelectionFullyMarked(manualFullyMarked);
          setContextHasManualSelectionMark(hasManualSelectionMark);
          setContextHasLookupOccurrenceMark(hasLookupOccurrence);
          setContextHasBookWordMark(hasBookRule);
          setContextBookWordMarkExcluded(isExcluded);
          setContextSelectionFullyMarked(
            manualFullyMarked || hasLookupOccurrence || (hasBookRule && !isExcluded),
          );
          setContextMarkStateLoading(false);
        }).catch(() => {
          if (contextMenuRequestRef.current === requestToken) setContextMarkStateLoading(false);
        });
      } else {
        setContextMarkStateLoading(false);
      }
      return;
    }
    contextMenuRequestRef.current += 1;
    setContextMenu(null);
    setContextMarkStateLoading(false);
    openLearningCard(interaction);
  }, [
    bookId,
    cancelPendingSelectionMenu,
    cancelPendingWordClick,
    getHighlightMutationPlan,
    getHighlightRemovalPlan,
    openLearningCard,
  ]);

  const handleLookupSuccess = useCallback((interaction: ReaderInteraction) => {
    if (!bookId
      || interaction.kind !== "word"
      || !interaction.location
      || !autoHighlightLookupsRef.current) return;

    if (markMatchingWordsRef.current && supportsWordMarkers) {
      invoke("ensure_word_mark_rule", { bookId, word: interaction.text, color: "lookup" })
        .then(() => window.dispatchEvent(new CustomEvent("word-mark-changed", { detail: { bookId } })))
        .catch(() => {});
      return;
    }

    if (!supportsManualAnnotations) return;
    invoke("ensure_lookup_occurrence_mark", {
      bookId,
      word: interaction.text,
      location: interaction.location,
    }).then(() => {
      window.dispatchEvent(new CustomEvent("lookup-mark-changed", { detail: { bookId } }));
    }).catch(() => {});
  }, [bookId, supportsManualAnnotations, supportsWordMarkers]);

  const handleTextBookError = useCallback((error: string) => {
    setReaderError(error);
    setBookReady(false);
  }, []);

  const registerTextBookNavigation = useCallback((navigateText: (location: string, flash?: boolean) => void) => {
    textReaderNavigateRef.current = navigateText;
    setTextNavigationRegistration((value) => value + 1);
  }, []);

  const registerTextBookPageNavigation = useCallback((navigation: { prev: () => void; next: () => void }) => {
    textReaderPageNavigationRef.current = navigation;
  }, []);

  const handleTextHighlightClick = useCallback((highlight: Highlight, rect: DOMRect, fallbackText?: string) => {
    const text = highlight.text_content?.trim() || fallbackText?.trim();
    if (!text) return;
    cancelPendingWordClick();
    pendingWordClickRef.current = window.setTimeout(() => {
      pendingWordClickRef.current = null;
      openLearningInteraction({
        trigger: "selection-menu",
        kind: classifySelection(text),
        text,
        normalizedText: normalizeInteractionText(text),
        context: text,
        location: highlight.cfi_range,
        anchorRect: serializableRect(rect),
        source: "text",
        format: "text",
      });
    }, 240);
  }, [cancelPendingWordClick, openLearningInteraction]);

  useEffect(() => {
    markerStyleRef.current = markerStyle;
    markMatchingWordsRef.current = markerStyle.markMatchingWords;
  }, [markerStyle]);

  useEffect(() => {
    zoomRef.current = zoom;
  }, [zoom]);

  const applyZoom = useCallback((value: number | "fit") => {
    const renderer = viewRef.current?.renderer;
    if (!renderer) return;
    renderer.setAttribute("zoom", value === "fit" ? "fit-width" : String(value / 100));
  }, []);

  const handleZoom = useCallback((delta: number) => {
    const base = zoomRef.current === "fit" ? fitPctRef.current : zoomRef.current;
    const next = Math.min(300, Math.max(50, Math.round((base + delta) / 10) * 10));
    applyZoom(next);
    setZoom(next);
  }, [applyZoom]);

  const turnReaderPage = useCallback((direction: "previous" | "next") => {
    setContextMenu(null);
    const performTurn = async () => {
      if (isTextBook) {
        textReaderPageNavigationRef.current?.[direction === "previous" ? "prev" : "next"]();
        return;
      }
      const view = viewRef.current;
      if (!view) return;
      await (direction === "previous" ? view.prev() : view.next());
    };
    const settings = readerSettingsRef.current;
    void runPageTurnTransition({
      animation: settings.readingMode === "paginated" ? settings.pageTurnAnimation : "none",
      direction,
      viewport: readerViewportRef.current,
      turn: performTurn,
    });
  }, [isTextBook, readerSettingsRef]);

  const {
    blockPageTurnKeyboard,
    handlePageTurnContextMenu,
    handlePageTurnKeyDown,
    handlePageTurnMouseDown,
    handlePageTurnWheel,
  } = usePageTurnInput({
    bookFormat: book?.format,
    settingsRef: readerSettingsRef,
    readerViewportRef,
    panelRef,
    overlayOpen: Boolean(settingsOpen || contextMenu || learningInteraction || translation),
    sidePanelOpen: Boolean(sidePanel),
    turnPage: turnReaderPage,
    onPdfZoom: handleZoom,
  });

  const tocChapters = useMemo(() => chapters.map((chapter, i) => ({
    title: chapter.title,
    page: i + 1,
    depth: chapter.depth,
    disabled: !chapter.targetHref,
  })), [chapters]);

  const chapterCounter = useMemo(() => {
    const readingUnits = chapters
      .map((chapter, index) => ({ chapter, index }))
      .filter(({ chapter, index }) => chapters[index + 1]?.depth <= chapter.depth || index === chapters.length - 1)
      .map(({ index }) => index);
    if (readingUnits.length === 0) return null;
    const current = readingUnits.findIndex((index) => index >= Math.max(0, currentChapterIndex));
    return {
      current: (current < 0 ? readingUnits.length - 1 : current) + 1,
      total: readingUnits.length,
    };
  }, [chapters, currentChapterIndex]);

  const {
    applyAnnotations,
    applyFoliateMarkerStyles,
    autoMarkersRef,
    flashNavigationTarget,
    refreshAnnotations,
    resetAnnotationState,
    wordMarkExceptionsRef,
    wordMarkWordsRef,
  } = useFoliateAnnotations({
    bookId,
    bookReady,
    isTextBook,
    supportsManualAnnotations,
    supportsWordMarkers,
    supportsCfiNavigation,
    supportsReflowSettings: capabilities.supportsReflowSettings,
    readerSettings,
    readerSettingsRef,
    viewRef,
    markerStyle,
    markerStyleRef,
    markMatchingWordsRef,
    setMarkerStyle,
    setReaderSettings,
    textReaderNavigateRef,
  });

  const navigateToSource = useCallback(async (source: CitedSource) => {
    if (isTextBook && source.charStart != null) {
      await flashNavigationTarget(textLocation(source.charStart, source.charEnd ?? source.charStart));
      return;
    }
    if (book?.format === "pdf" && viewRef.current) {
      await viewRef.current.goTo(source.sectionIndex);
      return;
    }
    const view = viewRef.current;
    if (!view) return;
    const probe = source.snippet
      ?.split("\n")[0]
      ?.trim()
      .slice(0, 80)
      .replace(/\s+\S*$/, "")
      .trim();
    if (probe && probe.length >= 8 && Number.isInteger(source.sectionIndex)) {
      try {
        let cfi: string | undefined;
        for await (const result of view.search({ query: probe, index: source.sectionIndex })) {
          if (result === "done") break;
          if (result.cfi) {
            cfi = result.cfi;
            break;
          }
        }
        view.clearSearch();
        if (cfi) {
          await flashNavigationTarget(cfi);
          return;
        }
      } catch {
        view.clearSearch();
      }
    }
    if (source.sectionHref) {
      await view.goTo(source.sectionHref);
    }
  }, [book?.format, flashNavigationTarget, isTextBook, viewRef]);

  // Load book metadata and default settings from DB
  useEffect(() => {
    if (!bookId) return;
    let cancelled = false;
    dbSettingsLoadedRef.current = null;
    setLoading(true);
    setReaderError(null);
    setBook(null);
    resetAnnotationState();
    currentCfiRef.current = null;
    chaptersRef.current = [];
    setChapters([]);
    setCurrentChapterIndex(-1);
    setProgress(0);
    setChapterProgress(0);
    setPageInfo(null);
    setBookReady(false);
    setTextInitialLocation(null);
    getBook(bookId)
      .then((b) => {
        if (cancelled) return;
        currentCfiRef.current = b.current_cfi;
        setTextInitialLocation(b.current_cfi);
        setBook(b);
        if (isStandaloneWindow && b) {
          appWindow.setTitle(b.title);
        }
      })
      .catch(() => {
        if (!cancelled) setBook(null);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });

    Promise.all([getAllSettings(), loadCustomFonts()]).then(([globalSettings]) => {
      if (cancelled) return;
      const saved = localStorage.getItem(`reader-settings-${bookId}`);
      const bookSettings = saved ? JSON.parse(saved) as Partial<ReaderSettingsState> : {};
      const g = globalSettings;
      readingAssistanceSettingsRef.current = g;
      applyReadingAssistanceSettings(g);
      setReaderSettings((prev) => {
        const next = mergeStoredReaderSettings(prev, bookSettings, g);
        readerSettingsRef.current = next;
        return next;
      });
      const savedZoom = localStorage.getItem(`reader-zoom-${bookId}`);
      if (savedZoom === "fit") {
        setZoom("fit");
      } else {
        const parsedZoom = savedZoom ? parseInt(savedZoom, 10) : NaN;
        if (Number.isFinite(parsedZoom) && parsedZoom >= 50 && parsedZoom <= 300) {
          setZoom(parsedZoom);
        }
      }
      dbSettingsLoadedRef.current = bookId;
    }).catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [
    applyReadingAssistanceSettings,
    bookId,
    dbSettingsLoadedRef,
    readerSettingsRef,
    resetAnnotationState,
    setReaderSettings,
  ]);

  // Persist per-book PDF zoom after load. Debounce to avoid thrashing during
  // rapid zoom-button clicks; only write once the user settles.
  useEffect(() => {
    if (dbSettingsLoadedRef.current !== bookId) return;
    if (book?.format !== "pdf") return;
    const handle = window.setTimeout(() => {
      localStorage.setItem(`reader-zoom-${bookId}`, zoom === "fit" ? "fit" : String(zoom));
    }, 500);
    return () => window.clearTimeout(handle);
  }, [zoom, bookId, book?.format, dbSettingsLoadedRef]);

  useWindowSizePersistence(bookId, isStandaloneWindow);
  const { availabilityState, retryAvailability } = useBookAvailability(book, setBook);
  const installDocumentInteractions = useReaderInteractions({
    supportsSelection,
    pendingSelectionMenuRef,
    pendingWordClickRef,
    readerInteractionGenerationRef,
    forceClickSuppressedUntilRef,
    annotationClickDocumentRef,
    doubleClickQuickLookupRef,
    pdfTextLayerNoticeTimerRef,
    cancelPendingSelectionMenu,
    cancelPendingWordClick,
    openLearningInteraction,
    setContextMenu,
    setPdfTextLayerNotice,
    handleZoom,
    handlePageTurnKeyDown,
    handlePageTurnMouseDown,
    handlePageTurnContextMenu,
    handlePageTurnWheel,
  });

  useFoliateView({
    book,
    bookId,
    bookReady,
    isTextBook,
    readerRetry,
    readerSettings,
    readerSettingsRef,
    capabilities,
    viewRef,
    viewerRef,
    currentCfiRef,
    chaptersRef,
    readerInteractionGenerationRef,
    pendingWordClickRef,
    pdfTextLayerNoticeTimerRef,
    annotationClickDocumentRef,
    contextMenuRequestRef,
    zoomRef,
    fitPctRef,
    markerStyleRef,
    wordMarkWordsRef,
    wordMarkExceptionsRef,
    autoMarkersRef,
    applyAnnotations,
    applyFoliateMarkerStyles,
    installDocumentInteractions,
    queueReadingProgress,
    cancelPendingWordClick,
    cancelPendingSelectionMenu,
    openLearningInteraction,
    setBookReady,
    setReaderError,
    setPdfTextLayerNotice,
    setCanGoBack,
    setChapters,
    setCurrentChapterIndex,
    setProgress,
    setChapterProgress,
    setPageInfo,
    setActiveVocabCfi,
    setSidePanel,
    setContextMenu,
  });

  useReaderNavigation({
    bookId,
    bookReady,
    isTextBook,
    supportsCfiNavigation,
    textNavigationRegistration,
    viewRef,
    textReaderNavigateRef,
    refreshAnnotations,
    setSidePanel,
    setInitialChatId,
  });

  useEffect(() => {
    const element = readerViewportRef.current;
    if (!element) return;
    let frame = 0;
    const update = () => {
      if (frame) cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        setReaderRect(serializableRect(element.getBoundingClientRect()));
      });
    };
    update();
    const observer = new ResizeObserver(update);
    observer.observe(element);
    window.addEventListener("resize", update);
    return () => {
      if (frame) cancelAnimationFrame(frame);
      observer.disconnect();
      window.removeEventListener("resize", update);
    };
  }, []);

  const togglePanel = (panel: "ai" | "bookmarks" | "vocab") => {
    setSidePanel((prev) => (prev === panel ? null : panel));
  };


  const navigateToChapter = useCallback((href: string) => {
    if (isTextBook) textReaderNavigateRef.current?.(href);
    else viewRef.current?.goTo(href);
  }, [isTextBook]);

  const navigateToCfi = useCallback((cfi: string) => {
    if (isTextBook) textReaderNavigateRef.current?.(cfi);
    else viewRef.current?.goTo(cfi);
  }, [isTextBook]);

  // Handle navigation state from ChatsPage ("Open in Reader")
  // Supports both location.state (main window) and URL search params (standalone window)
  useEffect(() => {
    const state = location.state as { openChat?: boolean; chatId?: string } | null;
    const searchParams = new URLSearchParams(window.location.search);
    const openChat = state?.openChat || searchParams.get("openChat") === "true";
    const chatId = state?.chatId || searchParams.get("chatId") || undefined;
    if (!openChat || !bookReady) return;
    setSidePanel("ai");
    if (chatId) setInitialChatId(chatId);
    if (!isStandaloneWindow) navigate(location.pathname, { replace: true });
  }, [bookReady, location.state, location.pathname, navigate]);

  // Handle source navigation and the optional vocabulary side panel.
  // Supports both location.state (main window) and URL search params (standalone window).
  useEffect(() => {
    const state = location.state as { openVocab?: boolean; cfi?: string; page?: number } | null;
    const searchParams = new URLSearchParams(window.location.search);
    const openVocab = state?.openVocab || searchParams.get("openVocab") === "true";
    const cfi = state?.cfi || searchParams.get("cfi") || undefined;
    const rawPage = state?.page ?? Number(searchParams.get("page"));
    const page = Number.isInteger(rawPage) && rawPage >= 0 ? rawPage : undefined;
    if (!bookReady || (!openVocab && !cfi && page == null)) return;
    if (openVocab) setSidePanel("vocab");
    if (cfi && supportsCfiNavigation) flashNavigationTarget(cfi).catch(() => {});
    if (page != null && book?.format === "pdf") viewRef.current?.goTo(page).catch(() => {});
    // Clear the state so it doesn't re-trigger
    if (!isStandaloneWindow) navigate(location.pathname, { replace: true });
  }, [book?.format, bookReady, flashNavigationTarget, location.state, location.pathname, navigate, supportsCfiNavigation, viewRef]);

  if (loading || (bookId !== undefined && book?.id !== bookId)) {
    return (
      <div className="flex flex-col items-center justify-center h-screen gap-3">
        <Loader2 size={24} className="animate-spin text-text-muted" />
        <p className="text-text-muted text-[14px]">{t("reader.loading")}</p>
      </div>
    );
  }

  if (!book) {
    return (
      <div className="flex items-center justify-center h-screen">
        <p>{t("reader.bookNotFound")}</p>
      </div>
    );
  }

  const returnToLibrary = () => {
    if (isStandaloneWindow) {
      appWindow.close().catch(() => navigate("/"));
    } else {
      navigate("/");
    }
  };

  if (readerError) {
    return (
      <div className="flex flex-col items-center justify-center h-screen gap-4 px-6 text-center">
        <p className="text-text-primary text-[16px] font-medium">{t("reader.initializationFailed")}</p>
        <p className="text-text-muted text-[13px] max-w-[520px] break-words">{readerError}</p>
        <div className="flex items-center gap-2">
          <Button
            variant="secondary"
            size="sm"
            onClick={() => {
              if (isTextBook && bookId) {
                invoke("retry_text_book_preparation", { bookId })
                  .then(() => getBook(bookId))
                  .then((updated) => {
                    setBook(updated);
                    setReaderError(null);
                    setReaderRetry((value) => value + 1);
                  })
                  .catch((error) => setReaderError(error instanceof Error ? error.message : String(error)));
                return;
              }
              setReaderError(null);
              setReaderRetry((value) => value + 1);
            }}
          >
            {t("reader.retry")}
          </Button>
          <Button variant="ghost" size="sm" onClick={returnToLibrary}>
            <ArrowLeft size={14} />
            {t("reader.returnToLibrary")}
          </Button>
        </div>
      </div>
    );
  }

  if (availabilityState) {
    const waitingForCloud = availabilityState === "checking" || availabilityState === "icloud_placeholder";
    const message = availabilityState === "missing"
      ? t("reader.fileUnavailable")
      : availabilityState === "error"
        ? t("reader.fileCheckFailed")
        : availabilityState === "timeout"
          ? t("reader.downloadTimeout")
          : waitingForCloud
            ? t("reader.downloadingFromICloud")
            : t("reader.fileCheckFailed");
    return (
      <div className="flex flex-col items-center justify-center h-screen gap-4 px-6 text-center">
        {waitingForCloud && <Loader2 size={24} className="animate-spin text-text-muted" />}
        <p className="text-text-muted text-[14px] max-w-[420px]">{message}</p>
        {!waitingForCloud && (
          <div className="flex items-center gap-2">
            <Button variant="secondary" size="sm" onClick={retryAvailability}>
              {t("reader.retry")}
            </Button>
            <Button variant="ghost" size="sm" onClick={returnToLibrary}>
              <ArrowLeft size={14} />
              {t("reader.returnToLibrary")}
            </Button>
          </div>
        )}
      </div>
    );
  }


  const toggleTocPanel = () => {
    setTocOpen((open) => !open);
    setSettingsOpen(false);
  };

  const handleTocNavigate = (page: number) => {
    const chapter = chapters[page - 1];
    if (chapter?.targetHref) navigateToChapter(chapter.targetHref);
  };

  return (
    <div className="flex flex-col h-screen bg-bg-page" style={getReaderThemeVars(readerSettings.theme) as React.CSSProperties}>
      {/* Invisible overlay to close popovers when clicking anywhere */}
      {settingsOpen && (
        <div
          className="fixed inset-0 z-40"
          onMouseDown={(e) => { e.preventDefault(); setSettingsOpen(false); }}
        />
      )}
      {/* Header */}
      <header
        className={`flex items-center justify-between px-section pt-8 pb-2 shrink-0 relative select-none ${isStandaloneWindow ? "" : "bg-bg-surface border-b border-border"}`}
        style={isStandaloneWindow ? {
          backgroundColor: getThemeStyles(readerSettings.theme).body,
          color: getThemeStyles(readerSettings.theme).text,
          borderBottom: `1px solid ${getThemeStyles(readerSettings.theme).text}1a`,
        } : undefined}
      >
        <div data-tauri-drag-region className="absolute top-0 left-0 right-0 h-8" />

        {/* Left section */}
        <div className="flex items-center gap-3">
          {isStandaloneWindow ? (
            <div className="size-10 rounded-lg bg-accent flex items-center justify-center">
              <BookOpen size={18} className="text-white" />
            </div>
          ) : (
            <>
              <Button variant="icon" size="md" onClick={() => navigate("/")}>
                <ArrowLeft size={16} />
              </Button>
              <div className="w-px h-6 bg-border" />
            </>
          )}

          {isStandaloneWindow ? (
            <>
              {/* TOC on left in standalone window */}
              <div className="w-px h-6 bg-current opacity-15" />
              <Button
                variant="icon"
                size="md"
                active={tocOpen}
                className={tocOpen ? "bg-accent-bg" : ""}
                aria-label={t(tocOpen ? "reader.tocClose" : "reader.tocOpen")}
                aria-expanded={tocOpen}
                title={t(tocOpen ? "reader.tocClose" : "reader.tocOpen")}
                onClick={toggleTocPanel}
              >
                <List size={16} />
              </Button>
            </>
          ) : (
            <>
              {/* Book icon + title on left in main window */}
              <div className="size-10 rounded-lg bg-accent flex items-center justify-center">
                <BookOpen size={18} className="text-white" />
              </div>
              <div className="flex flex-col">
                <h1 className="text-[16px] font-semibold text-text-primary leading-5">
                  {book.title}
                </h1>
                <span className="text-[13px] text-text-muted leading-4">
                  {book.format === "pdf"
                    ? pageInfo ? t("reader.pageOf", { current: pageInfo.current, total: pageInfo.total }) : ""
                    : chapterCounter ? t("reader.chapterOf", chapterCounter) : ""}
                </span>
              </div>
            </>
          )}
        </div>

        {/* Center — book title in standalone window */}
        {isStandaloneWindow && (
          <div className="absolute left-1/2 -translate-x-1/2 flex flex-col items-center pointer-events-none">
            <h1 className="text-[14px] font-semibold leading-5" style={{ color: "inherit" }}>
              {book.title}
            </h1>
            <span className="text-[12px] leading-4 opacity-60">
              {book.format === "pdf"
                ? pageInfo ? t("reader.pageOf", { current: pageInfo.current, total: pageInfo.total }) : ""
                : chapterCounter ? t("reader.chapterOf", chapterCounter) : ""}
            </span>
          </div>
        )}

        {/* Right section */}
        <div className="flex items-center">
          {/* TOC button in main window */}
          {!isStandaloneWindow && (
            <>
              <Button
                variant="icon"
                size="md"
                active={tocOpen}
                className={tocOpen ? "bg-accent-bg" : ""}
                aria-label={t(tocOpen ? "reader.tocClose" : "reader.tocOpen")}
                aria-expanded={tocOpen}
                title={t(tocOpen ? "reader.tocClose" : "reader.tocOpen")}
                onClick={toggleTocPanel}
              >
                <List size={16} />
              </Button>
            </>
          )}

          <button
            ref={settingsAnchorRef}
            onClick={() => {
              setSettingsOpen((open) => !open);
              setTocOpen(false);
            }}
            className={`flex items-center justify-center gap-1 size-9 rounded-lg cursor-pointer transition-colors ${
              settingsOpen ? "text-accent-text" : isStandaloneWindow ? "opacity-60 hover:opacity-100" : "text-text-muted hover:bg-bg-input"
            }`}
          >
            <span className="text-[16px] font-semibold leading-6">A</span>
            <span className="text-[12px] font-semibold leading-4">A</span>
          </button>
          <ReaderSettings
            open={settingsOpen}
            onClose={() => setSettingsOpen(false)}
            anchorRef={settingsAnchorRef}
            settings={readerSettings}
            onSettingsChange={handleReaderSettingsChange}
            capabilities={capabilities}
            onClearLookupMarks={bookId ? async () => {
              await invoke("clear_lookup_marks_for_book", { bookId });
              window.dispatchEvent(new CustomEvent("word-mark-changed", { detail: { bookId } }));
              window.dispatchEvent(new CustomEvent("lookup-mark-changed", { detail: { bookId } }));
              await refreshAnnotations();
            } : undefined}
          />

          {supportsCfiNavigation && <>
            <Button
              variant="icon"
              size="md"
              active={sidePanel === "bookmarks"}
              onClick={() => togglePanel("bookmarks")}
            >
              <Bookmark size={16} />
            </Button>

            <Button
              variant="icon"
              size="md"
              active={sidePanel === "vocab"}
              onClick={() => togglePanel("vocab")}
            >
              <Languages size={16} />
            </Button>
          </>}

          <div className="w-px h-6 bg-border mx-1" />

          <Button
            variant="icon"
            size="md"
            active={sidePanel === "ai"}
            aria-label={t("reader.aiAssistant")}
            title={t("reader.aiAssistant")}
            onClick={() => togglePanel("ai")}
          >
            <Bot size={16} />
          </Button>
        </div>
      </header>

      {/* Body */}
      <div
        className="flex flex-1 overflow-hidden"
        style={{ backgroundColor: getThemeStyles(readerSettings.theme).body }}
      >
        <TableOfContents
          open={tocOpen}
          chapters={tocChapters}
          currentPage={currentChapterIndex + 1}
          onNavigate={handleTocNavigate}
        />
        <div className="flex-1 flex flex-col min-w-0" style={{ backgroundColor: getThemeStyles(readerSettings.theme).body }}>
          <main
            ref={readerViewportRef}
            className="reader-page-viewport flex-1 relative overflow-hidden"
            style={{ backgroundColor: getThemeStyles(readerSettings.theme).body }}
            onClick={() => {
              setSettingsOpen(false);
              // Clicks inside the iframe (text content) don't bubble out
              // through the sandbox boundary, so this fires only for clicks
              // on the margins/white space around the page — i.e. "anywhere
              // else" from the reader's perspective. Drop the in-iframe text
              // selection so the highlight doesn't linger.
              viewRef.current?.deselect?.();
            }}
          >
            {isTextBook ? (
              <TextBookReader
                key={`${book.id}:${readerRetry}`}
                bookId={book.id}
                initialLocation={textInitialLocation}
                settings={readerSettings}
                onReady={handleTextBookReady}
                onProgress={handleTextBookProgress}
                onInteraction={openLearningInteraction}
                onError={handleTextBookError}
                onRegisterNavigation={registerTextBookNavigation}
                onRegisterPageNavigation={registerTextBookPageNavigation}
                onHighlightClick={handleTextHighlightClick}
                doubleClickQuickLookup={doubleClickQuickLookup}
                markerStyle={markerStyle}
              />
            ) : (
              <div
                ref={viewerRef}
                className="w-full h-full"
                style={book.format === "pdf" ? { backgroundColor: "#ffffff" } : undefined}
              />
            )}
            {book.format === "pdf" && (() => {
              const overlay = getPdfOverlays(readerSettings.theme);
              if (!overlay) return null;
              return overlay.layers.map((style, i) => (
                <div
                  key={i}
                  className="z-10 pointer-events-none absolute inset-0"
                  style={style}
                />
              ));
            })()}
            {book.format === "pdf" && pdfTextLayerNotice && (
              <div
                role="status"
                className="pointer-events-none absolute bottom-5 left-1/2 z-30 max-w-[min(420px,calc(100%_-_24px))] -translate-x-1/2 rounded-md border border-border bg-bg-surface px-3 py-2 text-center text-[12px] leading-5 text-text-secondary shadow-popover"
              >
                {t("reader.pdfNoTextLayer")}
              </div>
            )}
            {!bookReady && (
              <div className="absolute inset-0 z-20 bg-bg-surface flex items-center justify-center">
                <div className="flex flex-col items-center gap-3">
                  <Loader2 size={24} className="animate-spin text-text-muted" />
                  <span className="text-[14px] text-text-muted">{t("reader.preparingBook")}</span>
                </div>
              </div>
            )}
            {canGoBack && (
              <button
                onClick={() => viewRef.current?.history.back()}
                className="absolute bottom-4 left-6 z-20 flex items-center gap-1.5 px-3.5 py-2 rounded-full bg-accent-bg text-accent-text shadow-sm cursor-pointer transition-opacity hover:opacity-80"
              >
                <ArrowLeft size={14} />
                <span className="text-[13px] font-medium">{t("reader.back")}</span>
              </button>
            )}
          </main>

          {/* Bottom progress bar */}
          <footer
            className={`px-page pb-2 pt-0 shrink-0 ${isStandaloneWindow ? "" : "bg-bg-surface"}`}
            style={isStandaloneWindow ? {
              backgroundColor: getThemeStyles(readerSettings.theme).body,
              color: getThemeStyles(readerSettings.theme).text,
            } : undefined}
          >
            <div className="flex flex-col gap-2">
              <div className={`h-px w-full ${isStandaloneWindow ? "opacity-10" : "bg-border"}`} style={isStandaloneWindow ? { backgroundColor: "currentColor" } : undefined}>
                {(book.format === "pdf" || readerSettings.showChapterProgress) && (
                  <div
                    className="h-full transition-all"
                    style={{ width: `${book.format === "pdf" ? progress : chapterProgress}%`, backgroundColor: isStandaloneWindow ? "currentColor" : "#9f9fa9", opacity: isStandaloneWindow ? 0.4 : undefined }}
                  />
                )}
              </div>
              <div className="flex items-center justify-between h-8">
                <div className={`flex min-w-0 items-center gap-2 text-[12px] tabular-nums ${isStandaloneWindow ? "opacity-60" : "text-text-muted"}`}>
                  {book.format === "pdf" && pageInfo ? (
                    <span>{t("reader.pageOf", { current: pageInfo.current, total: pageInfo.total })}</span>
                  ) : readerSettings.showChapterProgress ? (
                    <span>{t("reader.chapterProgress", { progress: chapterProgress })}</span>
                  ) : null}
                  {readerSettings.showBookProgress && book.format !== "pdf" && (
                    <span className="border-l border-current/20 pl-2">
                      {t("reader.bookProgress", { progress })}
                    </span>
                  )}
                  {readerSettings.readingMode === "paginated" && readerSettings.showPageNumbers && pageInfo && book.format !== "pdf" && (
                    <span className="border-l border-current/20 pl-2">
                      {pageInfo.visibleEnd && pageInfo.visibleEnd > pageInfo.current
                        ? t("reader.pageRangeOf", { current: pageInfo.current, end: pageInfo.visibleEnd, total: pageInfo.total })
                        : t("reader.pageOf", { current: pageInfo.current, total: pageInfo.total })}
                    </span>
                  )}
                </div>
                {book.format === "pdf" && (
                  <div className="flex items-center gap-1">
                    <Button variant="icon" size="sm" onClick={() => handleZoom(-10)}>
                      <Minus size={12} />
                    </Button>
                    <button
                      type="button"
                      onClick={() => { applyZoom("fit"); setZoom("fit"); }}
                      title={t("reader.zoom.fitTooltip")}
                      className={`text-[12px] font-medium min-w-[36px] px-1 text-center tabular-nums hover:opacity-100 ${isStandaloneWindow ? "opacity-60" : "text-text-muted"} ${zoom === "fit" ? "" : "cursor-pointer"}`}
                    >
                      {zoom === "fit" ? t("reader.zoom.fit") : `${zoom}%`}
                    </button>
                    <Button variant="icon" size="sm" onClick={() => handleZoom(10)}>
                      <Plus size={12} />
                    </Button>
                  </div>
                )}
                <span className="w-8" aria-hidden="true" />
              </div>
            </div>
          </footer>
        </div>

        {sidePanel && (
          <div
            onPointerDown={handlePanelResizePointerDown}
            className="w-1 h-full shrink-0 cursor-col-resize touch-none hover:bg-accent/30 transition-colors z-10"
          />
        )}
        <div
          ref={panelRef}
          className={sidePanel ? "shrink-0 h-full" : "hidden"}
          style={{ width: panelWidth }}
          onPointerDownCapture={blockPageTurnKeyboard}
        >
          <div className={sidePanel === "ai" ? "h-full" : "hidden"}>
            <AiPanel
              bookId={bookId}
              bookTitle={book.title}
              bookAuthor={book.author}
              currentChapter={currentChapterIndex >= 0 && currentChapterIndex < chapters.length ? chapters[currentChapterIndex].title : undefined}
              context={aiContext}
              initialChatId={initialChatId}
              onContextConsumed={() => setAiContext(undefined)}
              onNavigateToCfi={(cfi) => {
                flashNavigationTarget(cfi).catch(() => {});
              }}
              onNavigateToSource={(source) => {
                navigateToSource(source).catch(() => {});
              }}
            />
          </div>
          {supportsCfiNavigation && sidePanel === "bookmarks" && bookId && (
            <BookmarksPanel
              bookId={bookId}
              onNavigate={navigateToCfi}
              getCurrentCfi={() => currentCfiRef.current}
              getCurrentLabel={() => {
                const idx = currentChapterIndex;
                return idx >= 0 && idx < chapters.length
                  ? chapters[idx].title
                  : t("common.bookmark");
              }}
              getPageFromCfi={() => {
                // foliate-js uses fraction-based progress, not location indices
                // Return page info from current state if available
                return pageInfo?.current ?? null;
              }}
            />
          )}
          {supportsCfiNavigation && sidePanel === "vocab" && bookId && (
            <DictionaryPanel
              bookId={bookId}
              bookTitle={book.title}
              onNavigate={(cfi) => {
                flashNavigationTarget(cfi).catch(() => {});
              }}
              getPageFromCfi={() => pageInfo?.current ?? null}
              initialWordCfi={activeVocabCfi}
              onWordDetailClosed={() => setActiveVocabCfi(null)}
            />
          )}
        </div>
      </div>

      {/* Context Menu */}
      {contextMenu && (
        <ReaderContextMenu
          anchorRect={contextMenu.anchorRect}
          text={contextMenu.text}
          kind={contextMenu.kind}
          marked={contextSelectionFullyMarked}
          hasBookWordMark={contextHasBookWordMark}
          markStateLoading={contextMarkStateLoading}
          order={learningCardConfig.selectionMenus[contextMenu.kind]
            .filter((item) => item.enabled)
            .map((item) => readerMenuActionMap[item.id])}
          onClose={() => {
            contextMenuRequestRef.current += 1;
            setContextMenu(null);
          }}
          onCopy={() => {
            navigator.clipboard.writeText(contextMenu.text);
            setContextMenu(null);
          }}
          onExplain={() => {
            openLearningCard({ ...contextMenu, trigger: "word-quick-lookup" });
            setContextMenu(null);
          }}
          onQuote={() => {
            setAiContext({
              text: contextMenu.text,
              cfi: contextMenu.location,
            });
            setSidePanel("ai");
            setContextMenu(null);
          }}
          onLookup={() => {
            openLearningCard({ ...contextMenu, trigger: "word-quick-lookup" });
            setContextMenu(null);
          }}
          onTranslate={() => {
            setTranslation({
              x: contextMenu.anchorRect.right,
              y: contextMenu.anchorRect.top,
              text: contextMenu.text,
              context: contextMenu.context,
              cfi: contextMenu.location,
            });
            setContextMenu(null);
          }}
          onSave={() => {
            if (!bookId) return;
            invoke("add_vocab_word", {
              bookId,
              word: contextMenu.text,
              definition: "",
              contextSentence: contextMenu.context || null,
              contextExplanation: null,
              cfi: contextMenu.location || null,
            }).then(() => {
              window.dispatchEvent(new CustomEvent("vocab-changed", { detail: { bookId, cfi: contextMenu.location } }));
            }).catch((error) => console.error("Failed to save selection:", error));
            setContextMenu(null);
          }}
          onToggleMark={supportsManualAnnotations ? (() => {
            const interaction = contextMenu;
            const manualFullyMarked = contextManualSelectionFullyMarked;
            const hasManualSelectionMark = contextHasManualSelectionMark;
            const hasLookupOccurrence = contextHasLookupOccurrenceMark;
            const hasBookRule = contextHasBookWordMark;
            const bookRuleExcluded = contextBookWordMarkExcluded;
            contextMenuRequestRef.current += 1;
            setContextMenu(null);
            if (!interaction.location || !bookId) return;
            const replaceManualMarks = async (plan: HighlightMutationPlan | null) => {
              if (!plan || (plan.removeIds.length === 0 && plan.additions.length === 0)) return;
              await invoke<Highlight[]>("replace_highlights", {
                bookId,
                removeIds: plan.removeIds,
                additions: plan.additions,
              });
              window.dispatchEvent(new CustomEvent("highlight-changed", { detail: { bookId } }));
            };
            (async () => {
              const highlights = await invoke<Highlight[]>("list_highlights", { bookId });
              if (interaction.kind === "word" && contextSelectionFullyMarked) {
                if (hasLookupOccurrence) {
                  await invoke("set_lookup_occurrence_mark_enabled", {
                    bookId,
                    word: interaction.text,
                    location: interaction.location,
                    enabled: false,
                  });
                  window.dispatchEvent(new CustomEvent("lookup-mark-changed", { detail: { bookId } }));
                }
                if (hasManualSelectionMark) {
                  await replaceManualMarks(await getHighlightRemovalPlan(interaction, highlights));
                }
                if (hasBookRule && !bookRuleExcluded) {
                  await invoke("set_word_mark_exception", {
                    bookId,
                    word: interaction.text,
                    location: interaction.location,
                    excluded: true,
                  });
                  window.dispatchEvent(new CustomEvent("word-mark-changed", { detail: { bookId } }));
                }
              } else if (interaction.kind === "word" && hasBookRule) {
                if (bookRuleExcluded) {
                  await invoke("set_word_mark_exception", {
                    bookId,
                    word: interaction.text,
                    location: interaction.location,
                    excluded: false,
                  });
                  window.dispatchEvent(new CustomEvent("word-mark-changed", { detail: { bookId } }));
                } else {
                  // The fully-marked branch above owns removal. This path is
                  // retained for defensive state refreshes only.
                  return;
                }
              } else if (interaction.kind === "word"
                && !manualFullyMarked
                && supportsWordMarkers
                && markMatchingWordsRef.current) {
                await invoke("set_word_mark_rule_enabled", {
                  bookId,
                  word: interaction.text,
                  enabled: true,
                  color: "lookup",
                });
                window.dispatchEvent(new CustomEvent("word-mark-changed", { detail: { bookId } }));
              } else if (interaction.kind === "word") {
                await replaceManualMarks(await getHighlightMutationPlan(interaction, highlights));
              } else {
                const plan = manualFullyMarked
                  ? await getHighlightRemovalPlan(interaction, highlights)
                  : await getHighlightMutationPlan(interaction, highlights);
                await replaceManualMarks(plan);
              }
              await refreshAnnotations();
            })().catch((err) => console.error("Failed to toggle mark:", err));
          }) : undefined}
          onRemoveBookWordMark={contextMenu.kind === "word" && contextHasBookWordMark ? (() => {
            const interaction = contextMenu;
            contextMenuRequestRef.current += 1;
            setContextMenu(null);
            if (!bookId) return;
            invoke("remove_word_mark", { bookId, word: interaction.text })
              .then(async () => {
                window.dispatchEvent(new CustomEvent("word-mark-changed", { detail: { bookId } }));
                window.dispatchEvent(new CustomEvent("lookup-mark-changed", { detail: { bookId } }));
                await refreshAnnotations();
              })
              .catch((error) => console.error("Failed to remove book word mark:", error));
          }) : undefined}
        />
      )}

      {learningInteraction && bookId && (
        <LearningCardController
          key={`${learningInteraction.kind}:${learningInteraction.location}:${learningInteraction.text}`}
          interaction={learningInteraction}
          bookId={bookId}
          bookTitle={book?.title}
          bookAuthor={book?.author}
          chapter={currentChapterIndex >= 0 && currentChapterIndex < chapters.length
            ? chapters[currentChapterIndex].title
            : undefined}
          config={learningCardConfig}
          readerRect={readerRect}
          onClose={() => setLearningInteraction(null)}
          onAskAi={(quote, cfi, analysis) => {
            setAiContext({ text: quote, cfi, analysis });
            setSidePanel("ai");
          }}
          onViewAllNotes={() => {
            invoke("open_library_on_main", { filter: "notes" }).catch(() => {});
          }}
          onLookupSuccess={handleLookupSuccess}
        />
      )}

      {translation && (
        <TranslationPopover
          x={translation.x}
          y={translation.y}
          text={translation.text}
          context={translation.context}
          bookId={bookId!}
          bookTitle={book?.title}
          bookAuthor={book?.author}
          chapter={currentChapterIndex >= 0 && currentChapterIndex < chapters.length
            ? chapters[currentChapterIndex].title
            : undefined}
          cfi={translation.cfi}
          onClose={() => setTranslation(null)}
        />
      )}

      {readerToast && <Toast>{readerToast}</Toast>}

    </div>
  );
}
