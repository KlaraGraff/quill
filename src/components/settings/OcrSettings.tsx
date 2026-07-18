import { useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  AlertCircle,
  Check,
  Copy,
  Download,
  HardDrive,
  Loader2,
  RotateCcw,
  Trash2,
} from "lucide-react";
import { useOcrAssetsOverview, useOcrPackage } from "../../hooks/useOcrPackage";
import { formatOcrBytes, type OcrAssetItem, type OcrPackageStatus } from "../../ocr/types";
import Button from "../ui/Button";
import ConfirmDialog from "./ConfirmDialog";
import { ROW_CONTROL_WIDTH } from "./types";

type Translate = ReturnType<typeof useTranslation>["t"];

function packageStateLabel(status: OcrPackageStatus | null, fallbackError: string | null, t: Translate) {
  if (fallbackError && !status) return t("ocr.package.state.unavailable");
  return t(`ocr.package.state.${status?.state ?? "loading"}`);
}

function packageSummary(status: OcrPackageStatus | null, t: Translate) {
  if (!status) return t("ocr.package.querying");
  if (status.state === "installed") {
    return t("ocr.package.installedSummary", {
      version: status.version ?? t("ocr.common.unknown"),
      size: formatOcrBytes(status.installedBytes),
    });
  }
  if (status.state === "downloading") {
    return t("ocr.package.downloadSummary", {
      downloaded: formatOcrBytes(status.downloadedBytes),
      total: formatOcrBytes(status.totalBytes),
    });
  }
  return t(`ocr.package.hint.${status.state}`);
}

function availabilityLabel(item: OcrAssetItem, t: Translate) {
  return t(`ocr.asset.availability.${item.availability}`, {
    defaultValue: t("ocr.asset.availability.unknown"),
  });
}

export default function OcrSettings() {
  const { t, i18n } = useTranslation();
  const packageState = useOcrPackage();
  const assets = useOcrAssetsOverview();
  const downloadButtonRef = useRef<HTMLButtonElement>(null);
  const [copied, setCopied] = useState(false);
  const [pendingDelete, setPendingDelete] = useState<OcrAssetItem | null>(null);
  const [confirmUninstall, setConfirmUninstall] = useState(false);
  const packageError = packageState.status?.errorCode ?? packageState.errorCode;
  const progress = useMemo(() => {
    const downloaded = packageState.status?.downloadedBytes;
    const total = packageState.status?.totalBytes;
    if (downloaded === undefined || total === undefined || total <= 0) return null;
    return Math.max(0, Math.min(100, (downloaded / total) * 100));
  }, [packageState.status?.downloadedBytes, packageState.status?.totalBytes]);

  const runPackageAction = (action: () => Promise<void>) => {
    void action().catch(() => {});
  };

  const renderPackageAction = () => {
    const status = packageState.status;
    if (packageState.loading || packageState.action) {
      return (
        <div className={`${ROW_CONTROL_WIDTH} flex justify-end`}>
          <Loader2 size={16} className="animate-spin text-text-muted" aria-label={t("ocr.common.working")} />
        </div>
      );
    }
    if (!status || status.state === "not_installed" || status.state === "failed") {
      return (
        <div className={`${ROW_CONTROL_WIDTH} flex justify-end`}>
          <Button
            ref={downloadButtonRef}
            size="sm"
            onClick={() => runPackageAction(packageState.download)}
          >
            {status?.state === "failed" ? <RotateCcw size={14} /> : <Download size={14} />}
            {status?.state === "failed" ? t("ocr.actions.retry") : t("ocr.actions.download")}
          </Button>
        </div>
      );
    }
    if (status.state === "downloading") {
      return (
        <div className={`${ROW_CONTROL_WIDTH} flex justify-end`}>
          <Button variant="secondary" size="sm" onClick={() => runPackageAction(packageState.cancel)}>
            {t("ocr.actions.cancelDownload")}
          </Button>
        </div>
      );
    }
    if (status.state === "installed") {
      return (
        <div className={`${ROW_CONTROL_WIDTH} flex justify-end`}>
          <Button variant="secondary" size="sm" onClick={() => setConfirmUninstall(true)}>
            <Trash2 size={14} />
            {t("ocr.actions.uninstall")}
          </Button>
        </div>
      );
    }
    return <div className={ROW_CONTROL_WIDTH} />;
  };

  return (
    <div className="mx-auto w-full max-w-[620px] pb-10">
      <section aria-labelledby="ocr-package-title">
        <div className="flex min-h-[73px] items-center justify-between gap-4 border-b border-border-light py-2">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              {packageState.status?.state === "installed" ? (
                <Check size={14} className="shrink-0 text-success-text" />
              ) : packageError ? (
                <AlertCircle size={14} className="shrink-0 text-danger-text" />
              ) : (
                <HardDrive size={14} className="shrink-0 text-text-muted" />
              )}
              <p id="ocr-package-title" className="text-[14px] font-medium text-text-primary">
                {t("ocr.package.title")}
              </p>
            </div>
            <p className="mt-0.5 break-words text-[12px] leading-5 text-text-muted">
              {packageState.errorCode && !packageState.status
                ? t("ocr.package.unavailableHint")
                : packageSummary(packageState.status, t)}
            </p>
          </div>
          <div className="flex shrink-0 flex-col items-end gap-1.5">
            <span className="text-[11px] font-medium text-text-secondary">
              {packageStateLabel(packageState.status, packageState.errorCode, t)}
            </span>
            {renderPackageAction()}
          </div>
        </div>

        {packageState.status?.state === "downloading" && (
          <div className="border-b border-border-light py-3">
            <div className="h-1.5 overflow-hidden rounded-full bg-bg-muted">
              <div
                className={`h-full bg-accent transition-[width] duration-200 ${progress === null ? "w-[18%] animate-pulse" : ""}`}
                style={progress === null ? undefined : { width: `${progress}%` }}
              />
            </div>
          </div>
        )}

        {packageError && (
          <div className="border-b border-border-light py-3">
            <div className="flex items-start justify-between gap-3 rounded-md bg-danger-bg px-3 py-2.5">
              <div className="min-w-0">
                <p className="text-[12px] font-medium text-danger-text">{t("ocr.package.errorTitle")}</p>
                <p className="mt-0.5 break-all font-mono text-[10px] leading-4 text-danger-text/80">
                  {packageError}
                </p>
              </div>
              <button
                type="button"
                title={t("ocr.actions.copyError")}
                aria-label={t("ocr.actions.copyError")}
                onClick={() => {
                  void navigator.clipboard.writeText(packageError).then(() => {
                    setCopied(true);
                    window.setTimeout(() => setCopied(false), 1500);
                  });
                }}
                className="flex size-8 shrink-0 items-center justify-center rounded-md text-danger-text hover:bg-bg-surface"
              >
                {copied ? <Check size={14} /> : <Copy size={14} />}
              </button>
            </div>
          </div>
        )}
      </section>

      <section aria-labelledby="ocr-storage-title" className="pt-5">
        <div className="flex items-end justify-between gap-4 border-b border-border-light pb-2">
          <div>
            <h4 id="ocr-storage-title" className="text-[13px] font-semibold text-text-primary">
              {t("ocr.storage.title")}
            </h4>
            <p className="mt-0.5 text-[11px] leading-4 text-text-muted">{t("ocr.storage.hint")}</p>
          </div>
          <span className="shrink-0 text-[12px] font-medium text-text-secondary">
            {assets.loading ? t("ocr.common.loading") : formatOcrBytes(assets.overview.totalBytes)}
          </span>
        </div>

        {!assets.loading && assets.overview.items.length === 0 && (
          <p className="py-5 text-center text-[12px] text-text-muted">{t("ocr.storage.empty")}</p>
        )}

        {assets.overview.items.map((item) => (
          <div key={item.assetId} className="flex min-h-[62px] items-center justify-between gap-3 border-b border-border-light py-2">
            <div className="min-w-0 flex-1">
              <p className="truncate text-[13px] font-medium text-text-primary">{item.title}</p>
              <p className="mt-0.5 text-[11px] text-text-muted">
                {formatOcrBytes(item.byteSize)} · {availabilityLabel(item, t)} · {new Intl.DateTimeFormat(i18n.language, {
                  dateStyle: "medium",
                }).format(new Date(item.createdAt))}
              </p>
            </div>
            <div className="flex shrink-0 items-center gap-1">
              <button
                type="button"
                title={t("ocr.storage.deleteLocal")}
                aria-label={t("ocr.storage.deleteLocalBook", { title: item.title })}
                disabled={assets.deletingAssetId === item.assetId}
                onClick={() => setPendingDelete(item)}
                className="flex size-8 items-center justify-center rounded-md text-text-muted hover:bg-bg-input hover:text-danger-text disabled:opacity-50"
              >
                {assets.deletingAssetId === item.assetId
                  ? <Loader2 size={14} className="animate-spin" />
                  : <Trash2 size={14} />}
              </button>
              <button
                type="button"
                title={t("ocr.storage.deleteAllUnavailable")}
                aria-label={t("ocr.storage.deleteAll")}
                disabled
                className="flex size-8 items-center justify-center rounded-md text-text-muted opacity-35"
              >
                <HardDrive size={14} />
              </button>
            </div>
          </div>
        ))}

        {assets.errorCode && (
          <p role="alert" className="mt-3 break-all text-[11px] leading-4 text-danger-text">
            {t("ocr.storage.loadFailed")}: {assets.errorCode}
          </p>
        )}
      </section>

      {pendingDelete && (
        <ConfirmDialog
          title={t("ocr.storage.deleteLocalTitle")}
          description={t("ocr.storage.deleteLocalDescription", { title: pendingDelete.title })}
          primaryLabel={t("ocr.storage.deleteLocal")}
          onPrimary={() => {
            const item = pendingDelete;
            setPendingDelete(null);
            void assets.deleteAsset(item.assetId, false).catch(() => {});
          }}
          secondaryLabel={t("common.cancel")}
          onSecondary={() => setPendingDelete(null)}
        />
      )}

      {confirmUninstall && (
        <ConfirmDialog
          title={t("ocr.package.uninstallTitle")}
          description={t("ocr.package.uninstallDescription")}
          primaryLabel={t("ocr.actions.uninstall")}
          onPrimary={() => {
            setConfirmUninstall(false);
            runPackageAction(packageState.uninstall);
          }}
          secondaryLabel={t("common.cancel")}
          onSecondary={() => setConfirmUninstall(false)}
        />
      )}
    </div>
  );
}
