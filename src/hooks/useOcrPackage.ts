import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { OcrAssetsOverview, OcrPackageStatus } from "../ocr/types";
import { errorCodeFromUnknown } from "../ocr/types";

const EMPTY_OVERVIEW: OcrAssetsOverview = { totalBytes: 0, items: [] };

export function useOcrPackage(enabled = true) {
  const [status, setStatus] = useState<OcrPackageStatus | null>(null);
  const [loading, setLoading] = useState(enabled);
  const [action, setAction] = useState<"download" | "cancel" | "uninstall" | null>(null);
  const [errorCode, setErrorCode] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!enabled) return null;
    try {
      const next = await invoke<OcrPackageStatus>("ocr_package_status");
      setStatus(next);
      setErrorCode(null);
      return next;
    } catch (error) {
      setErrorCode(errorCodeFromUnknown(error));
      return null;
    } finally {
      setLoading(false);
    }
  }, [enabled]);

  useEffect(() => {
    if (!enabled) {
      setLoading(false);
      return;
    }
    setLoading(true);
    void refresh();
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<OcrPackageStatus>("ocr-package-changed", () => {
      if (!disposed) void refresh();
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [enabled, refresh]);

  const run = useCallback(async (nextAction: NonNullable<typeof action>, command: string) => {
    setAction(nextAction);
    setErrorCode(null);
    try {
      await invoke(command);
      await refresh();
    } catch (error) {
      setErrorCode(errorCodeFromUnknown(error));
      throw error;
    } finally {
      setAction(null);
    }
  }, [refresh]);

  return {
    status,
    loading,
    action,
    errorCode,
    refresh,
    download: () => run("download", "ocr_package_download"),
    cancel: () => run("cancel", "ocr_package_cancel"),
    uninstall: () => run("uninstall", "ocr_package_uninstall"),
  };
}

export function useOcrAssetsOverview(enabled = true) {
  const [overview, setOverview] = useState<OcrAssetsOverview>(EMPTY_OVERVIEW);
  const [loading, setLoading] = useState(enabled);
  const [deletingAssetId, setDeletingAssetId] = useState<string | null>(null);
  const [errorCode, setErrorCode] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!enabled) return null;
    try {
      const next = await invoke<OcrAssetsOverview>("ocr_assets_overview");
      setOverview(next);
      setErrorCode(null);
      return next;
    } catch (error) {
      setErrorCode(errorCodeFromUnknown(error));
      return null;
    } finally {
      setLoading(false);
    }
  }, [enabled]);

  useEffect(() => {
    if (!enabled) {
      setLoading(false);
      return;
    }
    setLoading(true);
    void refresh();
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen("book-assets-changed", () => {
      if (!disposed) void refresh();
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [enabled, refresh]);

  const deleteAsset = useCallback(async (assetId: string, allDevices: boolean) => {
    setDeletingAssetId(assetId);
    setErrorCode(null);
    try {
      await invoke("ocr_asset_delete", { assetId, allDevices });
      await refresh();
    } catch (error) {
      setErrorCode(errorCodeFromUnknown(error));
      throw error;
    } finally {
      setDeletingAssetId(null);
    }
  }, [refresh]);

  return { overview, loading, deletingAssetId, errorCode, refresh, deleteAsset };
}
