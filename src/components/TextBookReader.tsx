import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getEffectivePageColumns, getFontFamily, getThemeStyles } from "./reader-settings";
import {
  effectiveAutomaticMarkerStyle,
  markerFontFamily,
  markerStyleCss,
  type MarkerStyleConfigV1,
} from "./marker-style";
import type { PageColumns, ReaderSettingsState } from "./ReaderSettings";
import { prefersReducedMotion } from "./page-turn-transition";
import type { Highlight } from "../hooks/useBookmarks";
import {
  classifySelection,
  contextForRange,
  expandRangeToWordBoundaries,
  isInteractiveReaderTarget,
  normalizeInteractionText,
  rangeFromSelectionSnapshotAtPoint,
  replaceDocumentSelection,
  selectedRange,
  snapshotSelectionRange,
  viewportRectForRange,
  wordRangeAtPoint,
  type ReaderInteraction,
  type ReaderSelectionSnapshot,
} from "./reader-interaction";
import { bindingFromKeyboardEvent } from "./reader-bindings";
import { expandWordForms } from "./word-forms";
import {
  resolveTextLocation,
  textLocation,
  type TextBookBlock,
  type TextBookDocument,
} from "./text-book-location";

interface TextBookReaderProps {
  bookId: string;
  initialLocation?: string | null;
  settings: ReaderSettingsState;
  onReady: (document: TextBookDocument) => void;
  onProgress: (
    progress: number,
    location: string,
    tocIndex: number,
    details?: TextBookProgressDetails,
  ) => void;
  onInteraction: (interaction: ReaderInteraction) => void;
  onError: (error: string) => void;
  onRegisterNavigation: (navigate: (location: string, flash?: boolean) => void) => void;
  onRegisterPageNavigation?: (navigation: TextBookPageNavigation) => void;
  onHighlightClick: (highlight: Highlight, rect: DOMRect, fallbackText?: string) => void;
  doubleClickQuickLookup?: boolean;
  markerStyle: MarkerStyleConfigV1;
  onReaderBinding?: (trigger: string, interaction: ReaderInteraction | null) => boolean;
}

export interface TextBookPageNavigation {
  prev: () => void;
  next: () => void;
}

export interface TextBookProgressDetails {
  chapterProgress: number;
  page?: {
    current: number;
    visibleEnd: number;
    total: number;
  };
}

interface WordMarkRule {
  normalized_word: string;
  enabled: boolean;
}

interface WordMarkException {
  normalized_word: string;
  location: string;
  excluded: boolean;
}

interface LookupOccurrenceMark {
  location: string;
  enabled: boolean;
}

function textOffsetInBlock(element: HTMLElement, node: Node, offset: number): number {
  if (node !== element && !element.contains(node)) return 0;
  const range = document.createRange();
  range.selectNodeContents(element);
  try {
    range.setEnd(node, offset);
    return range.toString().length;
  } catch {
    return 0;
  }
}

interface DomTextPosition {
  node: Node;
  offset: number;
}

function domPositionAtRenderedOffset(element: HTMLElement, renderedOffset: number): DomTextPosition {
  const targetOffset = Math.min(Math.max(0, renderedOffset), element.textContent?.length ?? 0);
  const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
  let remaining = targetOffset;
  let lastTextNode: Node | null = null;
  let current = walker.nextNode();
  while (current) {
    const length = current.textContent?.length ?? 0;
    if (remaining <= length) return { node: current, offset: remaining };
    remaining -= length;
    lastTextNode = current;
    current = walker.nextNode();
  }
  return lastTextNode
    ? { node: lastTextNode, offset: lastTextNode.textContent?.length ?? 0 }
    : { node: element, offset: 0 };
}

function usableRangeRect(range: Range, last = false): DOMRect | null {
  const rects = Array.from(range.getClientRects()).filter((rect) => (
    Number.isFinite(rect.top) && Number.isFinite(rect.bottom) && rect.height > 0
  ));
  return (last ? rects[rects.length - 1] : rects[0]) ?? null;
}

function rectAtRenderedOffset(element: HTMLElement, renderedOffset: number): DOMRect | null {
  const length = element.textContent?.length ?? 0;
  const offset = Math.min(Math.max(0, renderedOffset), length);
  const range = document.createRange();

  // Selecting the character at the offset gives a stable line box at soft-wrap
  // boundaries, where a collapsed range can resolve to the preceding line.
  for (let candidate = offset; candidate < Math.min(length, offset + 8); candidate += 1) {
    const start = domPositionAtRenderedOffset(element, candidate);
    const end = domPositionAtRenderedOffset(element, candidate + 1);
    range.setStart(start.node, start.offset);
    range.setEnd(end.node, end.offset);
    const rect = usableRangeRect(range);
    if (rect) return rect;
  }

  for (let candidate = offset; candidate > Math.max(0, offset - 8); candidate -= 1) {
    const start = domPositionAtRenderedOffset(element, candidate - 1);
    const end = domPositionAtRenderedOffset(element, candidate);
    range.setStart(start.node, start.offset);
    range.setEnd(end.node, end.offset);
    const rect = usableRangeRect(range, true);
    if (rect) return rect;
  }

  const position = domPositionAtRenderedOffset(element, offset);
  range.setStart(position.node, position.offset);
  range.collapse(true);
  return usableRangeRect(range);
}

function domPositionFromPoint(x: number, y: number): DomTextPosition | null {
  const caretDocument = document as Document & {
    caretPositionFromPoint?: (
      x: number,
      y: number,
    ) => { offsetNode: Node; offset: number } | null;
    caretRangeFromPoint?: (x: number, y: number) => Range | null;
  };
  const caretPosition = caretDocument.caretPositionFromPoint?.(x, y);
  if (caretPosition) return { node: caretPosition.offsetNode, offset: caretPosition.offset };
  const caretRange = caretDocument.caretRangeFromPoint?.(x, y);
  return caretRange
    ? { node: caretRange.startContainer, offset: caretRange.startOffset }
    : null;
}

function renderedOffsetFromCaretPoint(element: HTMLElement, x: number, y: number): number | null {
  const position = domPositionFromPoint(x, y);
  if (position && (position.node === element || element.contains(position.node))) {
    return textOffsetInBlock(element, position.node, position.offset);
  }
  return null;
}

function renderedOffsetNearPoint(element: HTMLElement, x: number, y: number): number {
  const caretOffset = renderedOffsetFromCaretPoint(element, x, y);
  const length = element.textContent?.length ?? 0;
  if (caretOffset !== null) return Math.min(Math.max(0, caretOffset), length);
  if (length === 0) return 0;

  // Range geometry is the fallback for engines that do not expose a caret API.
  // Find the first rendered character whose line box reaches the sample height.
  let low = 0;
  let high = length - 1;
  let bestOffset = 0;
  let bestDistance = Number.POSITIVE_INFINITY;
  while (low <= high) {
    const middle = Math.floor((low + high) / 2);
    const rect = rectAtRenderedOffset(element, middle);
    if (!rect) break;
    const distance = y < rect.top ? rect.top - y : y > rect.bottom ? y - rect.bottom : 0;
    if (distance < bestDistance || (distance === bestDistance && middle < bestOffset)) {
      bestOffset = middle;
      bestDistance = distance;
    }
    if (rect.bottom <= y) low = middle + 1;
    else high = middle - 1;
  }
  return bestOffset;
}

function scrollRenderedOffsetIntoView(
  container: HTMLElement,
  element: HTMLElement,
  renderedOffset: number,
  behavior: ScrollBehavior,
) {
  const targetRect = rectAtRenderedOffset(element, renderedOffset);
  if (!targetRect) {
    element.scrollIntoView({ behavior, block: "center" });
    return;
  }
  const containerRect = container.getBoundingClientRect();
  const targetTop = container.scrollTop + targetRect.top - containerRect.top;
  const centeredTop = targetTop - (container.clientHeight - targetRect.height) / 2;
  const maximumTop = Math.max(0, container.scrollHeight - container.clientHeight);
  container.scrollTo({
    top: Math.min(maximumTop, Math.max(0, centeredTop)),
    behavior,
  });
}

function blockFromNode(node: Node | null): HTMLElement | null {
  const element = node?.nodeType === Node.ELEMENT_NODE
    ? node as HTMLElement
    : node?.parentElement;
  return element?.closest<HTMLElement>("[data-text-source-start][data-text-source-end]") ?? null;
}

function blockRange(element: HTMLElement): { start: number; end: number } | null {
  const start = Number(element.dataset.textSourceStart);
  const end = Number(element.dataset.textSourceEnd);
  if (!Number.isSafeInteger(start) || !Number.isSafeInteger(end)) return null;
  return { start, end };
}

function sourceOffsetToRenderedOffset(block: TextBookBlock, sourceOffset: number): number {
  let previousRenderedEnd = 0;
  for (const span of block.source_spans) {
    const sourceEnd = span.source_start + span.length;
    if (sourceOffset < span.source_start) return previousRenderedEnd;
    if (sourceOffset <= sourceEnd) {
      return span.rendered_start + Math.min(span.length, sourceOffset - span.source_start);
    }
    previousRenderedEnd = span.rendered_start + span.length;
  }
  return block.text.length;
}

function renderedOffsetToSourceOffset(
  block: TextBookBlock,
  renderedOffset: number,
  boundary: "start" | "end",
): number {
  for (let index = 0; index < block.source_spans.length; index += 1) {
    const span = block.source_spans[index];
    const renderedEnd = span.rendered_start + span.length;
    if (renderedOffset < span.rendered_start) return span.source_start;
    if (renderedOffset < renderedEnd) {
      return span.source_start + renderedOffset - span.rendered_start;
    }
    if (renderedOffset === renderedEnd) {
      const next = block.source_spans[index + 1];
      if (boundary === "start" && next?.rendered_start === renderedOffset) return next.source_start;
      return span.source_start + span.length;
    }
  }
  return block.source_end;
}

function formatError(error: unknown) {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return String(error);
}

function renderHighlightedBlock(
  block: TextBookBlock,
  document: TextBookDocument,
  highlights: Highlight[],
  onHighlightClick: (highlight: Highlight, rect: DOMRect, fallbackText?: string) => void,
  markerStyle: MarkerStyleConfigV1,
  readerFontFamily: string,
  automaticWords: Set<string>,
  automaticExceptions: Set<string>,
  lookupOccurrenceMarks: LookupOccurrenceMark[],
): ReactNode[] {
  const manualRanges = highlights
    .flatMap((highlight, priority) => {
      const location = resolveTextLocation(highlight.cfi_range, document);
      if (!location || location.end <= block.source_start || location.start >= block.source_end) return [];
      const start = sourceOffsetToRenderedOffset(block, Math.max(location.start, block.source_start));
      const end = sourceOffsetToRenderedOffset(block, Math.min(location.end, block.source_end));
      return end > start ? [{ highlight, start, end, priority }] : [];
    })
    .sort((a, b) => a.start - b.start || a.end - b.end || a.priority - b.priority);

  const wholeBookAutomaticRanges = automaticWords.size === 0
    ? []
    : lexicalSegmentsForBlock(block).flatMap((segment) => {
        const word = normalizeInteractionText(segment.segment);
        if (!automaticWords.has(word)) return [];
        const end = segment.index + segment.segment.length;
        const location = textLocation(
          renderedOffsetToSourceOffset(block, segment.index, "start"),
          renderedOffsetToSourceOffset(block, end, "end"),
        );
        return automaticExceptions.has(`${word}\0${location}`) ? [] : [{ start: segment.index, end }];
      });
  const occurrenceAutomaticRanges = lookupOccurrenceMarks.flatMap((mark) => {
    if (!mark.enabled) return [];
    const location = resolveTextLocation(mark.location, document);
    if (!location || location.end <= block.source_start || location.start >= block.source_end) return [];
    const start = sourceOffsetToRenderedOffset(block, Math.max(location.start, block.source_start));
    const end = sourceOffsetToRenderedOffset(block, Math.min(location.end, block.source_end));
    return end > start ? [{ start, end }] : [];
  });
  const automaticRanges = [
    ...wholeBookAutomaticRanges,
    ...occurrenceAutomaticRanges,
  ].filter((range, index, ranges) => (
    ranges.findIndex((candidate) => candidate.start === range.start && candidate.end === range.end) === index
  ));

  const ranges = [
    ...manualRanges.map((range) => ({ ...range, kind: "manual" as const })),
    ...automaticRanges.map((range, priority) => ({
      ...range,
      highlight: null,
      priority: manualRanges.length + priority,
      kind: "automatic" as const,
    })),
  ].sort((a, b) => a.start - b.start || a.end - b.end || a.priority - b.priority);

  if (ranges.length === 0) return [block.text];

  // Split at every boundary instead of advancing a single cursor per stored
  // range. The old renderer skipped an entire later range when its start was
  // inside an earlier one, including the non-overlapping text after it.
  const boundaries = [...new Set(ranges.flatMap((range) => [range.start, range.end]))]
    .sort((left, right) => left - right);
  const segments: Array<{ start: number; end: number; highlight: Highlight | null; kind: "manual" | "automatic" | null }> = [];
  let previous = 0;
  for (let index = 0; index < boundaries.length - 1; index += 1) {
    const start = boundaries[index];
    const end = boundaries[index + 1];
    if (start > previous) segments.push({ start: previous, end: start, highlight: null, kind: null });
    const active = ranges
      .filter((range) => range.start < end && range.end > start)
      .sort((left, right) => left.priority - right.priority)[0];
    segments.push({ start, end, highlight: active?.highlight ?? null, kind: active?.kind ?? null });
    previous = end;
  }
  if (previous < block.text.length) {
    segments.push({ start: previous, end: block.text.length, highlight: null, kind: null });
  }

  const coalesced = segments.reduce<typeof segments>((result, segment) => {
    if (segment.end <= segment.start) return result;
    const last = result[result.length - 1];
    if (last?.end === segment.start
      && last.kind === segment.kind
      && last.highlight?.id === segment.highlight?.id) {
      last.end = segment.end;
    } else {
      result.push({ ...segment });
    }
    return result;
  }, []);

  const nodes: ReactNode[] = [];
  for (const segment of coalesced) {
    if (segment.kind === "automatic") {
      const automaticStyle = effectiveAutomaticMarkerStyle(markerStyle);
      nodes.push(
        <span
          key={`automatic:${segment.start}:${segment.end}`}
          style={markerStyleCss(
            automaticStyle,
            markerFontFamily(automaticStyle.font, readerFontFamily),
          )}
        >
          {block.text.slice(segment.start, segment.end)}
        </span>,
      );
      continue;
    }
    if (!segment.highlight) {
      nodes.push(block.text.slice(segment.start, segment.end));
      continue;
    }
    nodes.push(
      <mark
        key={`${segment.highlight.id}:${segment.start}:${segment.end}`}
        className="cursor-pointer"
        style={{
          ...markerStyleCss(
            markerStyle.manual,
            markerFontFamily(markerStyle.manual.font, readerFontFamily),
          ),
        }}
        onClick={(event) => {
          event.stopPropagation();
          onHighlightClick(
            segment.highlight!,
            event.currentTarget.getBoundingClientRect(),
            event.currentTarget.textContent ?? undefined,
          );
        }}
      >
        {block.text.slice(segment.start, segment.end)}
      </mark>,
    );
  }
  return nodes;
}

interface LexicalSegment {
  segment: string;
  index: number;
  isWordLike?: boolean;
}

const Segmenter = (Intl as typeof Intl & {
  Segmenter?: new (locale?: string, options?: { granularity: "word" }) => {
    segment(value: string): Iterable<LexicalSegment>;
  };
}).Segmenter;
const wordSegmenter = Segmenter ? new Segmenter(undefined, { granularity: "word" }) : null;
const lexicalSegmentCache = new WeakMap<TextBookBlock, LexicalSegment[]>();

function lexicalSegmentsForBlock(block: TextBookBlock) {
  const cached = lexicalSegmentCache.get(block);
  if (cached) return cached;
  const segments = wordSegmenter
    ? Array.from(wordSegmenter.segment(block.text))
      .filter((segment) => segment.isWordLike)
    : Array.from(block.text.matchAll(/[\p{L}\p{M}\p{N}]+(?:['’][\p{L}\p{M}\p{N}]+)*/gu))
      .map((match) => ({ segment: match[0], index: match.index ?? 0 }));
  lexicalSegmentCache.set(block, segments);
  return segments;
}

function tocIndexAtOffset(document: TextBookDocument, sourceOffset: number) {
  let low = 0;
  let high = document.toc.length - 1;
  let current = 0;
  while (low <= high) {
    const middle = Math.floor((low + high) / 2);
    if (document.toc[middle].source_offset <= sourceOffset) {
      current = middle;
      low = middle + 1;
    } else {
      high = middle - 1;
    }
  }
  return current;
}

function chapterProgressAtOffset(
  document: TextBookDocument,
  tocIndex: number,
  sourceOffset: number,
  sourceEnd: number,
) {
  const chapterStart = document.toc[tocIndex]?.source_offset ?? 0;
  let chapterEnd = sourceEnd;
  for (let index = tocIndex + 1; index < document.toc.length; index += 1) {
    if (document.toc[index].source_offset > chapterStart) {
      chapterEnd = document.toc[index].source_offset;
      break;
    }
  }
  const fraction = (sourceOffset - chapterStart) / Math.max(1, chapterEnd - chapterStart);
  return Math.min(100, Math.max(0, Math.round(fraction * 100)));
}

interface RenderedBlockEntry {
  element: HTMLElement;
  block: TextBookBlock;
  top: number;
  bottom: number;
}

function blockIndexAtOffset(blocks: TextBookBlock[], sourceOffset: number) {
  let low = 0;
  let high = blocks.length - 1;
  let result = blocks.length - 1;
  while (low <= high) {
    const middle = Math.floor((low + high) / 2);
    if (blocks[middle].source_end > sourceOffset) {
      result = middle;
      high = middle - 1;
    } else {
      low = middle + 1;
    }
  }
  return Math.max(0, result);
}

function visibleBlockIndex(entries: RenderedBlockEntry[], targetTop: number) {
  let low = 0;
  let high = entries.length - 1;
  let result = 0;
  while (low <= high) {
    const middle = Math.floor((low + high) / 2);
    if (entries[middle].bottom > targetTop) {
      result = middle;
      high = middle - 1;
    } else {
      low = middle + 1;
    }
  }
  return result;
}

function TextBookReader({
  bookId,
  initialLocation,
  settings,
  onReady,
  onProgress,
  onInteraction,
  onError,
  onRegisterNavigation,
  onRegisterPageNavigation,
  onHighlightClick,
  doubleClickQuickLookup = true,
  markerStyle,
  onReaderBinding,
}: TextBookReaderProps) {
  const [document, setDocument] = useState<TextBookDocument | null>(null);
  const [highlights, setHighlights] = useState<Highlight[]>([]);
  const [wordMarks, setWordMarks] = useState<WordMarkRule[]>([]);
  const [wordMarkExceptions, setWordMarkExceptions] = useState<WordMarkException[]>([]);
  const [lookupOccurrenceMarks, setLookupOccurrenceMarks] = useState<LookupOccurrenceMark[]>([]);
  const [wordFormWords, setWordFormWords] = useState<string[]>([]);
  const [preparationPending, setPreparationPending] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);
  const articleRef = useRef<HTMLElement>(null);
  const renderedBlocksRef = useRef<RenderedBlockEntry[]>([]);
  const renderedElementsByStartRef = useRef(new Map<number, HTMLElement>());
  const layoutFrameRef = useRef<number | null>(null);
  const scrollFrameRef = useRef<number | null>(null);
  const paginationSettleTimerRef = useRef<number | null>(null);
  const initialLocationRef = useRef(initialLocation);
  const progressReadyRef = useRef(false);
  const loadGenerationRef = useRef(0);
  const loadActiveRef = useRef(false);
  const loadInFlightRef = useRef<Promise<void> | null>(null);
  const loadedGenerationRef = useRef<number | null>(null);
  const flashTimerRef = useRef<number | null>(null);
  const wordClickTimerRef = useRef<number | null>(null);
  const selectionMenuTimerRef = useRef<number | null>(null);
  const activePointerIdRef = useRef<number | null>(null);
  const pointerCaptureTargetRef = useRef<HTMLElement | null>(null);
  const selectionNormalizationUntilRef = useRef(0);
  const forceClickSuppressedUntilRef = useRef(0);
  const doubleClickSelectionRef = useRef<ReaderSelectionSnapshot | null>(null);
  const [flashOffset, setFlashOffset] = useState<number | null>(null);
  const isPaginated = settings.readingMode === "paginated";
  const [effectivePageColumns, setEffectivePageColumns] = useState<PageColumns>(() => (
    isPaginated ? settings.pageColumns : 1
  ));
  const pageTurnAnimation = settings.pageTurnAnimation;
  const documentBlocks = useMemo(
    () => document?.chunks.flatMap((chunk) => chunk.blocks) ?? [],
    [document],
  );
  const documentBlocksByStart = useMemo(
    () => new Map(documentBlocks.map((block) => [block.source_start, block])),
    [documentBlocks],
  );
  const firstDocumentBlockStart = documentBlocks[0]?.source_start;
  const readerFontFamily = getFontFamily(settings.font);
  const automaticWordSet = useMemo(
    () => new Set(settings.showLookupMarkers
      ? [
          ...wordMarks.map((rule) => normalizeInteractionText(rule.normalized_word)),
          ...(markerStyle.wordMatchScope === "forms" ? wordFormWords : []),
        ]
      : []),
    [markerStyle.wordMatchScope, settings.showLookupMarkers, wordFormWords, wordMarks],
  );
  const automaticExceptionSet = useMemo(
    () => new Set(wordMarkExceptions.map((exception) => `${exception.normalized_word}\0${exception.location}`)),
    [wordMarkExceptions],
  );
  const visibleLookupOccurrenceMarks = useMemo(
    () => settings.showLookupMarkers ? lookupOccurrenceMarks : [],
    [lookupOccurrenceMarks, settings.showLookupMarkers],
  );

  const updateEffectivePageColumns = useCallback(() => {
    const container = containerRef.current;
    if (!container) return;
    const next = getEffectivePageColumns(
      { readingMode: settings.readingMode, pageColumns: settings.pageColumns },
      container.clientWidth,
      container.clientHeight,
    );
    setEffectivePageColumns((current) => current === next ? current : next);
  }, [settings.pageColumns, settings.readingMode]);

  useLayoutEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    updateEffectivePageColumns();
    const observer = new ResizeObserver(updateEffectivePageColumns);
    observer.observe(container);
    return () => observer.disconnect();
  }, [updateEffectivePageColumns]);

  const refreshHighlights = useCallback(async () => {
    const generation = loadGenerationRef.current;
    const result = await invoke<Highlight[]>("list_highlights", { bookId });
    if (!loadActiveRef.current || generation !== loadGenerationRef.current) return;
    setHighlights(result.filter((highlight) => highlight.cfi_range.startsWith("textloc:")));
  }, [bookId]);

  const refreshWordMarks = useCallback(async () => {
    const generation = loadGenerationRef.current;
    const [rules, exceptions, occurrences] = await Promise.all([
      invoke<WordMarkRule[]>("list_word_marks", { bookId }),
      invoke<WordMarkException[]>("list_word_mark_exceptions", { bookId }),
      invoke<LookupOccurrenceMark[]>("list_lookup_occurrence_marks", { bookId }),
    ]);
    if (!loadActiveRef.current || generation !== loadGenerationRef.current) return;
    setWordMarks(rules.filter((rule) => rule.enabled));
    setWordFormWords(await expandWordForms(
      rules.filter((rule) => rule.enabled).map((rule) => rule.normalized_word),
      markerStyle.wordMatchScope === "forms",
    ));
    setWordMarkExceptions(exceptions.filter((exception) => exception.excluded));
    setLookupOccurrenceMarks(occurrences.filter((mark) => mark.enabled));
  }, [bookId, markerStyle.wordMatchScope]);

  const loadDocument = useCallback(() => {
    if (!loadActiveRef.current) return Promise.resolve();
    if (loadedGenerationRef.current === loadGenerationRef.current) return Promise.resolve();
    if (loadInFlightRef.current) return loadInFlightRef.current;
    const generation = loadGenerationRef.current;
    const request = (async () => {
      try {
        const result = await invoke<TextBookDocument>("get_text_book_document", { bookId });
        if (!loadActiveRef.current || generation !== loadGenerationRef.current) return;
        setPreparationPending(false);
        loadedGenerationRef.current = generation;
        progressReadyRef.current = false;
        setDocument(result);
        onReady(result);
        await Promise.allSettled([refreshHighlights(), refreshWordMarks()]);
      } catch (error) {
        if (!loadActiveRef.current || generation !== loadGenerationRef.current) return;
        const message = formatError(error);
        if (message.includes("TEXT_PREPARATION_PENDING")) setPreparationPending(true);
        else {
          setPreparationPending(false);
          onError(message);
        }
      }
    })().finally(() => {
      if (loadInFlightRef.current === request) loadInFlightRef.current = null;
    });
    loadInFlightRef.current = request;
    return request;
  }, [bookId, onError, onReady, refreshHighlights, refreshWordMarks]);

  useEffect(() => {
    initialLocationRef.current = initialLocation;
  }, [bookId, initialLocation]);

  useEffect(() => {
    loadGenerationRef.current += 1;
    loadActiveRef.current = true;
    loadInFlightRef.current = null;
    loadedGenerationRef.current = null;
    setDocument(null);
    setPreparationPending(false);
    progressReadyRef.current = false;
    loadDocument().catch(() => {});
    return () => {
      loadActiveRef.current = false;
      loadGenerationRef.current += 1;
      loadInFlightRef.current = null;
      loadedGenerationRef.current = null;
    };
  }, [bookId, loadDocument]);

  useEffect(() => {
    if (!preparationPending) return;
    const timer = window.setInterval(() => loadDocument().catch(() => {}), 750);
    return () => window.clearInterval(timer);
  }, [loadDocument, preparationPending]);

  useEffect(() => {
    const generation = loadGenerationRef.current;
    const unlisten = listen<{ book_id?: string; state?: string }>("book-preparation-changed", (event) => {
      if (event.payload.book_id !== bookId) return;
      if (!loadActiveRef.current || generation !== loadGenerationRef.current) return;
      if (event.payload.state === "ready") loadDocument().catch(() => {});
      if (event.payload.state === "failed") {
        setPreparationPending(false);
        onError("TEXT_PREPARATION_FAILED");
      }
    });
    const refresh = (event: Event) => {
      const detail = (event as CustomEvent<{ bookId?: string }>).detail;
      if (!detail?.bookId || detail.bookId === bookId) refreshHighlights().catch(() => {});
    };
    const refreshMarks = (event: Event) => {
      const detail = (event as CustomEvent<{ bookId?: string }>).detail;
      if (!detail?.bookId || detail.bookId === bookId) refreshWordMarks().catch(() => {});
    };
    window.addEventListener("highlight-changed", refresh);
    window.addEventListener("word-mark-changed", refreshMarks);
    window.addEventListener("lookup-mark-changed", refreshMarks);
    window.addEventListener("word-forms-changed", refreshMarks);
    return () => {
      unlisten.then((stop) => stop());
      window.removeEventListener("highlight-changed", refresh);
      window.removeEventListener("word-mark-changed", refreshMarks);
      window.removeEventListener("lookup-mark-changed", refreshMarks);
      window.removeEventListener("word-forms-changed", refreshMarks);
    };
  }, [bookId, loadDocument, onError, refreshHighlights, refreshWordMarks]);

  const updateRenderedBlockCache = useCallback(() => {
    const container = containerRef.current;
    const article = articleRef.current;
    if (!container || !article) return;

    const marginPercent = Math.min(30, Math.max(0, settings.margins));
    if (isPaginated) {
      if (container.scrollTop !== 0) container.scrollTop = 0;
      const pageSlotWidth = container.clientWidth / effectivePageColumns;
      const pageMargin = Math.max(12, pageSlotWidth * marginPercent / 100);
      // The live reader viewport is the hard upper bound. A fixed minimum here
      // would make narrow two-page layouts wider than their allocated slot.
      const columnWidth = Math.max(1, pageSlotWidth - pageMargin * 2);
      const columnGap = pageMargin * 2;
      const widthValue = `${columnWidth}px`;
      const gapValue = `${columnGap}px`;
      if (article.style.columnWidth !== widthValue) article.style.columnWidth = widthValue;
      if (article.style.columnGap !== gapValue) article.style.columnGap = gapValue;
    } else {
      if (container.scrollLeft !== 0) container.scrollLeft = 0;
      article.style.removeProperty("column-width");
      article.style.removeProperty("column-gap");
    }

    const elements = Array.from(article.querySelectorAll<HTMLElement>(
      "[data-text-source-start][data-text-source-end]",
    ));
    const byStart = new Map<number, HTMLElement>();
    const containerRect = container.getBoundingClientRect();
    const scrollTop = container.scrollTop;
    const entries = elements.flatMap((element) => {
      const start = Number(element.dataset.textSourceStart);
      const block = documentBlocksByStart.get(start);
      if (!block) return [];
      byStart.set(start, element);
      if (isPaginated) return [{ element, block, top: 0, bottom: 0 }];
      const rect = element.getBoundingClientRect();
      return [{
        element,
        block,
        top: rect.top - containerRect.top + scrollTop,
        bottom: rect.bottom - containerRect.top + scrollTop,
      }];
    });
    renderedElementsByStartRef.current = byStart;
    renderedBlocksRef.current = entries;
  }, [documentBlocksByStart, effectivePageColumns, isPaginated, settings.margins]);

  const scheduleRenderedBlockCacheUpdate = useCallback(() => {
    if (layoutFrameRef.current !== null) return;
    layoutFrameRef.current = requestAnimationFrame(() => {
      layoutFrameRef.current = null;
      updateRenderedBlockCache();
    });
  }, [updateRenderedBlockCache]);

  useEffect(() => {
    if (!document || !containerRef.current || !articleRef.current) return;
    scheduleRenderedBlockCacheUpdate();
    const observer = new ResizeObserver(scheduleRenderedBlockCacheUpdate);
    observer.observe(containerRef.current);
    observer.observe(articleRef.current);
    window.document.fonts?.addEventListener?.("loadingdone", scheduleRenderedBlockCacheUpdate);
    window.addEventListener("resize", scheduleRenderedBlockCacheUpdate);
    return () => {
      observer.disconnect();
      window.document.fonts?.removeEventListener?.("loadingdone", scheduleRenderedBlockCacheUpdate);
      window.removeEventListener("resize", scheduleRenderedBlockCacheUpdate);
      if (layoutFrameRef.current !== null) {
        cancelAnimationFrame(layoutFrameRef.current);
        layoutFrameRef.current = null;
      }
      renderedBlocksRef.current = [];
      renderedElementsByStartRef.current.clear();
    };
  }, [document, scheduleRenderedBlockCacheUpdate]);

  const paginationInfo = useCallback(() => {
    const container = containerRef.current;
    if (!container || !isPaginated) return undefined;
    const viewportWidth = Math.max(1, container.clientWidth);
    const pageSlotWidth = viewportWidth / effectivePageColumns;
    const total = Math.max(1, Math.ceil((container.scrollWidth - 1) / pageSlotWidth));
    const current = Math.min(total, Math.max(1, Math.round(container.scrollLeft / pageSlotWidth) + 1));
    return {
      current,
      visibleEnd: Math.min(total, current + effectivePageColumns - 1),
      total,
    };
  }, [effectivePageColumns, isPaginated]);

  const scrollToSpread = useCallback((spreadIndex: number, behavior?: ScrollBehavior) => {
    const container = containerRef.current;
    if (!container || !isPaginated) return;
    const viewportWidth = Math.max(1, container.clientWidth);
    const maximumIndex = Math.max(0, Math.ceil(container.scrollWidth / viewportWidth) - 1);
    const targetIndex = Math.min(maximumIndex, Math.max(0, spreadIndex));
    container.scrollTo({
      left: targetIndex * viewportWidth,
      behavior: behavior ?? (
        pageTurnAnimation === "slide" && !prefersReducedMotion()
          ? "smooth"
          : "auto"
      ),
    });
  }, [isPaginated, pageTurnAnimation]);

  const navigateByPage = useCallback((direction: -1 | 1) => {
    const container = containerRef.current;
    if (!container) return;
    if (isPaginated) {
      const currentSpread = Math.round(container.scrollLeft / Math.max(1, container.clientWidth));
      scrollToSpread(currentSpread + direction);
      return;
    }
    container.scrollBy({
      top: direction * Math.max(1, container.clientHeight - 64),
      behavior: pageTurnAnimation === "slide" && !prefersReducedMotion() ? "smooth" : "auto",
    });
  }, [isPaginated, pageTurnAnimation, scrollToSpread]);

  useEffect(() => {
    onRegisterPageNavigation?.({
      prev: () => navigateByPage(-1),
      next: () => navigateByPage(1),
    });
  }, [navigateByPage, onRegisterPageNavigation]);

  const navigateToLocation = useCallback((
    location: string,
    flash = false,
    behavior: ScrollBehavior = "smooth",
  ) => {
    if (!document || !containerRef.current) return null;
    const resolved = resolveTextLocation(location, document);
    if (!resolved) return null;
    if (renderedElementsByStartRef.current.size === 0) updateRenderedBlockCache();
    const targetBlock = documentBlocks[blockIndexAtOffset(documentBlocks, resolved.start)];
    const target = targetBlock
      ? renderedElementsByStartRef.current.get(targetBlock.source_start)
      : undefined;
    if (!target) return null;
    const renderedOffset = targetBlock
      ? sourceOffsetToRenderedOffset(targetBlock, resolved.start)
      : 0;
    initialLocationRef.current = textLocation(resolved.start);
    const effectiveBehavior = behavior === "smooth" && prefersReducedMotion() ? "auto" : behavior;
    if (isPaginated) {
      const targetRect = rectAtRenderedOffset(target, renderedOffset) ?? target.getBoundingClientRect();
      const containerRect = containerRef.current.getBoundingClientRect();
      const viewportWidth = Math.max(1, containerRef.current.clientWidth);
      const pageSlotWidth = viewportWidth / effectivePageColumns;
      const targetLeft = containerRef.current.scrollLeft + targetRect.left - containerRect.left;
      const physicalPage = Math.max(0, Math.floor(targetLeft / Math.max(1, pageSlotWidth)));
      scrollToSpread(Math.floor(physicalPage / effectivePageColumns), effectiveBehavior);
    } else {
      scrollRenderedOffsetIntoView(containerRef.current, target, renderedOffset, effectiveBehavior);
    }
    if (flash) {
      setFlashOffset(resolved.start);
      if (flashTimerRef.current !== null) window.clearTimeout(flashTimerRef.current);
      flashTimerRef.current = window.setTimeout(() => setFlashOffset(null), 3000);
    }
    return resolved;
  }, [
    document,
    documentBlocks,
    effectivePageColumns,
    isPaginated,
    scrollToSpread,
    updateRenderedBlockCache,
  ]);

  useEffect(() => {
    const container = containerRef.current;
    if (!document || !container || !isPaginated) return;
    let reanchorTimer: number | null = null;
    const observer = new ResizeObserver(() => {
      if (reanchorTimer !== null) window.clearTimeout(reanchorTimer);
      reanchorTimer = window.setTimeout(() => {
        reanchorTimer = null;
        const location = initialLocationRef.current;
        if (location) navigateToLocation(location, false, "auto");
      }, 180);
    });
    observer.observe(container);
    return () => {
      observer.disconnect();
      if (reanchorTimer !== null) window.clearTimeout(reanchorTimer);
    };
  }, [document, isPaginated, navigateToLocation]);

  useEffect(() => {
    onRegisterNavigation(navigateToLocation);
    return () => {
      if (flashTimerRef.current !== null) window.clearTimeout(flashTimerRef.current);
      if (wordClickTimerRef.current !== null) window.clearTimeout(wordClickTimerRef.current);
    };
  }, [navigateToLocation, onRegisterNavigation]);

  useEffect(() => {
    if (!document) return;
    let cancelled = false;
    let settleFrame = 0;
    const frame = requestAnimationFrame(() => {
      const location = initialLocationRef.current;
      const restored = location ? navigateToLocation(location, false, "auto") : null;
      // Scrolling can dispatch synchronously. Enable persistence only after
      // the restoration scroll and its first scroll event have completed.
      settleFrame = requestAnimationFrame(() => {
        if (cancelled) return;
        if (restored) {
          const sourceEnd = documentBlocks[documentBlocks.length - 1]?.source_end ?? 0;
          const sourceOffset = Math.min(Math.max(0, restored.start), sourceEnd);
          const normalizedLocation = textLocation(sourceOffset);
          initialLocationRef.current = normalizedLocation;
          const progress = Math.round((sourceOffset / Math.max(1, sourceEnd)) * 100);
          const tocIndex = tocIndexAtOffset(document, sourceOffset);
          onProgress(
            Math.min(100, Math.max(0, progress)),
            normalizedLocation,
            tocIndex,
            {
              chapterProgress: chapterProgressAtOffset(document, tocIndex, sourceOffset, sourceEnd),
              page: paginationInfo(),
            },
          );
        }
        progressReadyRef.current = true;
      });
    });
    return () => {
      cancelled = true;
      cancelAnimationFrame(frame);
      if (settleFrame) cancelAnimationFrame(settleFrame);
    };
  }, [document, documentBlocks, navigateToLocation, onProgress, paginationInfo]);

  const reportScrollProgress = useCallback(() => {
    const container = containerRef.current;
    if (!container || !document || !progressReadyRef.current) return;
    const containerRect = container.getBoundingClientRect();
    let sourceOffset: number | null = null;

    if (isPaginated) {
      const marginPercent = Math.min(30, Math.max(0, settings.margins));
      const pageSlotWidth = container.clientWidth / effectivePageColumns;
      const pageMargin = Math.max(12, pageSlotWidth * marginPercent / 100);
      const sampleX = Math.min(containerRect.right - 2, containerRect.left + pageMargin + 2);
      const contentTop = containerRect.top + 48;
      const sampleYs = [
        contentTop + 2,
        contentTop + 24,
        contentTop + 64,
        containerRect.top + container.clientHeight * 0.3,
        containerRect.top + container.clientHeight * 0.5,
      ].filter((value) => value < containerRect.bottom - 2);
      for (const sampleY of sampleYs) {
        const position = domPositionFromPoint(sampleX, sampleY);
        const element = position ? blockFromNode(position.node) : null;
        const range = element ? blockRange(element) : null;
        const block = range ? documentBlocksByStart.get(range.start) : null;
        if (!position || !element || !block) continue;
        const renderedOffset = textOffsetInBlock(element, position.node, position.offset);
        sourceOffset = renderedOffsetToSourceOffset(block, renderedOffset, "start");
        break;
      }
    } else {
      if (renderedBlocksRef.current.length === 0) updateRenderedBlockCache();
      const entries = renderedBlocksRef.current;
      const visible = entries[visibleBlockIndex(entries, container.scrollTop + 24)];
      if (visible) {
        const blockRect = visible.element.getBoundingClientRect();
        const sampleY = Math.min(
          Math.max(containerRect.top + 24, blockRect.top + 1),
          Math.min(containerRect.bottom - 1, blockRect.bottom - 1),
        );
        const sampleX = Math.min(
          Math.max(containerRect.left + 1, blockRect.left + 2),
          Math.min(containerRect.right - 1, blockRect.right - 2),
        );
        const renderedOffset = renderedOffsetNearPoint(visible.element, sampleX, sampleY);
        sourceOffset = renderedOffsetToSourceOffset(visible.block, renderedOffset, "start");
      }
    }

    const sourceEnd = documentBlocks[documentBlocks.length - 1]?.source_end ?? 0;
    const atEnd = isPaginated
      ? container.scrollLeft >= container.scrollWidth - container.clientWidth - 1
      : container.scrollTop >= container.scrollHeight - container.clientHeight - 1;
    if (sourceOffset === null) {
      const numerator = isPaginated ? container.scrollLeft : container.scrollTop;
      const denominator = isPaginated
        ? container.scrollWidth - container.clientWidth
        : container.scrollHeight - container.clientHeight;
      sourceOffset = Math.round((numerator / Math.max(1, denominator)) * sourceEnd);
    }
    sourceOffset = atEnd ? sourceEnd : Math.min(sourceEnd, Math.max(0, sourceOffset));
    const progress = atEnd
      ? 100
      : Math.round((sourceOffset / Math.max(1, sourceEnd)) * 100);
    const location = textLocation(sourceOffset);
    initialLocationRef.current = location;
    const tocIndex = tocIndexAtOffset(document, sourceOffset);
    onProgress(
      Math.min(100, Math.max(0, progress)),
      location,
      tocIndex,
      {
        chapterProgress: chapterProgressAtOffset(document, tocIndex, sourceOffset, sourceEnd),
        page: paginationInfo(),
      },
    );
  }, [
    document,
    documentBlocks,
    documentBlocksByStart,
    effectivePageColumns,
    isPaginated,
    onProgress,
    paginationInfo,
    settings.margins,
    updateRenderedBlockCache,
  ]);

  const scheduleScrollProgress = useCallback(() => {
    if (scrollFrameRef.current !== null) return;
    scrollFrameRef.current = requestAnimationFrame(() => {
      scrollFrameRef.current = null;
      reportScrollProgress();
    });
  }, [reportScrollProgress]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const handleScroll = () => {
      scheduleScrollProgress();
      if (!isPaginated) return;
      if (paginationSettleTimerRef.current !== null) {
        window.clearTimeout(paginationSettleTimerRef.current);
      }
      paginationSettleTimerRef.current = window.setTimeout(() => {
        paginationSettleTimerRef.current = null;
        const spread = Math.round(container.scrollLeft / Math.max(1, container.clientWidth));
        scrollToSpread(spread);
      }, 140);
    };
    container.addEventListener("scroll", handleScroll, { passive: true });
    return () => {
      container.removeEventListener("scroll", handleScroll);
      if (scrollFrameRef.current !== null) {
        cancelAnimationFrame(scrollFrameRef.current);
        scrollFrameRef.current = null;
      }
      if (paginationSettleTimerRef.current !== null) {
        window.clearTimeout(paginationSettleTimerRef.current);
        paginationSettleTimerRef.current = null;
      }
    };
  }, [isPaginated, scheduleScrollProgress, scrollToSpread]);

  const interactionFromRange = useCallback((
    range: Range,
    trigger: ReaderInteraction["trigger"],
  ): ReaderInteraction | null => {
      const text = range.toString().trim();
      const normalizedText = normalizeInteractionText(text);
      if (!text || !normalizedText) return null;
      const startBlock = blockFromNode(range.startContainer);
      const endBlock = blockFromNode(range.endContainer);
      if (!startBlock || !endBlock) return null;
      const startRange = blockRange(startBlock);
      const endRange = blockRange(endBlock);
      if (!startRange || !endRange) return null;
      const startBlockData = documentBlocks
        .find((block) => block.source_start === startRange.start && block.source_end === startRange.end);
      const endBlockData = startBlock === endBlock
        ? startBlockData
        : documentBlocks
          .find((block) => block.source_start === endRange.start && block.source_end === endRange.end);
      if (!startBlockData || !endBlockData) return null;
      const start = renderedOffsetToSourceOffset(
        startBlockData,
        textOffsetInBlock(startBlock, range.startContainer, range.startOffset),
        "start",
      );
      const end = renderedOffsetToSourceOffset(
        endBlockData,
        textOffsetInBlock(endBlock, range.endContainer, range.endOffset),
        "end",
      );
      const locale = window.document.documentElement.lang || undefined;
      return {
        trigger,
        kind: trigger === "word-menu"
          ? "word"
          : classifySelection(text, locale),
        text,
        normalizedText,
        context: contextForRange(range, startBlock.textContent || text),
        location: textLocation(start, end),
        anchorRect: viewportRectForRange(range),
        source: "text",
        format: "text",
        locale,
      };
  }, [documentBlocks]);

  const cancelWordClick = useCallback(() => {
    if (wordClickTimerRef.current !== null) {
      window.clearTimeout(wordClickTimerRef.current);
      wordClickTimerRef.current = null;
    }
  }, []);

  const cancelSelectionMenu = useCallback(() => {
    if (selectionMenuTimerRef.current !== null) {
      window.clearTimeout(selectionMenuTimerRef.current);
      selectionMenuTimerRef.current = null;
    }
  }, []);

  const handleTextClick = useCallback((event: React.MouseEvent<HTMLDivElement>) => {
    cancelWordClick();
    if (Date.now() < forceClickSuppressedUntilRef.current) return;
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return;
    if (isInteractiveReaderTarget(event.target) || (event.target as Element | null)?.closest?.("mark")) return;
    const selection = window.getSelection();
    if (selection && !selection.isCollapsed) return;
    const selectionRange = rangeFromSelectionSnapshotAtPoint(
      doubleClickSelectionRef.current,
      event.clientX,
      event.clientY,
    );
    const range = selectionRange ?? wordRangeAtPoint(window.document, event.clientX, event.clientY);
    if (!range || !containerRef.current?.contains(range.startContainer)) {
      doubleClickSelectionRef.current = null;
      return;
    }
    replaceDocumentSelection(window.document, range);
    doubleClickSelectionRef.current = snapshotSelectionRange(range);
    const interaction = interactionFromRange(
      range,
      selectionRange ? "selection-menu" : "word-menu",
    );
    if (!interaction?.normalizedText) return;
    wordClickTimerRef.current = window.setTimeout(() => {
      wordClickTimerRef.current = null;
      onInteraction(interaction);
    }, 240);
  }, [cancelWordClick, interactionFromRange, onInteraction]);

  const handleTextDoubleClick = useCallback((event: React.MouseEvent<HTMLDivElement>) => {
    cancelWordClick();
    cancelSelectionMenu();
    if (event.button !== 0 || isInteractiveReaderTarget(event.target)) return;
    const range = rangeFromSelectionSnapshotAtPoint(
      doubleClickSelectionRef.current,
      event.clientX,
      event.clientY,
    ) ?? wordRangeAtPoint(window.document, event.clientX, event.clientY);
    if (!range || !containerRef.current?.contains(range.startContainer)) return;
    const interaction = interactionFromRange(range, "word-quick-lookup");
    if (!interaction?.normalizedText) return;
    if (!doubleClickQuickLookup) {
      if (onReaderBinding?.("mouse:double", interaction)) event.preventDefault();
      return;
    }
    event.preventDefault();
    replaceDocumentSelection(window.document, range);
    doubleClickSelectionRef.current = snapshotSelectionRange(range);
    onInteraction(interaction);
  }, [cancelSelectionMenu, cancelWordClick, doubleClickQuickLookup, interactionFromRange, onInteraction, onReaderBinding]);

  useEffect(() => {
    if (!onReaderBinding) return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if ((event.target as Element | null)?.closest("input,textarea,select,[contenteditable='true']")) return;
      const trigger = bindingFromKeyboardEvent(event);
      if (!trigger) return;
      const range = selectedRange(window.document);
      const interaction = range && containerRef.current?.contains(range.commonAncestorContainer)
        ? interactionFromRange(range, "selection-menu")
        : null;
      if (!onReaderBinding(trigger, interaction)) return;
      event.preventDefault();
      event.stopPropagation();
    };
    window.addEventListener("keydown", handleKeyDown, true);
    return () => window.removeEventListener("keydown", handleKeyDown, true);
  }, [interactionFromRange, onReaderBinding]);

  const scheduleSelectionMenu = useCallback((delay = 150, includeWord = false) => {
    cancelSelectionMenu();
    selectionMenuTimerRef.current = window.setTimeout(() => {
      selectionMenuTimerRef.current = null;
      const range = selectedRange(window.document);
      if (!range || !containerRef.current?.contains(range.commonAncestorContainer)) return;
      const interaction = interactionFromRange(range, "selection-menu");
      if (interaction && (includeWord || interaction.kind !== "word")) onInteraction(interaction);
    }, delay);
  }, [cancelSelectionMenu, interactionFromRange, onInteraction]);

  const handleTextContextMenu = useCallback((event: React.MouseEvent<HTMLDivElement>) => {
    cancelWordClick();
    cancelSelectionMenu();
    const range = selectedRange(window.document);
    if (!range || !containerRef.current?.contains(range.commonAncestorContainer)) return;
    const interaction = interactionFromRange(range, "selection-menu");
    if (!interaction) return;
    event.preventDefault();
    onInteraction(interaction);
  }, [cancelSelectionMenu, cancelWordClick, interactionFromRange, onInteraction]);

  useEffect(() => {
    const handleSelectionChange = () => {
      if (
        activePointerIdRef.current === null
        && Date.now() >= selectionNormalizationUntilRef.current
      ) {
        const range = selectedRange(window.document);
        doubleClickSelectionRef.current = range
          && containerRef.current?.contains(range.commonAncestorContainer)
          ? snapshotSelectionRange(range)
          : null;
        scheduleSelectionMenu();
      }
    };
    window.document.addEventListener("selectionchange", handleSelectionChange);
    return () => {
      window.document.removeEventListener("selectionchange", handleSelectionChange);
      cancelSelectionMenu();
    };
  }, [cancelSelectionMenu, scheduleSelectionMenu]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const preserveSystemForceClick = () => {
      forceClickSuppressedUntilRef.current = Date.now() + 600;
      cancelWordClick();
      cancelSelectionMenu();
    };
    container.addEventListener("webkitmouseforcedown", preserveSystemForceClick);
    return () => {
      container.removeEventListener("webkitmouseforcedown", preserveSystemForceClick);
    };
  }, [cancelSelectionMenu, cancelWordClick]);

  const finalizePointerSelection = useCallback((pointerId?: number, openMenu = true) => {
    const activePointerId = activePointerIdRef.current;
    if (activePointerId === null || (pointerId !== undefined && pointerId !== activePointerId)) return;
    activePointerIdRef.current = null;
    const captureTarget = pointerCaptureTargetRef.current;
    pointerCaptureTargetRef.current = null;
    try {
      if (captureTarget?.hasPointerCapture(activePointerId)) {
        captureTarget.releasePointerCapture(activePointerId);
      }
    } catch {
      // Pointer capture is best-effort and may already be released by WebKit.
    }
    if (!openMenu || Date.now() < forceClickSuppressedUntilRef.current) {
      cancelSelectionMenu();
      return;
    }
    const range = selectedRange(window.document);
    const expanded = range && containerRef.current?.contains(range.commonAncestorContainer)
      ? expandRangeToWordBoundaries(range, window.document.documentElement.lang || undefined)
      : null;
    if (expanded) {
      selectionNormalizationUntilRef.current = Date.now() + 80;
      replaceDocumentSelection(window.document, expanded);
      doubleClickSelectionRef.current = snapshotSelectionRange(expanded);
    }
    if (expanded) scheduleSelectionMenu(30, true);
    else cancelSelectionMenu();
  }, [cancelSelectionMenu, scheduleSelectionMenu]);

  const handleSelectionPointerDown = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    activePointerIdRef.current = event.pointerId;
    pointerCaptureTargetRef.current = event.currentTarget;
    try {
      event.currentTarget.setPointerCapture(event.pointerId);
    } catch {
      // Some WebKit reader surfaces reject capture; window listeners are the fallback.
    }
    cancelSelectionMenu();
  }, [cancelSelectionMenu]);

  const handleSelectionPointerUp = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    finalizePointerSelection(event.pointerId);
  }, [finalizePointerSelection]);

  const handleSelectionPointerCancel = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    finalizePointerSelection(event.pointerId, false);
  }, [finalizePointerSelection]);

  const handleLostPointerCapture = useCallback((event: React.PointerEvent<HTMLDivElement>) => {
    finalizePointerSelection(event.pointerId);
  }, [finalizePointerSelection]);

  useEffect(() => {
    const handlePointerUp = (event: PointerEvent) => finalizePointerSelection(event.pointerId);
    const handlePointerCancel = (event: PointerEvent) => finalizePointerSelection(event.pointerId, false);
    const handleWindowBlur = () => finalizePointerSelection(undefined, false);
    window.addEventListener("pointerup", handlePointerUp);
    window.addEventListener("pointercancel", handlePointerCancel);
    window.addEventListener("blur", handleWindowBlur);
    return () => {
      window.removeEventListener("pointerup", handlePointerUp);
      window.removeEventListener("pointercancel", handlePointerCancel);
      window.removeEventListener("blur", handleWindowBlur);
      activePointerIdRef.current = null;
      pointerCaptureTargetRef.current = null;
    };
  }, [finalizePointerSelection]);

  const typography = useMemo(() => ({
    backgroundColor: getThemeStyles(settings.theme, settings.customTheme).body,
    color: getThemeStyles(settings.theme, settings.customTheme).text,
    fontFamily: getFontFamily(settings.font),
    fontSize: `${settings.fontSize}px`,
    lineHeight: settings.lineSpacing,
    letterSpacing: settings.charSpacing === 0 ? undefined : `${settings.charSpacing * 0.01}em`,
    wordSpacing: settings.wordSpacing === 0 ? undefined : `${settings.wordSpacing * 0.01}em`,
    filter: `brightness(${settings.brightness / 100})`,
  }), [settings]);

  const renderedDocument = useMemo(() => document?.chunks.map((chunk, chunkIndex) => (
    <section key={chunkIndex}>
      {chunk.blocks.map((block) => {
        const isFlashing = flashOffset !== null
          && block.source_start <= flashOffset
          && block.source_end >= flashOffset;
        const className = `${isFlashing ? "outline outline-2 outline-purple-400 outline-offset-4" : ""} transition-colors`;
        const content = renderHighlightedBlock(
          block,
          document,
          highlights,
          onHighlightClick,
          markerStyle,
          readerFontFamily,
          automaticWordSet,
          automaticExceptionSet,
          visibleLookupOccurrenceMarks,
        );
        const attributes = {
          key: block.source_start,
          "data-text-source-start": block.source_start,
          "data-text-source-end": block.source_end,
        };
        if (block.kind === "heading") {
          const startsChapterPage = isPaginated
            && block.starts_page === true
            && block.source_start !== firstDocumentBlockStart;
          const headingStyle = startsChapterPage
            ? { breakBefore: "column", pageBreakBefore: "always" } as React.CSSProperties
            : undefined;
          return block.depth === 0 ? (
            <h2 {...attributes} style={headingStyle} className={`mb-8 mt-14 text-[1.35em] font-semibold leading-snug ${className}`}>
              {content}
            </h2>
          ) : (
            <h3 {...attributes} style={headingStyle} className={`mb-6 mt-10 text-[1.15em] font-semibold leading-snug ${className}`}>
              {content}
            </h3>
          );
        }
        return (
          <p {...attributes} className={`mb-5 whitespace-pre-wrap ${className}`}>
            {content}
          </p>
        );
      })}
    </section>
  )) ?? null, [
    automaticExceptionSet,
    automaticWordSet,
    document,
    firstDocumentBlockStart,
    flashOffset,
    highlights,
    isPaginated,
    markerStyle,
    onHighlightClick,
    readerFontFamily,
    visibleLookupOccurrenceMarks,
  ]);

  return (
    <div
      ref={containerRef}
      data-reader-theme={settings.theme}
      className={`text-book-reader h-full overscroll-contain ${isPaginated
        ? "overflow-x-auto overflow-y-hidden [&::-webkit-scrollbar]:hidden"
        : "overflow-y-auto overflow-x-hidden"}`}
      style={typography}
      onClick={handleTextClick}
      onDoubleClick={handleTextDoubleClick}
      onMouseDownCapture={() => {
        const range = selectedRange(window.document);
        if (range) doubleClickSelectionRef.current = snapshotSelectionRange(range);
      }}
      onContextMenu={handleTextContextMenu}
      onPointerDown={handleSelectionPointerDown}
      onPointerUp={handleSelectionPointerUp}
      onPointerCancel={handleSelectionPointerCancel}
      onLostPointerCapture={handleLostPointerCapture}
    >
      {document && (
        <article
          ref={articleRef}
          className={`w-full py-12 ${isPaginated ? "h-full" : "min-h-full"}`}
          style={{
            paddingLeft: `max(12px, ${isPaginated ? settings.margins / effectivePageColumns : settings.margins}%)`,
            paddingRight: `max(12px, ${isPaginated ? settings.margins / effectivePageColumns : settings.margins}%)`,
            columnFill: isPaginated ? "auto" : undefined,
          }}
        >
          {renderedDocument}
        </article>
      )}
    </div>
  );
}

export default memo(TextBookReader);
