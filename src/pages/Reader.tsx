import { useState, useRef, useCallback, useEffect, useMemo } from "react";
import { useParams, useNavigate, useLocation } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { emit } from "@tauri-apps/api/event";
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
import AiPanel from "../components/AiPanel";
import BookmarksPanel from "../components/BookmarksPanel";
import ReaderSettings, { type ReaderSettingsState } from "../components/ReaderSettings";
import { getFontFamily, getThemeStyles, getDefaultReaderTheme, getReaderCapabilities } from "../components/reader-settings";
import ReaderContextMenu, { type ReaderMenuAction } from "../components/ReaderContextMenu";
import HighlightToolbar from "../components/HighlightToolbar";
import LookupPopover from "../components/LookupPopover";
import ExplainPopover from "../components/ExplainPopover";
import DictionaryPanel from "../components/DictionaryPanel";
import TranslationPopover from "../components/TranslationPopover";
import TableOfContents from "../components/TableOfContents";
import TextBookReader from "../components/TextBookReader";
import { textLocation, type TextBookDocument } from "../components/text-book-location";
import {
  classifySelection,
  applyWordMarkHighlights,
  contextForRange,
  isInteractiveReaderTarget,
  normalizeInteractionText,
  serializableRect,
  selectedRange,
  viewportRectForRange,
  wordRangeAtPoint,
  type ReaderInteraction,
  type SerializableRect,
} from "../components/reader-interaction";
import {
  planCfiHighlightMutation,
  planTextHighlightMutation,
  type HighlightMutationPlan,
} from "../components/highlight-ranges";
import { getBook, updateReadingProgress, checkBookAvailable, type Book, type BookAvailabilityStatus } from "../hooks/useBooks";
import { getAllSettings } from "../hooks/useSettings";
import type { Highlight } from "../hooks/useBookmarks";
import {
  DEFAULT_CARD_DESIGN_CONFIG,
  LearningCardController,
  parseCardDesignConfig,
  type CardDesignConfigV1,
} from "../components/learning-card";

// foliate-js <foliate-view> web component interface
/* eslint-disable @typescript-eslint/no-explicit-any -- foliate-js has no TS definitions */
interface FoliateView extends HTMLElement {
  open(file: string | File | Blob): Promise<void>;
  init(opts: { lastLocation?: string; showTextStart?: boolean }): Promise<void>;
  goTo(target: string | number): Promise<any>;
  prev(): Promise<void>;
  next(): Promise<void>;
  close(): void;
  book: any;
  renderer: any;
  lastLocation: any;
  history: { back(): void; forward(): void; canGoBack: boolean; canGoForward: boolean; addEventListener: EventTarget["addEventListener"]; removeEventListener: EventTarget["removeEventListener"] };
  getCFI(index: number, range: Range): string;
  addAnnotation(annotation: { value: string; color?: string }): Promise<any>;
  deleteAnnotation(annotation: { value: string }): Promise<void>;
  deselect(): void;
}

interface LookupRecord {
  lookup_text: string;
  context_sentence: string | null;
  chapter: string | null;
  cfi: string | null;
}

interface VocabMarker {
  cfi: string | null;
  mastery: string;
}

interface WordMarkRule {
  normalized_word: string;
  enabled: boolean;
}

type MarkerKind = "lookup" | "vocab";
type Marker = { color: string; kind: MarkerKind };
type ReaderNavigation = {
  navigationId?: string;
  cfi?: string;
  openVocab?: boolean;
  openChat?: boolean;
  chatId?: string;
};
/* eslint-enable @typescript-eslint/no-explicit-any */

type PdfOverlay = { layers: React.CSSProperties[] } | null;

function getPdfOverlays(theme: string): PdfOverlay {
  switch (theme) {
    case "paper": return { layers: [{
      backgroundColor: getThemeStyles("paper").body,
      mixBlendMode: "multiply",
    }] };
    case "quiet": return { layers: [
      { backgroundColor: "#ffffff", mixBlendMode: "difference" },
      { backgroundColor: getThemeStyles("quiet").body, mixBlendMode: "screen" },
    ] };
    case "dark": return { layers: [
      { backgroundColor: "#ffffff", mixBlendMode: "difference" },
      { backgroundColor: getThemeStyles("dark").body, mixBlendMode: "screen" },
    ] };
    default: return null;
  }
}

function getReaderThemeVars(theme: string): Record<string, string> | undefined {
  switch (theme) {
    case "original": return {
      // Mirrors the :root light palette in src/index.css so the reader view
      // stays cohesive when the reader theme is Original but the system /
      // app theme is dark.
      "--color-bg-page": "#f4f4f5",
      "--color-bg-surface": "#ffffff",
      "--color-bg-muted": "#fafafa",
      "--color-bg-input": "#f3f3f5",
      "--color-text-primary": "#18181b",
      "--color-text-body": "#0a0a0a",
      "--color-text-secondary": "#52525c",
      "--color-text-muted": "#71717b",
      "--color-text-placeholder": "#a1a1aa",
      "--color-border": "#e4e4e7",
      "--color-border-light": "#f4f4f5",
      "--color-accent-bg": "#f3e8ff",
    };
    case "paper": return {
      "--color-bg-page": "#F4F0E7",
      "--color-bg-surface": "#FAF7F0",
      "--color-bg-muted": "#F7F3EB",
      "--color-bg-input": "#EFE9DD",
      "--color-text-primary": "#29251E",
      "--color-text-body": "#29251E",
      "--color-text-secondary": "#5F584D",
      "--color-text-muted": "#827969",
      "--color-text-placeholder": "#9A907F",
      "--color-border": "#DDD5C8",
      "--color-border-light": "#EEE8DD",
      "--color-accent": "#A36A31",
      "--color-accent-text": "#8A5728",
      "--color-accent-bg": "#F0E3D1",
    };
    case "quiet": return {
      "--color-bg-page": "#5A5A63",
      "--color-bg-surface": "#71717b",
      "--color-bg-muted": "#68686F",
      "--color-bg-input": "#5A5A63",
      "--color-text-primary": "#fafafa",
      "--color-text-body": "#fafafa",
      "--color-text-secondary": "#d4d4d8",
      "--color-text-muted": "#d4d4d8",
      "--color-text-placeholder": "#a1a1aa",
      "--color-border": "#9999a1",
      "--color-border-light": "#5A5A63",
      "--color-accent-bg": "#5A4D6E",
    };
    case "dark": return {
      "--color-bg-page": "#151518",
      "--color-bg-surface": "#18191d",
      "--color-bg-muted": "#1f2023",
      "--color-bg-input": "#25262c",
      "--color-text-primary": "#f4f4f5",
      "--color-text-body": "#e7e7ea",
      "--color-text-secondary": "#c9c9d1",
      "--color-text-muted": "#9a9aa4",
      "--color-text-placeholder": "#85858f",
      "--color-border": "#34343d",
      "--color-border-light": "#2a2b31",
      "--color-accent-bg": "#302647",
    };
    default: return undefined;
  }
}

const getReaderCSS = (settings: ReaderSettingsState) => {
  const themeColors = getThemeStyles(settings.theme);
  const fontFamily = getFontFamily(settings.font);
  const letterSpacing = settings.charSpacing === 0 ? "normal" : `${settings.charSpacing * 0.01}em`;
  const wordSpacing = settings.wordSpacing === 0 ? "normal" : `${settings.wordSpacing * 0.01}em`;
  return `
    body {
      background-color: ${themeColors.body} !important;
      color: ${themeColors.text} !important;
      font-family: ${fontFamily} !important;
      font-size: ${settings.fontSize}px !important;
      line-height: ${settings.lineSpacing} !important;
      letter-spacing: ${letterSpacing} !important;
      word-spacing: ${wordSpacing} !important;
    }
    p, span, div, li, td, th, h1, h2, h3, h4, h5, h6 {
      color: ${themeColors.text} !important;
      font-family: ${fontFamily} !important;
      line-height: ${settings.lineSpacing} !important;
    }
    ::-webkit-scrollbar { width: 8px; height: 8px; }
    ::-webkit-scrollbar-track { background: transparent; }
    ::-webkit-scrollbar-thumb { background: ${themeColors.text}33; border-radius: 9999px; }
    ::-webkit-scrollbar-thumb:hover { background: ${themeColors.text}55; }
    img, svg, video {
      max-width: 100% !important;
      height: auto !important;
      object-fit: contain !important;
      box-sizing: border-box !important;
    }
    figure {
      max-width: 100% !important;
      overflow: hidden !important;
    }
  `;
};

const PANEL_MIN_WIDTH = 320;
const PANEL_MAX_WIDTH = 700;
const PANEL_DEFAULT_WIDTH = 525;

type SidePanel = "ai" | "bookmarks" | "vocab" | null;

interface TocChapter {
  title: string;
  href?: string;
  targetHref?: string;
  depth: number;
}

const highlightColorMap: Record<string, string> = {
  yellow: "#FBBF24",
  green: "#34D399",
  blue: "#60A5FA",
  pink: "#F472B6",
  purple: "#A78BFA",
};

const wordMarkerColor = {
  lookup: "__lookup__",
  vocabNew: "__vocab_new__",
  learning: "__learning__",
  mastered: "__mastered__",
};

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

// Synthesize a foliate "fake" CFI for a PDF section index so legacy books
// without a saved CFI (only `progress`) still restore to the right page.
// Mirrors `CFI.fake.fromIndex` in epubcfi.js. Returning a CFI string (not a
// number) ensures `view.init` treats page 0 as present — it checks
// `lastLocation ? ...` and numeric 0 would be seen as absent.
function getPdfStartCfi(progress: number, pageCount: number | null | undefined): string | undefined {
  if (!Number.isFinite(progress) || progress <= 0 || !pageCount || pageCount <= 0) return undefined;
  const idx = Math.min(pageCount - 1, Math.max(0, Math.ceil((progress / 100) * pageCount) - 1));
  return `epubcfi(/6/${(idx + 1) * 2})`;
}

function withTimeout<T>(promise: Promise<T>, timeoutMs: number, code: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = window.setTimeout(() => reject(new Error(code)), timeoutMs);
    promise.then(
      (value) => {
        window.clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        window.clearTimeout(timer);
        reject(error);
      },
    );
  });
}

type ReaderAvailability = BookAvailabilityStatus | "checking" | "timeout" | "error";

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
  const [panelWidth, setPanelWidth] = useState(PANEL_DEFAULT_WIDTH);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [zoom, setZoom] = useState<number | "fit">("fit");
  const [tocOpen, setTocOpen] = useState(false);
  const [chapters, setChapters] = useState<TocChapter[]>([]);
  const [currentChapterIndex, setCurrentChapterIndex] = useState(-1);
  const [progress, setProgress] = useState(0);
  const [pageInfo, setPageInfo] = useState<{ current: number; total: number } | null>(null);
  const currentCfiRef = useRef<string | null>(null);
  const [bookReady, setBookReady] = useState(false);
  const [canGoBack, setCanGoBack] = useState(false);
  const backButtonTimerRef = useRef<number | null>(null);
  const [availabilityState, setAvailabilityState] = useState<ReaderAvailability | null>(null);
  const [availabilityRetry, setAvailabilityRetry] = useState(0);
  const [readerError, setReaderError] = useState<string | null>(null);
  const [readerRetry, setReaderRetry] = useState(0);
  const [textInitialLocation, setTextInitialLocation] = useState<string | null>(null);
  const [contextMenu, setContextMenu] = useState<ReaderInteraction | null>(null);
  const [contextSelectionFullyHighlighted, setContextSelectionFullyHighlighted] = useState(false);
  const [learningCardConfig, setLearningCardConfig] = useState<CardDesignConfigV1>(DEFAULT_CARD_DESIGN_CONFIG);
  const [learningInteraction, setLearningInteraction] = useState<ReaderInteraction | null>(null);
  const [readerRect, setReaderRect] = useState<SerializableRect | null>(null);
  const [aiContext, setAiContext] = useState<{ text: string; cfi?: string } | undefined>();
  const [initialChatId, setInitialChatId] = useState<string | undefined>();
  const [activeVocabCfi, setActiveVocabCfi] = useState<string | null>(null);
  const [lookup, setLookup] = useState<{
    x: number;
    y: number;
    word: string;
    sentence: string;
    bookTitle?: string;
    chapter?: string;
    cfi?: string;
  } | null>(null);
  const [explain, setExplain] = useState<{
    x: number;
    y: number;
    text: string;
    sentence: string;
    bookTitle?: string;
    chapter?: string;
    cfi?: string;
  } | null>(null);
  const [translation, setTranslation] = useState<{
    x: number;
    y: number;
    text: string;
    context?: string;
    cfi?: string;
  } | null>(null);
  const [highlightToolbar, setHighlightToolbar] = useState<{
    x: number;
    y: number;
    highlightId: string;
    cfiRange: string;
    color: string;
  } | null>(null);
  const [readerSettings, setReaderSettings] = useState<ReaderSettingsState>(() => ({
    theme: getDefaultReaderTheme(),
    font: "palatino",
    fontSize: 26,
    brightness: 100,
    readingMode: "scrolling",
    pageColumns: 2,
    lineSpacing: 1.8,
    charSpacing: 0,
    wordSpacing: 0,
    margins: 0,
    showLookupMarkers: true,
    showNewVocabMarkers: true,
    showLearningMarkers: true,
    showMasteredMarkers: false,
  }));
  const readerSettingsRef = useRef(readerSettings);
  const autoHighlightLookupsRef = useRef(true);

  const settingsAnchorRef = useRef<HTMLButtonElement>(null);
  const viewerRef = useRef<HTMLDivElement>(null);
  const readerViewportRef = useRef<HTMLElement>(null);
  const viewRef = useRef<FoliateView | null>(null);
  const zoomRef = useRef<number | "fit">(zoom);
  const fitPctRef = useRef(100);
  const autoMarkersRef = useRef(new Map<string, Marker>());
  const appliedAnnotationsRef = useRef(new Map<string, string>());
  const navigationFlashRef = useRef(new Map<string, number>());
  const pendingNavigationRef = useRef<ReaderNavigation | null>(null);
  const textReaderNavigateRef = useRef<((location: string, flash?: boolean) => void) | null>(null);
  const [textNavigationRegistration, setTextNavigationRegistration] = useState(0);
  const markerSnapshotRef = useRef<{
    highlights: Highlight[];
    lookups: LookupRecord[];
    vocab: VocabMarker[];
  } | null>(null);
  const isDragging = useRef(false);
  const chaptersRef = useRef<TocChapter[]>([]);
  const pendingWordClickRef = useRef<number | null>(null);
  const contextMenuRequestRef = useRef(0);
  const loadedInteractionDocumentsRef = useRef(new Set<Document>());
  const wordMarkWordsRef = useRef<string[]>([]);

  const handleTextBookReady = useCallback((document: TextBookDocument) => {
    const textChapters = document.toc.map((entry) => ({
      title: entry.title,
      href: textLocation(entry.source_offset),
      targetHref: textLocation(entry.source_offset),
      depth: entry.depth,
    }));
    chaptersRef.current = textChapters;
    setChapters(textChapters);
    setCurrentChapterIndex(0);
    setBookReady(true);
    setReaderError(null);
  }, []);

  const handleTextBookProgress = useCallback((nextProgress: number, textLocationValue: string, chapterIndex: number) => {
    setProgress(nextProgress);
    currentCfiRef.current = textLocationValue;
    setCurrentChapterIndex(chapterIndex);
    if (bookId) updateReadingProgress(bookId, nextProgress, textLocationValue).catch(() => {});
  }, [bookId]);

  const cancelPendingWordClick = useCallback(() => {
    if (pendingWordClickRef.current !== null) {
      window.clearTimeout(pendingWordClickRef.current);
      pendingWordClickRef.current = null;
    }
  }, []);

  const getHighlightMutationPlan = useCallback(async (
    interaction: ReaderInteraction,
    highlights: Highlight[],
  ): Promise<HighlightMutationPlan | null> => (
    interaction.source === "text"
      ? planTextHighlightMutation(interaction.location, highlights, "yellow", interaction.text)
      : planCfiHighlightMutation(interaction.location, highlights, "yellow", interaction.text)
  ), []);

  const openLearningInteraction = useCallback((interaction: ReaderInteraction) => {
    cancelPendingWordClick();
    if (interaction.trigger === "selection-contextmenu") {
      const requestToken = ++contextMenuRequestRef.current;
      setContextMenu(interaction);
      setContextSelectionFullyHighlighted(false);
      if (bookId) {
        invoke<Highlight[]>("list_highlights", { bookId }).then(async (highlights) => {
          if (contextMenuRequestRef.current !== requestToken) return;
          const plan = await getHighlightMutationPlan(interaction, highlights);
          if (contextMenuRequestRef.current !== requestToken) return;
          setContextSelectionFullyHighlighted(plan?.fullyHighlighted ?? false);
        }).catch(() => {});
      }
      return;
    }
    if (bookId && autoHighlightLookupsRef.current) {
      invoke("ensure_word_mark_rule", { bookId, word: interaction.text, color: "lookup" })
        .then(() => window.dispatchEvent(new CustomEvent("word-mark-changed", { detail: { bookId } })))
        .catch(() => {});
    }
    setLearningInteraction(interaction);
  }, [bookId, cancelPendingWordClick, getHighlightMutationPlan]);

  const handleTextBookError = useCallback((error: string) => {
    setReaderError(error);
    setBookReady(false);
  }, []);

  const registerTextBookNavigation = useCallback((navigateText: (location: string, flash?: boolean) => void) => {
    textReaderNavigateRef.current = navigateText;
    setTextNavigationRegistration((value) => value + 1);
  }, []);

  const handleTextHighlightClick = useCallback((highlight: Highlight, rect: DOMRect) => {
    setHighlightToolbar({
      x: rect.left + rect.width / 2,
      y: rect.top,
      highlightId: highlight.id,
      cfiRange: highlight.cfi_range,
      color: highlight.color,
    });
  }, []);

  useEffect(() => {
    readerSettingsRef.current = readerSettings;
  }, [readerSettings]);

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

  const applyAnnotations = useCallback(async (reapplyVisible = false) => {
    const view = viewRef.current;
    if (!view || !supportsManualAnnotations) return;

    const snapshot = markerSnapshotRef.current;
    if (!snapshot) return;
    const { highlights, lookups, vocab } = snapshot;
    const manual = new Set(highlights.map((highlight) => highlight.cfi_range));
    const settings = readerSettingsRef.current;
    const next = new Map<string, Marker>();
    if (supportsWordMarkers) {
      for (const lookup of lookups) {
        if (settings.showLookupMarkers && lookup.cfi && !manual.has(lookup.cfi)) {
          next.set(lookup.cfi, { color: wordMarkerColor.lookup, kind: "lookup" });
        }
      }
      for (const word of vocab) {
        if (!word.cfi || manual.has(word.cfi)) continue;
        if (word.mastery === "mastered" && settings.showMasteredMarkers) {
          next.set(word.cfi, { color: wordMarkerColor.mastered, kind: "vocab" });
        } else if (word.mastery === "learning" && settings.showLearningMarkers) {
          next.set(word.cfi, { color: wordMarkerColor.learning, kind: "vocab" });
        } else if (word.mastery !== "mastered" && word.mastery !== "learning" && settings.showNewVocabMarkers) {
          next.set(word.cfi, { color: wordMarkerColor.vocabNew, kind: "vocab" });
        }
      }
    }

    autoMarkersRef.current = next;
    const desired = new Map([...next.entries()].map(([cfi, marker]) => [cfi, marker.color]));
    for (const highlight of highlights) desired.set(highlight.cfi_range, highlight.color);
    const previous = appliedAnnotationsRef.current;
    const cfis = new Set([...previous.keys(), ...desired.keys()]);
    await Promise.all([...cfis].map(async (cfi) => {
      const oldColor = previous.get(cfi);
      const newColor = desired.get(cfi);
      if (!reapplyVisible && oldColor === newColor) return;
      if (oldColor !== undefined) await view.deleteAnnotation({ value: cfi }).catch(() => {});
      if (newColor !== undefined) await view.addAnnotation({ value: cfi, color: newColor }).catch(() => {});
    }));
    appliedAnnotationsRef.current = desired;
  }, [supportsManualAnnotations, supportsWordMarkers]);

  const flashNavigationTarget = useCallback(async (cfi: string) => {
    if (isTextBook) {
      textReaderNavigateRef.current?.(cfi, true);
      return;
    }
    const view = viewRef.current;
    if (!view || !supportsCfiNavigation) return;
    await view.goTo(cfi);
    await view.addAnnotation({ value: cfi, color: "#c27aff" }).catch(() => {});
    const token = Date.now() + Math.random();
    navigationFlashRef.current.set(cfi, token);
    window.setTimeout(async () => {
      if (navigationFlashRef.current.get(cfi) !== token || viewRef.current !== view) return;
      navigationFlashRef.current.delete(cfi);
      await view.deleteAnnotation({ value: cfi }).catch(() => {});
      const color = appliedAnnotationsRef.current.get(cfi);
      if (color) await view.addAnnotation({ value: cfi, color }).catch(() => {});
    }, 3000);
  }, [isTextBook, supportsCfiNavigation]);

  const refreshAnnotations = useCallback(async (reapplyVisible = false) => {
    if (isTextBook) return;
    if (!bookId || !viewRef.current || !supportsManualAnnotations) return;
    const highlights = await invoke<Highlight[]>("list_highlights", { bookId });
    const [lookups, vocab] = supportsWordMarkers
      ? await Promise.all([
        invoke<LookupRecord[]>("list_lookup_records", { bookId }),
        invoke<VocabMarker[]>("list_vocab_words", { bookId }),
      ])
      : [[], []] as [LookupRecord[], VocabMarker[]];
    markerSnapshotRef.current = { highlights, lookups, vocab };
    await applyAnnotations(reapplyVisible);
  }, [applyAnnotations, bookId, isTextBook, supportsManualAnnotations, supportsWordMarkers]);

  // Load book metadata and default settings from DB
  useEffect(() => {
    if (!bookId) return;
    autoMarkersRef.current.clear();
    appliedAnnotationsRef.current.clear();
    navigationFlashRef.current.clear();
    markerSnapshotRef.current = null;
    currentCfiRef.current = null;
    setTextInitialLocation(null);
    getBook(bookId)
      .then((b) => {
        currentCfiRef.current = b.current_cfi;
        setTextInitialLocation(b.current_cfi);
        setBook(b);
        if (isStandaloneWindow && b) {
          appWindow.setTitle(b.title);
        }
      })
      .catch(() => setBook(null))
      .finally(() => setLoading(false));

    getAllSettings().then((globalSettings) => {
      const saved = localStorage.getItem(`reader-settings-${bookId}`);
      const bookSettings = saved ? JSON.parse(saved) as Partial<ReaderSettingsState> : {};
      const g = globalSettings;
      autoHighlightLookupsRef.current = g.auto_highlight_lookup_words !== "false";
      setLearningCardConfig(parseCardDesignConfig(g.learning_card_config));
      setReaderSettings((prev) => ({
        ...prev,
        theme: bookSettings.theme || (g.reader_theme as ReaderSettingsState["theme"]) || prev.theme,
        brightness: bookSettings.brightness ?? (g.brightness ? parseInt(g.brightness) : prev.brightness),
        pageColumns: bookSettings.pageColumns ?? (g.page_columns ? parseInt(g.page_columns) as ReaderSettingsState["pageColumns"] : prev.pageColumns),
        font: bookSettings.font || (g.font_family as ReaderSettingsState["font"]) || prev.font,
        fontSize: bookSettings.fontSize ?? (g.font_size ? parseInt(g.font_size) : prev.fontSize),
        readingMode: bookSettings.readingMode || (g.reading_mode as ReaderSettingsState["readingMode"]) || prev.readingMode,
        lineSpacing: bookSettings.lineSpacing ?? (g.line_spacing ? parseFloat(g.line_spacing) : prev.lineSpacing),
        charSpacing: bookSettings.charSpacing ?? (g.char_spacing ? parseInt(g.char_spacing) : prev.charSpacing),
        wordSpacing: bookSettings.wordSpacing ?? (g.word_spacing ? parseInt(g.word_spacing) : prev.wordSpacing),
        margins: bookSettings.margins ?? (g.margins ? parseInt(g.margins) : prev.margins),
        showLookupMarkers: bookSettings.showLookupMarkers ?? prev.showLookupMarkers,
        showNewVocabMarkers: bookSettings.showNewVocabMarkers ?? prev.showNewVocabMarkers,
        showLearningMarkers: bookSettings.showLearningMarkers ?? prev.showLearningMarkers,
        showMasteredMarkers: bookSettings.showMasteredMarkers ?? prev.showMasteredMarkers,
      }));
      const savedZoom = localStorage.getItem(`reader-zoom-${bookId}`);
      if (savedZoom === "fit") {
        setZoom("fit");
      } else {
        const parsedZoom = savedZoom ? parseInt(savedZoom, 10) : NaN;
        if (Number.isFinite(parsedZoom) && parsedZoom >= 50 && parsedZoom <= 300) {
          setZoom(parsedZoom);
        }
      }
      dbSettingsLoaded.current = true;
    }).catch(() => {});
  }, [bookId]);

  // Persist reader settings to localStorage when they change — only after load completes
  // to avoid overwriting saved values with defaults during initialization
  const dbSettingsLoaded = useRef(false);
  useEffect(() => {
    if (!dbSettingsLoaded.current) return;
    localStorage.setItem(`reader-settings-${bookId}`, JSON.stringify(readerSettings));
  }, [bookId, readerSettings]);

  // Persist per-book PDF zoom after load. Debounce to avoid thrashing during
  // rapid zoom-button clicks; only write once the user settles.
  useEffect(() => {
    if (!dbSettingsLoaded.current) return;
    if (book?.format !== "pdf") return;
    const handle = window.setTimeout(() => {
      localStorage.setItem(`reader-zoom-${bookId}`, zoom === "fit" ? "fit" : String(zoom));
    }, 500);
    return () => window.clearTimeout(handle);
  }, [zoom, bookId, book?.format]);

  // Persist per-book reader window size. Standalone reader windows are
  // labelled `reader-${bookId}`; openReaderWindow reads this key back on
  // next open so the window restores to the size the user left it at.
  useEffect(() => {
    if (!isStandaloneWindow || !bookId) return;
    let timer: number | null = null;
    const unlistenPromise = appWindow.onResized(({ payload }) => {
      if (timer !== null) window.clearTimeout(timer);
      timer = window.setTimeout(async () => {
        try {
          const scale = await appWindow.scaleFactor();
          const logical = payload.toLogical(scale);
          localStorage.setItem(
            `reader-window-${bookId}`,
            JSON.stringify({ width: Math.round(logical.width), height: Math.round(logical.height) }),
          );
        } catch { /* window may have closed */ }
      }, 500);
    });
    return () => {
      if (timer !== null) window.clearTimeout(timer);
      unlistenPromise.then((fn) => fn()).catch(() => {});
    };
  }, [bookId]);

  // Wait for an evicted iCloud book, but fail fast for missing local files.
  useEffect(() => {
    if (!book || book.available !== false) {
      setAvailabilityState(null);
      return;
    }

    setAvailabilityState("checking");
    let cancelled = false;
    const startTime = Date.now();

    const poll = async () => {
      while (!cancelled) {
        if (Date.now() - startTime >= 60_000) {
          setAvailabilityState("timeout");
          return;
        }
        const result = await checkBookAvailable(book.id).catch(() => null);
        if (!result) {
          setAvailabilityState("error");
          return;
        }
        if (result.available) {
          // Re-fetch book to get updated available flag
          const updated = await getBook(book.id).catch(() => null);
          if (updated?.available !== false) {
            setBook(updated);
            setAvailabilityState(null);
          } else {
            setAvailabilityState("error");
          }
          return;
        }
        if (result.status === "missing") {
          setAvailabilityState("missing");
          return;
        }
        setAvailabilityState("icloud_placeholder");
        await new Promise((r) => setTimeout(r, 2000));
      }
    };

    poll();
    return () => { cancelled = true; };
  }, [book, availabilityRetry]);

  // Initialize foliate-js when book data is loaded
  useEffect(() => {
    if (!book || !viewerRef.current || book.available === false || isTextBook) return;

    const container = viewerRef.current;
    container.innerHTML = "";
    loadedInteractionDocumentsRef.current.clear();
    setBookReady(false);
    setReaderError(null);

    let cancelled = false;
    let activeView: FoliateView | null = null;

    const initFoliate = async () => {
      if (supportsWordMarkers && bookId) {
        const rules = await invoke<WordMarkRule[]>("list_word_marks", { bookId }).catch(() => []);
        wordMarkWordsRef.current = rules
          .filter((rule) => rule.enabled)
          .map((rule) => rule.normalized_word);
      } else {
        wordMarkWordsRef.current = [];
      }
      // Load foliate-js web components (from public/ dir, loaded as native ES module)
      if (!customElements.get("foliate-view")) {
        await withTimeout(new Promise<void>((resolve, reject) => {
          const script = document.createElement("script");
          script.type = "module";
          script.src = "/foliate-js/view.js";
          script.onload = () => resolve();
          script.onerror = () => reject(new Error("Failed to load foliate-js"));
          document.head.appendChild(script);
        }), 15_000, "READER_SCRIPT_TIMEOUT");
        // Wait for custom element to be defined
        await withTimeout(customElements.whenDefined("foliate-view"), 15_000, "READER_ELEMENT_TIMEOUT");
      }

      if (cancelled) return;

      const view = document.createElement("foliate-view") as FoliateView;
      activeView = view;
      view.style.display = "block";
      view.style.width = "100%";
      view.style.height = "100%";
      // Opt into the continuous-scroll PDF renderer before open(); view.js
      // branches its internal renderer pick on this attribute. EPUBs ignore it.
      if (book.format === "pdf" && readerSettings.readingMode === "scrolling") {
        view.setAttribute("pdf-mode", "scroll");
      }
      container.appendChild(view);
      viewRef.current = view;

      // Preserve the stored source extension so Foliate selects its native parser.
      const fileUrl = convertFileSrc(book.file_path);
      const response = await withTimeout(fetch(fileUrl), 30_000, "READER_FILE_TIMEOUT");
      if (!response.ok) {
        throw new Error(`READER_FILE_${response.status}`);
      }
      const extension = (book.render_format || book.format || "epub").toLowerCase();
      const mime = {
        epub: "application/epub+zip",
        pdf: "application/pdf",
        mobi: "application/x-mobipocket-ebook",
        azw: "application/x-mobipocket-ebook",
        azw3: "application/x-mobipocket-ebook",
        fb2: "application/x-fictionbook+xml",
        fbz: "application/x-zip-compressed-fb2",
        cbz: "application/vnd.comicbook+zip",
      }[extension] || "application/octet-stream";
      const blob = new File(
        [await withTimeout(response.blob(), 30_000, "READER_FILE_READ_TIMEOUT")],
        `book.${extension}`,
        { type: mime },
      );
      await withTimeout(view.open(blob), 45_000, "READER_OPEN_TIMEOUT");

      if (cancelled) return;

      // Only apply layout attributes the current renderer supports. In
      // particular, fixed/comic renderers must not receive EPUB flow, column,
      // or typography settings merely because they share a Foliate view.
      if (capabilities.supportsReflowSettings) {
        const isScrolling = readerSettings.readingMode === "scrolling";
        view.renderer.setAttribute("flow", isScrolling ? "scrolled" : "paginated");
        view.renderer.setAttribute("gap", "5%");
        view.renderer.setAttribute("max-inline-size", "1000px");
      }
      if (capabilities.supportsSpread) {
        view.renderer.setAttribute("max-column-count", String(readerSettings.pageColumns));
      }
      if (capabilities.supportsSpread && book.format === "pdf") {
        view.renderer.setAttribute("spread", readerSettings.pageColumns === 1 ? "none" : "auto");
      }
      if (capabilities.supportsZoom && book.format === "pdf") {
        // Seed zoom from localStorage BEFORE view.init so CFI restore lands on
        // the right scroll offset. Re-applying after init would trigger a
        // #layoutAll() that invalidates the pixel position for the restored CFI.
        const savedZoom = localStorage.getItem(`reader-zoom-${bookId}`);
        let zoomAttr = "fit-width";
        if (savedZoom && savedZoom !== "fit") {
          const n = parseInt(savedZoom, 10);
          if (Number.isFinite(n) && n >= 50 && n <= 300) zoomAttr = String(n / 100);
        }
        view.renderer.setAttribute("zoom", zoomAttr);
      }

      if (capabilities.supportsReflowSettings) {
        view.renderer.setStyles?.(getReaderCSS(readerSettings));
      }

      // PDF theming is handled by the overlay div in the JSX

      // Load TOC
      const toc = view.book.toc;
      if (toc) {
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const tocHref = (item: any): string | undefined => {
          if (typeof item.href !== "string" || item.href === "" || item.href === "null") return undefined;
          return item.href;
        };
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const firstHref = (item: any): string | undefined => {
          const href = tocHref(item);
          if (href) return href;
          // eslint-disable-next-line @typescript-eslint/no-explicit-any
          return item.subitems?.map((child: any) => firstHref(child)).find(Boolean);
        };
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        const flattenToc = (items: any[], depth = 0): TocChapter[] =>
          // eslint-disable-next-line @typescript-eslint/no-explicit-any
          items.flatMap((item: any) => [
            { title: item.label?.trim() || "", href: tocHref(item), targetHref: firstHref(item), depth },
            ...(item.subitems ? flattenToc(item.subitems, depth + 1) : []),
          ]);
        const chs = flattenToc(toc);
        chaptersRef.current = chs;
        setChapters(chs);
      }

      // Listen for location changes
      view.addEventListener("relocate", ((e: CustomEvent) => {
        const { fraction, location, tocItem, cfi } = e.detail;

        const pct = Math.round((fraction ?? 0) * 100);
        setProgress(pct);
        currentCfiRef.current = cfi;

        if (location) {
          setPageInfo({ current: location.current + 1, total: location.total });
        }

        if (tocItem) {
          const exactIdx = chaptersRef.current.findIndex((c) => c.href === tocItem.href);
          const idx = exactIdx !== -1
            ? exactIdx
            : chaptersRef.current.findIndex((c) => c.targetHref === tocItem.href);
          if (idx !== -1) setCurrentChapterIndex(idx);
        }

        if (bookId) {
          updateReadingProgress(bookId, pct, cfi).catch(() => {});
        }
      }) as EventListener);

      // Track navigation history for back button. Auto-hide 10s after the
      // last jump so it doesn't linger once the reader has settled.
      view.history.addEventListener("index-change", () => {
        const canBack = view.history.canGoBack;
        setCanGoBack(canBack);
        if (backButtonTimerRef.current !== null) {
          window.clearTimeout(backButtonTimerRef.current);
          backButtonTimerRef.current = null;
        }
        if (canBack) {
          backButtonTimerRef.current = window.setTimeout(() => {
            setCanGoBack(false);
            backButtonTimerRef.current = null;
          }, 10000);
        }
      });

      // Handle section loads — text selection, keyboard, highlights
      view.addEventListener("load", ((e: CustomEvent) => {
        const { doc, index } = e.detail;
        if (loadedInteractionDocumentsRef.current.has(doc)) return;
        loadedInteractionDocumentsRef.current.add(doc);
        if (supportsWordMarkers) {
          applyWordMarkHighlights(doc, wordMarkWordsRef.current);
        }

        // Context menu inside content
        doc.addEventListener("contextmenu", (ev: MouseEvent) => {
          cancelPendingWordClick();
          if (!supportsSelection) return;
          const range = selectedRange(doc);
          if (!range) return;
          const text = range.toString().trim();
          const location = view.getCFI(index, range);
          if (!text || !location) return;
          ev.preventDefault();
          openLearningInteraction({
            trigger: "selection-contextmenu",
            kind: classifySelection(text, doc.documentElement.lang || undefined),
            text,
            normalizedText: normalizeInteractionText(text),
            context: contextForRange(range, text),
            location,
            anchorRect: viewportRectForRange(range),
            source: "foliate",
            format: book.format === "pdf" ? "pdf" : "epub",
            locale: doc.documentElement.lang || undefined,
          });
        });

        // Keyboard navigation inside content docs
        doc.addEventListener("keydown", (ev: KeyboardEvent) => {
          if ((ev.metaKey || ev.ctrlKey) && ev.key === "[") {
            ev.preventDefault();
            view.history.back();
          } else if ((ev.metaKey || ev.ctrlKey) && ev.key === "]") {
            ev.preventDefault();
            view.history.forward();
          } else if (ev.key === "ArrowLeft") view.prev();
          else if (ev.key === "ArrowRight") view.next();
          else if ((ev.metaKey || ev.ctrlKey) && (ev.key === "=" || ev.key === "+")) {
            ev.preventDefault();
            if (book?.format === "pdf") handleZoom(10);
          } else if ((ev.metaKey || ev.ctrlKey) && ev.key === "-") {
            ev.preventDefault();
            if (book?.format === "pdf") handleZoom(-10);
          }
        });

        // A short delay keeps single-click lookup from racing double-click
        // selection and Foliate's existing-annotation activation.
        doc.addEventListener("click", (ev: MouseEvent) => {
          setContextMenu(null);
          setHighlightToolbar(null);
          cancelPendingWordClick();
          if (!supportsSelection || ev.button !== 0 || ev.metaKey || ev.ctrlKey || ev.altKey || ev.shiftKey) return;
          if (isInteractiveReaderTarget(ev.target)) return;
          const selection = doc.getSelection?.();
          if (selection && !selection.isCollapsed) return;
          const range = wordRangeAtPoint(doc, ev.clientX, ev.clientY, doc.documentElement.lang || undefined);
          if (!range) return;
          const text = range.toString().trim();
          const location = view.getCFI(index, range);
          const normalizedText = normalizeInteractionText(text);
          if (!text || !normalizedText || !location) return;
          const interaction: ReaderInteraction = {
            trigger: "word-click",
            kind: "word",
            text,
            normalizedText,
            context: contextForRange(range, text),
            location,
            anchorRect: viewportRectForRange(range),
            source: "foliate",
            format: book.format === "pdf" ? "pdf" : "epub",
            locale: doc.documentElement.lang || undefined,
          };
          pendingWordClickRef.current = window.setTimeout(() => {
            pendingWordClickRef.current = null;
            openLearningInteraction(interaction);
          }, 180);
        });
        doc.addEventListener("dblclick", cancelPendingWordClick);

        // Cross-iframe deselect: each PDF page renders in its own iframe
        // with its own Selection object, so clicking on a neighbor page
        // (e.g. the right page in two-page spread mode, or any other page
        // in scroll mode) wouldn't normally clear the selection on the
        // page you came from. Wipe other iframes' selections on mousedown
        // — the current iframe is left alone so the browser can place a
        // fresh caret at the click point.
        doc.addEventListener("mousedown", () => {
          // getContents() lives on the renderer, not the View element.
          const contents = view.renderer?.getContents?.() ?? [];
          for (const { doc: otherDoc } of contents) {
            if (otherDoc && otherDoc !== doc) {
              otherDoc.defaultView?.getSelection()?.removeAllRanges();
            }
          }
          setHighlightToolbar(null);
        });
      }) as EventListener);

      // Highlights and automatic word markers use CFI anchors. Manual
      // highlights always take precedence at the same location.
      view.addEventListener("create-overlay", (() => {
        if (bookId) {
          // Foliate recreates overlays during pagination and resizing. The
          // snapshot was refreshed when the reader became ready or when data
          // changed; reapply it without fetching every record in the book.
          if (supportsManualAnnotations) {
            applyAnnotations(true).catch(() => {});
          }
        }
      }) as EventListener);

      view.addEventListener("draw-annotation", ((e: CustomEvent) => {
        const { draw, annotation } = e.detail;
        if (annotation.color === wordMarkerColor.lookup || annotation.color === wordMarkerColor.vocabNew || annotation.color === wordMarkerColor.learning || annotation.color === wordMarkerColor.mastered) {
          const color = annotation.color === wordMarkerColor.learning
            ? "#68A68A"
            : annotation.color === wordMarkerColor.vocabNew ? "#B78538" : annotation.color === wordMarkerColor.mastered ? "#789B8D" : "#8D7C65";
          draw((rects: DOMRectList) => {
            const g = document.createElementNS("http://www.w3.org/2000/svg", "g");
            g.setAttribute("fill", color);
            g.setAttribute("opacity", annotation.color === wordMarkerColor.lookup ? "0.55" : annotation.color === wordMarkerColor.mastered ? "0.45" : "0.72");
            for (const { left, top, height, width } of rects) {
              const el = document.createElementNS("http://www.w3.org/2000/svg", "rect");
              el.setAttribute("x", String(left));
              el.setAttribute("y", String(top + height - 1.5));
              el.setAttribute("height", "1.5");
              el.setAttribute("width", String(width));
              el.setAttribute("rx", "0.75");
              g.append(el);
            }
            return g;
          });
          return;
        }
        const color = highlightColorMap[annotation.color] || highlightColorMap.yellow;
        const isPdf = book?.format === "pdf";
        draw((rects: DOMRectList) => {
          const g = document.createElementNS("http://www.w3.org/2000/svg", "g");
          g.setAttribute("fill", color);
          g.setAttribute("opacity", "0.35");
          g.style.mixBlendMode = "multiply";
          for (const { left, top, height, width } of rects) {
            const el = document.createElementNS("http://www.w3.org/2000/svg", "rect");
            // PDF text layer spans have sub-pixel gaps; pad rects to close them
            const pad = isPdf ? 1 : 0;
            el.setAttribute("x", String(Math.floor(left)));
            el.setAttribute("y", String(top - pad));
            el.setAttribute("height", String(height + pad * 2));
            el.setAttribute("width", String(Math.ceil(width)));
            el.setAttribute("rx", isPdf ? "1" : "0");
            g.append(el);
          }
          return g;
        });
      }) as EventListener);

      view.addEventListener("show-annotation", ((e: CustomEvent) => {
        cancelPendingWordClick();
        const { value, range } = e.detail;
        const marker = autoMarkersRef.current.get(value);
        if (marker) {
          if (marker.kind === "vocab") {
            setActiveVocabCfi(value);
            setSidePanel("vocab");
          } else if (range && bookId) {
            invoke<LookupRecord[]>("list_lookup_records", { bookId }).then((records) => {
              const record = records.find((item) => item.cfi === value);
              if (!record) return;
              const rect = range.getBoundingClientRect();
              const iframe = range.startContainer?.ownerDocument?.defaultView?.frameElement as HTMLElement | null;
              const iframeRect = iframe?.getBoundingClientRect();
              setLookup({
                x: rect.left + (iframeRect?.left ?? 0) + rect.width / 2,
                y: rect.top + (iframeRect?.top ?? 0),
                word: record.lookup_text,
                sentence: record.context_sentence || record.lookup_text,
                bookTitle: book?.title,
                chapter: record.chapter || undefined,
                cfi: record.cfi || undefined,
              });
            }).catch(() => {});
          }
          return;
        }
        // Find the highlight in the DB to get its id and color
        if (bookId) {
          invoke<Highlight[]>("list_highlights", { bookId }).then((hls) => {
            const hl = hls.find((h) => h.cfi_range === value);
            if (hl && range) {
              const rect = range.getBoundingClientRect();
              // The range is inside an iframe, offset to main viewport
              const iframe = range.startContainer?.ownerDocument?.defaultView?.frameElement as HTMLElement | null;
              let offsetX = 0, offsetY = 0;
              if (iframe) {
                const iframeRect = iframe.getBoundingClientRect();
                offsetX = iframeRect.left;
                offsetY = iframeRect.top;
              }
              setHighlightToolbar({
                x: rect.left + offsetX + rect.width / 2,
                y: rect.top + offsetY,
                highlightId: hl.id,
                cfiRange: hl.cfi_range,
                color: hl.color,
              });
            }
          }).catch(() => {});
        }
      }) as EventListener);

      // Navigate to saved position. Prefer the in-memory cursor (kept fresh
      // by the relocate handler) over the stale `book.current_cfi` from
      // initial load — matters when re-init is triggered by a reading-mode
      // toggle mid-session.
      const startCfi = currentCfiRef.current || book.current_cfi;
      let startLocation: string | undefined = startCfi || undefined;
      if (!startLocation && book.format === "pdf") {
        const pageCount = view.book?.sections?.length ?? book.pages;
        startLocation = getPdfStartCfi(book.progress, pageCount);
      }
      await withTimeout(
        view.init({ lastLocation: startLocation, showTextStart: !startLocation }),
        45_000,
        "READER_INIT_TIMEOUT",
      );

      if (cancelled) return;

      // Apply brightness
      if (viewerRef.current) {
        viewerRef.current.style.filter = `brightness(${readerSettings.brightness / 100})`;
      }

      setBookReady(true);
    };

    initFoliate().catch((err) => {
      if (cancelled) return;
      console.error("Failed to initialize foliate-js:", err);
      activeView?.close();
      activeView?.remove();
      if (viewRef.current === activeView) viewRef.current = null;
      setReaderError(err instanceof Error ? err.message : "READER_INIT_FAILED");
      setBookReady(false);
    });

    const loadedDocuments = loadedInteractionDocumentsRef.current;
    return () => {
      cancelled = true;
      cancelPendingWordClick();
      if (backButtonTimerRef.current !== null) {
        window.clearTimeout(backButtonTimerRef.current);
        backButtonTimerRef.current = null;
      }
      setCanGoBack(false);
      if (viewRef.current) {
        viewRef.current.close();
        viewRef.current.remove();
        viewRef.current = null;
      }
      loadedDocuments.clear();
    };
    // PDFs need a fresh `view` element when reading mode flips because
    // `pdf-mode="scroll"` is read once inside view.js's renderer pick.
    // EPUBs handle scroll/paginated switching live via the reactive effect
    // below, so the derived dep stays `null` for them and the effect won't
    // re-run.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [book, book?.format === "pdf" ? readerSettings.readingMode : null, applyAnnotations, capabilities, isTextBook, supportsManualAnnotations, supportsSelection, readerRetry]);

  // Apply reader settings reactively
  useEffect(() => {
    const view = viewRef.current;
    if (!view?.renderer) return;

    if (capabilities.supportsReflowSettings) {
      view.renderer.setStyles?.(getReaderCSS(readerSettings));
      view.renderer.setAttribute("flow",
        readerSettings.readingMode === "scrolling" ? "scrolled" : "paginated",
      );
      const baseWidth = 1000;
      const marginOffset = readerSettings.margins * 2;
      view.renderer.setAttribute("max-inline-size", `${Math.max(400, baseWidth - marginOffset)}px`);
    }

    if (capabilities.supportsSpread) {
      view.renderer.setAttribute("max-column-count", String(readerSettings.pageColumns));
    }
    if (capabilities.supportsSpread && book?.format === "pdf") {
      view.renderer.setAttribute("spread", readerSettings.pageColumns === 1 ? "none" : "auto");
    }

    // Update brightness
    if (viewerRef.current) {
      viewerRef.current.style.filter = `brightness(${readerSettings.brightness / 100})`;
    }
    // PDF theming is handled by the overlay div in the JSX
    // bookReady is in deps so this re-runs once foliate finishes init —
    // fixes a race where DB-loaded settings arrive before view.renderer exists.
  }, [readerSettings, book?.format, bookReady, capabilities]);

  // Re-layout alone does not remove stale annotation DOM, so marker changes
  // update the mounted annotations directly.
  const markerVisibility = [
    readerSettings.showLookupMarkers,
    readerSettings.showNewVocabMarkers,
    readerSettings.showLearningMarkers,
    readerSettings.showMasteredMarkers,
  ].join(":");
  useEffect(() => {
    refreshAnnotations().catch(() => {});
  }, [bookReady, markerVisibility, refreshAnnotations]);

  useEffect(() => {
    if (!bookId || !supportsWordMarkers || isTextBook) return;
    const refresh = (event: Event) => {
      const detail = (event as CustomEvent<{ bookId?: string }>).detail;
      if (detail?.bookId && detail.bookId !== bookId) return;
      invoke<WordMarkRule[]>("list_word_marks", { bookId }).then((rules) => {
        wordMarkWordsRef.current = rules
          .filter((rule) => rule.enabled)
          .map((rule) => rule.normalized_word);
        for (const doc of loadedInteractionDocumentsRef.current) {
          applyWordMarkHighlights(doc, wordMarkWordsRef.current);
        }
      }).catch(() => {});
    };
    window.addEventListener("word-mark-changed", refresh);
    return () => window.removeEventListener("word-mark-changed", refresh);
  }, [bookId, isTextBook, supportsWordMarkers]);

  useEffect(() => {
    const refreshForCurrentBook = (event: Event) => {
      const detail = (event as CustomEvent<{ bookId?: string }>).detail;
      if (!detail?.bookId || detail.bookId === bookId) refreshAnnotations().catch(() => {});
    };
    window.addEventListener("lookup-record-changed", refreshForCurrentBook);
    window.addEventListener("vocab-changed", refreshForCurrentBook);
    window.addEventListener("highlight-changed", refreshForCurrentBook);
    window.addEventListener("focus", refreshForCurrentBook);
    return () => {
      window.removeEventListener("lookup-record-changed", refreshForCurrentBook);
      window.removeEventListener("vocab-changed", refreshForCurrentBook);
      window.removeEventListener("highlight-changed", refreshForCurrentBook);
      window.removeEventListener("focus", refreshForCurrentBook);
    };
  }, [bookId, refreshAnnotations]);

  useEffect(() => {
    const applyNavigation = async (target: ReaderNavigation) => {
      if (!bookReady || (!isTextBook && !viewRef.current) || (isTextBook && !textReaderNavigateRef.current)) {
        pendingNavigationRef.current = target;
        return;
      }
      pendingNavigationRef.current = null;
      if (target.cfi && supportsCfiNavigation) {
        if (isTextBook) textReaderNavigateRef.current?.(target.cfi, true);
        else await viewRef.current?.goTo(target.cfi);
      }
      if (target.openVocab && supportsCfiNavigation) setSidePanel("vocab");
      if (target.openChat) {
        setSidePanel("ai");
        if (target.chatId) setInitialChatId(target.chatId);
      }
      if (target.navigationId) {
        await emit("reader:navigate:ack", { navigationId: target.navigationId, bookId });
      }
    };
    const unlisten = Promise.all([
      appWindow.listen<{ bookId?: string }>("lookup-record-changed", (event) => {
        if (!event.payload.bookId || event.payload.bookId === bookId) refreshAnnotations().catch(() => {});
      }),
      appWindow.listen<{ bookId?: string }>("vocab-changed", (event) => {
        if (!event.payload.bookId || event.payload.bookId === bookId) refreshAnnotations().catch(() => {});
      }),
      appWindow.listen<ReaderNavigation>(
        "reader:navigate",
        (event) => {
          applyNavigation(event.payload).catch(() => {});
        },
      ),
    ]);
    const pending = pendingNavigationRef.current;
    if (pending) applyNavigation(pending).catch(() => {});
    return () => { unlisten.then((fns) => fns.forEach((fn) => fn())).catch(() => {}); };
  }, [bookId, bookReady, isTextBook, refreshAnnotations, supportsCfiNavigation, textNavigationRegistration]);

  // Track the current fit-width scale so +/- from fit mode lands near the
  // visible size. Observes the renderer and sums the natural widths of the
  // pages in a single row — one page in single mode, two in spread mode —
  // to match what fixed-layout.js / pdf-scroll.js actually fit.
  useEffect(() => {
    if (!bookReady || book?.format !== "pdf") return;
    const renderer = viewRef.current?.renderer;
    const foliateBook = viewRef.current?.book;
    if (!renderer || !foliateBook?.getPageSize) return;
    const isSpread = readerSettings.pageColumns === 2;
    let cancelled = false;
    const update = async () => {
      try {
        const first = await foliateBook.getPageSize(0);
        const second = isSpread ? await foliateBook.getPageSize(1).catch(() => null) : null;
        if (cancelled || !first?.width) return;
        const rowWidth = first.width + (second?.width ?? 0);
        const containerW = Math.max(renderer.clientWidth - 24, 1);
        fitPctRef.current = Math.round((containerW / rowWidth) * 100);
      } catch { /* book may have closed */ }
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(renderer);
    return () => { cancelled = true; ro.disconnect(); };
  }, [bookReady, book?.format, readerSettings.readingMode, readerSettings.pageColumns]);

  useEffect(() => {
    if (!bookReady || book?.format !== "pdf" || !viewerRef.current) return;
    let raf = 0;
    const relayoutPdf = () => {
      const renderer = viewRef.current?.renderer as (HTMLElement & { relayout?: () => void }) | undefined;
      if (!renderer) return;
      if (renderer.hasAttribute("resize-dragging")) return;
      if (typeof renderer.relayout === "function") {
        renderer.relayout();
        return;
      }
      const value = zoomRef.current;
      renderer.setAttribute("zoom", value === "fit" ? "fit-width" : String(value / 100));
    };
    const ro = new ResizeObserver(() => {
      if (raf) cancelAnimationFrame(raf);
      raf = requestAnimationFrame(relayoutPdf);
    });
    ro.observe(viewerRef.current);
    return () => {
      if (raf) cancelAnimationFrame(raf);
      ro.disconnect();
    };
  }, [bookReady, book?.format]);

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

  // Keyboard navigation — parent document listener
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement)?.tagName;
      if (tag === "INPUT" || tag === "TEXTAREA") return;
      if (e.key === "ArrowLeft") viewRef.current?.prev();
      else if (e.key === "ArrowRight") viewRef.current?.next();
      // Cmd+/Cmd- zoom for PDFs
      else if ((e.metaKey || e.ctrlKey) && (e.key === "=" || e.key === "+")) {
        e.preventDefault();
        if (book?.format === "pdf") handleZoom(10);
      } else if ((e.metaKey || e.ctrlKey) && e.key === "-") {
        e.preventDefault();
        if (book?.format === "pdf") handleZoom(-10);
      }
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [book?.format, handleZoom]);

  const panelRef = useRef<HTMLDivElement>(null);
  const panelWidthRef = useRef(panelWidth);

  useEffect(() => {
    panelWidthRef.current = panelWidth;
  }, [panelWidth]);

  const handlePanelResizePointerDown = useCallback((e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    isDragging.current = true;
    const handle = e.currentTarget;
    const pointerId = e.pointerId;
    const startX = e.clientX;
    const startWidth = panelWidthRef.current;
    let rafId = 0;
    let latestWidth = startWidth;
    let finished = false;

    // Signal to foliate-js renderer to skip expensive re-renders during drag
    const renderer = (viewRef.current as unknown as HTMLElement)?.shadowRoot
      ?.querySelector("foliate-paginator, foliate-fxl, foliate-pdf-scroll");
    renderer?.setAttribute("resize-dragging", "");

    const widthFromClientX = (clientX: number) => {
      const delta = startX - clientX;
      return Math.min(
        PANEL_MAX_WIDTH,
        Math.max(PANEL_MIN_WIDTH, startWidth + delta)
      );
    };

    const schedulePanelWidth = (width: number) => {
      latestWidth = width;
      if (rafId) return;
      rafId = requestAnimationFrame(() => {
        if (panelRef.current) {
          panelRef.current.style.width = `${latestWidth}px`;
        }
        rafId = 0;
      });
    };

    const cleanup = () => {
      window.removeEventListener("pointermove", handlePointerMove);
      window.removeEventListener("pointerup", handlePointerUp);
      window.removeEventListener("pointercancel", handlePointerCancel);
      window.removeEventListener("blur", handleWindowBlur);
      handle.removeEventListener("lostpointercapture", handleLostPointerCapture);
      try {
        if (handle.hasPointerCapture(pointerId)) {
          handle.releasePointerCapture(pointerId);
        }
      } catch { /* pointer capture can already be gone */ }
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      // Remove drag signal — triggers one final render via attributeChangedCallback
      renderer?.removeAttribute("resize-dragging");
    };

    const finishDrag = (clientX?: number) => {
      if (finished) return;
      finished = true;
      isDragging.current = false;
      if (typeof clientX === "number") {
        latestWidth = widthFromClientX(clientX);
      }
      if (rafId) cancelAnimationFrame(rafId);
      if (panelRef.current) {
        panelRef.current.style.width = `${latestWidth}px`;
      }
      cleanup();
      setPanelWidth(latestWidth);
    };

    function handlePointerMove(e: PointerEvent) {
      if (!isDragging.current) return;
      schedulePanelWidth(widthFromClientX(e.clientX));
    }

    function handlePointerUp(e: PointerEvent) {
      finishDrag(e.clientX);
    }

    function handlePointerCancel() {
      finishDrag();
    }

    function handleWindowBlur() {
      finishDrag();
    }

    function handleLostPointerCapture() {
      finishDrag();
    }

    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
    try {
      handle.setPointerCapture(pointerId);
    } catch { /* pointer capture is best-effort */ }
    window.addEventListener("pointermove", handlePointerMove);
    window.addEventListener("pointerup", handlePointerUp);
    window.addEventListener("pointercancel", handlePointerCancel);
    window.addEventListener("blur", handleWindowBlur);
    handle.addEventListener("lostpointercapture", handleLostPointerCapture);
  }, []);

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
    const state = location.state as { openVocab?: boolean; cfi?: string } | null;
    const searchParams = new URLSearchParams(window.location.search);
    const openVocab = state?.openVocab || searchParams.get("openVocab") === "true";
    const cfi = state?.cfi || searchParams.get("cfi") || undefined;
    if (!bookReady || (!openVocab && !cfi)) return;
    if (openVocab) setSidePanel("vocab");
    if (cfi && supportsCfiNavigation) flashNavigationTarget(cfi).catch(() => {});
    // Clear the state so it doesn't re-trigger
    if (!isStandaloneWindow) navigate(location.pathname, { replace: true });
  }, [bookReady, flashNavigationTarget, location.state, location.pathname, navigate, supportsCfiNavigation]);

  if (loading) {
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
            <Button variant="secondary" size="sm" onClick={() => setAvailabilityRetry((value) => value + 1)}>
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
            onSettingsChange={setReaderSettings}
            capabilities={capabilities}
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

          <button
            onClick={() => togglePanel("ai")}
            className={`flex items-center gap-2 h-8 px-2.5 rounded-lg cursor-pointer transition-colors ${
              sidePanel === "ai"
                ? "text-accent-text"
                : isStandaloneWindow ? "opacity-60 hover:opacity-100" : "hover:bg-bg-input text-text-muted"
            }`}
          >
            <Bot size={16} />
            <span className="text-[14px] font-medium tracking-[-0.15px]">
              {t("reader.aiAssistant")}
            </span>
          </button>
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
            className="flex-1 relative overflow-hidden"
            style={{ backgroundColor: getThemeStyles(readerSettings.theme).body }}
            onClick={() => {
              setSettingsOpen(false);
              // Clicks inside the iframe (text content) don't bubble out
              // through the sandbox boundary, so this fires only for clicks
              // on the margins/white space around the page — i.e. "anywhere
              // else" from the reader's perspective. Drop the in-iframe text
              // selection so the highlight doesn't linger.
              viewRef.current?.deselect?.();
              setHighlightToolbar(null);
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
                onHighlightClick={handleTextHighlightClick}
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
                <div
                  className="h-full transition-all"
                  style={{ width: `${progress}%`, backgroundColor: isStandaloneWindow ? "currentColor" : "#9f9fa9", opacity: isStandaloneWindow ? 0.4 : undefined }}
                />
              </div>
              <div className="flex items-center justify-between h-8">
                <span className={`text-[12px] ${isStandaloneWindow ? "opacity-60" : "text-text-muted"}`}>
                  {pageInfo ? t("reader.pageOf", { current: pageInfo.current, total: pageInfo.total }) : `${progress}%`}
                </span>
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
                <span className={`text-[12px] ${isStandaloneWindow ? "opacity-50" : "text-text-muted"}`}>
                  {pageInfo && pageInfo.total > pageInfo.current
                    ? t("reader.pagesLeft", { count: pageInfo.total - pageInfo.current })
                    : ""}
                </span>
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
        <div ref={panelRef} className={sidePanel ? "shrink-0 h-full" : "hidden"} style={{ width: panelWidth }}>
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
          x={contextMenu.anchorRect.right}
          y={contextMenu.anchorRect.top}
          text={contextMenu.text}
          kind={contextMenu.kind}
          highlighted={contextSelectionFullyHighlighted}
          order={learningCardConfig.selectionMenus[contextMenu.kind === "passage" ? "passage" : "phrase"]
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
            setLearningInteraction(contextMenu);
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
            setLearningInteraction(contextMenu);
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
          onToggleHighlight={supportsManualAnnotations ? (() => {
            const interaction = contextMenu;
            contextMenuRequestRef.current += 1;
            setContextMenu(null);
            if (!interaction.location || !bookId) return;
            invoke<Highlight[]>("list_highlights", { bookId }).then(async (highlights) => {
              const plan = await getHighlightMutationPlan(interaction, highlights);
              if (!plan) return;
              return invoke<Highlight[]>("replace_highlights", {
                bookId,
                removeIds: plan.removeIds,
                additions: plan.additions,
              }).then(async () => {
                window.dispatchEvent(new CustomEvent("highlight-changed", { detail: { bookId } }));
                await refreshAnnotations();
              });
            }).catch((err) => console.error("Failed to toggle highlight:", err));
          }) : undefined}
        />
      )}

      {learningInteraction && bookId && (
        <LearningCardController
          key={`${learningInteraction.kind}:${learningInteraction.location}:${learningInteraction.text}`}
          interaction={learningInteraction}
          bookId={bookId}
          bookTitle={book?.title}
          chapter={currentChapterIndex >= 0 && currentChapterIndex < chapters.length
            ? chapters[currentChapterIndex].title
            : undefined}
          config={learningCardConfig}
          readerRect={readerRect}
          onClose={() => setLearningInteraction(null)}
          onAskAi={(quote, cfi) => {
            setAiContext({ text: quote, cfi });
            setSidePanel("ai");
          }}
          onViewAllNotes={() => {
            invoke("open_library_on_main", { filter: "notes" }).catch(() => {});
          }}
        />
      )}

      {lookup && (
        <LookupPopover
          x={lookup.x}
          y={lookup.y}
          word={lookup.word}
          sentence={lookup.sentence}
          bookTitle={lookup.bookTitle}
          chapter={lookup.chapter}
          bookId={bookId!}
          cfi={lookup.cfi}
          onAskFollowUp={(quote, cfi) => {
            setAiContext({ text: quote, cfi });
            setSidePanel("ai");
          }}
          onClose={() => setLookup(null)}
        />
      )}

      {explain && (
        <ExplainPopover
          x={explain.x}
          y={explain.y}
          text={explain.text}
          sentence={explain.sentence}
          bookTitle={explain.bookTitle}
          chapter={explain.chapter}
          bookId={bookId!}
          cfi={explain.cfi}
          onClose={() => setExplain(null)}
        />
      )}

      {translation && (
        <TranslationPopover
          x={translation.x}
          y={translation.y}
          text={translation.text}
          context={translation.context}
          bookId={bookId!}
          cfi={translation.cfi}
          onClose={() => setTranslation(null)}
        />
      )}

      {highlightToolbar && (
        <HighlightToolbar
          x={highlightToolbar.x}
          y={highlightToolbar.y}
          currentColor={highlightToolbar.color}
          onChangeColor={(color) => {
            invoke("update_highlight_color", { id: highlightToolbar.highlightId, color })
              .then(async () => {
                window.dispatchEvent(new CustomEvent("highlight-changed", { detail: { bookId } }));
                await refreshAnnotations();
                setHighlightToolbar((prev) => prev ? { ...prev, color } : null);
              })
              .catch((err) => console.error("Failed to update highlight color:", err));
          }}
          onDelete={() => {
            invoke("remove_highlight", { id: highlightToolbar.highlightId })
              .then(async () => {
                window.dispatchEvent(new CustomEvent("highlight-changed", { detail: { bookId } }));
                await refreshAnnotations();
                setHighlightToolbar(null);
              })
              .catch((err) => console.error("Failed to remove highlight:", err));
          }}
          onClose={() => setHighlightToolbar(null)}
        />
      )}
    </div>
  );
}
