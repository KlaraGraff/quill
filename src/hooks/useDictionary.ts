import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emitTo } from "@tauri-apps/api/event";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";

function notifyReaderWindows(
  detail: { bookId?: string; cfi?: string | null },
  event = "vocab-changed",
) {
  if (!detail.bookId) return Promise.resolve([]);
  return WebviewWindow.getAll().then((windows) =>
    Promise.all(
      windows
        .filter((window) => window.label === `reader-${detail.bookId}`)
        .map((window) => emitTo(window.label, event, detail)),
    ),
  );
}

export interface DictionaryWord {
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
  last_review_rating: "again" | "hard" | "good" | "easy" | null;
  created_at: number;
  updated_at: number;
  book_title: string | null;
}

export interface LookupRecord {
  id: string;
  book_id: string;
  lookup_text: string;
  normalized_text: string;
  context_sentence: string | null;
  chapter: string | null;
  cfi: string | null;
  definition: string;
  context_explanation: string | null;
  created_at: number;
  last_looked_up_at: number;
  lookup_count: number;
  book_title: string | null;
}

export interface LookupRecordPage {
  records: LookupRecord[];
  next_cursor: string | null;
  total: number;
  books: LookupBookFacet[];
}

export interface LookupBookFacet {
  book_id: string;
  book_title: string | null;
  count: number;
}

export function useDictionary(bookId: string) {
  const [words, setWords] = useState<DictionaryWord[]>([]);

  const refresh = useCallback(async () => {
    try {
      const result = await invoke<DictionaryWord[]>("list_vocab_words", { bookId });
      setWords(result);
    } catch (err) {
      console.error("Failed to load vocab words:", err);
    }
  }, [bookId]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const add = useCallback(
    async (
      word: string,
      definition: string,
      contextSentence?: string,
      cfi?: string,
      contextExplanation?: string
    ) => {
      const dictionaryWord = await invoke<DictionaryWord>("add_vocab_word", {
        bookId,
        word,
        definition,
        contextSentence: contextSentence || null,
        contextExplanation: contextExplanation || null,
        cfi: cfi || null,
      });
      setWords((prev) => [dictionaryWord, ...prev]);
      window.dispatchEvent(new CustomEvent("vocab-changed", { detail: { bookId, cfi } }));
      notifyReaderWindows({ bookId, cfi }).catch(() => {});
      return dictionaryWord;
    },
    [bookId]
  );

  const remove = useCallback(async (id: string) => {
    const word = words.find((item) => item.id === id);
    await invoke("remove_vocab_word", { id });
    setWords((prev) => prev.filter((w) => w.id !== id));
    window.dispatchEvent(new CustomEvent("vocab-changed", { detail: { bookId: word?.book_id, cfi: word?.cfi } }));
    notifyReaderWindows({ bookId: word?.book_id, cfi: word?.cfi }).catch(() => {});
  }, [words]);

  const checkExists = useCallback(
    async (word: string): Promise<string | null> => {
      return invoke<string | null>("check_vocab_exists", { bookId, word });
    },
    [bookId]
  );

  return { words, refresh, add, remove, checkExists };
}

export function useAllDictionary() {
  const [words, setWords] = useState<DictionaryWord[]>([]);

  const refresh = useCallback(async () => {
    try {
      const result = await invoke<DictionaryWord[]>("list_all_vocab_words");
      setWords(result);
    } catch (err) {
      console.error("Failed to load all vocab words:", err);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const remove = useCallback(async (id: string) => {
    const word = words.find((item) => item.id === id);
    await invoke("remove_vocab_word", { id });
    setWords((prev) => prev.filter((w) => w.id !== id));
    window.dispatchEvent(new CustomEvent("vocab-changed", { detail: { bookId: word?.book_id, cfi: word?.cfi } }));
    notifyReaderWindows({ bookId: word?.book_id, cfi: word?.cfi }).catch(() => {});
  }, [words]);

  const updateMastery = useCallback(async (id: string, mastery: "new" | "learning" | "mastered", nextReviewAt: number | null) => {
    const changed = words.find((word) => word.id === id);
    await invoke("update_vocab_mastery", { id, mastery, nextReviewAt });
    setWords((prev) => prev.map((word) => word.id === id
      ? { ...word, mastery, next_review_at: nextReviewAt, updated_at: Date.now() }
      : word));
    window.dispatchEvent(new CustomEvent("vocab-changed", { detail: { bookId: changed?.book_id, cfi: changed?.cfi } }));
    notifyReaderWindows({ bookId: changed?.book_id, cfi: changed?.cfi }).catch(() => {});
  }, [words]);

  const recordReview = useCallback(async (id: string, rating: "again" | "hard" | "good" | "easy") => {
    const reviewed = await invoke<DictionaryWord>("record_vocab_review", { id, rating });
    setWords((prev) => prev.map((word) => word.id === id ? { ...word, ...reviewed } : word));
    window.dispatchEvent(new CustomEvent("vocab-changed", { detail: { bookId: reviewed.book_id, cfi: reviewed.cfi } }));
    notifyReaderWindows({ bookId: reviewed.book_id, cfi: reviewed.cfi }).catch(() => {});
    return reviewed;
  }, []);

  return { words, refresh, remove, updateMastery, recordReview };
}

export function useAllLookupHistory() {
  const [records, setRecords] = useState<LookupRecord[]>([]);
  const [total, setTotal] = useState(0);
  const [cursor, setCursor] = useState<string | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);
  const [books, setBooks] = useState<LookupBookFacet[]>([]);

  const refresh = useCallback(async (search?: string, bookId?: string) => {
    try {
      const page = await invoke<LookupRecordPage>("list_all_lookup_records", {
        search: search || null,
        bookId: bookId || null,
        cursor: null,
        limit: 50,
      });
      setRecords(page.records);
      setTotal(page.total);
      setCursor(page.next_cursor);
      setBooks(page.books);
    } catch (err) {
      console.error("Failed to load lookup history:", err);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const loadMore = useCallback(async (search?: string, bookId?: string) => {
    if (!cursor || loadingMore) return;
    setLoadingMore(true);
    try {
      const page = await invoke<LookupRecordPage>("list_all_lookup_records", {
        search: search || null,
        bookId: bookId || null,
        cursor,
        limit: 50,
      });
      setRecords((previous) => [...previous, ...page.records]);
      setCursor(page.next_cursor);
    } finally {
      setLoadingMore(false);
    }
  }, [cursor, loadingMore]);

  const remove = useCallback(async (id: string) => {
    const record = records.find((item) => item.id === id);
    await invoke("delete_lookup_record", { id });
    setRecords((previous) => previous.filter((item) => item.id !== id));
    setTotal((previous) => Math.max(0, previous - 1));
    window.dispatchEvent(new CustomEvent("lookup-record-changed", { detail: { bookId: record?.book_id, cfi: record?.cfi } }));
    notifyReaderWindows({ bookId: record?.book_id, cfi: record?.cfi }, "lookup-record-changed").catch(() => {});
  }, [records]);

  const clear = useCallback(async (bookId?: string) => {
    await invoke("clear_lookup_records", { bookId: bookId || null });
    setRecords([]);
    setTotal(0);
    setCursor(null);
    window.dispatchEvent(new CustomEvent("lookup-record-changed", { detail: { bookId } }));
    notifyReaderWindows({ bookId }, "lookup-record-changed").catch(() => {});
  }, []);

  useEffect(() => {
    const refreshForChange = () => { refresh(); };
    window.addEventListener("lookup-record-changed", refreshForChange);
    return () => window.removeEventListener("lookup-record-changed", refreshForChange);
  }, [refresh]);

  return { records, total, books, hasMore: cursor !== null, loadingMore, refresh, loadMore, remove, clear };
}
