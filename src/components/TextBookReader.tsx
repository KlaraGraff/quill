import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getFontFamily, getThemeStyles } from "./reader-settings";
import type { ReaderSettingsState } from "./ReaderSettings";
import type { Highlight } from "../hooks/useBookmarks";
import { parseTextLocation, textLocation, type TextBookDocument } from "./text-book-location";

interface TextBookReaderProps {
  bookId: string;
  initialLocation?: string | null;
  settings: ReaderSettingsState;
  onReady: (document: TextBookDocument) => void;
  onProgress: (progress: number, location: string, chapterIndex: number) => void;
  onSelection: (selection: { text: string; location: string; sentence: string }) => void;
  onError: (error: string) => void;
  onRegisterNavigation: (navigate: (location: string, flash?: boolean) => void) => void;
  onHighlightClick: (highlight: Highlight, rect: DOMRect) => void;
}

const HIGHLIGHT_COLORS: Record<string, string> = {
  yellow: "#FBBF24",
  green: "#34D399",
  blue: "#60A5FA",
  pink: "#F472B6",
  purple: "#A78BFA",
};


function compareParagraph(chapter: number, paragraph: number, otherChapter: number, otherParagraph: number) {
  if (chapter !== otherChapter) return chapter - otherChapter;
  return paragraph - otherParagraph;
}

function textOffsetInParagraph(element: HTMLElement, node: Node, offset: number): number {
  const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
  let total = 0;
  let current = walker.nextNode();
  while (current) {
    if (current === node) return total + offset;
    total += current.textContent?.length ?? 0;
    current = walker.nextNode();
  }
  return total;
}

function paragraphFromNode(node: Node | null): HTMLElement | null {
  const element = node?.nodeType === Node.ELEMENT_NODE
    ? node as HTMLElement
    : node?.parentElement;
  return element?.closest<HTMLElement>("[data-text-chapter][data-text-paragraph]") ?? null;
}

function paragraphPosition(element: HTMLElement): { chapter: number; paragraph: number } | null {
  const chapter = Number(element.dataset.textChapter);
  const paragraph = Number(element.dataset.textParagraph);
  if (!Number.isSafeInteger(chapter) || !Number.isSafeInteger(paragraph)) return null;
  return { chapter, paragraph };
}

function formatError(error: unknown) {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return String(error);
}

function renderHighlightedParagraph(
  text: string,
  chapter: number,
  paragraph: number,
  highlights: Highlight[],
  onHighlightClick: (highlight: Highlight, rect: DOMRect) => void,
): ReactNode[] {
  const ranges = highlights
    .flatMap((highlight) => {
      const location = parseTextLocation(highlight.cfi_range);
      if (!location) return [];
      if (compareParagraph(chapter, paragraph, location.startChapter, location.startParagraph) < 0
        || compareParagraph(chapter, paragraph, location.endChapter, location.endParagraph) > 0) {
        return [];
      }
      const start = chapter === location.startChapter && paragraph === location.startParagraph
        ? Math.min(location.startOffset, text.length)
        : 0;
      const end = chapter === location.endChapter && paragraph === location.endParagraph
        ? Math.min(location.endOffset, text.length)
        : text.length;
      return end > start ? [{ highlight, start, end }] : [];
    })
    .sort((a, b) => a.start - b.start || a.end - b.end);

  if (ranges.length === 0) return [text];

  const nodes: ReactNode[] = [];
  let cursor = 0;
  for (const range of ranges) {
    if (range.start < cursor) continue;
    if (range.start > cursor) nodes.push(text.slice(cursor, range.start));
    nodes.push(
      <mark
        key={`${range.highlight.id}:${range.start}:${range.end}`}
        className="cursor-pointer"
        style={{ backgroundColor: `${HIGHLIGHT_COLORS[range.highlight.color] || HIGHLIGHT_COLORS.yellow}88` }}
        onClick={(event) => {
          event.stopPropagation();
          onHighlightClick(range.highlight, event.currentTarget.getBoundingClientRect());
        }}
      >
        {text.slice(range.start, range.end)}
      </mark>,
    );
    cursor = range.end;
  }
  if (cursor < text.length) nodes.push(text.slice(cursor));
  return nodes;
}

export default function TextBookReader({
  bookId,
  initialLocation,
  settings,
  onReady,
  onProgress,
  onSelection,
  onError,
  onRegisterNavigation,
  onHighlightClick,
}: TextBookReaderProps) {
  const [document, setDocument] = useState<TextBookDocument | null>(null);
  const [highlights, setHighlights] = useState<Highlight[]>([]);
  const containerRef = useRef<HTMLDivElement>(null);
  const initialLocationRef = useRef(initialLocation);
  const flashTimerRef = useRef<number | null>(null);
  const [flashLocation, setFlashLocation] = useState<string | null>(null);

  const refreshHighlights = useCallback(async () => {
    const result = await invoke<Highlight[]>("list_highlights", { bookId });
    setHighlights(result.filter((highlight) => highlight.cfi_range.startsWith("textloc:")));
  }, [bookId]);

  const loadDocument = useCallback(async () => {
    try {
      const result = await invoke<TextBookDocument>("get_text_book_document", { bookId });
      setDocument(result);
      onReady(result);
      await refreshHighlights();
    } catch (error) {
      const message = formatError(error);
      if (!message.includes("TEXT_PREPARATION_PENDING")) onError(message);
    }
  }, [bookId, onError, onReady, refreshHighlights]);

  useEffect(() => {
    setDocument(null);
    initialLocationRef.current = initialLocation;
    loadDocument().catch(() => {});
  }, [bookId, initialLocation, loadDocument]);

  useEffect(() => {
    const unlisten = listen<{ book_id?: string; state?: string }>("book-preparation-changed", (event) => {
      if (event.payload.book_id !== bookId) return;
      if (event.payload.state === "ready") loadDocument().catch(() => {});
      if (event.payload.state === "failed") onError("TEXT_PREPARATION_FAILED");
    });
    const refresh = (event: Event) => {
      const detail = (event as CustomEvent<{ bookId?: string }>).detail;
      if (!detail?.bookId || detail.bookId === bookId) refreshHighlights().catch(() => {});
    };
    window.addEventListener("highlight-changed", refresh);
    return () => {
      unlisten.then((stop) => stop());
      window.removeEventListener("highlight-changed", refresh);
    };
  }, [bookId, loadDocument, onError, refreshHighlights]);

  const navigateToLocation = useCallback((location: string, flash = false) => {
    const parsed = parseTextLocation(location);
    if (!parsed || !containerRef.current) return;
    const target = containerRef.current.querySelector<HTMLElement>(
      `[data-text-chapter="${parsed.startChapter}"][data-text-paragraph="${parsed.startParagraph}"]`,
    );
    target?.scrollIntoView({ behavior: "smooth", block: "center" });
    if (flash) {
      setFlashLocation(location);
      if (flashTimerRef.current !== null) window.clearTimeout(flashTimerRef.current);
      flashTimerRef.current = window.setTimeout(() => setFlashLocation(null), 3000);
    }
  }, []);

  useEffect(() => {
    onRegisterNavigation(navigateToLocation);
    return () => {
      if (flashTimerRef.current !== null) window.clearTimeout(flashTimerRef.current);
    };
  }, [navigateToLocation, onRegisterNavigation]);

  useEffect(() => {
    if (!document || !initialLocationRef.current) return;
    const location = initialLocationRef.current;
    initialLocationRef.current = null;
    requestAnimationFrame(() => navigateToLocation(location));
  }, [document, navigateToLocation]);

  const handleScroll = useCallback(() => {
    const container = containerRef.current;
    if (!container || !document) return;
    const progress = Math.round((container.scrollTop / Math.max(1, container.scrollHeight - container.clientHeight)) * 100);
    const paragraphs = [...container.querySelectorAll<HTMLElement>("[data-text-chapter][data-text-paragraph]")];
    const visible = paragraphs.find((paragraph) => paragraph.getBoundingClientRect().bottom > container.getBoundingClientRect().top + 24) || paragraphs[0];
    const position = visible ? paragraphPosition(visible) : null;
    if (!position) return;
    onProgress(
      Math.min(100, Math.max(0, progress)),
      textLocation(position.chapter, position.paragraph, 0),
      position.chapter,
    );
  }, [document, onProgress]);

  const handleMouseUp = useCallback(() => {
    requestAnimationFrame(() => {
      const selection = window.getSelection();
      const text = selection?.toString().trim();
      if (!selection || !text || selection.rangeCount === 0) return;
      const range = selection.getRangeAt(0);
      const startParagraph = paragraphFromNode(range.startContainer);
      const endParagraph = paragraphFromNode(range.endContainer);
      if (!startParagraph || !endParagraph) return;
      const start = paragraphPosition(startParagraph);
      const end = paragraphPosition(endParagraph);
      if (!start || !end) return;
      const location = textLocation(
        start.chapter,
        start.paragraph,
        textOffsetInParagraph(startParagraph, range.startContainer, range.startOffset),
        end.chapter,
        end.paragraph,
        textOffsetInParagraph(endParagraph, range.endContainer, range.endOffset),
      );
      const sentence = (startParagraph.textContent || text).trim().slice(0, 500);
      onSelection({ text, location, sentence });
    });
  }, [onSelection]);

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
      onMouseUp={handleMouseUp}
    >
      {document && (
        <article
          className="mx-auto py-12"
          style={{ maxWidth: `${Math.max(560, 900 - settings.margins * 2)}px`, paddingLeft: `${Math.max(24, settings.margins)}px`, paddingRight: `${Math.max(24, settings.margins)}px` }}
        >
          {document.chapters.map((chapter, chapterIndex) => (
            <section key={`${chapterIndex}:${chapter.title}`} className="mb-14">
              <h2 className="mb-8 text-[1.35em] font-semibold leading-snug">{chapter.title}</h2>
              {chapter.paragraphs.map((paragraph, paragraphIndex) => {
                const location = textLocation(chapterIndex, paragraphIndex, 0);
                const isFlashing = flashLocation === location;
                return (
                  <p
                    key={`${chapterIndex}:${paragraphIndex}`}
                    data-text-chapter={chapterIndex}
                    data-text-paragraph={paragraphIndex}
                    className={`mb-5 whitespace-pre-wrap transition-colors ${isFlashing ? "outline outline-2 outline-purple-400 outline-offset-4" : ""}`}
                  >
                    {renderHighlightedParagraph(paragraph, chapterIndex, paragraphIndex, highlights, onHighlightClick)}
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
