import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Database, Loader2, RefreshCw, Save, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import Button from "./ui/Button";
import { createUuid } from "../utils/randomUuid";

interface IndexSummary {
  sectionIndex?: number | null;
  sectionTitle?: string | null;
  content: string;
  userEdited: boolean;
}

interface IndexDetails {
  status: "ready" | "building" | "failed" | "unsupported" | "missing";
  error?: string | null;
  chunkCount: number;
  embeddedCount: number;
  embeddingModel?: string | null;
  indexedAt?: number | null;
  overview?: IndexSummary | null;
  sections: IndexSummary[];
  chunks: Array<{ index: number; sectionTitle?: string | null; snippet: string }>;
}

export default function IndexManagerModal({ bookId, onClose }: { bookId: string; onClose(): void }) {
  const { t } = useTranslation();
  const [details, setDetails] = useState<IndexDetails | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [overviewDraft, setOverviewDraft] = useState("");
  const [sectionDrafts, setSectionDrafts] = useState<Record<number, string>>({});
  const hasEditedSummary = Boolean(details?.overview?.userEdited || details?.sections.some((section) => section.userEdited));

  const load = useCallback(async () => {
    const next = await invoke<IndexDetails>("ai_index_details", { bookId });
    setDetails(next);
    setOverviewDraft(next.overview?.content ?? "");
    setSectionDrafts(Object.fromEntries(next.sections
      .filter((section) => section.sectionIndex != null)
      .map((section) => [section.sectionIndex!, section.content])));
  }, [bookId]);

  useEffect(() => { void load().catch((reason) => setError(String(reason))); }, [load]);

  const run = async (name: string, action: () => Promise<unknown>) => {
    setBusy(name);
    setError(null);
    try {
      await action();
      await load();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="fixed inset-0 z-[70] flex items-center justify-center bg-overlay" onClick={(event) => event.target === event.currentTarget && onClose()}>
      <div role="dialog" aria-modal="true" className="flex max-h-[86vh] w-[min(760px,calc(100vw-32px))] flex-col overflow-hidden rounded-lg border border-border bg-bg-surface shadow-popover">
        <header className="flex items-center justify-between border-b border-border px-5 py-4">
          <div className="flex items-center gap-2">
            <Database size={17} className="text-accent-text" />
            <h3 className="text-[15px] font-semibold text-text-primary">{t("indexManager.title")}</h3>
          </div>
          <button type="button" onClick={onClose} aria-label={t("common.close")} className="flex size-7 items-center justify-center rounded-md hover:bg-bg-input"><X size={15} /></button>
        </header>
        <div className="flex-1 overflow-auto px-5 py-4">
          {!details ? <Loader2 size={18} className="animate-spin text-text-muted" /> : (
            <>
              <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
                {[
                  [t("indexManager.status"), t(`indexManager.status.${details.status}`)],
                  [t("indexManager.chunks"), details.chunkCount],
                  [t("indexManager.embeddings"), `${details.embeddedCount}/${details.chunkCount}`],
                  [t("indexManager.model"), details.embeddingModel || "-"],
                ].map(([label, value]) => <div key={String(label)} className="rounded-md border border-border p-3"><p className="text-[10px] text-text-muted">{label}</p><p className="mt-1 truncate text-[13px] font-medium text-text-primary">{value}</p></div>)}
              </div>
              {details.error && <p className="mt-3 text-[12px] text-danger-text">{details.error}</p>}
              <section className="mt-5">
                <div className="mb-2 flex items-center justify-between"><h4 className="text-[13px] font-medium text-text-primary">{t("indexManager.overview")}</h4>{details.overview?.userEdited && <span className="text-[10px] text-accent-text">{t("indexManager.edited")}</span>}</div>
                <textarea value={overviewDraft} onChange={(event) => setOverviewDraft(event.target.value)} className="min-h-28 w-full resize-y rounded-md border border-border bg-bg-input p-3 text-[13px] text-text-primary outline-none focus:border-accent" />
                <Button className="mt-2" size="sm" variant="secondary" disabled={!details.overview || busy != null} onClick={() => void run("overview", () => invoke("update_book_overview", { bookId, content: overviewDraft }))}><Save size={13} />{t("indexManager.saveOverview")}</Button>
              </section>
              <section className="mt-5 space-y-2">
                <h4 className="text-[13px] font-medium text-text-primary">{t("indexManager.sections")}</h4>
                {details.sections.map((section) => section.sectionIndex == null ? null : (
                  <details key={section.sectionIndex} className="rounded-md border border-border p-3">
                    <summary className="cursor-pointer text-[12px] font-medium text-text-primary">{section.sectionTitle || t("indexManager.section", { index: section.sectionIndex + 1 })}{section.userEdited ? ` · ${t("indexManager.edited")}` : ""}</summary>
                    <textarea value={sectionDrafts[section.sectionIndex] ?? ""} onChange={(event) => setSectionDrafts((current) => ({ ...current, [section.sectionIndex!]: event.target.value }))} className="mt-3 min-h-24 w-full resize-y rounded-md border border-border bg-bg-input p-3 text-[12px] text-text-primary outline-none focus:border-accent" />
                    <Button className="mt-2" size="sm" variant="secondary" disabled={busy != null} onClick={() => void run(`section-${section.sectionIndex}`, () => invoke("update_book_section_summary", { bookId, sectionIndex: section.sectionIndex, content: sectionDrafts[section.sectionIndex!] }))}><Save size={13} />{t("indexManager.saveSection")}</Button>
                  </details>
                ))}
              </section>
              <details className="mt-5"><summary className="cursor-pointer text-[13px] font-medium text-text-primary">{t("indexManager.chunkPreview")}</summary><div className="mt-2 space-y-2">{details.chunks.map((chunk) => <div key={chunk.index} className="rounded-md bg-bg-input p-3 text-[11px] leading-5 text-text-secondary"><p className="font-medium text-text-primary">{chunk.sectionTitle || `#${chunk.index + 1}`}</p>{chunk.snippet}</div>)}</div></details>
            </>
          )}
          {error && <p role="alert" className="mt-3 text-[12px] text-danger-text">{error}</p>}
        </div>
        <footer className="flex flex-wrap justify-end gap-2 border-t border-border px-5 py-3">
          <Button variant="secondary" size="sm" disabled={busy != null} onClick={() => void run("update", () => invoke("ai_update_book_index", { bookId }))}>{busy === "update" ? <Loader2 size={13} className="animate-spin" /> : <RefreshCw size={13} />}{t("indexManager.update")}</Button>
          <Button variant="secondary" size="sm" disabled={busy != null} onClick={() => void run("rebuild", () => invoke("ai_reindex_book", { bookId }))}>{t("indexManager.rebuild")}</Button>
          <Button variant="primary" size="sm" disabled={busy != null} onClick={() => void run("summaries", () => invoke("ai_regenerate_book_summaries", { bookId, requestId: createUuid(), overwriteEdited: false }))}>{t("indexManager.regenerate")}</Button>
          {hasEditedSummary && (
            <Button
              variant="secondary"
              size="sm"
              disabled={busy != null}
              onClick={() => {
                if (!window.confirm(t("indexManager.overwriteConfirm"))) return;
                void run("overwrite", () => invoke("ai_regenerate_book_summaries", {
                  bookId,
                  requestId: createUuid(),
                  overwriteEdited: true,
                }));
              }}
            >
              {t("indexManager.overwriteEdited")}
            </Button>
          )}
        </footer>
      </div>
    </div>
  );
}
