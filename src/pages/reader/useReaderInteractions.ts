import {
  useCallback,
  type MutableRefObject,
} from "react";
import {
  classifySelection,
  contextForRange,
  expandRangeToWordBoundaries,
  forwardReaderContextMenuKey,
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
} from "../../components/reader-interaction";
import { bindingFromKeyboardEvent } from "../../components/reader-bindings";

interface InteractionView {
  getCFI(index: number, range: Range): string;
  history: {
    back(): void;
    forward(): void;
  };
  renderer?: {
    getContents?(): Array<{ doc?: Document }>;
  };
}

interface InstallDocumentInteractionsOptions {
  doc: Document;
  index: number;
  view: InteractionView;
  bookFormat: string;
  interactionGeneration: number;
}

interface ReaderInteractionsOptions {
  supportsSelection: boolean;
  pendingSelectionMenuRef: MutableRefObject<number | null>;
  pendingWordClickRef: MutableRefObject<number | null>;
  readerInteractionGenerationRef: MutableRefObject<number>;
  forceClickSuppressedUntilRef: MutableRefObject<number>;
  annotationClickDocumentRef: MutableRefObject<Document | null>;
  doubleClickQuickLookupRef: MutableRefObject<boolean>;
  cancelPendingSelectionMenu(): void;
  cancelPendingWordClick(): void;
  openLearningInteraction(interaction: ReaderInteraction): void;
  setContextMenu(value: ReaderInteraction | null): void;
  onMissingPdfTextIntent(pageIndex: number): void;
  handleZoom(delta: number): void;
  handlePageTurnKeyDown(event: KeyboardEvent): void;
  handlePageTurnMouseDown(event: MouseEvent): void;
  handlePageTurnContextMenu(event: MouseEvent): void;
  handlePageTurnWheel(event: WheelEvent): void;
  handleReaderBinding(trigger: string, interaction: ReaderInteraction | null): boolean;
}

function canvasHasVisibleContent(canvas: HTMLCanvasElement): boolean {
  try {
    const context = canvas.getContext("2d", { willReadFrequently: true });
    if (!context) return true;
    const stepX = Math.max(1, Math.floor(canvas.width / 32));
    const stepY = Math.max(1, Math.floor(canvas.height / 32));
    let visibleSamples = 0;
    for (let y = Math.floor(stepY / 2); y < canvas.height; y += stepY) {
      for (let x = Math.floor(stepX / 2); x < canvas.width; x += stepX) {
        const [red, green, blue, alpha] = context.getImageData(x, y, 1, 1).data;
        if (alpha > 16 && (red < 246 || green < 246 || blue < 246)) {
          visibleSamples += 1;
          if (visibleSamples >= 3) return true;
        }
      }
    }
    return false;
  } catch {
    // A rendered page whose pixels cannot be sampled is still a valid text-intent target.
    return true;
  }
}

export function useReaderInteractions({
  supportsSelection,
  pendingSelectionMenuRef,
  pendingWordClickRef,
  readerInteractionGenerationRef,
  forceClickSuppressedUntilRef,
  annotationClickDocumentRef,
  doubleClickQuickLookupRef,
  cancelPendingSelectionMenu,
  cancelPendingWordClick,
  openLearningInteraction,
  setContextMenu,
  onMissingPdfTextIntent,
  handleZoom,
  handlePageTurnKeyDown,
  handlePageTurnMouseDown,
  handlePageTurnContextMenu,
  handlePageTurnWheel,
  handleReaderBinding,
}: ReaderInteractionsOptions) {
  const installDocumentInteractions = useCallback(({
    doc,
    index,
    view,
    bookFormat,
    interactionGeneration,
  }: InstallDocumentInteractionsOptions) => {
    const missingPdfTextLayer = () => {
      if (bookFormat !== "pdf") return false;
      const canvas = doc.querySelector("#canvas > canvas") as HTMLCanvasElement | null;
      const textLayer = doc.querySelector(".textLayer") as HTMLElement | null;
      return Boolean(
        canvas
        && canvas.width > 0
        && canvas.height > 0
        && textLayer?.querySelector(".endOfContent")
        && !textLayer.textContent?.trim()
        && canvasHasVisibleContent(canvas),
      );
    };
    const showMissingPdfTextIntent = () => {
      if (!missingPdfTextLayer()) return false;
      onMissingPdfTextIntent(index);
      return true;
    };

    const interactionForSelection = (
      trigger: ReaderInteraction["trigger"],
    ): ReaderInteraction | null => {
      if (!supportsSelection) return null;
      const range = selectedRange(doc);
      if (!range) return null;
      const text = range.toString().trim();
      const normalizedText = normalizeInteractionText(text);
      const location = view.getCFI(index, range);
      if (!text || !normalizedText || !location) return null;
      return {
        trigger,
        kind: classifySelection(text, doc.documentElement.lang || undefined),
        text,
        normalizedText,
        context: contextForRange(range, text),
        location,
        anchorRect: viewportRectForRange(range),
        source: "foliate",
        format: bookFormat === "pdf" ? "pdf" : "epub",
        locale: doc.documentElement.lang || undefined,
      };
    };

    let activePointerId: number | null = null;
    let selectionSnapshot: ReaderSelectionSnapshot | null = null;
    let pointerCaptureTarget: Element | null = null;
    let pointerStart: { x: number; y: number } | null = null;
    let pointerMoved = false;
    let selectionNormalizationUntil = 0;
    const scheduleSelectionMenu = (delay = 150, includeWord = false) => {
      cancelPendingSelectionMenu();
      pendingSelectionMenuRef.current = window.setTimeout(() => {
        pendingSelectionMenuRef.current = null;
        if (readerInteractionGenerationRef.current !== interactionGeneration) return;
        const interaction = interactionForSelection("selection-menu");
        if (interaction && (includeWord || interaction.kind !== "word")) {
          openLearningInteraction(interaction);
        }
      }, delay);
    };

    doc.addEventListener("selectionchange", () => {
      if (activePointerId === null && Date.now() >= selectionNormalizationUntil) {
        const range = selectedRange(doc);
        selectionSnapshot = snapshotSelectionRange(range);
        scheduleSelectionMenu();
      }
    });

    const finalizePointerSelection = (pointerId?: number, openMenu = true) => {
      if (
        activePointerId === null
        || (pointerId !== undefined && pointerId !== activePointerId)
      ) return;
      const completedPointerId = activePointerId;
      activePointerId = null;
      const captureTarget = pointerCaptureTarget;
      pointerCaptureTarget = null;
      const completedDrag = pointerMoved;
      pointerStart = null;
      pointerMoved = false;
      try {
        if (captureTarget?.hasPointerCapture(completedPointerId)) {
          captureTarget.releasePointerCapture(completedPointerId);
        }
      } catch {
        // WebKit can release capture before dispatching lostpointercapture.
      }
      if (!openMenu || Date.now() < forceClickSuppressedUntilRef.current) {
        cancelPendingSelectionMenu();
        return;
      }
      const range = selectedRange(doc);
      const expanded = range
        ? expandRangeToWordBoundaries(range, doc.documentElement.lang || undefined)
        : null;
      if (expanded) {
        selectionNormalizationUntil = Date.now() + 80;
        replaceDocumentSelection(doc, expanded);
        selectionSnapshot = snapshotSelectionRange(expanded);
      }
      if (expanded) scheduleSelectionMenu(30, true);
      else {
        cancelPendingSelectionMenu();
        if (completedDrag) showMissingPdfTextIntent();
      }
    };

    doc.addEventListener("pointerdown", (event: PointerEvent) => {
      if (event.button !== 0) return;
      activePointerId = event.pointerId;
      pointerCaptureTarget = event.target as Element | null;
      pointerStart = { x: event.clientX, y: event.clientY };
      pointerMoved = false;
      try {
        pointerCaptureTarget?.setPointerCapture(event.pointerId);
      } catch {
        // Some iframe surfaces reject capture; document/window listeners remain active.
      }
      cancelPendingSelectionMenu();
    });
    doc.addEventListener("pointermove", (event: PointerEvent) => {
      if (event.pointerId !== activePointerId || !pointerStart) return;
      if (Math.hypot(event.clientX - pointerStart.x, event.clientY - pointerStart.y) >= 5) {
        pointerMoved = true;
      }
    });
    doc.addEventListener("pointerup", (event: PointerEvent) => {
      finalizePointerSelection(event.pointerId);
    });
    doc.addEventListener("pointercancel", (event: PointerEvent) => {
      finalizePointerSelection(event.pointerId, false);
    });
    doc.addEventListener("lostpointercapture", (event: PointerEvent) => {
      finalizePointerSelection(event.pointerId);
    });
    const contentWindow = doc.defaultView;
    const handleContentPointerUp = (event: PointerEvent) => {
      finalizePointerSelection(event.pointerId);
    };
    const handleContentPointerCancel = (event: PointerEvent) => {
      finalizePointerSelection(event.pointerId, false);
    };
    const handleContentBlur = () => {
      if (window.document.hasFocus()) return;
      finalizePointerSelection(undefined, false);
    };
    const handleHostPointerUp = () => finalizePointerSelection();
    const handleHostPointerCancel = () => finalizePointerSelection(undefined, false);
    const handleHostBlur = () => finalizePointerSelection(undefined, false);
    contentWindow?.addEventListener("pointerup", handleContentPointerUp);
    contentWindow?.addEventListener("pointercancel", handleContentPointerCancel);
    contentWindow?.addEventListener("blur", handleContentBlur);
    if (contentWindow && contentWindow !== window) {
      window.addEventListener("pointerup", handleHostPointerUp);
      window.addEventListener("pointercancel", handleHostPointerCancel);
      window.addEventListener("blur", handleHostBlur);
      contentWindow?.addEventListener("unload", () => {
        window.removeEventListener("pointerup", handleHostPointerUp);
        window.removeEventListener("pointercancel", handleHostPointerCancel);
        window.removeEventListener("blur", handleHostBlur);
      }, { once: true });
    }
    doc.addEventListener("contextmenu", (event: MouseEvent) => {
      cancelPendingWordClick();
      cancelPendingSelectionMenu();
      const interaction = interactionForSelection("selection-menu");
      if (!interaction) {
        if (showMissingPdfTextIntent()) {
          event.preventDefault();
          event.stopPropagation();
        }
        return;
      }
      event.preventDefault();
      openLearningInteraction(interaction);
    });
    const preserveSystemForceClick = () => {
      forceClickSuppressedUntilRef.current = Date.now() + 600;
      cancelPendingWordClick();
      cancelPendingSelectionMenu();
    };
    doc.addEventListener("webkitmouseforcedown", preserveSystemForceClick);

    doc.addEventListener("keydown", (event: KeyboardEvent) => {
      if ((event.target as Element | null)?.closest("input,textarea,select,[contenteditable='true']")) return;
      if (forwardReaderContextMenuKey(event)) {
        event.preventDefault();
        event.stopPropagation();
        return;
      }
      const trigger = bindingFromKeyboardEvent(event);
      if (trigger) {
        const interaction = interactionForSelection("selection-menu");
        if (!interaction && showMissingPdfTextIntent()) {
          event.preventDefault();
          event.stopPropagation();
          return;
        }
        if (handleReaderBinding(trigger, interaction)) {
          event.preventDefault();
          event.stopPropagation();
          return;
        }
      }
      if ((event.metaKey || event.ctrlKey) && event.key === "[") {
        event.preventDefault();
        view.history.back();
      } else if ((event.metaKey || event.ctrlKey) && event.key === "]") {
        event.preventDefault();
        view.history.forward();
      } else if ((event.metaKey || event.ctrlKey) && (event.key === "=" || event.key === "+")) {
        event.preventDefault();
        if (bookFormat === "pdf") handleZoom(10);
      } else if ((event.metaKey || event.ctrlKey) && event.key === "-") {
        event.preventDefault();
        if (bookFormat === "pdf") handleZoom(-10);
      } else handlePageTurnKeyDown(event);
    });
    doc.addEventListener("mousedown", (event: MouseEvent) => {
      const range = selectedRange(doc);
      if (range) selectionSnapshot = snapshotSelectionRange(range);
      handlePageTurnMouseDown(event);
    }, true);
    doc.addEventListener("contextmenu", handlePageTurnContextMenu, true);
    doc.addEventListener("wheel", handlePageTurnWheel, { passive: false });

    doc.addEventListener("click", (event: MouseEvent) => {
      if (annotationClickDocumentRef.current === doc) return;
      setContextMenu(null);
      cancelPendingWordClick();
      if (Date.now() < forceClickSuppressedUntilRef.current) return;
      if (
        !supportsSelection
        || event.button !== 0
        || event.metaKey
        || event.ctrlKey
        || event.altKey
        || event.shiftKey
      ) return;
      if (isInteractiveReaderTarget(event.target)) return;
      const selection = doc.getSelection?.();
      if (selection && !selection.isCollapsed) return;
      if (showMissingPdfTextIntent()) return;
      const selectionRange = rangeFromSelectionSnapshotAtPoint(
        selectionSnapshot,
        event.clientX,
        event.clientY,
      );
      const range = selectionRange ?? wordRangeAtPoint(
        doc,
        event.clientX,
        event.clientY,
        doc.documentElement.lang || undefined,
      );
      if (!range) {
        selectionSnapshot = null;
        return;
      }
      replaceDocumentSelection(doc, range);
      selectionSnapshot = snapshotSelectionRange(range);
      const text = range.toString().trim();
      const location = view.getCFI(index, range);
      const normalizedText = normalizeInteractionText(text);
      if (!text || !normalizedText || !location) return;
      const interaction: ReaderInteraction = {
        trigger: selectionRange ? "selection-menu" : "word-menu",
        kind: selectionRange
          ? classifySelection(text, doc.documentElement.lang || undefined)
          : "word",
        text,
        normalizedText,
        context: contextForRange(range, text),
        location,
        anchorRect: viewportRectForRange(range),
        source: "foliate",
        format: bookFormat === "pdf" ? "pdf" : "epub",
        locale: doc.documentElement.lang || undefined,
      };
      pendingWordClickRef.current = window.setTimeout(() => {
        pendingWordClickRef.current = null;
        openLearningInteraction(interaction);
      }, 240);
    });
    doc.addEventListener("dblclick", (event: MouseEvent) => {
      cancelPendingWordClick();
      cancelPendingSelectionMenu();
      if (!supportsSelection || isInteractiveReaderTarget(event.target)) return;
      if (showMissingPdfTextIntent()) {
        event.preventDefault();
        return;
      }
      const range = rangeFromSelectionSnapshotAtPoint(
        selectionSnapshot,
        event.clientX,
        event.clientY,
      ) ?? wordRangeAtPoint(
        doc,
        event.clientX,
        event.clientY,
        doc.documentElement.lang || undefined,
      );
      if (!range) return;
      const text = range.toString().trim();
      const location = view.getCFI(index, range);
      const normalizedText = normalizeInteractionText(text);
      if (!text || !normalizedText || !location) return;
      const interaction: ReaderInteraction = {
        trigger: "word-quick-lookup",
        kind: classifySelection(text, doc.documentElement.lang || undefined),
        text,
        normalizedText,
        context: contextForRange(range, text),
        location,
        anchorRect: viewportRectForRange(range),
        source: "foliate",
        format: bookFormat === "pdf" ? "pdf" : "epub",
        locale: doc.documentElement.lang || undefined,
      };
      if (!doubleClickQuickLookupRef.current) {
        if (handleReaderBinding("mouse:double", interaction)) event.preventDefault();
        return;
      }
      event.preventDefault();
      replaceDocumentSelection(doc, range);
      selectionSnapshot = snapshotSelectionRange(range);
      openLearningInteraction(interaction);
    });

    doc.addEventListener("mousedown", () => {
      const contents = view.renderer?.getContents?.() ?? [];
      for (const { doc: otherDoc } of contents) {
        if (otherDoc && otherDoc !== doc) {
          otherDoc.defaultView?.getSelection()?.removeAllRanges();
        }
      }
    });
  }, [
    annotationClickDocumentRef,
    cancelPendingSelectionMenu,
    cancelPendingWordClick,
    doubleClickQuickLookupRef,
    forceClickSuppressedUntilRef,
    handlePageTurnContextMenu,
    handleReaderBinding,
    handlePageTurnKeyDown,
    handlePageTurnMouseDown,
    handlePageTurnWheel,
    handleZoom,
    openLearningInteraction,
    pendingSelectionMenuRef,
    pendingWordClickRef,
    readerInteractionGenerationRef,
    setContextMenu,
    onMissingPdfTextIntent,
    supportsSelection,
  ]);

  return installDocumentInteractions;
}
