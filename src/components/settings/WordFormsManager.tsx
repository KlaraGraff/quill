import { invoke } from "@tauri-apps/api/core";
import { Loader2, RefreshCw, Search, Sparkles, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { createUuid } from "../../utils/randomUuid";

export interface WordFormsEntry {
  normalized_word: string;
  display_word: string;
  forms: string[];
  source: "model" | "user" | null;
  created_at: number;
  updated_at: number | null;
}

const chunks = <T,>(items: T[], size: number) => Array.from(
  { length: Math.ceil(items.length / size) },
  (_, index) => items.slice(index * size, (index + 1) * size),
);

export default function WordFormsManager() {
  const { t } = useTranslation();
  const [entries, setEntries] = useState<WordFormsEntry[]>([]);
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [query, setQuery] = useState("");
  const [running, setRunning] = useState(false);
  const [progress, setProgress] = useState({ done: 0, total: 0, failed: 0 });
  const [busyWord, setBusyWord] = useState<string | null>(null);
  const cancelledRef = useRef(false);
  const requestsRef = useRef(new Set<string>());

  const refresh = useCallback(async () => {
    const values = await invoke<WordFormsEntry[]>("list_word_forms");
    setEntries(values);
    setDrafts(Object.fromEntries(values.map((entry) => [entry.normalized_word, entry.forms.join(", ")])));
  }, []);

  useEffect(() => { void refresh(); }, [refresh]);

  const save = async (word: string, value: string, source: "model" | "user") => {
    const forms = value.split(/[,，]/).map((item) => item.trim()).filter(Boolean);
    const saved = await invoke<string[]>("set_word_forms", { word, forms, source });
    setDrafts((current) => ({ ...current, [word]: saved.join(", ") }));
    setEntries((current) => current.map((entry) => entry.normalized_word === word
      ? { ...entry, forms: saved, source, updated_at: Date.now() }
      : entry));
    window.dispatchEvent(new CustomEvent("word-forms-changed"));
  };

  const requestForms = async (words: string[]) => {
    const requestId = createUuid();
    requestsRef.current.add(requestId);
    try {
      return await invoke<Record<string, string[]>>("ai_word_forms", { words, requestId });
    } finally {
      requestsRef.current.delete(requestId);
    }
  };

  const fetchOne = async (word: string) => {
    setBusyWord(word);
    try {
      const result = await requestForms([word]);
      await save(word, (result[word] ?? []).join(", "), "model");
    } finally {
      setBusyWord(null);
    }
  };

  const fetchAll = async () => {
    const missing = entries.filter((entry) => entry.updated_at == null).slice(0, 500);
    if (missing.length === 0) return;
    const batches = chunks(missing.map((entry) => entry.normalized_word), 10);
    cancelledRef.current = false;
    setRunning(true);
    setProgress({ done: 0, total: missing.length, failed: 0 });
    let cursor = 0;
    const worker = async () => {
      while (!cancelledRef.current) {
        const batch = batches[cursor++];
        if (!batch) return;
        let result: Record<string, string[]> | null = null;
        for (let attempt = 0; attempt < 2 && !cancelledRef.current; attempt += 1) {
          try { result = await requestForms(batch); break; } catch { /* retry once */ }
        }
        if (cancelledRef.current) return;
        if (!result) {
          setProgress((current) => ({ ...current, done: current.done + batch.length, failed: current.failed + batch.length }));
          continue;
        }
        await Promise.all(batch.map((word) => save(word, (result?.[word] ?? []).join(", "), "model")));
        setProgress((current) => ({ ...current, done: current.done + batch.length }));
      }
    };
    try {
      await Promise.all(Array.from({ length: Math.min(5, batches.length) }, () => worker()));
      await refresh();
    } finally {
      setRunning(false);
    }
  };

  const filtered = useMemo(() => entries.filter((entry) => (
    !query.trim() || entry.display_word.toLowerCase().includes(query.trim().toLowerCase())
  )), [entries, query]);

  return (
    <div className="mb-4 rounded-md border border-border-light bg-bg-muted p-3">
      <div className="flex items-center gap-2">
        <label className="relative min-w-0 flex-1">
          <Search size={13} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-text-placeholder" />
          <input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={t("settings.tools.wordForms.search")} className="h-8 w-full rounded-md border border-border bg-bg-surface pl-8 pr-2 text-[11px] text-text-primary outline-none focus:border-accent" />
        </label>
        {running ? <>
          <span className="text-[11px] tabular-nums text-text-muted">{progress.done}/{progress.total}</span>
          <button type="button" title={t("common.cancel")} onClick={() => {
            cancelledRef.current = true;
            for (const requestId of requestsRef.current) void invoke("ai_cancel", { requestId });
          }} className="flex size-8 items-center justify-center rounded-md text-text-muted hover:bg-bg-input"><X size={14} /></button>
        </> : (
          <button type="button" title={t("settings.tools.wordForms.fetchAll")} onClick={() => void fetchAll()} disabled={!entries.some((entry) => entry.updated_at == null)} className="flex size-8 items-center justify-center rounded-md text-text-muted hover:bg-bg-input hover:text-accent-text disabled:opacity-30"><Sparkles size={14} /></button>
        )}
      </div>
      {progress.failed > 0 && !running && <p className="mt-2 text-[10px] text-danger-text">{t("settings.tools.wordForms.failed", { count: progress.failed })}</p>}
      <div className="mt-2 max-h-[360px] overflow-y-auto border-y border-border-light">
        {filtered.map((entry) => (
          <div key={entry.normalized_word} className="flex min-h-11 items-center gap-2 border-t border-border-light py-1.5 first:border-t-0">
            <span className="w-[104px] shrink-0 truncate text-[11px] font-medium text-text-primary">{entry.display_word}</span>
            <input value={drafts[entry.normalized_word] ?? ""} onChange={(event) => setDrafts((current) => ({ ...current, [entry.normalized_word]: event.target.value }))} onBlur={(event) => {
              if (event.target.value !== entry.forms.join(", ")) void save(entry.normalized_word, event.target.value, "user");
            }} placeholder={t("settings.tools.wordForms.unset")} className="h-8 min-w-0 flex-1 rounded-md border border-border bg-bg-surface px-2 text-[11px] text-text-primary outline-none focus:border-accent" />
            <button type="button" title={t("settings.tools.wordForms.refetch")} disabled={busyWord !== null || running} onClick={() => void fetchOne(entry.normalized_word)} className="flex size-8 items-center justify-center rounded-md text-text-muted hover:bg-bg-input disabled:opacity-30">
              {busyWord === entry.normalized_word ? <Loader2 size={13} className="animate-spin" /> : <RefreshCw size={13} />}
            </button>
          </div>
        ))}
        {filtered.length === 0 && <p className="py-6 text-center text-[11px] text-text-muted">{t("settings.tools.wordForms.empty")}</p>}
      </div>
    </div>
  );
}
