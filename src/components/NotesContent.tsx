import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { BookOpen, Check, Download, FileText, Loader2, Pencil, Search, Trash2, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import Input from "./ui/Input";
import Select from "./ui/Select";
import { openReaderWindow } from "../utils/openReaderWindow";

interface Note {
  id: string;
  book_id: string | null;
  book_title: string | null;
  anchor_kind: "word" | "selection";
  normalized_word: string | null;
  scope: "book" | "global" | "detached";
  location: string | null;
  selected_text: string | null;
  content: string;
  created_at: number;
  updated_at: number;
}

interface NotePage {
  notes: Note[];
  next_cursor: string | null;
  total: number;
}

const PAGE_SIZE = 100;

export default function NotesContent() {
  const { t, i18n } = useTranslation();
  const [notes, setNotes] = useState<Note[]>([]);
  const [bookCatalog, setBookCatalog] = useState<Map<string, string>>(new Map());
  const [search, setSearch] = useState("");
  const [bookId, setBookId] = useState("");
  const [anchorKind, setAnchorKind] = useState("");
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [total, setTotal] = useState(0);
  const [updatedAfter, setUpdatedAfter] = useState("");
  const [updatedBefore, setUpdatedBefore] = useState("");
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [deletingId, setDeletingId] = useState<string | null>(null);

  const dateBoundary = (value: string, endOfDay = false) => {
    if (!value) return null;
    const date = new Date(`${value}T${endOfDay ? "23:59:59.999" : "00:00:00.000"}`);
    return Number.isNaN(date.getTime()) ? null : date.getTime();
  };

  const queryPage = useCallback((cursor: string | null, limit = PAGE_SIZE) => invoke<NotePage>("list_notes", {
    bookId: bookId || null,
    anchorKind: anchorKind || null,
    search: search.trim() || null,
    updatedAfter: dateBoundary(updatedAfter),
    updatedBefore: dateBoundary(updatedBefore, true),
    cursor,
    limit,
  }), [anchorKind, bookId, search, updatedAfter, updatedBefore]);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const page = await queryPage(null);
      setNotes(page.notes);
      setNextCursor(page.next_cursor);
      setTotal(page.total);
      setBookCatalog((current) => {
        const next = new Map(current);
        for (const note of page.notes) {
          if (note.book_id) next.set(note.book_id, note.book_title || t("common.unknownBook"));
        }
        return next;
      });
    } finally {
      setLoading(false);
    }
  }, [queryPage, t]);

  useEffect(() => {
    invoke<NotePage>("list_notes", {
      bookId: null, anchorKind: null, search: null, updatedAfter: null,
      updatedBefore: null, cursor: null, limit: 500,
    })
      .then((page) => {
        const next = new Map<string, string>();
        for (const note of page.notes) {
          if (note.book_id) next.set(note.book_id, note.book_title || t("common.unknownBook"));
        }
        setBookCatalog(next);
      })
      .catch(() => {});
  }, [t]);

  useEffect(() => {
    const timer = window.setTimeout(() => refresh().catch(() => {}), 180);
    return () => window.clearTimeout(timer);
  }, [refresh]);

  const loadMore = async () => {
    if (!nextCursor || loadingMore) return;
    setLoadingMore(true);
    try {
      const page = await queryPage(nextCursor);
      setNotes((current) => [...current, ...page.notes]);
      setNextCursor(page.next_cursor);
      setTotal(page.total);
    } finally {
      setLoadingMore(false);
    }
  };

  const downloadCsv = async () => {
    const allNotes: Note[] = [];
    let cursor: string | null = null;
    do {
      const page = await queryPage(cursor, 500);
      allNotes.push(...page.notes);
      cursor = page.next_cursor;
    } while (cursor);
    const escape = (value: unknown) => `"${String(value ?? "").replace(/"/g, '""')}"`;
    const rows = [
      ["type", "scope", "book", "source_text", "word", "note", "updated_at"],
      ...allNotes.map((note) => [
        note.anchor_kind, note.scope, note.book_title, note.selected_text,
        note.normalized_word, note.content, new Date(note.updated_at).toISOString(),
      ]),
    ];
    const href = URL.createObjectURL(new Blob([`\uFEFF${rows.map((row) => row.map(escape).join(",")).join("\n")}`], { type: "text/csv;charset=utf-8" }));
    const link = document.createElement("a");
    link.href = href;
    link.download = "quill-notes.csv";
    link.click();
    window.setTimeout(() => URL.revokeObjectURL(href), 0);
  };

  const bookOptions = useMemo(() => {
    return [
      { value: "", label: t("notes.filters.allBooks") },
      ...Array.from(bookCatalog, ([value, label]) => ({ value, label }))
        .sort((left, right) => left.label.localeCompare(right.label, i18n.language)),
    ];
  }, [bookCatalog, i18n.language, t]);

  const formatter = useMemo(() => new Intl.DateTimeFormat(i18n.language, {
    year: "numeric",
    month: "short",
    day: "numeric",
  }), [i18n.language]);

  const saveEdit = async (note: Note) => {
    if (!draft.trim()) return;
    await invoke("save_note", {
      id: note.id,
      bookId: note.book_id,
      anchorKind: note.anchor_kind,
      word: note.normalized_word,
      scope: note.scope,
      location: note.location,
      selectedText: note.selected_text,
      content: draft.trim(),
    });
    setEditingId(null);
    setDraft("");
    await refresh();
  };

  return (
    <main className="flex min-w-0 flex-1 flex-col bg-bg-surface">
      <header className="relative shrink-0 border-b border-border px-page pb-5 pt-11">
        <div data-tauri-drag-region className="absolute inset-x-0 top-0 h-11" />
        <div className="mb-4 flex items-end justify-between gap-4">
          <div>
            <h1 className="text-[24px] font-semibold text-text-primary">{t("notes.title")}</h1>
            <p className="mt-1 text-[13px] text-text-muted">{t("notes.subtitle")}</p>
          </div>
          <div className="flex items-center gap-2">
            <span className="text-[12px] text-text-muted">{t("notes.count", { count: total })}</span>
            <button type="button" onClick={() => downloadCsv().catch(() => {})} title={t("notes.export")} aria-label={t("notes.export")} className="flex size-8 items-center justify-center rounded-md text-text-muted hover:bg-bg-input"><Download size={15} /></button>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Input
            icon={<Search size={16} />}
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder={t("notes.search")}
            className="min-w-[280px] flex-1"
          />
          <Select className="w-[180px]" value={bookId} onChange={setBookId} options={bookOptions} />
          <Select
            className="w-[150px]"
            value={anchorKind}
            onChange={setAnchorKind}
            options={[
              { value: "", label: t("notes.filters.allTypes") },
              { value: "word", label: t("notes.filters.words") },
              { value: "selection", label: t("notes.filters.selections") },
            ]}
          />
          <label className="flex h-9 items-center gap-1.5 rounded-md border border-border bg-bg-input px-2 text-[11px] text-text-muted">
            {t("notes.filters.from")}
            <input type="date" value={updatedAfter} max={updatedBefore || undefined} onChange={(event) => setUpdatedAfter(event.target.value)} className="bg-transparent text-[12px] text-text-secondary outline-none" />
          </label>
          <label className="flex h-9 items-center gap-1.5 rounded-md border border-border bg-bg-input px-2 text-[11px] text-text-muted">
            {t("notes.filters.to")}
            <input type="date" value={updatedBefore} min={updatedAfter || undefined} onChange={(event) => setUpdatedBefore(event.target.value)} className="bg-transparent text-[12px] text-text-secondary outline-none" />
          </label>
        </div>
      </header>

      <div className="min-h-0 flex-1 overflow-y-auto p-page">
        {loading ? (
          <p className="text-[13px] text-text-muted">{t("home.loading")}</p>
        ) : notes.length === 0 ? (
          <div className="flex h-full flex-col items-center justify-center gap-3 text-center">
            <FileText size={28} className="text-text-placeholder" />
            <p className="text-[14px] font-medium text-text-secondary">{t("notes.empty")}</p>
            <p className="max-w-[360px] text-[12px] text-text-muted">{t("notes.emptyHint")}</p>
          </div>
        ) : (
          <div className="mx-auto max-w-[920px] divide-y divide-border">
            {notes.map((note) => (
              <article key={note.id} className="py-5 first:pt-0">
                <div className="mb-2 flex items-start gap-3">
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                      <p className="break-words text-[13px] font-semibold text-text-primary">
                        {note.selected_text || note.normalized_word || t("notes.untitled")}
                      </p>
                      <span className="rounded-sm bg-bg-input px-1.5 py-0.5 text-[10px] text-text-muted">
                        {t(`notes.type.${note.anchor_kind}`)}
                      </span>
                      {note.scope === "global" ? (
                        <span className="rounded-sm bg-accent-bg px-1.5 py-0.5 text-[10px] text-accent-text">
                          {t("learningCard.notes.scope.global")}
                        </span>
                      ) : note.scope === "detached" ? (
                        <span className="rounded-sm bg-bg-input px-1.5 py-0.5 text-[10px] text-text-muted">
                          {t("learningCard.notes.scope.detached")}
                        </span>
                      ) : null}
                    </div>
                    <p className="mt-1 text-[11px] text-text-muted">
                      {note.book_title || (note.scope === "detached" ? t("notes.detachedSource") : t("common.unknownBook"))} · {formatter.format(note.updated_at)}
                    </p>
                  </div>
                  <div className="flex shrink-0 items-center gap-1">
                    {note.book_id && note.location ? (
                      <button type="button" onClick={() => openReaderWindow(note.book_id!, { cfi: note.location })} title={t("notes.locate")} aria-label={t("notes.locate")} className="flex size-8 items-center justify-center rounded-md text-text-muted hover:bg-bg-input hover:text-accent-text">
                        <BookOpen size={14} />
                      </button>
                    ) : null}
                    <button type="button" onClick={() => { setEditingId(note.id); setDraft(note.content); }} title={t("common.edit")} aria-label={t("common.edit")} className="flex size-8 items-center justify-center rounded-md text-text-muted hover:bg-bg-input">
                      <Pencil size={14} />
                    </button>
                    {deletingId === note.id ? (
                      <>
                        <button type="button" onClick={() => { invoke("delete_note", { id: note.id }).then(refresh).catch(() => {}); setDeletingId(null); }} title={t("common.confirm")} aria-label={t("common.confirm")} className="flex size-8 items-center justify-center rounded-md bg-danger-bg text-danger-text">
                          <Check size={14} />
                        </button>
                        <button type="button" onClick={() => setDeletingId(null)} title={t("common.cancel")} aria-label={t("common.cancel")} className="flex size-8 items-center justify-center rounded-md text-text-muted hover:bg-bg-input">
                          <X size={14} />
                        </button>
                      </>
                    ) : (
                      <button type="button" onClick={() => setDeletingId(note.id)} title={t("common.delete")} aria-label={t("common.delete")} className="flex size-8 items-center justify-center rounded-md text-text-muted hover:bg-danger-bg hover:text-danger-text">
                        <Trash2 size={14} />
                      </button>
                    )}
                  </div>
                </div>
                {editingId === note.id ? (
                  <div>
                    <textarea autoFocus rows={4} value={draft} onChange={(event) => setDraft(event.target.value)} className="w-full resize-y rounded-md border border-border bg-bg-input px-3 py-2 text-[13px] leading-[1.6] text-text-primary outline-none focus:border-accent" />
                    <div className="mt-2 flex justify-end gap-2">
                      <button type="button" onClick={() => { setEditingId(null); setDraft(""); }} className="h-8 px-3 text-[12px] text-text-muted">{t("common.cancel")}</button>
                      <button type="button" disabled={!draft.trim()} onClick={() => saveEdit(note)} className="h-8 rounded-md bg-accent px-3 text-[12px] font-medium text-white disabled:opacity-40">{t("common.save")}</button>
                    </div>
                  </div>
                ) : (
                  <p className="whitespace-pre-wrap break-words text-[13px] leading-[1.7] text-text-secondary">{note.content}</p>
                )}
              </article>
            ))}
            {nextCursor && (
              <div className="flex justify-center py-5">
                <button type="button" disabled={loadingMore} onClick={() => loadMore().catch(() => {})} className="flex h-9 items-center gap-2 rounded-md px-3 text-[12px] text-text-muted hover:bg-bg-input disabled:opacity-50">
                  {loadingMore && <Loader2 size={14} className="animate-spin" />}
                  {t("notes.loadMore")}
                </button>
              </div>
            )}
          </div>
        )}
      </div>
    </main>
  );
}
