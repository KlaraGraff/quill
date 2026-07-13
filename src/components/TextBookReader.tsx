import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getFontFamily, getThemeStyles } from "./reader-settings";
import type { ReaderSettingsState } from "./ReaderSettings";
import type { Highlight } from "../hooks/useBookmarks";
import {
  classifySelection,
  applyWordMarkHighlights,
  contextForRange,
  isInteractiveReaderTarget,
  normalizeInteractionText,
  selectedRange,
  viewportRectForRange,
  wordRangeAtPoint,
  type ReaderInteraction,
} from "./reader-interaction";
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
  onProgress: (progress: number, location: string, tocIndex: number) => void;
  onInteraction: (interaction: ReaderInteraction) => void;
  onError: (error: string) => void;
  onRegisterNavigation: (navigate: (location: string, flash?: boolean) => void) => void;
  onHighlightClick: (highlight: Highlight, rect: DOMRect) => void;
}

interface WordMarkRule {
  normalized_word: string;
  enabled: boolean;
}

const HIGHLIGHT_COLORS: Record<string, string> = {
  yellow: "#FBBF24",
  green: "#34D399",
  blue: "#60A5FA",
  pink: "#F472B6",
  purple: "#A78BFA",
};

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

function renderedOffsetFromCaretPoint(element: HTMLElement, x: number, y: number): number | null {
  const caretDocument = document as Document & {
    caretPositionFromPoint?: (
      x: number,
      y: number,
    ) => { offsetNode: Node; offset: number } | null;
    caretRangeFromPoint?: (x: number, y: number) => Range | null;
  };
  const caretPosition = caretDocument.caretPositionFromPoint?.(x, y);
  if (caretPosition && (caretPosition.offsetNode === element || element.contains(caretPosition.offsetNode))) {
    return textOffsetInBlock(element, caretPosition.offsetNode, caretPosition.offset);
  }
  const caretRange = caretDocument.caretRangeFromPoint?.(x, y);
  if (caretRange && (caretRange.startContainer === element || element.contains(caretRange.startContainer))) {
    return textOffsetInBlock(element, caretRange.startContainer, caretRange.startOffset);
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
  onHighlightClick: (highlight: Highlight, rect: DOMRect) => void,
): ReactNode[] {
  const ranges = highlights
    .flatMap((highlight, priority) => {
      const location = resolveTextLocation(highlight.cfi_range, document);
      if (!location || location.end <= block.source_start || location.start >= block.source_end) return [];
      const start = sourceOffsetToRenderedOffset(block, Math.max(location.start, block.source_start));
      const end = sourceOffsetToRenderedOffset(block, Math.min(location.end, block.source_end));
      return end > start ? [{ highlight, start, end, priority }] : [];
    })
    .sort((a, b) => a.start - b.start || a.end - b.end || a.priority - b.priority);

  if (ranges.length === 0) return [block.text];

  // Split at every boundary instead of advancing a single cursor per stored
  // range. The old renderer skipped an entire later range when its start was
  // inside an earlier one, including the non-overlapping text after it.
  const boundaries = [...new Set(ranges.flatMap((range) => [range.start, range.end]))]
    .sort((left, right) => left - right);
  const segments: Array<{ start: number; end: number; highlight: Highlight | null }> = [];
  let previous = 0;
  for (let index = 0; index < boundaries.length - 1; index += 1) {
    const start = boundaries[index];
    const end = boundaries[index + 1];
    if (start > previous) segments.push({ start: previous, end: start, highlight: null });
    const active = ranges
      .filter((range) => range.start < end && range.end > start)
      .sort((left, right) => left.priority - right.priority)[0];
    segments.push({ start, end, highlight: active?.highlight ?? null });
    previous = end;
  }
  if (previous < block.text.length) {
    segments.push({ start: previous, end: block.text.length, highlight: null });
  }

  const coalesced = segments.reduce<typeof segments>((result, segment) => {
    if (segment.end <= segment.start) return result;
    const last = result[result.length - 1];
    if (last?.end === segment.start && last.highlight?.id === segment.highlight?.id) {
      last.end = segment.end;
    } else {
      result.push({ ...segment });
    }
    return result;
  }, []);

  const nodes: ReactNode[] = [];
  for (const segment of coalesced) {
    if (!segment.highlight) {
      nodes.push(block.text.slice(segment.start, segment.end));
      continue;
    }
    nodes.push(
      <mark
        key={`${segment.highlight.id}:${segment.start}:${segment.end}`}
        className="cursor-pointer"
        style={{ backgroundColor: `${HIGHLIGHT_COLORS[segment.highlight.color] || HIGHLIGHT_COLORS.yellow}88` }}
        onClick={(event) => {
          event.stopPropagation();
          onHighlightClick(segment.highlight!, event.currentTarget.getBoundingClientRect());
        }}
      >
        {block.text.slice(segment.start, segment.end)}
      </mark>,
    );
  }
  return nodes;
}

function tocIndexAtOffset(document: TextBookDocument, sourceOffset: number) {
  let current = 0;
  for (let index = 0; index < document.toc.length; index += 1) {
    if (document.toc[index].source_offset > sourceOffset) break;
    current = index;
  }
  return current;
}

export default function TextBookReader({
  bookId,
  initialLocation,
  settings,
  onReady,
  onProgress,
  onInteraction,
  onError,
  onRegisterNavigation,
  onHighlightClick,
}: TextBookReaderProps) {
  const [document, setDocument] = useState<TextBookDocument | null>(null);
  const [highlights, setHighlights] = useState<Highlight[]>([]);
  const [wordMarks, setWordMarks] = useState<WordMarkRule[]>([]);
  const [preparationPending, setPreparationPending] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);
  const initialLocationRef = useRef(initialLocation);
  const progressReadyRef = useRef(false);
  const flashTimerRef = useRef<number | null>(null);
  const wordClickTimerRef = useRef<number | null>(null);
  const [flashOffset, setFlashOffset] = useState<number | null>(null);
  const documentBlocks = useMemo(
    () => document?.chunks.flatMap((chunk) => chunk.blocks) ?? [],
    [document],
  );

  const refreshHighlights = useCallback(async () => {
    const result = await invoke<Highlight[]>("list_highlights", { bookId });
    setHighlights(result.filter((highlight) => highlight.cfi_range.startsWith("textloc:")));
  }, [bookId]);

  const refreshWordMarks = useCallback(async () => {
    const result = await invoke<WordMarkRule[]>("list_word_marks", { bookId });
    setWordMarks(result.filter((rule) => rule.enabled));
  }, [bookId]);

  const loadDocument = useCallback(async () => {
    try {
      const result = await invoke<TextBookDocument>("get_text_book_document", { bookId });
      setPreparationPending(false);
      setDocument(result);
      onReady(result);
      await Promise.all([refreshHighlights(), refreshWordMarks()]);
    } catch (error) {
      const message = formatError(error);
      if (message.includes("TEXT_PREPARATION_PENDING")) setPreparationPending(true);
      else {
        setPreparationPending(false);
        onError(message);
      }
    }
  }, [bookId, onError, onReady, refreshHighlights, refreshWordMarks]);

  useEffect(() => {
    setDocument(null);
    setPreparationPending(false);
    progressReadyRef.current = false;
    initialLocationRef.current = initialLocation;
    loadDocument().catch(() => {});
  }, [bookId, initialLocation, loadDocument]);

  useEffect(() => {
    if (!preparationPending) return;
    const timer = window.setInterval(() => loadDocument().catch(() => {}), 750);
    return () => window.clearInterval(timer);
  }, [loadDocument, preparationPending]);

  useEffect(() => {
    const unlisten = listen<{ book_id?: string; state?: string }>("book-preparation-changed", (event) => {
      if (event.payload.book_id !== bookId) return;
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
    return () => {
      unlisten.then((stop) => stop());
      window.removeEventListener("highlight-changed", refresh);
      window.removeEventListener("word-mark-changed", refreshMarks);
    };
  }, [bookId, loadDocument, onError, refreshHighlights, refreshWordMarks]);

  useEffect(() => {
    if (!document || !containerRef.current) return;
    const frame = requestAnimationFrame(() => {
      applyWordMarkHighlights(
        window.document,
        wordMarks.map((rule) => rule.normalized_word),
        `quill-word-marks-${bookId}`,
        containerRef.current ?? undefined,
      );
    });
    return () => cancelAnimationFrame(frame);
  }, [bookId, document, highlights, wordMarks]);

  const navigateToLocation = useCallback((
    location: string,
    flash = false,
    behavior: ScrollBehavior = "smooth",
  ) => {
    if (!document || !containerRef.current) return null;
    const resolved = resolveTextLocation(location, document);
    if (!resolved) return null;
    const blocks = [...containerRef.current.querySelectorAll<HTMLElement>(
      "[data-text-source-start][data-text-source-end]",
    )];
    const target = blocks.find((block) => {
      const range = blockRange(block);
      return range && range.start <= resolved.start && range.end > resolved.start;
    }) ?? blocks.find((block) => {
      const range = blockRange(block);
      return range && range.start >= resolved.start;
    }) ?? blocks[blocks.length - 1];
    if (!target) return null;
    const targetRange = blockRange(target);
    const targetBlock = targetRange
      ? documentBlocks.find((block) => (
          block.source_start === targetRange.start && block.source_end === targetRange.end
        ))
      : null;
    const renderedOffset = targetBlock
      ? sourceOffsetToRenderedOffset(targetBlock, resolved.start)
      : 0;
    scrollRenderedOffsetIntoView(containerRef.current, target, renderedOffset, behavior);
    if (flash) {
      setFlashOffset(resolved.start);
      if (flashTimerRef.current !== null) window.clearTimeout(flashTimerRef.current);
      flashTimerRef.current = window.setTimeout(() => setFlashOffset(null), 3000);
    }
    return resolved;
  }, [document, documentBlocks]);

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
      initialLocationRef.current = null;
      const restored = location ? navigateToLocation(location, false, "auto") : null;
      // Scrolling can dispatch synchronously. Enable persistence only after
      // the restoration scroll and its first scroll event have completed.
      settleFrame = requestAnimationFrame(() => {
        if (cancelled) return;
        progressReadyRef.current = true;
        if (!restored) return;
        const sourceEnd = documentBlocks[documentBlocks.length - 1]?.source_end ?? 0;
        const sourceOffset = Math.min(Math.max(0, restored.start), sourceEnd);
        const progress = Math.round((sourceOffset / Math.max(1, sourceEnd)) * 100);
        onProgress(
          Math.min(100, Math.max(0, progress)),
          textLocation(sourceOffset),
          tocIndexAtOffset(document, sourceOffset),
        );
      });
    });
    return () => {
      cancelled = true;
      cancelAnimationFrame(frame);
      if (settleFrame) cancelAnimationFrame(settleFrame);
    };
  }, [document, documentBlocks, navigateToLocation, onProgress]);

  const handleScroll = useCallback(() => {
    const container = containerRef.current;
    if (!container || !document || !progressReadyRef.current) return;
    const blocks = [...container.querySelectorAll<HTMLElement>("[data-text-source-start][data-text-source-end]")];
    const containerRect = container.getBoundingClientRect();
    const visible = blocks.find((block) => block.getBoundingClientRect().bottom > containerRect.top + 24)
      ?? blocks[0];
    const range = visible ? blockRange(visible) : null;
    if (!range) return;
    const block = documentBlocks.find((candidate) => (
      candidate.source_start === range.start && candidate.source_end === range.end
    ));
    let sourceOffset = range.start;
    if (visible && block) {
      const blockRect = visible.getBoundingClientRect();
      const sampleY = Math.min(
        Math.max(containerRect.top + 24, blockRect.top + 1),
        Math.min(containerRect.bottom - 1, blockRect.bottom - 1),
      );
      const sampleX = Math.min(
        Math.max(containerRect.left + 1, blockRect.left + 2),
        Math.min(containerRect.right - 1, blockRect.right - 2),
      );
      const renderedOffset = renderedOffsetNearPoint(visible, sampleX, sampleY);
      sourceOffset = renderedOffsetToSourceOffset(block, renderedOffset, "start");
    }
    const sourceEnd = documentBlocks[documentBlocks.length - 1]?.source_end ?? 0;
    const atEnd = container.scrollTop >= container.scrollHeight - container.clientHeight - 1;
    const progress = atEnd
      ? 100
      : Math.round((sourceOffset / Math.max(1, sourceEnd)) * 100);
    onProgress(
      Math.min(100, Math.max(0, progress)),
      textLocation(sourceOffset),
      tocIndexAtOffset(document, sourceOffset),
    );
  }, [document, documentBlocks, onProgress]);

  const interactionFromRange = useCallback((
    range: Range,
    trigger: ReaderInteraction["trigger"],
  ): ReaderInteraction | null => {
      const text = range.toString().trim();
      if (!text) return null;
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
        kind: trigger === "word-click" ? "word" : classifySelection(text, locale),
        text,
        normalizedText: normalizeInteractionText(text),
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

  const handleTextClick = useCallback((event: React.MouseEvent<HTMLDivElement>) => {
    cancelWordClick();
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return;
    if (isInteractiveReaderTarget(event.target) || (event.target as Element | null)?.closest?.("mark")) return;
    const selection = window.getSelection();
    if (selection && !selection.isCollapsed) return;
    const range = wordRangeAtPoint(window.document, event.clientX, event.clientY);
    if (!range || !containerRef.current?.contains(range.startContainer)) return;
    const interaction = interactionFromRange(range, "word-click");
    if (!interaction?.normalizedText) return;
    wordClickTimerRef.current = window.setTimeout(() => {
      wordClickTimerRef.current = null;
      onInteraction(interaction);
    }, 180);
  }, [cancelWordClick, interactionFromRange, onInteraction]);

  const handleTextContextMenu = useCallback((event: React.MouseEvent<HTMLDivElement>) => {
    cancelWordClick();
    const range = selectedRange(window.document);
    if (!range || !containerRef.current?.contains(range.commonAncestorContainer)) return;
    const interaction = interactionFromRange(range, "selection-contextmenu");
    if (!interaction) return;
    event.preventDefault();
    onInteraction(interaction);
  }, [cancelWordClick, interactionFromRange, onInteraction]);

  const typography = useMemo(() => ({
    backgroundColor: getThemeStyles(settings.theme).body,
    color: getThemeStyles(settings.theme).text,
    fontFamily: getFontFamily(settings.font),
    fontSize: `${settings.fontSize}px`,
    lineHeight: settings.lineSpacing,
    letterSpacing: settings.charSpacing === 0 ? undefined : `${settings.charSpacing * 0.01}em`,
    wordSpacing: settings.wordSpacing === 0 ? undefined : `${settings.wordSpacing * 0.01}em`,
    filter: `brightness(${settings.brightness / 100})`,
  }), [settings]);

  return (
    <div
      ref={containerRef}
      className="h-full overflow-y-auto overscroll-contain"
      style={typography}
      onScroll={handleScroll}
      onClick={handleTextClick}
      onDoubleClick={cancelWordClick}
      onContextMenu={handleTextContextMenu}
    >
      {document && (
        <article
          className="mx-auto py-12"
          style={{ maxWidth: `${Math.max(560, 900 - settings.margins * 2)}px`, paddingLeft: `${Math.max(24, settings.margins)}px`, paddingRight: `${Math.max(24, settings.margins)}px` }}
        >
          {document.chunks.map((chunk, chunkIndex) => (
            <section key={chunkIndex}>
              {chunk.blocks.map((block) => {
                const isFlashing = flashOffset !== null
                  && block.source_start <= flashOffset
                  && block.source_end >= flashOffset;
                const className = `${isFlashing ? "outline outline-2 outline-purple-400 outline-offset-4" : ""} transition-colors`;
                const content = renderHighlightedBlock(block, document, highlights, onHighlightClick);
                const attributes = {
                  key: block.source_start,
                  "data-text-source-start": block.source_start,
                  "data-text-source-end": block.source_end,
                };
                if (block.kind === "heading") {
                  return block.depth === 0 ? (
                    <h2 {...attributes} className={`mb-8 mt-14 text-[1.35em] font-semibold leading-snug ${className}`}>
                      {content}
                    </h2>
                  ) : (
                    <h3 {...attributes} className={`mb-6 mt-10 text-[1.15em] font-semibold leading-snug ${className}`}>
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
          ))}
        </article>
      )}
    </div>
  );
}
