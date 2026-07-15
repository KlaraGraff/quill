import { useEffect } from "react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

const readerWindow = getCurrentWebviewWindow();

export function useWindowSizePersistence(bookId: string | undefined, enabled: boolean): void {
  useEffect(() => {
    if (!enabled || !bookId) return;
    let timer: number | null = null;
    const unlistenPromise = readerWindow.onResized(({ payload }) => {
      if (timer !== null) window.clearTimeout(timer);
      timer = window.setTimeout(async () => {
        try {
          const scale = await readerWindow.scaleFactor();
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
      unlistenPromise.then((unlisten) => unlisten()).catch(() => {});
    };
  }, [bookId, enabled]);
}
