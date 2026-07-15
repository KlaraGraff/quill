import {
  useCallback,
  useEffect,
  useRef,
  type MutableRefObject,
  type RefObject,
} from "react";
import type { ReaderSettingsState } from "../../components/ReaderSettings";
import {
  keyboardEventMatchesBinding,
  mouseEventMatchesBinding,
} from "../../components/page-turn-bindings";

type PageDirection = "previous" | "next";

interface PageTurnInputOptions {
  bookFormat?: string;
  settingsRef: MutableRefObject<ReaderSettingsState>;
  readerViewportRef: RefObject<HTMLElement | null>;
  panelRef: RefObject<HTMLElement | null>;
  overlayOpen: boolean;
  sidePanelOpen: boolean;
  turnPage(direction: PageDirection): void;
  onPdfZoom(delta: number): void;
}

function isEditableEventTarget(target: EventTarget | null): boolean {
  const element = target as HTMLElement | null;
  return Boolean(element?.closest?.("input, textarea, select, [contenteditable='true'], [role='textbox']"));
}

export function usePageTurnInput({
  bookFormat,
  settingsRef,
  readerViewportRef,
  panelRef,
  overlayOpen,
  sidePanelOpen,
  turnPage,
  onPdfZoom,
}: PageTurnInputOptions) {
  const suppressContextMenuUntilRef = useRef(0);
  const keyboardBlockedRef = useRef(false);
  const overlayOpenRef = useRef(false);

  useEffect(() => {
    overlayOpenRef.current = overlayOpen;
  }, [overlayOpen]);

  useEffect(() => {
    if (!sidePanelOpen) keyboardBlockedRef.current = false;
  }, [sidePanelOpen]);

  const handlePageTurnKeyDown = useCallback((event: KeyboardEvent) => {
    if (event.defaultPrevented) return;
    const target = event.target as HTMLElement | null;
    if (target && panelRef.current?.contains(target)) {
      keyboardBlockedRef.current = true;
      return;
    }
    if (
      overlayOpenRef.current
      || keyboardBlockedRef.current
      || isEditableEventTarget(target)
    ) return;
    if (
      bookFormat === "pdf"
      && (event.metaKey || event.ctrlKey)
      && (event.key === "=" || event.key === "+" || event.key === "-")
    ) {
      event.preventDefault();
      event.stopPropagation();
      onPdfZoom(event.key === "-" ? -10 : 10);
      return;
    }
    if (target?.closest?.("button, a, [role='button'], [data-reader-settings]")) return;
    const settings = settingsRef.current;
    let direction: PageDirection | null = null;
    if (keyboardEventMatchesBinding(event, settings.previousPageBinding)) direction = "previous";
    else if (keyboardEventMatchesBinding(event, settings.nextPageBinding)) direction = "next";
    else if (!event.metaKey && !event.ctrlKey && !event.altKey && !event.shiftKey) {
      if (event.key === "ArrowLeft" || event.key === "ArrowUp" || event.key === "PageUp") direction = "previous";
      else if (event.key === "ArrowRight" || event.key === "ArrowDown" || event.key === "PageDown" || event.key === " ") direction = "next";
    } else if (!event.metaKey && !event.ctrlKey && !event.altKey && event.shiftKey && event.key === " ") {
      direction = "previous";
    }
    if (!direction) return;
    event.preventDefault();
    event.stopPropagation();
    turnPage(direction);
  }, [bookFormat, onPdfZoom, panelRef, settingsRef, turnPage]);

  const handlePageTurnMouseDown = useCallback((event: MouseEvent) => {
    keyboardBlockedRef.current = false;
    if (event.defaultPrevented || isEditableEventTarget(event.target)) return;
    const settings = settingsRef.current;
    const direction = mouseEventMatchesBinding(event, settings.previousPageBinding)
      ? "previous"
      : mouseEventMatchesBinding(event, settings.nextPageBinding) ? "next" : null;
    if (!direction) return;
    if (event.button === 2) suppressContextMenuUntilRef.current = Date.now() + 800;
    event.preventDefault();
    event.stopPropagation();
    turnPage(direction);
  }, [settingsRef, turnPage]);

  const handlePageTurnContextMenu = useCallback((event: MouseEvent) => {
    if (Date.now() > suppressContextMenuUntilRef.current) return;
    suppressContextMenuUntilRef.current = 0;
    event.preventDefault();
    event.stopPropagation();
  }, []);

  useEffect(() => {
    const viewport = readerViewportRef.current;
    if (!viewport) return;
    window.addEventListener("keydown", handlePageTurnKeyDown);
    viewport.addEventListener("mousedown", handlePageTurnMouseDown, true);
    viewport.addEventListener("contextmenu", handlePageTurnContextMenu, true);
    return () => {
      window.removeEventListener("keydown", handlePageTurnKeyDown);
      viewport.removeEventListener("mousedown", handlePageTurnMouseDown, true);
      viewport.removeEventListener("contextmenu", handlePageTurnContextMenu, true);
    };
  }, [handlePageTurnContextMenu, handlePageTurnKeyDown, handlePageTurnMouseDown, readerViewportRef]);

  const blockPageTurnKeyboard = useCallback(() => {
    keyboardBlockedRef.current = true;
  }, []);

  return {
    blockPageTurnKeyboard,
    handlePageTurnContextMenu,
    handlePageTurnKeyDown,
    handlePageTurnMouseDown,
  };
}
