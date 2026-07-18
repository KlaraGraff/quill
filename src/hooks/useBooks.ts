import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";

export interface Book {
  id: string;
  title: string;
  author: string;
  description: string | null;
  cover_path: string | null;
  file_path: string;
  // Text-like source files are prepared into a local reader document. Native
  // formats retain their source extension for Foliate's parser selection.
  format: "epub" | "pdf" | "text" | "mobi" | "azw" | "azw3" | "fb2" | "fbz" | "cbz";
  source_format: string | null;
  source_sha256?: string | null;
  render_format: string | null;
  preparation_state: "pending" | "preparing" | "ready" | "failed";
  preparation_error: string | null;
  genre: string | null;
  pages: number | null;
  status: "reading" | "finished" | "unread";
  progress: number;
  current_cfi: string | null;
  created_at: number;
  updated_at: number;
  available: boolean;
  cover_data: string | null;
}

export type BookAvailabilityStatus = "available" | "icloud_placeholder" | "missing";

export interface BookAvailability {
  status: BookAvailabilityStatus;
  available: boolean;
}

interface BookPage {
  books: Book[];
  next_cursor: string | null;
  total: number;
}

export function useBooks(filter?: string, search?: string, collectionId?: string) {
  const [books, setBooks] = useState<Book[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [cursor, setCursor] = useState<string | null>(null);
  const [hasMore, setHasMore] = useState(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const page = await invoke<BookPage>("list_books", {
        filter: filter || null,
        search: search || null,
        collectionId: collectionId || null,
        cursor: null,
        limit: null,
      });
      setBooks(page.books);
      setTotal(page.total);
      setCursor(page.next_cursor);
      setHasMore(page.next_cursor !== null);
    } catch (err) {
      console.error("Failed to load books:", err);
    } finally {
      setLoading(false);
    }
  }, [filter, search, collectionId]);

  const loadMore = useCallback(async () => {
    if (!cursor || loadingMore) return;
    setLoadingMore(true);
    try {
      const page = await invoke<BookPage>("list_books", {
        filter: filter || null,
        search: search || null,
        collectionId: collectionId || null,
        cursor,
        limit: null,
      });
      setBooks((prev) => [...prev, ...page.books]);
      setCursor(page.next_cursor);
      setHasMore(page.next_cursor !== null);
    } catch (err) {
      console.error("Failed to load more books:", err);
    } finally {
      setLoadingMore(false);
    }
  }, [cursor, filter, search, collectionId, loadingMore]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  return { books, total, loading, loadingMore, hasMore, loadMore, refresh };
}

async function importFile(): Promise<Book | null> {
  return invoke<Book | null>("import_book_from_dialog");
}

export const importBookDialog = { importFile };

export async function getBook(id: string): Promise<Book> {
  return invoke<Book>("get_book", { id });
}

export async function deleteBook(id: string, preserveNotes = false): Promise<void> {
  return invoke("delete_book", { id, preserveNotes });
}

export async function updateReadingProgress(
  id: string,
  progress: number,
  cfi?: string
): Promise<void> {
  return invoke("update_reading_progress", {
    id,
    progress,
    cfi: cfi || null,
  });
}

export async function markFinished(id: string): Promise<void> {
  return invoke("mark_finished", { id });
}

export async function updateBookStatus(id: string, status: "reading" | "finished" | "unread"): Promise<void> {
  return invoke("update_book_status", { id, status });
}

export async function updateBookMetadata(
  id: string,
  title: string,
  author: string
): Promise<void> {
  return invoke("update_book_metadata", { id, title, author });
}

export async function updateBookCover(id: string, imagePath: string): Promise<void> {
  return invoke("update_book_cover", { id, imagePath });
}

export async function checkBookAvailable(id: string): Promise<BookAvailability> {
  return invoke<BookAvailability>("check_book_available", { id });
}

export async function retryTextBookPreparation(id: string): Promise<void> {
  return invoke("retry_text_book_preparation", { bookId: id });
}

export async function retryBookConversion(id: string): Promise<void> {
  return invoke("retry_book_conversion", { bookId: id });
}

/** A book whose reader format is EPUB but whose source is a different format
 * (MOBI/AZW3, later scanned PDF) — it is served from a locally converted EPUB. */
export function isConversionBook(book: Book): boolean {
  return (
    book.render_format === "epub" &&
    book.source_format !== null &&
    book.source_format !== "epub"
  );
}

/** Books that must finish background preparation (text conversion or
 * source→EPUB conversion) before the reader can open them. */
export function needsPreparation(book: Book): boolean {
  return book.render_format === "text" || isConversionBook(book);
}

/** True while such a book is not yet ready to open. */
export function isPendingPreparation(book: Book): boolean {
  return needsPreparation(book) && book.preparation_state !== "ready";
}

/** Dispatch a preparation retry to the right backend command by book kind. */
export async function retryPreparation(book: Book): Promise<void> {
  if (isConversionBook(book)) return retryBookConversion(book.id);
  return retryTextBookPreparation(book.id);
}
