export type OcrPackageState =
  | "not_installed"
  | "downloading"
  | "verifying"
  | "installing"
  | "installed"
  | "uninstalling"
  | "failed";

export interface OcrPackageStatus {
  state: OcrPackageState;
  version?: string;
  downloadedBytes?: number;
  totalBytes?: number;
  installedBytes?: number;
  errorCode?: string;
}

export type OcrJobState =
  | "queued"
  | "waiting_source"
  | "preparing"
  | "recognizing"
  | "validating"
  | "publishing"
  | "ready"
  | "failed"
  | "cancelled";

export interface OcrJobView {
  state: OcrJobState;
  pagesDone?: number;
  pagesTotal?: number;
  errorCode?: string;
}

export interface OcrAssetItem {
  assetId: string;
  bookId: string;
  title: string;
  byteSize: number;
  createdAt: number;
  availability: string;
}

export interface OcrAssetsOverview {
  totalBytes: number;
  items: OcrAssetItem[];
}

export const OCR_ACTIVE_JOB_STATES: ReadonlySet<OcrJobState> = new Set([
  "queued",
  "waiting_source",
  "preparing",
  "recognizing",
  "validating",
  "publishing",
]);

export function errorCodeFromUnknown(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  try {
    return JSON.stringify(error);
  } catch {
    return String(error);
  }
}

export function formatOcrBytes(value: number | undefined): string {
  if (value === undefined || !Number.isFinite(value) || value < 0) return "--";
  if (value < 1024) return `${Math.round(value)} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let amount = value / 1024;
  let unit = 0;
  while (amount >= 1024 && unit < units.length - 1) {
    amount /= 1024;
    unit += 1;
  }
  const digits = amount >= 100 ? 0 : amount >= 10 ? 1 : 2;
  return `${amount.toFixed(digits)} ${units[unit]}`;
}
