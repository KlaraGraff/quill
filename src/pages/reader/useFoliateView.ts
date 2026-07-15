import {
  useEffect,
  useRef,
  type Dispatch,
  type MutableRefObject,
  type SetStateAction,
} from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { ReaderSettingsState } from "../../components/ReaderSettings";
import {
  getEffectivePageColumns,
  type ReaderCapabilities,
} from "../../components/reader-settings";
import {
  classifySelection,
  contextForRange,
  normalizeInteractionText,
  type ReaderInteraction,
} from "../../components/reader-interaction";
import { installCustomFontFacesInDocument } from "../../components/custom-fonts";
import type { Highlight } from "../../hooks/useBookmarks";
import type { Book } from "../../hooks/useBooks";
import { logIgnoredError } from "../../utils/logIgnoredError";
import { createChapterPaginationMarker } from "./chapter-pagination";
import {
  applyPdfLayout,
  applyReflowLayout,
  getReaderCSS,
} from "./reader-theme";
import {
  drawFoliateAnnotation,
  type FoliateMarker,
  type WordMarkException,
  type WordMarkRule,
} from "./useFoliateAnnotations";
import type {
  FoliateView,
  ReaderPageInfo,
  TocChapter,
} from "./foliate-types";

function getPdfStartCfi(
  progress: number,
  pageCount: number | null | undefined,
): string | undefined {
  if (!Number.isFinite(progress) || progress <= 0 || !pageCount || pageCount <= 0) return undefined;
  const index = Math.min(
    pageCount - 1,
    Math.max(0, Math.ceil((progress / 100) * pageCount) - 1),
  );
  return `epubcfi(/6/${(index + 1) * 2})`;
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

interface InstallDocumentInteractionsOptions {
  doc: Document;
  index: number;
  view: FoliateView;
  bookFormat: string;
  interactionGeneration: number;
}

type SidePanel = "ai" | "bookmarks" | "vocab" | null;

interface UseFoliateViewOptions {
  book: Book | null;
  bookId?: string;
  bookReady: boolean;
  isTextBook: boolean;
  readerRetry: number;
  readerSettings: ReaderSettingsState;
  readerSettingsRef: MutableRefObject<ReaderSettingsState>;
  capabilities: ReaderCapabilities;
  viewRef: MutableRefObject<FoliateView | null>;
  viewerRef: MutableRefObject<HTMLDivElement | null>;
  currentCfiRef: MutableRefObject<string | null>;
  chaptersRef: MutableRefObject<TocChapter[]>;
  readerInteractionGenerationRef: MutableRefObject<number>;
  pendingWordClickRef: MutableRefObject<number | null>;
  pdfTextLayerNoticeTimerRef: MutableRefObject<number | null>;
  annotationClickDocumentRef: MutableRefObject<Document | null>;
  contextMenuRequestRef: MutableRefObject<number>;
  zoomRef: MutableRefObject<number | "fit">;
  fitPctRef: MutableRefObject<number>;
  markerStyleRef: MutableRefObject<Parameters<typeof drawFoliateAnnotation>[1]>;
  wordMarkWordsRef: MutableRefObject<string[]>;
  wordMarkExceptionsRef: MutableRefObject<Set<string>>;
  autoMarkersRef: MutableRefObject<Map<string, FoliateMarker>>;
  applyAnnotations(reapplyVisible?: boolean): Promise<void>;
  applyFoliateMarkerStyles(): void;
  installDocumentInteractions(options: InstallDocumentInteractionsOptions): void;
  queueReadingProgress(bookId: string, progress: number, cfi: string): void;
  cancelPendingWordClick(): void;
  cancelPendingSelectionMenu(): void;
  openLearningInteraction(interaction: ReaderInteraction): void;
  setBookReady: Dispatch<SetStateAction<boolean>>;
  setReaderError: Dispatch<SetStateAction<string | null>>;
  setPdfTextLayerNotice: Dispatch<SetStateAction<boolean>>;
  setCanGoBack: Dispatch<SetStateAction<boolean>>;
  setChapters: Dispatch<SetStateAction<TocChapter[]>>;
  setCurrentChapterIndex: Dispatch<SetStateAction<number>>;
  setProgress: Dispatch<SetStateAction<number>>;
  setChapterProgress: Dispatch<SetStateAction<number>>;
  setPageInfo: Dispatch<SetStateAction<ReaderPageInfo | null>>;
  setActiveVocabCfi: Dispatch<SetStateAction<string | null>>;
  setSidePanel: Dispatch<SetStateAction<SidePanel>>;
  setContextMenu: Dispatch<SetStateAction<ReaderInteraction | null>>;
}

function flattenToc(items: unknown[], depth = 0): TocChapter[] {
  const tocHref = (item: Record<string, unknown>): string | undefined => (
    typeof item.href === "string" && item.href !== "" && item.href !== "null"
      ? item.href
      : undefined
  );
  const firstHref = (item: Record<string, unknown>): string | undefined => {
    const href = tocHref(item);
    if (href) return href;
    return Array.isArray(item.subitems)
      ? item.subitems
        .map((child) => firstHref(child as Record<string, unknown>))
        .find((value): value is string => Boolean(value))
      : undefined;
  };
  return items.flatMap((value) => {
    const item = value as Record<string, unknown>;
    const label = item.label;
    return [
      {
        title: typeof label === "string" ? label.trim() : "",
        href: tocHref(item),
        targetHref: firstHref(item),
        depth,
      },
      ...(Array.isArray(item.subitems) ? flattenToc(item.subitems, depth + 1) : []),
    ];
  });
}

export function useFoliateView({
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
}: UseFoliateViewOptions) {
  const backButtonTimerRef = useRef<number | null>(null);
  const loadedInteractionDocumentsRef = useRef(new WeakSet<Document>());
  const pdfReadingMode = book?.format === "pdf" ? readerSettings.readingMode : null;

  useEffect(() => {
    if (!book || !viewerRef.current || book.available === false || isTextBook) return;

    const interactionGeneration = ++readerInteractionGenerationRef.current;
    const container = viewerRef.current;
    container.innerHTML = "";
    loadedInteractionDocumentsRef.current = new WeakSet<Document>();
    setBookReady(false);
    setReaderError(null);
    let cancelled = false;
    let activeView: FoliateView | null = null;

    const initFoliate = async () => {
      if (capabilities.supportsWordMarkers && bookId) {
        const [rules, exceptions] = await Promise.all([
          invoke<WordMarkRule[]>("list_word_marks", { bookId }).catch((error) => {
            logIgnoredError("reader.load-word-marks", error);
            return [];
          }),
          invoke<WordMarkException[]>("list_word_mark_exceptions", { bookId }).catch((error) => {
            logIgnoredError("reader.load-word-mark-exceptions", error);
            return [];
          }),
        ]);
        wordMarkWordsRef.current = rules
          .filter((rule) => rule.enabled)
          .map((rule) => rule.normalized_word);
        wordMarkExceptionsRef.current = new Set(exceptions
          .filter((exception) => exception.excluded)
          .map((exception) => `${exception.normalized_word}\0${exception.location}`));
      } else {
        wordMarkWordsRef.current = [];
        wordMarkExceptionsRef.current.clear();
      }

      if (!customElements.get("foliate-view")) {
        await withTimeout(new Promise<void>((resolve, reject) => {
          const script = document.createElement("script");
          script.type = "module";
          script.src = "/foliate-js/view.js";
          script.onload = () => resolve();
          script.onerror = () => reject(new Error("Failed to load foliate-js"));
          document.head.appendChild(script);
        }), 15_000, "READER_SCRIPT_TIMEOUT");
        await withTimeout(
          customElements.whenDefined("foliate-view"),
          15_000,
          "READER_ELEMENT_TIMEOUT",
        );
      }
      if (cancelled) return;

      const view = document.createElement("foliate-view") as FoliateView;
      activeView = view;
      view.style.display = "block";
      view.style.width = "100%";
      view.style.height = "100%";
      if (book.format === "pdf" && readerSettings.readingMode === "scrolling") {
        view.setAttribute("pdf-mode", "scroll");
      }
      container.appendChild(view);
      viewRef.current = view;

      const response = await withTimeout(
        fetch(convertFileSrc(book.file_path)),
        30_000,
        "READER_FILE_TIMEOUT",
      );
      if (!response.ok) throw new Error(`READER_FILE_${response.status}`);
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
      const file = new File(
        [await withTimeout(response.blob(), 30_000, "READER_FILE_READ_TIMEOUT")],
        `book.${extension}`,
        { type: mime },
      );
      await withTimeout(view.open(file), 45_000, "READER_OPEN_TIMEOUT");
      if (cancelled) return;

      const markChapterStarts = await createChapterPaginationMarker(view.book);
      if (cancelled) return;
      if (capabilities.supportsReflowSettings) {
        applyReflowLayout(view, readerSettings, container.clientWidth, container.clientHeight);
      } else if (capabilities.supportsSpread) {
        if (book.format === "pdf") {
          applyPdfLayout(view, readerSettings, container.clientWidth, container.clientHeight);
        } else {
          view.renderer.setAttribute("max-column-count", String(readerSettings.pageColumns));
        }
      }
      if (capabilities.supportsZoom && book.format === "pdf") {
        const savedZoom = localStorage.getItem(`reader-zoom-${bookId}`);
        let zoomAttr = "fit-width";
        if (savedZoom && savedZoom !== "fit") {
          const value = parseInt(savedZoom, 10);
          if (Number.isFinite(value) && value >= 50 && value <= 300) {
            zoomAttr = String(value / 100);
          }
        }
        view.renderer.setAttribute("zoom", zoomAttr);
      }
      if (capabilities.supportsReflowSettings) {
        view.renderer.setStyles?.(getReaderCSS(readerSettings));
      }

      if (Array.isArray(view.book.toc)) {
        const chapters = flattenToc(view.book.toc);
        chaptersRef.current = chapters;
        setChapters(chapters);
      }

      view.addEventListener("relocate", ((event: CustomEvent) => {
        const { fraction, section, tocItem, cfi } = event.detail;
        const nextProgress = Math.round((fraction ?? 0) * 100);
        setProgress(nextProgress);
        const sectionIndex = typeof section?.current === "number" ? section.current : -1;
        const sectionFractions = view.getSectionFractions?.() ?? [];
        const sectionStart = sectionFractions[sectionIndex];
        const sectionEnd = sectionFractions[sectionIndex + 1];
        const sectionProgress = Number.isFinite(sectionStart)
          && Number.isFinite(sectionEnd)
          && sectionEnd > sectionStart
          ? ((fraction ?? sectionStart) - sectionStart) / (sectionEnd - sectionStart)
          : 0;
        setChapterProgress(Math.round(Math.max(0, Math.min(1, sectionProgress)) * 100));
        currentCfiRef.current = cfi;

        const activeSettings = readerSettingsRef.current;
        if (capabilities.supportsReflowSettings && activeSettings.readingMode === "paginated") {
          const current = Number(view.renderer?.page);
          const total = Math.max(1, Number(view.renderer?.pages) - 2);
          setPageInfo(Number.isFinite(current) && Number.isFinite(total) ? {
            current: Math.max(1, Math.min(total, current)),
            total,
          } : null);
        } else if (book.format === "pdf" && sectionIndex >= 0 && section?.total > 0) {
          const current = sectionIndex + 1;
          const effectiveColumns = getEffectivePageColumns(
            activeSettings,
            container.clientWidth,
            container.clientHeight,
          );
          setPageInfo({
            current,
            visibleEnd: effectiveColumns === 2
              ? Math.min(section.total, current + 1)
              : current,
            total: section.total,
          });
        } else {
          setPageInfo(null);
        }
        if (tocItem) {
          const exactIndex = chaptersRef.current.findIndex((chapter) => chapter.href === tocItem.href);
          const chapterIndex = exactIndex !== -1
            ? exactIndex
            : chaptersRef.current.findIndex((chapter) => chapter.targetHref === tocItem.href);
          if (chapterIndex !== -1) setCurrentChapterIndex(chapterIndex);
        }
        if (bookId && cfi) queueReadingProgress(bookId, nextProgress, cfi);
      }) as EventListener);

      view.history.addEventListener("index-change", () => {
        const canGoBack = view.history.canGoBack;
        setCanGoBack(canGoBack);
        if (backButtonTimerRef.current !== null) {
          window.clearTimeout(backButtonTimerRef.current);
          backButtonTimerRef.current = null;
        }
        if (canGoBack) {
          backButtonTimerRef.current = window.setTimeout(() => {
            setCanGoBack(false);
            backButtonTimerRef.current = null;
          }, 10_000);
        }
      });

      view.addEventListener("load", ((event: CustomEvent) => {
        const { doc, index } = event.detail as { doc: Document; index: number };
        markChapterStarts(doc, index);
        installCustomFontFacesInDocument(doc);
        if (loadedInteractionDocumentsRef.current.has(doc)) return;
        loadedInteractionDocumentsRef.current.add(doc);
        window.requestAnimationFrame(applyFoliateMarkerStyles);
        installDocumentInteractions({
          doc,
          index,
          view,
          bookFormat: book.format,
          interactionGeneration,
        });
      }) as EventListener);

      view.addEventListener("create-overlay", (() => {
        if (bookId && capabilities.supportsManualAnnotations) {
          applyAnnotations(true).catch(() => {});
        }
      }) as EventListener);
      view.addEventListener("draw-annotation", ((event: CustomEvent) => {
        drawFoliateAnnotation(event.detail, markerStyleRef.current, book.format === "pdf");
      }) as EventListener);
      view.addEventListener("show-annotation", ((event: CustomEvent) => {
        cancelPendingWordClick();
        const { value, range } = event.detail;
        const ownerDocument = range?.startContainer?.ownerDocument ?? null;
        annotationClickDocumentRef.current = ownerDocument;
        queueMicrotask(() => {
          if (annotationClickDocumentRef.current === ownerDocument) {
            annotationClickDocumentRef.current = null;
          }
        });
        const marker = autoMarkersRef.current.get(value);
        if (marker?.kind === "vocab") {
          setActiveVocabCfi(value);
          setSidePanel("vocab");
          return;
        }
        if (marker?.kind === "lookup" && range) {
          const rect = range.getBoundingClientRect();
          const iframe = range.startContainer?.ownerDocument?.defaultView?.frameElement as HTMLElement | null;
          const iframeRect = iframe?.getBoundingClientRect();
          const text = range.toString().trim();
          if (!text) return;
          openLearningInteraction({
            trigger: "selection-menu",
            kind: "word",
            text,
            normalizedText: normalizeInteractionText(text),
            context: contextForRange(range, text),
            location: value,
            anchorRect: {
              left: rect.left + (iframeRect?.left ?? 0),
              top: rect.top + (iframeRect?.top ?? 0),
              right: rect.right + (iframeRect?.left ?? 0),
              bottom: rect.bottom + (iframeRect?.top ?? 0),
              width: rect.width,
              height: rect.height,
            },
            source: "foliate",
            format: book.format === "pdf" ? "pdf" : "epub",
            locale: range.startContainer.ownerDocument?.documentElement.lang || undefined,
          });
          return;
        }
        if (bookId && range) {
          const requestToken = ++contextMenuRequestRef.current;
          setContextMenu(null);
          pendingWordClickRef.current = window.setTimeout(() => {
            pendingWordClickRef.current = null;
            invoke<Highlight[]>("list_highlights", { bookId }).then((highlights) => {
              if (contextMenuRequestRef.current !== requestToken) return;
              const highlight = highlights.find((item) => item.cfi_range === value);
              if (!highlight) return;
              const rect = range.getBoundingClientRect();
              const iframe = range.startContainer?.ownerDocument?.defaultView?.frameElement as HTMLElement | null;
              const iframeRect = iframe?.getBoundingClientRect();
              const text = highlight.text_content?.trim() || range.toString().trim();
              if (!text || contextMenuRequestRef.current !== requestToken) return;
              openLearningInteraction({
                trigger: "selection-menu",
                kind: classifySelection(
                  text,
                  range.startContainer.ownerDocument?.documentElement.lang || undefined,
                ),
                text,
                normalizedText: normalizeInteractionText(text),
                context: contextForRange(range, text),
                location: highlight.cfi_range,
                anchorRect: {
                  left: rect.left + (iframeRect?.left ?? 0),
                  top: rect.top + (iframeRect?.top ?? 0),
                  right: rect.right + (iframeRect?.left ?? 0),
                  bottom: rect.bottom + (iframeRect?.top ?? 0),
                  width: rect.width,
                  height: rect.height,
                },
                source: "foliate",
                format: book.format === "pdf" ? "pdf" : "epub",
                locale: range.startContainer.ownerDocument?.documentElement.lang || undefined,
              });
            }).catch(() => {});
          }, 240);
        }
      }) as EventListener);

      const savedLocation = currentCfiRef.current || book.current_cfi;
      let startLocation: string | undefined = savedLocation || undefined;
      if (!startLocation && book.format === "pdf") {
        startLocation = getPdfStartCfi(
          book.progress,
          view.book?.sections?.length ?? book.pages,
        );
      }
      await withTimeout(
        view.init({ lastLocation: startLocation, showTextStart: !startLocation }),
        45_000,
        "READER_INIT_TIMEOUT",
      );
      if (cancelled) return;
      if (viewerRef.current) {
        viewerRef.current.style.filter = `brightness(${readerSettings.brightness / 100})`;
      }
      setBookReady(true);
    };

    initFoliate().catch((error) => {
      if (cancelled) return;
      console.error("Failed to initialize foliate-js:", error);
      activeView?.close();
      activeView?.remove();
      if (viewRef.current === activeView) viewRef.current = null;
      setReaderError(error instanceof Error ? error.message : "READER_INIT_FAILED");
      setBookReady(false);
    });

    return () => {
      cancelled = true;
      readerInteractionGenerationRef.current += 1;
      cancelPendingWordClick();
      cancelPendingSelectionMenu();
      if (pdfTextLayerNoticeTimerRef.current !== null) {
        window.clearTimeout(pdfTextLayerNoticeTimerRef.current);
        pdfTextLayerNoticeTimerRef.current = null;
      }
      setPdfTextLayerNotice(false);
      annotationClickDocumentRef.current = null;
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
    };
    // PDFs select their renderer from `pdf-mode` during open; EPUBs relayout live.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    book,
    pdfReadingMode,
    applyAnnotations,
    applyFoliateMarkerStyles,
    capabilities,
    installDocumentInteractions,
    isTextBook,
    queueReadingProgress,
    readerRetry,
  ]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view?.renderer) return;
    if (capabilities.supportsReflowSettings) {
      view.renderer.setStyles?.(getReaderCSS(readerSettings));
      const viewport = viewerRef.current ?? view;
      applyReflowLayout(view, readerSettings, viewport.clientWidth, viewport.clientHeight);
    } else if (capabilities.supportsSpread) {
      if (book?.format === "pdf") {
        const viewport = viewerRef.current ?? view;
        applyPdfLayout(view, readerSettings, viewport.clientWidth, viewport.clientHeight);
      } else {
        view.renderer.setAttribute("max-column-count", String(readerSettings.pageColumns));
      }
    }
    if (viewerRef.current) {
      viewerRef.current.style.filter = `brightness(${readerSettings.brightness / 100})`;
    }
  }, [book?.format, bookReady, capabilities, readerSettings, viewRef, viewerRef]);

  useEffect(() => {
    if (!bookReady || !capabilities.supportsReflowSettings || !viewerRef.current) return;
    let frame = 0;
    const viewer = viewerRef.current;
    const resize = () => {
      if (frame) cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const view = viewRef.current;
        if (view?.renderer) {
          applyReflowLayout(
            view,
            readerSettingsRef.current,
            viewer.clientWidth,
            viewer.clientHeight,
          );
        }
      });
    };
    const observer = new ResizeObserver(resize);
    const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
    observer.observe(viewer);
    reducedMotion.addEventListener("change", resize);
    return () => {
      if (frame) cancelAnimationFrame(frame);
      observer.disconnect();
      reducedMotion.removeEventListener("change", resize);
    };
  }, [bookReady, capabilities.supportsReflowSettings, readerSettingsRef, viewRef, viewerRef]);

  useEffect(() => {
    if (!bookReady || book?.format !== "pdf") return;
    const renderer = viewRef.current?.renderer;
    const foliateBook = viewRef.current?.book;
    if (!renderer || !foliateBook?.getPageSize) return;
    let cancelled = false;
    const update = async () => {
      try {
        const effectiveColumns = getEffectivePageColumns(
          readerSettingsRef.current,
          renderer.clientWidth,
          renderer.clientHeight,
        );
        const first = await foliateBook.getPageSize(0);
        const second = effectiveColumns === 2
          ? await foliateBook.getPageSize(1).catch(() => null)
          : null;
        if (cancelled || !first?.width) return;
        const rowWidth = first.width + (second?.width ?? 0);
        fitPctRef.current = Math.round(
          (Math.max(renderer.clientWidth - 24, 1) / rowWidth) * 100,
        );
      } catch {
        // The book can close while an async page-size read is in flight.
      }
    };
    void update();
    const observer = new ResizeObserver(update);
    observer.observe(renderer);
    return () => {
      cancelled = true;
      observer.disconnect();
    };
  }, [
    book?.format,
    bookReady,
    fitPctRef,
    readerSettings.pageColumns,
    readerSettings.readingMode,
    readerSettingsRef,
    viewRef,
  ]);

  useEffect(() => {
    if (!bookReady || book?.format !== "pdf" || !viewerRef.current) return;
    let frame = 0;
    const viewer = viewerRef.current;
    const relayoutPdf = () => {
      const renderer = viewRef.current?.renderer as (HTMLElement & {
        relayout?: () => void;
      }) | undefined;
      if (!renderer || renderer.hasAttribute("resize-dragging")) return;
      const view = viewRef.current;
      if (view) {
        applyPdfLayout(
          view,
          readerSettingsRef.current,
          viewer.clientWidth,
          viewer.clientHeight,
        );
      }
      if (typeof renderer.relayout === "function") {
        renderer.relayout();
        return;
      }
      const zoom = zoomRef.current;
      renderer.setAttribute("zoom", zoom === "fit" ? "fit-width" : String(zoom / 100));
    };
    const observer = new ResizeObserver(() => {
      if (frame) cancelAnimationFrame(frame);
      frame = requestAnimationFrame(relayoutPdf);
    });
    observer.observe(viewer);
    return () => {
      if (frame) cancelAnimationFrame(frame);
      observer.disconnect();
    };
  }, [book?.format, bookReady, readerSettingsRef, viewRef, viewerRef, zoomRef]);
}
