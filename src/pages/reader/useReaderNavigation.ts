import {
  useEffect,
  useRef,
  type Dispatch,
  type MutableRefObject,
  type SetStateAction,
} from "react";
import { emit } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { FoliateView, ReaderNavigation } from "./foliate-types";

type SidePanel = "ai" | "bookmarks" | "vocab" | null;

interface UseReaderNavigationOptions {
  bookId?: string;
  bookReady: boolean;
  isTextBook: boolean;
  supportsCfiNavigation: boolean;
  textNavigationRegistration: number;
  viewRef: MutableRefObject<FoliateView | null>;
  textReaderNavigateRef: MutableRefObject<((location: string, flash?: boolean) => void) | null>;
  refreshAnnotations(): Promise<void>;
  setSidePanel: Dispatch<SetStateAction<SidePanel>>;
  setInitialChatId: Dispatch<SetStateAction<string | undefined>>;
}

export function useReaderNavigation({
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
}: UseReaderNavigationOptions) {
  const pendingNavigationRef = useRef<ReaderNavigation | null>(null);

  useEffect(() => {
    const applyNavigation = async (target: ReaderNavigation) => {
      if (
        !bookReady
        || (!isTextBook && !viewRef.current)
        || (isTextBook && !textReaderNavigateRef.current)
      ) {
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

    const appWindow = getCurrentWebviewWindow();
    const unlisten = Promise.all([
      appWindow.listen<{ bookId?: string }>("lookup-record-changed", (event) => {
        if (!event.payload.bookId || event.payload.bookId === bookId) {
          refreshAnnotations().catch(() => {});
        }
      }),
      appWindow.listen<{ bookId?: string }>("vocab-changed", (event) => {
        if (!event.payload.bookId || event.payload.bookId === bookId) {
          refreshAnnotations().catch(() => {});
        }
      }),
      appWindow.listen<ReaderNavigation>("reader:navigate", (event) => {
        applyNavigation(event.payload).catch(() => {});
      }),
    ]);
    const pending = pendingNavigationRef.current;
    if (pending) applyNavigation(pending).catch(() => {});
    return () => {
      unlisten.then((callbacks) => callbacks.forEach((callback) => callback())).catch(() => {});
    };
  }, [
    bookId,
    bookReady,
    isTextBook,
    refreshAnnotations,
    setInitialChatId,
    setSidePanel,
    supportsCfiNavigation,
    textNavigationRegistration,
    textReaderNavigateRef,
    viewRef,
  ]);
}
