import {
  useCallback,
  useEffect,
  useRef,
  type Dispatch,
  type MutableRefObject,
  type SetStateAction,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import type { ReaderSettingsState } from "../../components/ReaderSettings";
import {
  applyWordMarkHighlights,
} from "../../components/reader-interaction";
import type { Highlight } from "../../hooks/useBookmarks";
import {
  MARKER_STYLE_SETTING_KEY,
  effectiveAutomaticMarkerStyle,
  markerHighlightCss,
  markerOverlayStyle,
  parseMarkerStyleConfig,
  type MarkerStyleConfigV1,
  type MarkerVisualStyleV1,
} from "../../components/marker-style";
import {
  installCustomFontFacesInDocument,
  type CustomFontRecord,
} from "../../components/custom-fonts";
import { getReaderCSS } from "./reader-theme";
import type { AnnotationStyleKind, FoliateView } from "./foliate-types";

export interface VocabMarker {
  cfi: string | null;
  mastery: string;
}

export interface WordMarkRule {
  normalized_word: string;
  enabled: boolean;
}

export interface WordMarkException {
  normalized_word: string;
  location: string;
  excluded: boolean;
}

export interface LookupOccurrenceMark {
  location: string;
  enabled: boolean;
}

type MarkerKind = "lookup" | "vocab";
export type FoliateMarker = { color: string; kind: MarkerKind };
type AppliedAnnotation = { color: string; styleKind: AnnotationStyleKind };

export const wordMarkerColor = {
  lookup: "__lookup__",
  vocabNew: "__vocab_new__",
  learning: "__learning__",
  mastered: "__mastered__",
} as const;

const highlightColorMap: Record<string, string> = {
  yellow: "#FBBF24",
  green: "#34D399",
  blue: "#60A5FA",
  pink: "#F472B6",
  purple: "#A78BFA",
};

function drawMarkerRects(
  rects: DOMRectList,
  style: MarkerVisualStyleV1,
  isPdf: boolean,
) {
  const group = document.createElementNS("http://www.w3.org/2000/svg", "g");
  group.setAttribute("fill", style.color);
  for (const { left, top, bottom, height, width } of rects) {
    if (style.background) {
      const background = document.createElementNS("http://www.w3.org/2000/svg", "rect");
      const pad = isPdf ? 1 : 0;
      background.setAttribute("x", String(Math.floor(left)));
      background.setAttribute("y", String(top - pad));
      background.setAttribute("height", String(height + pad * 2));
      background.setAttribute("width", String(Math.ceil(width)));
      background.setAttribute("rx", isPdf ? "1" : "0");
      background.setAttribute("opacity", String(style.opacity / 100));
      group.append(background);
    }
    if (style.underline) {
      const underline = document.createElementNS("http://www.w3.org/2000/svg", "rect");
      underline.setAttribute("x", String(left));
      underline.setAttribute("y", String(bottom - 1.5));
      underline.setAttribute("height", "1.5");
      underline.setAttribute("width", String(width));
      underline.setAttribute("rx", "0.75");
      group.append(underline);
    }
  }
  return group;
}

interface DrawAnnotationDetail {
  draw(renderer: (rects: DOMRectList) => SVGGElement): void;
  annotation: { color: string; styleKind?: AnnotationStyleKind };
}

export function drawFoliateAnnotation(
  { draw, annotation }: DrawAnnotationDetail,
  markerStyle: MarkerStyleConfigV1,
  isPdf: boolean,
) {
  if (annotation.styleKind === "manual" || annotation.styleKind === "automatic") {
    const style = annotation.styleKind === "manual"
      ? markerStyle.manual
      : effectiveAutomaticMarkerStyle(markerStyle);
    draw((rects) => drawMarkerRects(rects, markerOverlayStyle(style), isPdf));
    return;
  }
  if (Object.values(wordMarkerColor).includes(annotation.color as typeof wordMarkerColor[keyof typeof wordMarkerColor])) {
    const color = annotation.color === wordMarkerColor.learning
      ? "#68A68A"
      : annotation.color === wordMarkerColor.vocabNew
        ? "#B78538"
        : annotation.color === wordMarkerColor.mastered ? "#789B8D" : "#8D7C65";
    draw((rects) => {
      const group = document.createElementNS("http://www.w3.org/2000/svg", "g");
      group.setAttribute("fill", color);
      group.setAttribute(
        "opacity",
        annotation.color === wordMarkerColor.lookup
          ? "0.55"
          : annotation.color === wordMarkerColor.mastered ? "0.45" : "0.72",
      );
      for (const { left, top, height, width } of rects) {
        const line = document.createElementNS("http://www.w3.org/2000/svg", "rect");
        line.setAttribute("x", String(left));
        line.setAttribute("y", String(top + height - 1.5));
        line.setAttribute("height", "1.5");
        line.setAttribute("width", String(width));
        line.setAttribute("rx", "0.75");
        group.append(line);
      }
      return group;
    });
    return;
  }
  const color = highlightColorMap[annotation.color] || highlightColorMap.yellow;
  draw((rects) => {
    const group = document.createElementNS("http://www.w3.org/2000/svg", "g");
    group.setAttribute("fill", color);
    group.setAttribute("opacity", "0.35");
    group.style.mixBlendMode = "multiply";
    for (const { left, top, height, width } of rects) {
      const rect = document.createElementNS("http://www.w3.org/2000/svg", "rect");
      const pad = isPdf ? 1 : 0;
      rect.setAttribute("x", String(Math.floor(left)));
      rect.setAttribute("y", String(top - pad));
      rect.setAttribute("height", String(height + pad * 2));
      rect.setAttribute("width", String(Math.ceil(width)));
      rect.setAttribute("rx", isPdf ? "1" : "0");
      group.append(rect);
    }
    return group;
  });
}

interface UseFoliateAnnotationsOptions {
  bookId?: string;
  bookReady: boolean;
  isTextBook: boolean;
  supportsManualAnnotations: boolean;
  supportsWordMarkers: boolean;
  supportsCfiNavigation: boolean;
  supportsReflowSettings: boolean;
  readerSettings: ReaderSettingsState;
  readerSettingsRef: MutableRefObject<ReaderSettingsState>;
  viewRef: MutableRefObject<FoliateView | null>;
  markerStyle: MarkerStyleConfigV1;
  markerStyleRef: MutableRefObject<MarkerStyleConfigV1>;
  markMatchingWordsRef: MutableRefObject<boolean>;
  setMarkerStyle: Dispatch<SetStateAction<MarkerStyleConfigV1>>;
  setReaderSettings: Dispatch<SetStateAction<ReaderSettingsState>>;
  textReaderNavigateRef: MutableRefObject<((location: string, flash?: boolean) => void) | null>;
}

export function useFoliateAnnotations({
  bookId,
  bookReady,
  isTextBook,
  supportsManualAnnotations,
  supportsWordMarkers,
  supportsCfiNavigation,
  supportsReflowSettings,
  readerSettings,
  readerSettingsRef,
  viewRef,
  markerStyle,
  markerStyleRef,
  markMatchingWordsRef,
  setMarkerStyle,
  setReaderSettings,
  textReaderNavigateRef,
}: UseFoliateAnnotationsOptions) {
  const autoMarkersRef = useRef(new Map<string, FoliateMarker>());
  const appliedAnnotationsRef = useRef(new Map<string, AppliedAnnotation>());
  const navigationFlashRef = useRef(new Map<string, number>());
  const markerSnapshotRef = useRef<{
    highlights: Highlight[];
    vocab: VocabMarker[];
    lookupOccurrences: LookupOccurrenceMark[];
  } | null>(null);
  const wordMarkWordsRef = useRef<string[]>([]);
  const wordMarkExceptionsRef = useRef(new Set<string>());

  const applyAnnotations = useCallback(async (reapplyVisible = false) => {
    const view = viewRef.current;
    if (!view || !supportsManualAnnotations) return;
    const snapshot = markerSnapshotRef.current;
    if (!snapshot) return;
    const { highlights, vocab, lookupOccurrences } = snapshot;
    const manual = new Set(highlights.map((highlight) => highlight.cfi_range));
    const settings = readerSettingsRef.current;
    const next = new Map<string, FoliateMarker>();
    if (settings.showLookupMarkers) {
      for (const mark of lookupOccurrences) {
        if (mark.enabled && mark.location && !manual.has(mark.location)) {
          next.set(mark.location, { color: wordMarkerColor.lookup, kind: "lookup" });
        }
      }
    }
    if (supportsWordMarkers) {
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
    const desired = new Map<string, AppliedAnnotation>([...next.entries()].map(([cfi, marker]) => [
      cfi,
      { color: marker.color, styleKind: marker.kind === "lookup" ? "automatic" : "vocab" },
    ]));
    for (const highlight of highlights) {
      desired.set(highlight.cfi_range, { color: highlight.color, styleKind: "manual" });
    }
    const previous = appliedAnnotationsRef.current;
    const cfis = new Set([...previous.keys(), ...desired.keys()]);
    await Promise.all([...cfis].map(async (cfi) => {
      const oldAnnotation = previous.get(cfi);
      const newAnnotation = desired.get(cfi);
      if (!reapplyVisible
        && oldAnnotation?.color === newAnnotation?.color
        && oldAnnotation?.styleKind === newAnnotation?.styleKind) return;
      if (oldAnnotation !== undefined) await view.deleteAnnotation({ value: cfi }).catch(() => {});
      if (newAnnotation !== undefined) {
        await view.addAnnotation({
          value: cfi,
          color: newAnnotation.color,
          styleKind: newAnnotation.styleKind,
        }).catch(() => {});
      }
    }));
    appliedAnnotationsRef.current = desired;
  }, [readerSettingsRef, supportsManualAnnotations, supportsWordMarkers, viewRef]);

  const applyFoliateMarkerStyles = useCallback(() => {
    const view = viewRef.current;
    if (!view || !supportsReflowSettings) return;
    const automaticStyle = effectiveAutomaticMarkerStyle(markerStyleRef.current);
    for (const { doc, index } of view.renderer?.getContents?.() ?? []) {
      if (!doc || typeof index !== "number") continue;
      installCustomFontFacesInDocument(doc);
      applyWordMarkHighlights(
        doc,
        readerSettingsRef.current.showLookupMarkers ? wordMarkWordsRef.current : [],
        "quill-word-marks",
        undefined,
        (word, range) => {
          const location = view.getCFI(index, range);
          return !wordMarkExceptionsRef.current.has(`${word}\0${location}`);
        },
        markerHighlightCss(markerOverlayStyle(automaticStyle)),
      );
    }
  }, [markerStyleRef, readerSettingsRef, supportsReflowSettings, viewRef]);

  const refreshAnnotations = useCallback(async (reapplyVisible = false) => {
    if (isTextBook || !bookId || !viewRef.current || !supportsManualAnnotations) return;
    const [highlights, vocab, lookupOccurrences] = await Promise.all([
      invoke<Highlight[]>("list_highlights", { bookId }),
      supportsWordMarkers
        ? invoke<VocabMarker[]>("list_vocab_words", { bookId })
        : Promise.resolve([]),
      invoke<LookupOccurrenceMark[]>("list_lookup_occurrence_marks", { bookId }),
    ]);
    markerSnapshotRef.current = { highlights, vocab, lookupOccurrences };
    await applyAnnotations(reapplyVisible);
    applyFoliateMarkerStyles();
  }, [
    applyAnnotations,
    applyFoliateMarkerStyles,
    bookId,
    isTextBook,
    supportsManualAnnotations,
    supportsWordMarkers,
    viewRef,
  ]);

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
      const annotation = appliedAnnotationsRef.current.get(cfi);
      if (annotation) await view.addAnnotation({ value: cfi, ...annotation }).catch(() => {});
    }, 3000);
  }, [isTextBook, supportsCfiNavigation, textReaderNavigateRef, viewRef]);

  const resetAnnotationState = useCallback(() => {
    autoMarkersRef.current.clear();
    appliedAnnotationsRef.current.clear();
    navigationFlashRef.current.clear();
    markerSnapshotRef.current = null;
    wordMarkWordsRef.current = [];
    wordMarkExceptionsRef.current.clear();
  }, []);

  useEffect(() => {
    const refreshFonts = async (event: Event) => {
      const records = (event as CustomEvent<CustomFontRecord[]>).detail ?? [];
      const available = new Set(records.map((font) => font.id));
      setReaderSettings((current) => {
        if (!current.font.startsWith("custom-") || available.has(current.font)) return current;
        const next = { ...current, font: "system" };
        readerSettingsRef.current = next;
        if (bookId) localStorage.setItem(`reader-settings-${bookId}`, JSON.stringify(next));
        return next;
      });
      const storedMarkerStyle = await invoke<string | null>("get_setting", {
        key: MARKER_STYLE_SETTING_KEY,
      }).catch(() => null);
      const nextMarkerStyle = parseMarkerStyleConfig(storedMarkerStyle);
      markerStyleRef.current = nextMarkerStyle;
      markMatchingWordsRef.current = nextMarkerStyle.markMatchingWords;
      setMarkerStyle(nextMarkerStyle);
      const view = viewRef.current;
      if (!view) return;
      for (const { doc } of view.renderer?.getContents?.() ?? []) {
        if (doc) installCustomFontFacesInDocument(doc);
      }
      if (supportsReflowSettings) view.renderer?.setStyles?.(getReaderCSS(readerSettingsRef.current));
      applyFoliateMarkerStyles();
    };
    window.addEventListener("custom-font-faces-loaded", refreshFonts);
    return () => window.removeEventListener("custom-font-faces-loaded", refreshFonts);
  }, [
    applyFoliateMarkerStyles,
    bookId,
    markMatchingWordsRef,
    markerStyleRef,
    readerSettingsRef,
    setMarkerStyle,
    setReaderSettings,
    supportsReflowSettings,
    viewRef,
  ]);

  const markerVisibility = [
    readerSettings.showLookupMarkers,
    readerSettings.showNewVocabMarkers,
    readerSettings.showLearningMarkers,
    readerSettings.showMasteredMarkers,
  ].join(":");
  useEffect(() => {
    refreshAnnotations().catch(() => {});
  }, [bookReady, markerVisibility, markerStyle, refreshAnnotations]);

  useEffect(() => {
    if (!bookId || !supportsWordMarkers || isTextBook) return;
    const refresh = (event: Event) => {
      const detail = (event as CustomEvent<{ bookId?: string }>).detail;
      if (detail?.bookId && detail.bookId !== bookId) return;
      Promise.all([
        invoke<WordMarkRule[]>("list_word_marks", { bookId }),
        invoke<WordMarkException[]>("list_word_mark_exceptions", { bookId }),
      ]).then(([rules, exceptions]) => {
        wordMarkWordsRef.current = rules.filter((rule) => rule.enabled).map((rule) => rule.normalized_word);
        wordMarkExceptionsRef.current = new Set(exceptions
          .filter((exception) => exception.excluded)
          .map((exception) => `${exception.normalized_word}\0${exception.location}`));
        applyFoliateMarkerStyles();
      }).catch(() => {});
    };
    window.addEventListener("word-mark-changed", refresh);
    return () => window.removeEventListener("word-mark-changed", refresh);
  }, [applyFoliateMarkerStyles, bookId, isTextBook, supportsWordMarkers]);

  useEffect(() => {
    if (!bookId || isTextBook) return;
    const refresh = (event: Event) => {
      const detail = (event as CustomEvent<{ bookId?: string }>).detail;
      if (!detail?.bookId || detail.bookId === bookId) refreshAnnotations().catch(() => {});
    };
    window.addEventListener("lookup-mark-changed", refresh);
    return () => window.removeEventListener("lookup-mark-changed", refresh);
  }, [bookId, isTextBook, refreshAnnotations]);

  useEffect(() => {
    const refresh = (event: Event) => {
      const detail = (event as CustomEvent<{ bookId?: string }>).detail;
      if (!detail?.bookId || detail.bookId === bookId) refreshAnnotations().catch(() => {});
    };
    window.addEventListener("lookup-record-changed", refresh);
    window.addEventListener("vocab-changed", refresh);
    window.addEventListener("highlight-changed", refresh);
    window.addEventListener("focus", refresh);
    return () => {
      window.removeEventListener("lookup-record-changed", refresh);
      window.removeEventListener("vocab-changed", refresh);
      window.removeEventListener("highlight-changed", refresh);
      window.removeEventListener("focus", refresh);
    };
  }, [bookId, refreshAnnotations]);

  return {
    applyAnnotations,
    applyFoliateMarkerStyles,
    autoMarkersRef,
    flashNavigationTarget,
    refreshAnnotations,
    resetAnnotationState,
    wordMarkExceptionsRef,
    wordMarkWordsRef,
  };
}
