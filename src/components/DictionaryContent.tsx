import { useState, useMemo, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { readTextFile } from "@tauri-apps/plugin-fs";
import { useTranslation } from "react-i18next";
import {
  Languages,
  Search,
  BookOpen,
  Clock,
  FileText,
  Trash2,
  LayoutGrid,
  List,
  ArrowDownAZ,
  ArrowDownWideNarrow,
  ArrowUpWideNarrow,
  Download,
  Upload,
  CheckSquare,
  Square,
  X,
  Check,
  GraduationCap,
  CheckCircle2,
  History,
  RotateCcw,
} from "lucide-react";
import Button from "./ui/Button";
import { useAllDictionary, useAllLookupHistory, type DictionaryWord, type LookupRecord, type LookupRecordPage } from "../hooks/useDictionary";
import { timeAgo } from "../utils/timeAgo";
import VocabDetailModal from "./VocabDetailModal";
import { openReaderWindow } from "../utils/openReaderWindow";
import {
  LearningCardModules,
  parseCardDesignConfig,
  type LearningCardResult,
} from "./learning-card";

type SortMode = "newest" | "oldest" | "az";
type ViewMode = "list" | "card";
type ContentTab = "vocab" | "history";
type BackupFormat = "json" | "csv";
type ImportConflictPolicy = "skip" | "overwrite";

interface VocabBackupWord {
  id: string;
  book_id: string;
  word: string;
  definition: string;
  context_sentence: string | null;
  context_explanation: string | null;
  cfi: string | null;
  mastery: string;
  review_count: number;
  next_review_at: number | null;
  review_interval_days: number;
  last_reviewed_at: number | null;
  last_review_rating: string | null;
  fsrs_stability: number | null;
  fsrs_difficulty: number | null;
  fsrs_version: number;
  created_at: number;
  updated_at: number;
}

interface VocabBackup {
  schema: "quill-vocabulary";
  version: number;
  exported_at: number;
  words: VocabBackupWord[];
}

interface VocabImportPreview {
  valid: number;
  new_words: number;
  conflicts: number;
  missing_books: number;
  duplicate_rows: number;
  invalid_rows: number;
}

interface VocabImportResult {
  preview: VocabImportPreview;
  imported: number;
  replaced: number;
  skipped: number;
  dry_run: boolean;
}

const VOCAB_BACKUP_CSV_HEADERS = [
  "backup_schema",
  "backup_version",
  "id",
  "book_id",
  "word",
  "definition",
  "context_sentence",
  "context_explanation",
  "cfi",
  "mastery",
  "review_count",
  "next_review_at",
  "review_interval_days",
  "last_reviewed_at",
  "last_review_rating",
  "fsrs_stability",
  "fsrs_difficulty",
  "fsrs_version",
  "created_at",
  "updated_at",
];

export default function DictionaryContent() {
  const { t } = useTranslation();
  const { words, remove, updateMastery, recordReview, refresh: refreshWords } = useAllDictionary();
  const { records, total: historyTotal, books: historyBooks, hasMore: historyHasMore, loadingMore: historyLoadingMore, refresh: refreshHistory, loadMore: loadMoreHistory, remove: removeHistoryRecord, clear: clearHistory } = useAllLookupHistory();
  const [sort, setSort] = useState<SortMode>("newest");
  const [view, setView] = useState<ViewMode>("list");
  const [search, setSearch] = useState("");
  const [bookFilter, setBookFilter] = useState<string | null>(null);
  const [activeWord, setActiveWord] = useState<DictionaryWord | null>(null);
  const [reviewOnly, setReviewOnly] = useState(false);
  const [contentTab, setContentTab] = useState<ContentTab>("vocab");
  const [now, setNow] = useState(0);
  const [reviewing, setReviewing] = useState<DictionaryWord | null>(null);
  const [historyClearConfirming, setHistoryClearConfirming] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [backupMenuOpen, setBackupMenuOpen] = useState(false);
  const [importing, setImporting] = useState(false);
  const [importData, setImportData] = useState<string | null>(null);
  const [importFormat, setImportFormat] = useState<BackupFormat | null>(null);
  const [importPreview, setImportPreview] = useState<VocabImportPreview | null>(null);
  const [importPolicy, setImportPolicy] = useState<ImportConflictPolicy>("skip");
  const [importError, setImportError] = useState<string | null>(null);
  const [selectedWordIds, setSelectedWordIds] = useState<Set<string>>(() => new Set());
  const [bulkMastery, setBulkMastery] = useState<"new" | "learning" | "mastered">("learning");
  const [bulkBusy, setBulkBusy] = useState(false);
  const [confirmBulkDelete, setConfirmBulkDelete] = useState(false);
  const [learningCardConfig, setLearningCardConfig] = useState(() => parseCardDesignConfig(undefined));
  const clearConfirmationTimer = useRef<number | null>(null);

  useEffect(() => {
    invoke<Record<string, string>>("get_all_settings")
      .then((settings) => setLearningCardConfig(parseCardDesignConfig(settings.learning_card_config)))
      .catch(() => {});
  }, []);

  const historySearch = contentTab === "history" ? search.trim() : "";
  const historyBookFilter = contentTab === "history" ? bookFilter ?? undefined : undefined;
  useEffect(() => {
    if (contentTab !== "history") return;
    const timer = window.setTimeout(() => refreshHistory(historySearch, historyBookFilter), 200);
    return () => window.clearTimeout(timer);
  }, [contentTab, historySearch, historyBookFilter, refreshHistory]);

  useEffect(() => {
    const updateNow = () => setNow(Date.now());
    updateNow();
    const timer = window.setInterval(updateNow, 60_000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => () => {
    if (clearConfirmationTimer.current !== null) {
      window.clearTimeout(clearConfirmationTimer.current);
    }
  }, []);

  const dueWords = useMemo(() => words.filter((word) => word.next_review_at !== null && word.next_review_at <= now), [now, words]);

  const filtered = useMemo(() => {
    let result = words;
    if (search) {
      const q = search.toLowerCase();
      result = result.filter((w) => [w.word, w.definition, w.context_sentence, w.book_title]
        .filter(Boolean)
        .some((value) => value!.toLowerCase().includes(q)));
    }
    if (bookFilter) {
      result = result.filter((w) => w.book_id === bookFilter);
    }
    if (reviewOnly) {
      result = result.filter((w) => w.next_review_at !== null && w.next_review_at <= now);
    }
    return result;
  }, [words, search, bookFilter, reviewOnly, now]);

  const sorted = useMemo(() => {
    const copy = [...filtered];
    if (sort === "oldest") {
      copy.sort((a, b) => a.created_at - b.created_at);
    } else if (sort === "az") {
      copy.sort((a, b) => a.word.localeCompare(b.word, undefined, { sensitivity: "base" }));
    }
    return copy;
  }, [filtered, sort]);

  const groupedByBook = useMemo(() => {
    const map = new Map<string, { title: string; words: DictionaryWord[] }>();
    for (const w of sorted) {
      if (!map.has(w.book_id)) {
        map.set(w.book_id, { title: w.book_title || t("common.unknownBook"), words: [] });
      }
      map.get(w.book_id)!.words.push(w);
    }
    return Array.from(map.entries()).map(([id, group]) => ({ id, ...group }));
  }, [sorted, t]);

  const groupedByLetter = useMemo(() => {
    const map = new Map<string, DictionaryWord[]>();
    for (const w of sorted) {
      const letter = w.word[0]?.toUpperCase() || "#";
      if (!map.has(letter)) map.set(letter, []);
      map.get(letter)!.push(w);
    }
    return Array.from(map.entries()).sort(([a], [b]) => a.localeCompare(b));
  }, [sorted]);

  const bookPills = useMemo(() => {
    const map = new Map<string, { title: string; count: number }>();
    for (const w of words) {
      if (!map.has(w.book_id)) {
        map.set(w.book_id, { title: w.book_title || t("common.unknownBook"), count: 0 });
      }
      map.get(w.book_id)!.count++;
    }
    return Array.from(map.entries()).map(([id, { title, count }]) => ({ id, title, count }));
  }, [words, t]);

  const isEmpty = words.length === 0;
  const filteredRecords = records;
  const historyBookPills = useMemo(() => {
    return historyBooks.map((book) => ({
      id: book.book_id,
      title: book.book_title || t("common.unknownBook"),
      count: book.count,
    }));
  }, [historyBooks, t]);

  const scheduleLearning = (word: DictionaryWord) => updateMastery(word.id, "learning", now + 24 * 60 * 60 * 1000);
  const markMastered = (word: DictionaryWord) => updateMastery(word.id, "mastered", null);
  const completeReview = async (rating: "again" | "hard" | "good" | "easy") => {
    if (!reviewing) return;
    await recordReview(reviewing.id, rating);
    setReviewing(null);
  };
  const downloadCsv = (filename: string, headers: string[], rows: Array<Array<string | number | null | undefined>>) => {
    const escape = (value: string | number | null | undefined) => `"${String(value ?? "").replace(/"/g, '""')}"`;
    const lines = [headers.map(escape).join(","), ...rows.map((row) => row.map(escape).join(","))];
    const href = URL.createObjectURL(new Blob([`\uFEFF${lines.join("\n")}`], { type: "text/csv;charset=utf-8" }));
    const link = document.createElement("a");
    link.href = href;
    link.download = filename;
    link.click();
    window.setTimeout(() => URL.revokeObjectURL(href), 0);
  };
  const exportVocabBackup = async (format: BackupFormat) => {
    setExporting(true);
    try {
      const backup = await invoke<VocabBackup>("export_vocab_backup");
      if (format === "json") {
        const href = URL.createObjectURL(new Blob([JSON.stringify(backup, null, 2)], { type: "application/json" }));
        const link = document.createElement("a");
        link.href = href;
        link.download = "quill-vocabulary.json";
        link.click();
        window.setTimeout(() => URL.revokeObjectURL(href), 0);
      } else {
        downloadCsv(
          "quill-vocabulary.csv",
          VOCAB_BACKUP_CSV_HEADERS,
          backup.words.map((word) => [
            backup.schema, backup.version, word.id, word.book_id, word.word, word.definition,
            word.context_sentence, word.context_explanation, word.cfi, word.mastery,
            word.review_count, word.next_review_at, word.review_interval_days,
            word.last_reviewed_at, word.last_review_rating, word.fsrs_stability,
            word.fsrs_difficulty, word.fsrs_version, word.created_at, word.updated_at,
          ]),
        );
      }
    } catch (error) {
      console.error("Failed to export vocabulary backup:", error);
    } finally {
      setExporting(false);
      setBackupMenuOpen(false);
    }
  };
  const exportCsv = async () => {
    setExporting(true);
    try {
      const allRecords: LookupRecord[] = [];
      let cursor: string | null = null;
      do {
        const page: LookupRecordPage = await invoke<LookupRecordPage>("list_all_lookup_records", {
          search: historySearch || null,
          bookId: historyBookFilter || null,
          cursor,
          limit: 200,
        });
        allRecords.push(...page.records);
        cursor = page.next_cursor;
      } while (cursor !== null);
      downloadCsv(
        "quill-lookup-history.csv",
        ["lookup", "definition", "context_explanation", "context", "chapter", "book", "first_looked_up_at", "last_looked_up_at", "lookup_count"],
        allRecords.map((record) => [
          record.lookup_text,
          record.definition,
          record.context_explanation,
          record.context_sentence,
          record.chapter,
          record.book_title,
          new Date(record.created_at).toISOString(),
          new Date(record.last_looked_up_at).toISOString(),
          String(record.lookup_count),
        ]),
      );
    } finally {
      setExporting(false);
    }
  };
  const resetImport = () => {
    setImportData(null);
    setImportFormat(null);
    setImportPreview(null);
    setImportPolicy("skip");
    setImportError(null);
  };
  const chooseVocabBackup = async () => {
    setImportError(null);
    const selected = await open({
      multiple: false,
      filters: [{ name: "Vocabulary backup", extensions: ["json", "csv"] }],
      fileAccessMode: "scoped",
    });
    if (typeof selected !== "string") return;
    const extension = selected.split(".").pop()?.toLowerCase();
    const format: BackupFormat | null = extension === "json" || extension === "csv" ? extension : null;
    if (!format) {
      setImportError(t("vocab.backup.unsupportedFile"));
      return;
    }
    setImporting(true);
    try {
      const data = await readTextFile(selected);
      const preview = await invoke<VocabImportPreview>("preview_vocab_import", { data, format });
      setImportData(data);
      setImportFormat(format);
      setImportPreview(preview);
    } catch (error) {
      console.error("Failed to preview vocabulary backup:", error);
      setImportError(t("vocab.backup.importFailed"));
    } finally {
      setImporting(false);
    }
  };
  const importVocabBackup = async () => {
    if (!importData || !importFormat) return;
    setImporting(true);
    setImportError(null);
    try {
      await invoke<VocabImportResult>("import_vocab_backup", {
        data: importData,
        format: importFormat,
        conflictPolicy: importPolicy,
        dryRun: false,
      });
      await refreshWords();
      resetImport();
    } catch (error) {
      console.error("Failed to import vocabulary backup:", error);
      setImportError(t("vocab.backup.importFailed"));
    } finally {
      setImporting(false);
    }
  };
  const toggleWordSelection = (id: string) => {
    setSelectedWordIds((previous) => {
      const next = new Set(previous);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };
  const toggleSelectVisible = () => {
    setSelectedWordIds((previous) => {
      const visibleIds = sorted.map((word) => word.id);
      const allVisibleSelected = visibleIds.length > 0 && visibleIds.every((id) => previous.has(id));
      const next = new Set(previous);
      for (const id of visibleIds) {
        if (allVisibleSelected) next.delete(id);
        else next.add(id);
      }
      return next;
    });
  };
  const applyBulkMastery = async () => {
    if (selectedWordIds.size === 0) return;
    const nextReviewAt = bulkMastery === "learning" ? Date.now() + 24 * 60 * 60 * 1000 : null;
    setBulkBusy(true);
    try {
      await invoke<number>("bulk_update_vocab_mastery", {
        ids: Array.from(selectedWordIds),
        mastery: bulkMastery,
        nextReviewAt,
      });
      await refreshWords();
      setSelectedWordIds(new Set());
    } catch (error) {
      console.error("Failed to update vocabulary mastery in bulk:", error);
    } finally {
      setBulkBusy(false);
    }
  };
  const deleteSelectedWords = async () => {
    if (selectedWordIds.size === 0) return;
    setBulkBusy(true);
    try {
      await invoke<number>("bulk_delete_vocab_words", { ids: Array.from(selectedWordIds) });
      await refreshWords();
      setSelectedWordIds(new Set());
      setConfirmBulkDelete(false);
    } catch (error) {
      console.error("Failed to delete vocabulary in bulk:", error);
    } finally {
      setBulkBusy(false);
    }
  };
  const requestClearHistory = async () => {
    if (!historyClearConfirming) {
      setHistoryClearConfirming(true);
      clearConfirmationTimer.current = window.setTimeout(() => {
        setHistoryClearConfirming(false);
        clearConfirmationTimer.current = null;
      }, 3000);
      return;
    }
    if (clearConfirmationTimer.current !== null) {
      window.clearTimeout(clearConfirmationTimer.current);
      clearConfirmationTimer.current = null;
    }
    await clearHistory(historyBookFilter);
    setHistoryClearConfirming(false);
  };

  return (
    <div className="flex-1 flex flex-col min-w-0">
      {/* Header */}
      <div className="px-page pb-2 relative select-none">
        <div data-tauri-drag-region className="absolute top-0 left-0 right-0 h-11" />
        <div className="pt-11 flex items-center justify-between mb-6">
          <h1 className="text-[24px] font-semibold text-text-primary tracking-[0.07px]">
            {contentTab === "vocab" ? t("vocab.title") : t("vocab.history")}
          </h1>
          <div className="flex items-center gap-0">
            {contentTab === "vocab" ? (
              <>
                <div className="relative">
                  <button
                    type="button"
                    title={t("vocab.backup.export")}
                    aria-label={t("vocab.backup.export")}
                    onClick={() => setBackupMenuOpen((open) => !open)}
                    disabled={exporting || importing}
                    className="size-9 flex items-center justify-center rounded-lg text-text-muted hover:bg-bg-input disabled:opacity-50 cursor-pointer"
                  >
                    <Download size={16} />
                  </button>
                  {backupMenuOpen && (
                    <div className="absolute right-0 top-10 z-30 w-44 border border-border bg-bg-surface shadow-popover rounded-lg p-1">
                      <button type="button" onClick={() => exportVocabBackup("json")} className="flex w-full h-8 items-center px-2 rounded text-left text-[12px] text-text-secondary hover:bg-bg-input cursor-pointer">
                        {t("vocab.backup.exportJson")}
                      </button>
                      <button type="button" onClick={() => exportVocabBackup("csv")} className="flex w-full h-8 items-center px-2 rounded text-left text-[12px] text-text-secondary hover:bg-bg-input cursor-pointer">
                        {t("vocab.backup.exportCsv")}
                      </button>
                    </div>
                  )}
                </div>
                <button
                  type="button"
                  title={t("vocab.backup.import")}
                  aria-label={t("vocab.backup.import")}
                  onClick={() => chooseVocabBackup().catch(() => {})}
                  disabled={exporting || importing}
                  className="size-9 flex items-center justify-center rounded-lg text-text-muted hover:bg-bg-input disabled:opacity-50 cursor-pointer"
                >
                  <Upload size={16} />
                </button>
              </>
            ) : (
              <button
                type="button"
                title={t("vocab.export")}
                aria-label={t("vocab.export")}
                onClick={exportCsv}
                disabled={exporting}
                className="size-9 flex items-center justify-center rounded-lg text-text-muted hover:bg-bg-input disabled:opacity-50 cursor-pointer"
              >
                <Download size={16} />
              </button>
            )}
            <Button variant="icon" size="md" active={view === "card"} onClick={() => setView("card")}>
              <LayoutGrid size={16} />
            </Button>
            <Button variant="icon" size="md" active={view === "list"} onClick={() => setView("list")}>
              <List size={16} />
            </Button>
          </div>
        </div>

        <div className="flex items-center gap-2 h-9 px-3 rounded-lg bg-bg-input max-w-[448px]">
          <Search size={16} className="text-text-muted shrink-0" />
          <input
            type="search"
            placeholder={t("vocab.search")}
            defaultValue=""
            onInput={(e) => setSearch((e.target as HTMLInputElement).value)}
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
            spellCheck={false}
            className="flex-1 text-[14px] text-text-primary bg-transparent outline-none placeholder:text-text-placeholder [&::-webkit-search-cancel-button]:hidden"
          />
        </div>
      </div>

      <div className="flex items-center gap-1 px-page pb-3 border-b border-border">
        <Button variant="ghost" size="sm" active={contentTab === "vocab"} onClick={() => setContentTab("vocab")}>
          <Languages size={14} />
          {t("vocab.savedTab")}
        </Button>
        <Button variant="ghost" size="sm" active={contentTab === "history"} onClick={() => setContentTab("history")}>
          <History size={14} />
          {t("vocab.historyTab")}
          <span className="text-[11px] text-text-muted">{historyTotal}</span>
        </Button>
      </div>

      {contentTab === "vocab" && selectedWordIds.size > 0 && (
        <div className="flex flex-wrap items-center gap-2 border-b border-border px-page py-2 bg-bg-surface">
          <span className="mr-1 text-[12px] font-medium text-text-secondary">{t("vocab.bulk.selected", { count: selectedWordIds.size })}</span>
          <select
            value={bulkMastery}
            onChange={(event) => setBulkMastery(event.target.value as "new" | "learning" | "mastered")}
            className="h-8 rounded-md border border-border bg-bg-surface px-2 text-[12px] text-text-secondary outline-none"
          >
            <option value="new">{t("vocab.mastery.new")}</option>
            <option value="learning">{t("vocab.mastery.learning")}</option>
            <option value="mastered">{t("vocab.mastery.mastered")}</option>
          </select>
          <button type="button" onClick={() => applyBulkMastery().catch(() => {})} disabled={bulkBusy} className="flex h-8 items-center gap-1 rounded-md border border-border px-2 text-[12px] text-text-secondary hover:bg-bg-input disabled:opacity-50 cursor-pointer">
            <Check size={13} /> {t("vocab.bulk.apply")}
          </button>
          <button type="button" onClick={() => setConfirmBulkDelete(true)} disabled={bulkBusy} className="flex h-8 items-center gap-1 rounded-md px-2 text-[12px] text-danger-text hover:bg-bg-input disabled:opacity-50 cursor-pointer">
            <Trash2 size={13} /> {t("common.delete")}
          </button>
          <button type="button" onClick={() => setSelectedWordIds(new Set())} className="ml-auto size-8 flex items-center justify-center rounded-md text-text-muted hover:bg-bg-input cursor-pointer" title={t("common.cancel")} aria-label={t("common.cancel")}>
            <X size={15} />
          </button>
        </div>
      )}

      {/* Book filter pills + sort */}
      {(contentTab === "vocab" ? !isEmpty : records.length > 0) && (
        <div className="flex items-center gap-2 px-page pt-2 pb-4 overflow-x-auto border-b border-border">
          {contentTab === "vocab" && <button
            type="button"
            onClick={() => setReviewOnly((value) => !value)}
            className={`flex items-center gap-1.5 h-8 px-[13px] rounded-full text-[12px] font-medium cursor-pointer shrink-0 transition-colors border ${
              reviewOnly ? "bg-accent-bg border-accent/30 text-accent-text" : "bg-bg-surface border-border text-text-secondary hover:bg-bg-muted"
            }`}
          >
            <GraduationCap size={12} />
            {t("vocab.reviewDue")}
            <span className="text-[11px]">{dueWords.length}</span>
          </button>}
          <button
            onClick={() => setBookFilter(null)}
            className={`flex items-center gap-1.5 h-8 px-[13px] rounded-full text-[12px] font-medium cursor-pointer shrink-0 transition-colors border ${
              bookFilter === null
                ? "bg-accent-bg border-accent/30 text-accent-text"
                : "bg-bg-surface border-border text-text-secondary hover:bg-bg-muted"
            }`}
          >
            <BookOpen size={12} className={bookFilter === null ? "text-accent-text" : ""} />
            {t("common.allBooks")}
            <span className={`text-[11px] ${bookFilter === null ? "text-accent-text" : "text-text-muted"}`}>
              {contentTab === "vocab" ? words.length : historyBooks.reduce((sum, book) => sum + book.count, 0)}
            </span>
          </button>
          {(contentTab === "vocab" ? bookPills : historyBookPills).map((pill) => (
            <button
              key={pill.id}
              onClick={() => setBookFilter(bookFilter === pill.id ? null : pill.id)}
              className={`flex items-center gap-1.5 h-8 px-[13px] rounded-full text-[12px] font-medium cursor-pointer shrink-0 transition-colors border ${
                bookFilter === pill.id
                  ? "bg-accent-bg border-accent/30 text-accent-text"
                  : "bg-bg-surface border-border text-text-secondary hover:bg-bg-muted"
              }`}
            >
              <BookOpen size={12} className={bookFilter === pill.id ? "text-accent-text" : ""} />
              <span className="truncate max-w-[120px]">{pill.title}</span>
              <span className={`text-[11px] ${bookFilter === pill.id ? "text-accent-text" : "text-text-muted"}`}>
                {pill.count}
              </span>
            </button>
          ))}

          {contentTab === "vocab" && <div className="ml-auto flex items-center gap-1 shrink-0">
            <button
              type="button"
              onClick={toggleSelectVisible}
              title={t("vocab.bulk.selectVisible")}
              aria-label={t("vocab.bulk.selectVisible")}
              className="size-7 rounded-md flex items-center justify-center text-text-muted hover:bg-bg-input cursor-pointer"
            >
              {sorted.length > 0 && sorted.every((word) => selectedWordIds.has(word.id)) ? <CheckSquare size={14} /> : <Square size={14} />}
            </button>
            <button
              onClick={() => setSort("newest")}
              className={`flex items-center gap-1 h-7 px-2.5 rounded-lg text-[11px] font-medium cursor-pointer transition-colors ${
                sort === "newest" ? "text-accent-text" : "text-text-muted hover:text-text-primary"
              }`}
            >
              <ArrowDownWideNarrow size={12} />
              {t("vocab.newest")}
            </button>
            <button
              onClick={() => setSort("oldest")}
              className={`flex items-center gap-1 h-7 px-2.5 rounded-lg text-[11px] font-medium cursor-pointer transition-colors ${
                sort === "oldest" ? "text-accent-text" : "text-text-muted hover:text-text-primary"
              }`}
            >
              <ArrowUpWideNarrow size={12} />
              {t("vocab.oldest")}
            </button>
            <button
              onClick={() => { setSort("az"); setView("list"); }}
              className={`flex items-center gap-1 h-7 px-2.5 rounded-lg text-[11px] font-medium cursor-pointer transition-colors ${
                sort === "az" ? "text-accent-text" : "text-text-muted hover:text-text-primary"
              }`}
            >
              <ArrowDownAZ size={12} />
              {t("vocab.az")}
            </button>
          </div>}
        </div>
      )}

      {/* Content */}
      <div className="flex-1 overflow-auto p-page pb-20">
        {contentTab === "history" ? (
          historyTotal === 0 ? (
            <div className="flex flex-col items-center justify-center h-full">
              <div className="size-16 rounded-full bg-bg-input flex items-center justify-center mb-4">
                <History size={28} className="text-text-muted" />
              </div>
              <h2 className="text-[18px] font-medium text-text-primary mb-2">{t("vocab.historyEmpty")}</h2>
              <p className="text-[14px] text-text-muted text-center max-w-[296px]">{t("vocab.historyEmptySub")}</p>
            </div>
          ) : (
            <div className="max-w-[720px] space-y-2">
              {filteredRecords.map((record) => (
                <div key={record.id} className="border border-border rounded-lg bg-bg-surface px-4 py-3">
                  <div className="flex items-start justify-between gap-4">
                    <div className="min-w-0">
                      <p className="text-[15px] font-semibold text-text-primary">{record.lookup_text}</p>
                      {!record.result_json && <p className="mt-1 text-[13px] text-text-secondary line-clamp-2 whitespace-pre-line">{record.definition}</p>}
                    </div>
                    <span className="shrink-0 text-[11px] text-text-muted">{timeAgo(record.last_looked_up_at)}</span>
                  </div>
                  {record.context_sentence && <p className="mt-2 text-[12px] italic text-text-muted line-clamp-2">"{record.context_sentence}"</p>}
                  {record.result_json && (() => {
                    try {
                      const result = JSON.parse(record.result_json) as LearningCardResult;
                      if (
                        result.version !== 1
                        || !["word", "phrase", "passage"].includes(result.kind)
                        || !result.modules
                      ) return null;
                      return (
                        <div className="mt-2 border-t border-border-light pt-2">
                          {result.modules.context_meaning?.summary && (
                            <p className="text-[13px] leading-[1.6] text-text-secondary">
                              {result.modules.context_meaning.summary}
                            </p>
                          )}
                          <details className="mt-2">
                            <summary className="cursor-pointer text-[11px] font-medium text-accent-text">
                              {t("vocab.showStructuredResult", { defaultValue: "查看完整学习卡片" })}
                            </summary>
                            <div className="mt-2 divide-y divide-border-light border-y border-border-light">
                              <LearningCardModules
                                card={learningCardConfig.cards[result.kind]}
                                kind={result.kind}
                                content={result.modules}
                              />
                            </div>
                          </details>
                        </div>
                      );
                    } catch {
                      return <p className="mt-1 text-[13px] text-text-secondary whitespace-pre-line">{record.definition}</p>;
                    }
                  })()}
                  <div className="mt-2 flex items-center gap-3 text-[11px] text-text-muted">
                    <span className="flex items-center gap-1 min-w-0"><BookOpen size={12} /><span className="truncate">{record.book_title || t("common.unknownBook")}</span></span>
                    {record.chapter && <span className="truncate">{record.chapter}</span>}
                    {record.lookup_count > 1 && <span>{t("vocab.lookedUpCount", { count: record.lookup_count })}</span>}
                    {record.cfi && (
                      <button
                        type="button"
                        onClick={() => openReaderWindow(record.book_id, { openVocab: true, cfi: record.cfi })}
                        className="ml-auto flex items-center gap-1 text-accent-text hover:opacity-70 cursor-pointer"
                      >
                        {t("vocab.openInReader")} <FileText size={12} />
                      </button>
                    )}
                    <button
                      type="button"
                      title={t("vocab.deleteHistory")}
                      aria-label={t("vocab.deleteHistory")}
                      onClick={() => removeHistoryRecord(record.id)}
                      className="size-6 flex items-center justify-center rounded text-text-muted hover:bg-bg-input hover:text-danger-text cursor-pointer"
                    >
                      <Trash2 size={13} />
                    </button>
                  </div>
                </div>
              ))}
              {filteredRecords.length === 0 && <p className="pt-8 text-center text-[14px] text-text-muted">{t("vocab.noMatches")}</p>}
              {historyHasMore && (
                <button
                  type="button"
                  onClick={() => loadMoreHistory(historySearch, historyBookFilter)}
                  disabled={historyLoadingMore}
                  className="mx-auto mt-4 flex h-9 items-center rounded-md border border-border px-3 text-[12px] font-medium text-text-secondary hover:bg-bg-input disabled:opacity-50 cursor-pointer"
                >
                  {historyLoadingMore ? t("home.loading") : t("vocab.loadMore")}
                </button>
              )}
              {historyTotal > 0 && (
                <button
                  type="button"
                  onClick={() => requestClearHistory().catch(() => {})}
                  className="mx-auto mt-4 flex h-8 items-center text-[12px] text-text-muted hover:text-danger-text cursor-pointer"
                >
                  {historyClearConfirming ? t("vocab.clearHistoryConfirm") : t("vocab.clearHistory")}
                </button>
              )}
            </div>
          )
        ) : isEmpty ? (
          <div className="flex flex-col items-center justify-center h-full">
            <div className="size-16 rounded-full bg-bg-input flex items-center justify-center mb-4">
              <Languages size={28} className="text-text-muted" />
            </div>
            <h2 className="text-[18px] font-medium text-text-primary mb-2">
              {t("vocab.empty")}
            </h2>
            <p className="text-[14px] text-text-muted text-center max-w-[296px]">
              {t("vocab.emptySub")}
            </p>
          </div>
        ) : view === "list" ? (
          <div key="list">
            {groupedByLetter.map(([letter, letterWords]) => (
              <div key={letter} className="mb-6">
                <div className="flex items-center gap-3 mb-2">
                  <span className="text-[18px] font-bold text-accent">{letter}</span>
                  <div className="flex-1 h-px bg-border-light" />
                  <span className="text-[11px] text-text-muted">{letterWords.length}</span>
                </div>
                {letterWords.map((word) => {
                  const parts = word.definition.split("\n\n");
                  const defText = parts[0] || "";
                  const ctxText = parts.length > 1 ? parts.slice(1).join(" ") : null;
                  return (
                    <div
                      key={word.id}
                      className="flex items-start gap-4 px-3 pt-3 pb-3 rounded-[10px] hover:bg-bg-input group w-full text-left cursor-pointer"
                    >
                      <button
                        type="button"
                        onClick={() => toggleWordSelection(word.id)}
                        aria-label={selectedWordIds.has(word.id) ? t("vocab.bulk.unselect") : t("vocab.bulk.select")}
                        className="mt-1 size-5 shrink-0 flex items-center justify-center text-text-muted hover:text-accent-text cursor-pointer"
                      >
                        {selectedWordIds.has(word.id) ? <CheckSquare size={15} className="text-accent-text" /> : <Square size={15} />}
                      </button>
                      <button
                        type="button"
                        onClick={() => setActiveWord(word)}
                        className="flex min-w-0 flex-1 items-start gap-4 text-left"
                      >
                        <div className="w-[160px] shrink-0">
                        <span className="block text-[14px] font-semibold text-text-primary leading-5">
                          {word.word}
                        </span>
                        <span className={`inline-flex mt-1 text-[10px] font-medium ${word.mastery === "mastered" ? "text-success-text" : word.mastery === "learning" ? "text-accent-text" : "text-text-muted"}`}>
                          {t(`vocab.mastery.${word.mastery}`)}
                        </span>
                        {word.book_title && (
                          <span className="flex items-center gap-1 text-[11px] text-text-muted mt-0.5">
                            <BookOpen size={10} />
                            <span className="truncate">{word.book_title}</span>
                          </span>
                        )}
                      </div>
                        <div className="flex-1 min-w-0">
                        <p className="text-[13px] text-text-secondary leading-5 truncate">{defText}</p>
                        {ctxText && (
                          <p className="text-[11px] italic text-text-muted leading-4 truncate mt-0.5">
                            "{ctxText}"
                          </p>
                        )}
                        </div>
                      </button>
                      <div className="flex items-center gap-2 shrink-0">
                        {word.next_review_at !== null && word.next_review_at <= now && (
                          <button
                            type="button"
                            onClick={(event) => { event.stopPropagation(); setReviewing(word); }}
                            title={t("vocab.review")}
                            className="size-7 rounded-md flex items-center justify-center text-text-muted hover:bg-bg-surface hover:text-accent-text cursor-pointer"
                          >
                            <RotateCcw size={14} />
                          </button>
                        )}
                        {word.mastery !== "mastered" && (
                          <button
                            type="button"
                            onClick={(event) => { event.stopPropagation(); markMastered(word); }}
                            title={t("vocab.markMastered")}
                            className="size-7 rounded-md flex items-center justify-center text-text-muted hover:bg-bg-surface hover:text-success-text cursor-pointer"
                          >
                            <CheckCircle2 size={14} />
                          </button>
                        )}
                        {word.mastery !== "learning" && word.mastery !== "mastered" && (
                          <button
                            type="button"
                            onClick={(event) => { event.stopPropagation(); scheduleLearning(word); }}
                            title={t("vocab.startLearning")}
                            className="size-7 rounded-md flex items-center justify-center text-text-muted hover:bg-bg-surface hover:text-accent-text cursor-pointer"
                          >
                            <GraduationCap size={14} />
                          </button>
                        )}
                        <span className="text-[11px] text-text-muted">{timeAgo(word.created_at)}</span>
                        <button
                          onClick={(e) => {
                            e.stopPropagation();
                            remove(word.id);
                          }}
                          className="p-1 rounded hover:bg-bg-surface/80 cursor-pointer opacity-0 group-hover:opacity-100 transition-opacity"
                        >
                          <Trash2 size={14} className="text-text-muted" />
                        </button>
                      </div>
                    </div>
                  );
                })}
              </div>
            ))}
          </div>
        ) : (
          <div key="card" className="max-w-[525px] space-y-6">
            {groupedByBook.map((group) => (
              <div key={group.id}>
                <div className="flex items-center gap-2 mb-3">
                  <BookOpen size={14} className="text-text-muted" />
                  <span className="text-[12px] font-semibold uppercase text-text-muted tracking-[0.3px]">
                    {group.title}
                  </span>
                  <span className="text-[11px] text-text-muted">({group.words.length})</span>
                </div>
                <div className="space-y-3">
                  {group.words.map((word) => {
                    const parts = word.definition.split("\n\n");
                    const defText = parts[0] || "";
                    const ctxText = parts.length > 1 ? parts.slice(1).join(" ") : null;
                  return (
                    <div
                      key={word.id}
                      className="group relative bg-bg-muted border border-border rounded-[14px] p-[17px] flex flex-col gap-2 w-full text-left cursor-pointer hover:bg-bg-input transition-colors"
                    >
                      <button
                        type="button"
                        onClick={() => toggleWordSelection(word.id)}
                        aria-label={selectedWordIds.has(word.id) ? t("vocab.bulk.unselect") : t("vocab.bulk.select")}
                        className="absolute top-4 left-4 size-5 flex items-center justify-center text-text-muted hover:text-accent-text cursor-pointer"
                      >
                        {selectedWordIds.has(word.id) ? <CheckSquare size={15} className="text-accent-text" /> : <Square size={15} />}
                      </button>
                      <button
                          onClick={(e) => {
                            e.stopPropagation();
                            remove(word.id);
                          }}
                        className="absolute top-4 right-4 p-1 rounded hover:bg-bg-surface/80 cursor-pointer opacity-0 group-hover:opacity-100 transition-opacity"
                        >
                          <Trash2 size={15} className="text-text-muted" />
                        </button>
                        <button type="button" onClick={() => setActiveWord(word)} className="flex flex-col items-start gap-2 pl-6 text-left">
                          <span className="text-[15px] font-semibold text-text-primary leading-[22.5px] tracking-[-0.23px]">
                            {word.word}
                          </span>
                          <p className="text-[13px] text-text-secondary leading-[20.15px] tracking-[-0.08px] line-clamp-3 w-[460px] max-w-full">
                            {defText}
                          </p>
                          {ctxText && (
                            <div className="border-l-2 border-accent/30 pl-2 overflow-hidden">
                              <p className="text-[11px] italic text-text-muted leading-[16.5px] tracking-[0.06px] line-clamp-2">
                                {ctxText}
                              </p>
                            </div>
                          )}
                          <div className="flex items-center gap-3">
                            <span className="flex items-center gap-1 text-[11px] text-text-muted tracking-[0.06px]">
                              <Clock size={12} />
                              {timeAgo(word.created_at)}
                            </span>
                          </div>
                        </button>
                      </div>
                    );
                  })}
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      <VocabDetailModal
        word={activeWord}
        onClose={() => setActiveWord(null)}
        onDelete={async (id) => {
          await remove(id);
          setActiveWord(null);
        }}
      />
      {importPreview && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-overlay px-4" onClick={resetImport}>
          <div className="w-[480px] max-w-full rounded-lg border border-border bg-bg-surface shadow-popover p-5" onClick={(event) => event.stopPropagation()}>
            <div className="flex items-center justify-between gap-4">
              <div>
                <h2 className="text-[16px] font-semibold text-text-primary">{t("vocab.backup.importPreview")}</h2>
                <p className="mt-1 text-[12px] text-text-muted">{t("vocab.backup.format", { format: importFormat?.toUpperCase() })}</p>
              </div>
              <button type="button" onClick={resetImport} className="size-8 rounded-md flex items-center justify-center text-text-muted hover:bg-bg-input cursor-pointer" aria-label={t("common.cancel")}>
                <X size={16} />
              </button>
            </div>
            <div className="mt-4 grid grid-cols-2 gap-2 text-[12px]">
              {[
                ["vocab.backup.valid", importPreview.valid],
                ["vocab.backup.newWords", importPreview.new_words],
                ["vocab.backup.conflicts", importPreview.conflicts],
                ["vocab.backup.missingBooks", importPreview.missing_books],
                ["vocab.backup.duplicateRows", importPreview.duplicate_rows],
                ["vocab.backup.invalidRows", importPreview.invalid_rows],
              ].map(([label, count]) => (
                <div key={label as string} className="flex items-center justify-between rounded-md bg-bg-input px-3 py-2 text-text-secondary">
                  <span>{t(label as string)}</span><span className="font-medium text-text-primary">{count as number}</span>
                </div>
              ))}
            </div>
            {importPreview.conflicts > 0 && (
              <label className="mt-4 flex items-start gap-2 rounded-md border border-border p-3 text-[12px] text-text-secondary cursor-pointer">
                <input type="checkbox" checked={importPolicy === "overwrite"} onChange={(event) => setImportPolicy(event.target.checked ? "overwrite" : "skip")} className="mt-0.5 accent-accent" />
                <span>{t("vocab.backup.overwriteConflicts")}</span>
              </label>
            )}
            {(importPreview.missing_books > 0 || importPreview.invalid_rows > 0 || importPreview.duplicate_rows > 0) && (
              <p className="mt-3 text-[12px] leading-5 text-text-muted">{t("vocab.backup.importNotice")}</p>
            )}
            {importError && <p className="mt-3 text-[12px] text-danger-text">{importError}</p>}
            <div className="mt-5 flex justify-end gap-2">
              <Button variant="ghost" size="md" onClick={resetImport}>{t("common.cancel")}</Button>
              <Button variant="primary" size="md" onClick={() => importVocabBackup().catch(() => {})} disabled={importing || importPreview.valid === 0}>
                {importing ? t("home.loading") : t("vocab.backup.confirmImport")}
              </Button>
            </div>
          </div>
        </div>
      )}
      {importError && !importPreview && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-overlay px-4" onClick={() => setImportError(null)}>
          <div className="w-[400px] max-w-full rounded-lg border border-border bg-bg-surface shadow-popover p-5" onClick={(event) => event.stopPropagation()}>
            <h2 className="text-[16px] font-semibold text-text-primary">{t("vocab.backup.import")}</h2>
            <p className="mt-3 text-[13px] leading-5 text-text-secondary">{importError}</p>
            <div className="mt-5 flex justify-end"><Button variant="primary" size="md" onClick={() => setImportError(null)}>{t("common.cancel")}</Button></div>
          </div>
        </div>
      )}
      {confirmBulkDelete && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-overlay px-4" onClick={() => setConfirmBulkDelete(false)}>
          <div className="w-[400px] max-w-full rounded-lg border border-border bg-bg-surface shadow-popover p-5" onClick={(event) => event.stopPropagation()}>
            <h2 className="text-[16px] font-semibold text-text-primary">{t("vocab.bulk.deleteTitle")}</h2>
            <p className="mt-2 text-[13px] leading-5 text-text-secondary">{t("vocab.bulk.deleteBody", { count: selectedWordIds.size })}</p>
            <div className="mt-5 flex justify-end gap-2">
              <Button variant="ghost" size="md" onClick={() => setConfirmBulkDelete(false)}>{t("common.cancel")}</Button>
              <button type="button" onClick={() => deleteSelectedWords().catch(() => {})} disabled={bulkBusy} className="h-9 rounded-md bg-red-500 px-3 text-[13px] font-medium text-white hover:bg-red-600 disabled:opacity-50 cursor-pointer">{t("common.delete")}</button>
            </div>
          </div>
        </div>
      )}
      {reviewing && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-overlay backdrop-blur-sm" onClick={() => setReviewing(null)}>
          <div className="w-[440px] max-w-[calc(100vw-32px)] bg-bg-surface border border-border rounded-lg shadow-popover p-5" onClick={(event) => event.stopPropagation()}>
            <div className="flex items-center gap-2 text-text-primary">
              <RotateCcw size={17} className="text-accent" />
              <h2 className="text-[16px] font-semibold">{t("vocab.review")}</h2>
            </div>
            <p className="mt-4 text-[20px] font-semibold text-text-primary">{reviewing.word}</p>
            <p className="mt-2 text-[14px] leading-6 text-text-secondary whitespace-pre-line">{reviewing.definition}</p>
            {reviewing.context_sentence && <p className="mt-3 text-[13px] italic text-text-muted">&ldquo;{reviewing.context_sentence}&rdquo;</p>}
            <div className="mt-5 grid grid-cols-4 gap-2">
              {(["again", "hard", "good", "easy"] as const).map((rating) => (
                <button
                  key={rating}
                  type="button"
                  onClick={() => completeReview(rating)}
                  className="h-9 rounded-md border border-border bg-bg-surface text-[12px] font-medium text-text-secondary hover:border-accent hover:bg-accent-bg hover:text-accent-text cursor-pointer"
                >
                  {t(`vocab.rating.${rating}`)}
                </button>
              ))}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
