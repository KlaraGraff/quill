import { useState, useEffect, useRef } from "react";
import { openReaderWindow } from "../utils/openReaderWindow";
import { AlertCircle, Check, CloudDownload, Loader2 } from "lucide-react";
import type { Book } from "../hooks/useBooks";
import { deleteBook, markFinished, retryTextBookPreparation, updateBookStatus } from "../hooks/useBooks";
import BookContextMenu from "./BookContextMenu";
import EditMetadataModal from "./EditMetadataModal";
import { useTranslation } from "react-i18next";
import DeleteBookDialog, { type DeleteBookNotePolicy } from "./DeleteBookDialog";

function CoverImage({ src, alt, title }: { src: string; alt: string; title: string }) {
  const [failed, setFailed] = useState(false);
  if (failed) {
    return (
      <div className="w-full h-full flex items-center justify-center bg-bg-muted">
        <span className="text-[10px] text-text-muted text-center px-1">{title}</span>
      </div>
    );
  }
  return (
    <img
      src={src}
      alt={alt}
      className="w-full h-full object-cover"
      onError={() => setFailed(true)}
    />
  );
}

interface BookListProps {
  books: Book[];
  hasMore?: boolean;
  loadMore?: () => void;
  loadingMore?: boolean;
  activeCollectionId?: string;
  onBooksChanged?: () => void;
}

export default function BookList({ books, hasMore, loadMore, loadingMore, activeCollectionId, onBooksChanged }: BookListProps) {
  const { t } = useTranslation();
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    book: Book;
  } | null>(null);
  const [editBook, setEditBook] = useState<Book | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Book | null>(null);

  const handleContextMenu = (e: React.MouseEvent, book: Book) => {
    e.preventDefault();
    setContextMenu({ x: e.clientX, y: e.clientY, book });
  };

  const isPendingTextBook = (book: Book) => book.render_format === "text" && book.preparation_state !== "ready";

  const openBook = async (book: Book) => {
    if (book.available === false) return;
    if (book.render_format === "text" && book.preparation_state === "failed") {
      await retryTextBookPreparation(book.id);
      onBooksChanged?.();
      return;
    }
    if (!isPendingTextBook(book)) openReaderWindow(book.id);
  };

  return (
    <>
      <div className="flex flex-col gap-4 p-page">
        {books.map((book) => (
          <button
            key={book.id}
            onClick={() => { openBook(book).catch(() => {}); }}
            onContextMenu={(e) => handleContextMenu(e, book)}
            className={`flex items-start gap-4 p-4 border border-border rounded-lg text-left cursor-pointer hover:bg-bg-muted transition-colors ${book.available === false ? "opacity-60" : ""} ${isPendingTextBook(book) ? "cursor-wait" : ""}`}
          >
            {/* Cover */}
            <div className="relative w-[96px] h-[144px] shrink-0 rounded-lg overflow-hidden bg-border shadow-card">
              {book.cover_data ? (
                <CoverImage src={book.cover_data} alt={book.title} title={book.title} />
              ) : (
                <div className="w-full h-full flex items-center justify-center bg-bg-muted">
                  <span className="text-[10px] text-text-muted text-center px-1">
                    {book.title}
                  </span>
                </div>
              )}
              {book.available === false && (
                <div className="absolute inset-0 flex items-center justify-center bg-black/30">
                  <CloudDownload size={24} className="text-white" />
                </div>
              )}
              {isPendingTextBook(book) && (
                <div className="absolute inset-0 flex items-center justify-center bg-black/45">
                  {book.preparation_state === "failed" ? <AlertCircle size={24} className="text-white" /> : <Loader2 size={24} className="animate-spin text-white" />}
                </div>
              )}
              {book.status === "finished" && book.available !== false && (
                <div className="absolute top-1 right-1 w-[22px] h-[20px] bg-success rounded-full flex items-center justify-center">
                  <Check size={12} className="text-white" strokeWidth={3} />
                </div>
              )}
            </div>

            {/* Info */}
            <div className="flex-1 min-w-0 h-[144px] flex flex-col">
              <h3 className="text-[18px] font-semibold text-text-primary tracking-[-0.44px] leading-[27px]">
                {book.title}
              </h3>
              <p className="text-[14px] text-text-secondary tracking-[-0.15px] leading-5">
                {book.author}
              </p>
              {book.description && (
                <p className="text-[12px] text-text-muted leading-4 mt-1 truncate">
                  {book.description}
                </p>
              )}
              {isPendingTextBook(book) && (
                <p className="mt-1 text-[12px] text-text-muted">
                  {book.preparation_state === "failed" ? t("book.preparationFailed") : t("book.preparing")}
                </p>
              )}

              <div className="mt-auto">
                <div className="flex flex-col gap-1">
                  <div className="flex items-center justify-between">
                    <span className="text-[12px] text-text-secondary">
                      {book.status === "finished"
                        ? "Finished"
                        : book.status === "reading"
                          ? `${book.progress}% complete`
                          : "Not started"}
                    </span>
                    {book.pages != null && (
                      <span className="text-[12px] text-text-secondary">
                        {book.pages} pages
                      </span>
                    )}
                  </div>
                  <div className="w-full h-1.5 bg-accent/20 rounded-full overflow-hidden">
                    <div
                      className="h-full bg-accent rounded-full"
                      style={{ width: `${book.status === "finished" ? 100 : book.progress}%` }}
                    />
                  </div>
                </div>
              </div>
            </div>
          </button>
        ))}
      </div>

      {contextMenu && (
        <BookContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          bookId={contextMenu.book.id}
          bookStatus={contextMenu.book.status}
          activeCollectionId={activeCollectionId}
          onClose={() => setContextMenu(null)}
          onMarkFinished={async () => {
            await markFinished(contextMenu.book.id);
            setContextMenu(null);
            onBooksChanged?.();
          }}
          onMarkReading={async () => {
            await updateBookStatus(contextMenu.book.id, "reading");
            setContextMenu(null);
            onBooksChanged?.();
          }}
          onMarkUnread={async () => {
            await updateBookStatus(contextMenu.book.id, "unread");
            setContextMenu(null);
            onBooksChanged?.();
          }}
          onEditInfo={() => {
            setEditBook(contextMenu.book);
            setContextMenu(null);
          }}
          onDelete={() => {
            setDeleteTarget(contextMenu.book);
            setContextMenu(null);
          }}
          onBooksChanged={onBooksChanged}
        />
      )}

      {deleteTarget && (
        <DeleteBookDialog
          title={deleteTarget.title}
          onCancel={() => setDeleteTarget(null)}
          onConfirm={async (policy: DeleteBookNotePolicy) => {
            await deleteBook(deleteTarget.id, policy === "preserve");
            setDeleteTarget(null);
            onBooksChanged?.();
          }}
        />
      )}

      {editBook && (
        <EditMetadataModal
          bookId={editBook.id}
          currentTitle={editBook.title}
          currentAuthor={editBook.author}
          onClose={() => setEditBook(null)}
          onSaved={() => {
            setEditBook(null);
            onBooksChanged?.();
          }}
        />
      )}

      {hasMore && <LoadMoreSentinel loadMore={loadMore} loadingMore={loadingMore} />}
    </>
  );
}

function LoadMoreSentinel({ loadMore, loadingMore }: { loadMore?: () => void; loadingMore?: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el || !loadMore) return;
    const observer = new IntersectionObserver(
      ([entry]) => { if (entry.isIntersecting) loadMore(); },
      { rootMargin: "200px" },
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, [loadMore]);

  return (
    <div ref={ref} className="flex justify-center py-4">
      {loadingMore && <Loader2 size={20} className="text-text-muted animate-spin" />}
    </div>
  );
}
