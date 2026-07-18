import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { OcrJobView } from "../ocr/types";
import { errorCodeFromUnknown } from "../ocr/types";

export function useOcrJob(bookId: string | undefined, enabled = true) {
  const [job, setJob] = useState<OcrJobView | null>(null);
  const [loading, setLoading] = useState(Boolean(bookId && enabled));
  const [action, setAction] = useState<"start" | "cancel" | "retry" | null>(null);
  const [errorCode, setErrorCode] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!bookId || !enabled) return null;
    try {
      const next = await invoke<OcrJobView | null>("ocr_job_status", { bookId });
      setJob(next);
      setErrorCode(null);
      return next;
    } catch (error) {
      setErrorCode(errorCodeFromUnknown(error));
      return null;
    } finally {
      setLoading(false);
    }
  }, [bookId, enabled]);

  useEffect(() => {
    if (!bookId || !enabled) {
      setJob(null);
      setLoading(false);
      return;
    }
    setLoading(true);
    void refresh();
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<OcrJobView>("ocr-job-changed", () => {
      if (!disposed) void refresh();
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [bookId, enabled, refresh]);

  const run = useCallback(async (nextAction: NonNullable<typeof action>, command: string) => {
    if (!bookId) return;
    setAction(nextAction);
    setErrorCode(null);
    try {
      await invoke(command, { bookId });
      await refresh();
    } catch (error) {
      setErrorCode(errorCodeFromUnknown(error));
      throw error;
    } finally {
      setAction(null);
    }
  }, [bookId, refresh]);

  return {
    job,
    loading,
    action,
    errorCode,
    refresh,
    start: () => run("start", "ocr_start"),
    cancel: () => run("cancel", "ocr_cancel"),
    retry: () => run("retry", "ocr_retry"),
  };
}
