import type { ReactNode } from "react";
import { AlertCircle, Download, Loader2, RotateCcw, ScanText, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import Button from "./ui/Button";
import type { OcrJobView, OcrPackageStatus } from "../ocr/types";
import { formatOcrBytes } from "../ocr/types";

interface OcrReaderHudProps {
  packageStatus: OcrPackageStatus | null;
  packageError: string | null;
  job: OcrJobView | null;
  jobError: string | null;
  busyAction: "start" | "cancel" | "retry" | null;
  onOpenSettings(): void;
  onStart(): void;
  onCancel(): void;
  onRetry(): void;
  onDismiss(): void;
}

type Translate = ReturnType<typeof useTranslation>["t"];

function progressLabel(job: OcrJobView, t: Translate) {
  if (job.pagesDone !== undefined && job.pagesTotal !== undefined && job.pagesTotal > 0) {
    return t("ocr.reader.pageProgress", { done: job.pagesDone, total: job.pagesTotal });
  }
  return t("ocr.reader.working");
}

export default function OcrReaderHud({
  packageStatus,
  packageError,
  job,
  jobError,
  busyAction,
  onOpenSettings,
  onStart,
  onCancel,
  onRetry,
  onDismiss,
}: OcrReaderHudProps) {
  const { t } = useTranslation();
  let icon = <ScanText size={15} className="shrink-0 text-accent-text" />;
  let message = t("ocr.reader.installPrompt");
  let action: ReactNode = (
    <Button size="sm" onClick={onOpenSettings}>
      <Download size={14} />
      {t("ocr.reader.goToDownload")}
    </Button>
  );

  if (packageStatus?.state === "downloading") {
    icon = <Loader2 size={15} className="shrink-0 animate-spin text-accent-text" />;
    message = t("ocr.reader.packageDownloading", {
      downloaded: formatOcrBytes(packageStatus.downloadedBytes),
      total: formatOcrBytes(packageStatus.totalBytes),
    });
    action = <Button variant="secondary" size="sm" onClick={onOpenSettings}>{t("ocr.reader.viewProgress")}</Button>;
  } else if (packageStatus && ["verifying", "installing", "uninstalling"].includes(packageStatus.state)) {
    icon = <Loader2 size={15} className="shrink-0 animate-spin text-accent-text" />;
    message = t(`ocr.reader.package.${packageStatus.state}`);
    action = <Button variant="secondary" size="sm" onClick={onOpenSettings}>{t("ocr.reader.viewProgress")}</Button>;
  } else if (packageStatus?.state === "failed" || packageError) {
    icon = <AlertCircle size={15} className="shrink-0 text-danger-text" />;
    message = t("ocr.reader.packageFailed");
    action = <Button size="sm" onClick={onOpenSettings}><RotateCcw size={14} />{t("ocr.reader.viewDetails")}</Button>;
  } else if (packageStatus?.state === "installed") {
    if (!job || job.state === "cancelled") {
      message = t("ocr.reader.startPrompt");
      action = <Button size="sm" disabled={busyAction !== null} onClick={onStart}>{t("ocr.reader.start")}</Button>;
    } else if (job.state === "queued" || job.state === "waiting_source") {
      icon = <Loader2 size={15} className="shrink-0 animate-spin text-accent-text" />;
      message = job.state === "queued" ? t("ocr.reader.queued") : t("ocr.reader.waitingSource");
      action = <Button variant="secondary" size="sm" disabled={busyAction !== null} onClick={onCancel}>{t("ocr.actions.cancel")}</Button>;
    } else if (["preparing", "recognizing"].includes(job.state)) {
      icon = <Loader2 size={15} className="shrink-0 animate-spin text-accent-text" />;
      message = job.state === "recognizing" ? progressLabel(job, t) : t("ocr.reader.preparing");
      action = <Button variant="secondary" size="sm" disabled={busyAction !== null} onClick={onCancel}>{t("ocr.actions.cancel")}</Button>;
    } else if (["validating", "publishing"].includes(job.state)) {
      icon = <Loader2 size={15} className="shrink-0 animate-spin text-accent-text" />;
      message = job.state === "validating" ? t("ocr.reader.validating") : t("ocr.reader.publishing");
      action = null;
    } else if (job.state === "failed" || jobError) {
      icon = <AlertCircle size={15} className="shrink-0 text-danger-text" />;
      message = t("ocr.reader.failed");
      action = (
        <>
          <Button variant="secondary" size="sm" onClick={onOpenSettings}>{t("ocr.reader.viewDetails")}</Button>
          <Button size="sm" disabled={busyAction !== null} onClick={onRetry}><RotateCcw size={14} />{t("ocr.actions.retry")}</Button>
        </>
      );
    } else if (job.state === "ready") {
      return null;
    }
  }

  return (
    <div
      role="status"
      aria-live="polite"
      className="pointer-events-auto flex max-w-[min(680px,calc(100%_-_24px))] items-center gap-3 rounded-md border border-border bg-bg-surface px-3 py-2 shadow-popover"
    >
      {icon}
      <p className="min-w-0 flex-1 text-[12px] leading-5 text-text-secondary">{message}</p>
      <div className="flex shrink-0 items-center gap-2">{action}</div>
      <button
        type="button"
        title={t("common.close")}
        aria-label={t("common.close")}
        onClick={onDismiss}
        className="flex size-7 shrink-0 items-center justify-center rounded-md text-text-muted hover:bg-bg-input"
      >
        <X size={14} />
      </button>
    </div>
  );
}
