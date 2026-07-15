//! Deterministic per-event merge.
//!
//! `apply_event(tx, event)` folds one peer event into the local SQLite
//! materialized view. Three rules:
//!
//! 1. **Add events** (`*.add`, `*.create`, `*.import`) use `INSERT OR IGNORE`
//!    and are guarded by a tombstone check on the entity's id. Once a row is
//!    deleted, a later add with the same id never resurrects it — the user
//!    must mint a new id.
//! 2. **LWW updates** (`*.set`, `*.rename`, `*.color.set`, …) compare the
//!    tuple `(stored.updated_at, stored.updated_by_device)` against
//!    `(event.ts, event.device)`. Strict-less-than wins; equality means we've
//!    already applied this exact write. The compare lives in the `WHERE`
//!    clause so SQLite skips the row in one statement.
//! 3. **Deletes** drop the row plus all children manually (explicit
//!    cascading — the app does not rely on `ON DELETE CASCADE`),
//!    then `INSERT OR IGNORE` a tombstone keyed `(entity, id)`.
//!
//! Foreign keys are off at the connection level (the app never enables
//! `PRAGMA foreign_keys`). All cascading deletes are explicit. This
//! means cross-device ordering can safely deliver a child event before
//! its parent — the orphan row lands and becomes visible once the
//! parent arrives on a later tick.

use std::collections::BTreeMap;

use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::Value;

use crate::error::{AppError, AppResult};

use super::events::{
    word_mark_exception_id, BookImportPayload, BookSummaryPayload, BookmarkPayload,
    ChatMessagePayload, Event, EventBody, HighlightPayload, LookupOccurrenceMarkPayload,
    NotePayload, VocabPayload, WordMarkExceptionPayload, WordMarkPayload,
};

/// Fold `event` into `tx`. Idempotent — applying the same event twice is a
/// no-op (LWW equality and `INSERT OR IGNORE` both short-circuit).
pub fn apply_event(tx: &Transaction, event: &Event) -> AppResult<()> {
    super::validation::validate_event(event, &event.device)?;
    match &event.body {
        EventBody::BookImport(p) => apply_book_import(tx, event, p),
        EventBody::BookDelete { id } => apply_book_delete(tx, event, id),
        EventBody::BookProgressSet {
            book,
            progress,
            cfi,
        } => apply_book_progress(tx, event, book, *progress, cfi.as_deref()),
        EventBody::BookStatusSet { book, status } => apply_book_status(tx, event, book, status),
        EventBody::BookMetadataSet { book, field, value } => {
            apply_book_metadata(tx, event, book, field, value)
        }

        EventBody::HighlightAdd(p) => apply_highlight_add(tx, event, p),
        EventBody::HighlightDelete { id } => apply_highlight_delete(tx, event, id),
        EventBody::HighlightColorSet { id, color } => apply_highlight_color(tx, event, id, color),
        EventBody::HighlightNoteSet { id, note } => {
            apply_highlight_note(tx, event, id, note.as_deref())
        }

        EventBody::BookmarkAdd(p) => apply_bookmark_add(tx, event, p),
        EventBody::BookmarkDelete { id } => apply_bookmark_delete(tx, event, id),

        EventBody::VocabAdd(p) => apply_vocab_add(tx, event, p),
        EventBody::VocabMasterySet {
            id,
            mastery,
            next_review_at,
            review_count,
            review_interval_days,
            last_reviewed_at,
            last_review_rating,
            fsrs_stability,
            fsrs_difficulty,
            fsrs_version,
        } => apply_vocab_mastery(
            tx,
            event,
            id,
            VocabMasteryUpdate {
                mastery,
                next_review_at: *next_review_at,
                review_count: *review_count,
                review_interval_days: *review_interval_days,
                last_reviewed_at: *last_reviewed_at,
                last_review_rating: last_review_rating.as_deref(),
                fsrs_stability: *fsrs_stability,
                fsrs_difficulty: *fsrs_difficulty,
                fsrs_version: *fsrs_version,
            },
        ),
        EventBody::VocabDelete { id } => apply_vocab_delete(tx, event, id),

        EventBody::NoteUpsert(payload) => apply_note_upsert(tx, event, payload),
        EventBody::NoteDelete { id } => apply_note_delete(tx, event, id),
        EventBody::WordMarkUpsert(payload) => apply_word_mark_upsert(tx, event, payload),
        EventBody::WordMarkDelete { id } => apply_word_mark_delete(tx, event, id),
        EventBody::WordMarkExceptionSet(payload) => {
            apply_word_mark_exception_set(tx, event, payload)
        }
        EventBody::LookupOccurrenceMarkSet(payload) => {
            apply_lookup_occurrence_mark_set(tx, event, payload)
        }
        EventBody::BookSummaryUpsert(payload) => apply_book_summary_upsert(tx, event, payload),

        EventBody::TranslationAdd(_) | EventBody::TranslationDelete { .. } => Ok(()),

        EventBody::CollectionCreate {
            id,
            name,
            sort_order,
        } => apply_collection_create(tx, event, id, name, *sort_order),
        EventBody::CollectionRename { id, name } => apply_collection_rename(tx, event, id, name),
        EventBody::CollectionReorder { id, sort_order } => {
            apply_collection_reorder(tx, event, id, *sort_order)
        }
        EventBody::CollectionDelete { id } => apply_collection_delete(tx, event, id),
        EventBody::CollectionBookAdd { collection, book } => {
            apply_collection_book_add(tx, event, collection, book)
        }
        EventBody::CollectionBookRemove { collection, book } => {
            apply_collection_book_remove(tx, event, collection, book)
        }

        EventBody::ChatCreate {
            id,
            book,
            title,
            model,
        } => apply_chat_create(tx, event, id, book, title, model.as_deref()),
        EventBody::ChatRename { id, title } => apply_chat_rename(tx, event, id, title),
        EventBody::ChatDelete { id } => apply_chat_delete(tx, event, id),
        EventBody::ChatMessageAdd(p) => apply_chat_message_add(tx, event, p),
        EventBody::ChatMessageReplace(p) => apply_chat_message_replace(tx, event, p),
    }
}

// ---------------------------------------------------------------------------
// Tombstone helpers.
// ---------------------------------------------------------------------------

/// Tombstone entity tags. Stable strings — they appear on disk in
/// `_tombstones.entity` and inside snapshots, so don't rename casually.
pub mod entity {
    pub const BOOK: &str = "book";
    pub const HIGHLIGHT: &str = "highlight";
    pub const BOOKMARK: &str = "bookmark";
    pub const VOCAB: &str = "vocab";
    pub const NOTE: &str = "note";
    pub const WORD_MARK: &str = "word_mark";
    pub const WORD_MARK_EXCEPTION: &str = "word_mark_exception";
    pub const LOOKUP_OCCURRENCE_MARK: &str = "lookup_occurrence_mark";
    pub const COLLECTION: &str = "collection";
    /// Composite-key entity for `collection_books`. Id format:
    /// `"<collection_id>:<book_id>"`.
    pub const COLLECTION_BOOK: &str = "collection_book";
    pub const CHAT: &str = "chat";
    pub const CHAT_MESSAGE: &str = "chat_message";
}

pub fn is_tombstoned(tx: &Transaction, entity: &str, id: &str) -> AppResult<bool> {
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM _tombstones WHERE entity = ?1 AND id = ?2)",
        params![entity, id],
        |r| r.get(0),
    )?;
    Ok(exists)
}

pub fn tombstone_timestamp(tx: &Transaction, entity: &str, id: &str) -> AppResult<Option<i64>> {
    tx.query_row(
        "SELECT ts FROM _tombstones WHERE entity = ?1 AND id = ?2",
        params![entity, id],
        |row| row.get(0),
    )
    .map(Some)
    .or_else(|error| match error {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other.into()),
    })
}

pub fn insert_tombstone(tx: &Transaction, entity: &str, id: &str, ts: i64) -> AppResult<()> {
    tx.execute(
        "INSERT INTO _tombstones (entity, id, ts) VALUES (?1, ?2, ?3)
         ON CONFLICT(entity, id) DO UPDATE SET ts = MAX(_tombstones.ts, excluded.ts)",
        params![entity, id, ts],
    )?;
    Ok(())
}

/// True if any of the given `(entity, id)` pairs has a tombstone. Used by
/// child `*.add` arms to suppress events whose parent has been deleted —
/// otherwise a late event published by an offline peer can re-create
/// orphan rows for a permanently-tombstoned book/collection/chat (the
/// own-tombstone check on the child id alone is not enough because the
/// child may never have existed locally, so it has no tombstone of its
/// own).
fn parent_tombstoned(tx: &Transaction, parents: &[(&str, &str)]) -> AppResult<bool> {
    for (entity, id) in parents {
        if is_tombstoned(tx, entity, id)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Drop the row identified by `(entity, id)` plus every FK-child the event
/// path would cascade to. Does NOT write the tombstone for `(entity, id)`
/// itself — callers must call `insert_tombstone` separately for that.
/// Idempotent: if the row is already gone, every DELETE is a no-op.
///
/// `ts` is the deletion timestamp threaded through to any per-child
/// tombstones we have to write inline (e.g. cascaded chats — see
/// `cascade_delete_book`). Using the event's ts (and snapshot tombstones'
/// stored ts) instead of wall-clock keeps `_tombstones` rows
/// byte-identical across replay runs, which the design doc calls out as
/// a Chunk 4 invariant for snapshot equivalence.
///
/// Used by both the event-path delete arms and `Snapshot::apply_peer`'s
/// tombstone pass so the two paths stay byte-equivalent. For the composite
/// `collection_book` entity, `id` must be `"<col>:<book>"` — the same
/// format the merge engine uses when writing those tombstones.
pub fn cascade_delete(tx: &Transaction, entity: &str, id: &str, ts: i64) -> AppResult<()> {
    match entity {
        entity::BOOK => cascade_delete_book(tx, id, ts),
        entity::COLLECTION => cascade_delete_collection(tx, id),
        entity::CHAT => cascade_delete_chat(tx, id),
        entity::COLLECTION_BOOK => cascade_delete_collection_book(tx, id),
        entity::HIGHLIGHT => {
            tx.execute("DELETE FROM highlights WHERE id = ?1", params![id])?;
            Ok(())
        }
        entity::BOOKMARK => {
            tx.execute("DELETE FROM bookmarks WHERE id = ?1", params![id])?;
            Ok(())
        }
        entity::VOCAB => {
            tx.execute("DELETE FROM vocab_words WHERE id = ?1", params![id])?;
            Ok(())
        }
        entity::NOTE => {
            tx.execute("DELETE FROM notes WHERE id = ?1", params![id])?;
            Ok(())
        }
        entity::WORD_MARK => {
            tx.execute(
                "UPDATE word_mark_rules SET enabled = 0, updated_at = MAX(updated_at, ?2)
                 WHERE id = ?1",
                params![id, ts],
            )?;
            Ok(())
        }
        entity::WORD_MARK_EXCEPTION => {
            tx.execute(
                "UPDATE word_mark_exceptions SET excluded = 0, updated_at = MAX(updated_at, ?2)
                 WHERE id = ?1",
                params![id, ts],
            )?;
            Ok(())
        }
        entity::LOOKUP_OCCURRENCE_MARK => {
            tx.execute(
                "UPDATE lookup_occurrence_marks SET enabled = 0, updated_at = MAX(updated_at, ?2)
                 WHERE id = ?1",
                params![id, ts],
            )?;
            Ok(())
        }
        "translation" => Ok(()),
        entity::CHAT_MESSAGE => {
            tx.execute("DELETE FROM chat_messages WHERE id = ?1", params![id])?;
            Ok(())
        }
        other => {
            log::warn!("sync: cascade_delete called with unknown entity {other:?}");
            Ok(())
        }
    }
}

fn cascade_delete_book(tx: &Transaction, id: &str, ts: i64) -> AppResult<()> {
    // Mirror the `apply_book_delete` cascade exactly. Replay runs with FK
    // off, so we can't rely on ON DELETE CASCADE.
    //
    // For the direct-child tables (highlights, bookmarks, vocab_words,
    // collection_books) we don't write per-row tombstones —
    // late `*.add` events for those tables are caught by their parent-
    // tombstone check on `('book', id)`.
    //
    // For chats we DO tombstone each cascaded chat, because the chat-
    // message merge arm checks `('chat', chat_id)`, not `('book', book_id)`,
    // so without this an orphan chat.message.add could resurrect after the
    // book is gone. The tombstone ts must be the parent-delete event ts
    // (not wall-clock) — `_tombstones` rows ride along in snapshots and
    // need to be byte-identical across replay runs.
    // Grounding chunks and their FTS index are local-only derived data (see
    // docs/impls/1-grounded-book-chat-overview.md D2), so they must never be
    // emitted as sync events or snapshots. They still need local cleanup here
    // because this is shared by direct deletes and replayed book deletes.
    tx.execute(
        "DELETE FROM book_chunks_fts WHERE book_id = ?1",
        params![id],
    )?;
    let vector_table_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'book_chunk_vectors')",
        [],
        |row| row.get(0),
    )?;
    if vector_table_exists {
        tx.execute(
            "DELETE FROM book_chunk_vectors WHERE book_id = ?1",
            params![id],
        )?;
    }
    tx.execute(
        "DELETE FROM book_chunk_embeddings WHERE book_id = ?1",
        params![id],
    )?;
    tx.execute("DELETE FROM book_chunks WHERE book_id = ?1", params![id])?;
    tx.execute(
        "DELETE FROM book_index_state WHERE book_id = ?1",
        params![id],
    )?;
    tx.execute("DELETE FROM book_summaries WHERE book_id = ?1", params![id])?;
    tx.execute("DELETE FROM bookmarks WHERE book_id = ?1", params![id])?;
    tx.execute("DELETE FROM highlights WHERE book_id = ?1", params![id])?;
    tx.execute("DELETE FROM vocab_words WHERE book_id = ?1", params![id])?;
    tx.execute("DELETE FROM lookup_records WHERE book_id = ?1", params![id])?;
    tx.execute(
        "DELETE FROM word_mark_exceptions WHERE book_id = ?1",
        params![id],
    )?;
    tx.execute(
        "DELETE FROM word_mark_rules WHERE book_id = ?1",
        params![id],
    )?;
    tx.execute(
        "DELETE FROM lookup_occurrence_marks WHERE book_id = ?1",
        params![id],
    )?;
    tx.execute(
        "DELETE FROM notes WHERE book_id = ?1 AND scope = 'book'",
        params![id],
    )?;
    tx.execute(
        "UPDATE notes SET book_id = NULL WHERE book_id = ?1 AND scope = 'global'",
        params![id],
    )?;
    tx.execute(
        "DELETE FROM collection_books WHERE book_id = ?1",
        params![id],
    )?;
    tx.execute("DELETE FROM book_settings WHERE book_id = ?1", params![id])?;
    tx.execute(
        "DELETE FROM settings WHERE key = ?1",
        params![format!("book_spoiler_guard_{id}")],
    )?;
    let chat_ids: Vec<String> = {
        let mut stmt = tx.prepare("SELECT id FROM chats WHERE book_id = ?1")?;
        let collected: Vec<String> = stmt
            .query_map(params![id], |r| r.get::<_, String>(0))?
            .collect::<Result<_, _>>()?;
        collected
    };
    for chat_id in &chat_ids {
        tx.execute(
            "DELETE FROM chat_messages WHERE chat_id = ?1",
            params![chat_id],
        )?;
        insert_tombstone(tx, entity::CHAT, chat_id, ts)?;
    }
    tx.execute("DELETE FROM chats WHERE book_id = ?1", params![id])?;
    tx.execute("DELETE FROM books WHERE id = ?1", params![id])?;
    Ok(())
}

fn cascade_delete_collection(tx: &Transaction, id: &str) -> AppResult<()> {
    tx.execute(
        "DELETE FROM collection_books WHERE collection_id = ?1",
        params![id],
    )?;
    tx.execute("DELETE FROM collections WHERE id = ?1", params![id])?;
    Ok(())
}

fn cascade_delete_chat(tx: &Transaction, id: &str) -> AppResult<()> {
    tx.execute("DELETE FROM chat_messages WHERE chat_id = ?1", params![id])?;
    tx.execute("DELETE FROM chats WHERE id = ?1", params![id])?;
    Ok(())
}

fn cascade_delete_collection_book(tx: &Transaction, key: &str) -> AppResult<()> {
    let Some((col, book)) = key.split_once(':') else {
        log::warn!(
            "sync: cascade_delete_collection_book got malformed key {key:?}, expected '<col>:<book>'"
        );
        return Ok(());
    };
    tx.execute(
        "DELETE FROM collection_books WHERE collection_id = ?1 AND book_id = ?2",
        params![col, book],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// books
// ---------------------------------------------------------------------------

fn apply_book_import(tx: &Transaction, event: &Event, p: &BookImportPayload) -> AppResult<()> {
    if is_tombstoned(tx, entity::BOOK, &p.id)? {
        return Ok(());
    }
    tx.execute(
        "INSERT OR IGNORE INTO books
         (id, title, author, description, cover_path, file_path, genre, pages,
          format, source_format, render_format, source_file_path, source_sha256, conversion_version, preparation_state, preparation_error, status, progress, current_cfi, created_at, updated_at, updated_by_device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                 CASE WHEN ?11 = 'text' THEN 'pending' ELSE 'ready' END, NULL,
                 'unread', 0, NULL, ?15, ?15, ?16)",
        params![
            p.id,
            p.title,
            p.author,
            p.description,
            p.cover_path,
            p.file_path,
            p.genre,
            p.pages,
            p.format,
            p.source_format.as_deref().unwrap_or(&p.format),
            p.render_format.as_deref().unwrap_or(&p.format),
            p.source_file_path,
            p.source_sha256,
            p.conversion_version,
            event.ts,
            event.device,
        ],
    )?;
    Ok(())
}

fn apply_book_delete(tx: &Transaction, event: &Event, id: &str) -> AppResult<()> {
    cascade_delete(tx, entity::BOOK, id, event.ts)?;
    insert_tombstone(tx, entity::BOOK, id, event.ts)?;
    Ok(())
}

fn apply_book_progress(
    tx: &Transaction,
    event: &Event,
    book: &str,
    progress: i32,
    cfi: Option<&str>,
) -> AppResult<()> {
    tx.execute(
        "UPDATE books
         SET progress = ?1, current_cfi = ?2, updated_at = ?3, updated_by_device = ?4
         WHERE id = ?5
           AND (updated_at < ?3 OR (updated_at = ?3 AND updated_by_device < ?4))",
        params![progress, cfi, event.ts, event.device, book],
    )?;
    Ok(())
}

fn apply_book_status(tx: &Transaction, event: &Event, book: &str, status: &str) -> AppResult<()> {
    tx.execute(
        "UPDATE books
         SET status = ?1, updated_at = ?2, updated_by_device = ?3
         WHERE id = ?4
           AND (updated_at < ?2 OR (updated_at = ?2 AND updated_by_device < ?3))",
        params![status, event.ts, event.device, book],
    )?;
    Ok(())
}

fn apply_book_metadata(
    tx: &Transaction,
    event: &Event,
    book: &str,
    field: &str,
    value: &Value,
) -> AppResult<()> {
    // Allowlist — only fields the metadata-set event is allowed to touch.
    // Unknown fields (e.g. from a future schema) are dropped silently rather
    // than blowing up a whole replay tick.
    let column = match field {
        "title" | "author" | "description" | "cover_path" | "genre" | "file_path" => field,
        "pages" => "pages",
        _ => {
            log::warn!("sync: unknown book.metadata.set field {field:?}, skipping");
            return Ok(());
        }
    };

    // Use `<=` rather than `<`: the live `update_book_metadata` command
    // emits one event per field changed (see Step 3 of the spec), so a
    // multi-field edit like "rename + author" produces two events with
    // identical `(ts, device)`. With strict `<` the second would lose the
    // tuple compare and be silently skipped. `<=` lets every event in the
    // group land while staying idempotent on re-apply (the column already
    // holds `value`, so the UPDATE is a no-op write).
    let sql = format!(
        "UPDATE books
         SET {column} = ?1, updated_at = ?2, updated_by_device = ?3
         WHERE id = ?4
           AND (updated_at < ?2 OR (updated_at = ?2 AND updated_by_device <= ?3))"
    );

    if column == "pages" {
        let int_val: Option<i64> = match value {
            Value::Null => None,
            Value::Number(n) => n.as_i64().or_else(|| n.as_u64().map(|u| u as i64)),
            other => {
                return Err(AppError::Other(format!(
                    "book.metadata.set pages expects number/null, got {other:?}"
                )));
            }
        };
        tx.execute(&sql, params![int_val, event.ts, event.device, book])?;
    } else {
        let str_val: Option<String> = match value {
            Value::Null => None,
            Value::String(s) => Some(s.clone()),
            other => {
                return Err(AppError::Other(format!(
                    "book.metadata.set {field} expects string/null, got {other:?}"
                )));
            }
        };
        tx.execute(&sql, params![str_val, event.ts, event.device, book])?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// highlights
// ---------------------------------------------------------------------------

fn apply_highlight_add(tx: &Transaction, event: &Event, p: &HighlightPayload) -> AppResult<()> {
    if is_tombstoned(tx, entity::HIGHLIGHT, &p.id)?
        || parent_tombstoned(tx, &[(entity::BOOK, &p.book_id)])?
    {
        return Ok(());
    }
    tx.execute(
        "INSERT OR IGNORE INTO highlights
         (id, book_id, cfi_range, color, note, text_content,
          created_at, updated_at, updated_by_device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8)",
        params![
            p.id,
            p.book_id,
            p.cfi_range,
            p.color,
            p.note,
            p.text_content,
            event.ts,
            event.device,
        ],
    )?;
    Ok(())
}

fn apply_highlight_delete(tx: &Transaction, event: &Event, id: &str) -> AppResult<()> {
    tx.execute("DELETE FROM highlights WHERE id = ?1", params![id])?;
    insert_tombstone(tx, entity::HIGHLIGHT, id, event.ts)?;
    Ok(())
}

fn apply_highlight_color(tx: &Transaction, event: &Event, id: &str, color: &str) -> AppResult<()> {
    tx.execute(
        "UPDATE highlights
         SET color = ?1, updated_at = ?2, updated_by_device = ?3
         WHERE id = ?4
           AND (updated_at < ?2 OR (updated_at = ?2 AND updated_by_device < ?3))",
        params![color, event.ts, event.device, id],
    )?;
    Ok(())
}

fn apply_highlight_note(
    tx: &Transaction,
    event: &Event,
    id: &str,
    note: Option<&str>,
) -> AppResult<()> {
    tx.execute(
        "UPDATE highlights
         SET note = ?1, updated_at = ?2, updated_by_device = ?3
         WHERE id = ?4
           AND (updated_at < ?2 OR (updated_at = ?2 AND updated_by_device < ?3))",
        params![note, event.ts, event.device, id],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// bookmarks (append-only — no LWW, no updated_by_device)
// ---------------------------------------------------------------------------

fn apply_bookmark_add(tx: &Transaction, event: &Event, p: &BookmarkPayload) -> AppResult<()> {
    if is_tombstoned(tx, entity::BOOKMARK, &p.id)?
        || parent_tombstoned(tx, &[(entity::BOOK, &p.book_id)])?
    {
        return Ok(());
    }
    tx.execute(
        "INSERT OR IGNORE INTO bookmarks (id, book_id, cfi, label, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![p.id, p.book_id, p.cfi, p.label, event.ts],
    )?;
    Ok(())
}

fn apply_bookmark_delete(tx: &Transaction, event: &Event, id: &str) -> AppResult<()> {
    tx.execute("DELETE FROM bookmarks WHERE id = ?1", params![id])?;
    insert_tombstone(tx, entity::BOOKMARK, id, event.ts)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// vocab
// ---------------------------------------------------------------------------

fn apply_vocab_add(tx: &Transaction, event: &Event, p: &VocabPayload) -> AppResult<()> {
    if is_tombstoned(tx, entity::VOCAB, &p.id)?
        || parent_tombstoned(tx, &[(entity::BOOK, &p.book_id)])?
    {
        return Ok(());
    }
    tx.execute(
        "INSERT OR IGNORE INTO vocab_words
         (id, book_id, word, definition, context_sentence, context_explanation, cfi,
          mastery, review_count, next_review_at,
          review_interval_days, last_reviewed_at, last_review_rating,
          fsrs_stability, fsrs_difficulty, fsrs_version,
          created_at, updated_at, updated_by_device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
        params![
            p.id,
            p.book_id,
            p.word,
            p.definition,
            p.context_sentence,
            p.context_explanation,
            p.cfi,
            p.mastery,
            p.review_count,
            p.next_review_at,
            p.review_interval_days,
            p.last_reviewed_at,
            p.last_review_rating,
            p.fsrs_stability,
            p.fsrs_difficulty,
            p.fsrs_version,
            p.created_at.unwrap_or(event.ts),
            event.ts,
            event.device,
        ],
    )?;
    Ok(())
}

struct VocabMasteryUpdate<'a> {
    mastery: &'a str,
    next_review_at: Option<i64>,
    review_count: i64,
    review_interval_days: i64,
    last_reviewed_at: Option<i64>,
    last_review_rating: Option<&'a str>,
    fsrs_stability: Option<f64>,
    fsrs_difficulty: Option<f64>,
    fsrs_version: i64,
}

fn apply_vocab_mastery(
    tx: &Transaction,
    event: &Event,
    id: &str,
    update: VocabMasteryUpdate<'_>,
) -> AppResult<()> {
    tx.execute(
        "UPDATE vocab_words
         SET mastery = ?1,
             next_review_at = ?2,
             review_count = ?3,
             review_interval_days = ?4,
             last_reviewed_at = ?5,
             last_review_rating = ?6,
             fsrs_stability = ?7,
             fsrs_difficulty = ?8,
             fsrs_version = ?9,
             updated_at = ?10,
             updated_by_device = ?11
         WHERE id = ?12
           AND (updated_at < ?10 OR (updated_at = ?10 AND updated_by_device < ?11))",
        params![
            update.mastery,
            update.next_review_at,
            update.review_count,
            update.review_interval_days,
            update.last_reviewed_at,
            update.last_review_rating,
            update.fsrs_stability,
            update.fsrs_difficulty,
            update.fsrs_version,
            event.ts,
            event.device,
            id
        ],
    )?;
    Ok(())
}

fn apply_vocab_delete(tx: &Transaction, event: &Event, id: &str) -> AppResult<()> {
    tx.execute("DELETE FROM vocab_words WHERE id = ?1", params![id])?;
    insert_tombstone(tx, entity::VOCAB, id, event.ts)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// notes and whole-book word markers
// ---------------------------------------------------------------------------

fn apply_note_upsert(tx: &Transaction, event: &Event, payload: &NotePayload) -> AppResult<()> {
    if is_tombstoned(tx, entity::NOTE, &payload.id)? {
        return Ok(());
    }
    let effective_book_id = match payload.book_id.as_deref() {
        Some(book_id) if parent_tombstoned(tx, &[(entity::BOOK, book_id)])? => {
            if payload.scope == "book" {
                return Ok(());
            }
            None
        }
        value => value,
    };
    if effective_book_id.is_none() && payload.book_id.is_some() {
        // Parent deletion is an invariant, not a competing note edit. Repair
        // rows reattached by an older client even if their note LWW tuple is
        // newer than this incoming edit.
        tx.execute(
            "UPDATE notes SET book_id = NULL WHERE id = ?1",
            params![payload.id],
        )?;
    }
    tx.execute(
        "INSERT INTO notes (id, book_id, anchor_kind, normalized_word, scope, location, selected_text, content, content_format, created_at, updated_at, updated_by_device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(id) DO UPDATE SET book_id = excluded.book_id, anchor_kind = excluded.anchor_kind,
           normalized_word = excluded.normalized_word, scope = excluded.scope, location = excluded.location,
           selected_text = excluded.selected_text, content = excluded.content,
           content_format = excluded.content_format, updated_at = excluded.updated_at,
           updated_by_device = excluded.updated_by_device
         WHERE notes.updated_at < excluded.updated_at
            OR (notes.updated_at = excluded.updated_at AND notes.updated_by_device < excluded.updated_by_device)",
        params![
            payload.id,
            effective_book_id,
            payload.anchor_kind,
            payload.normalized_word,
            payload.scope,
            payload.location,
            payload.selected_text,
            payload.content,
            payload.content_format,
            payload.created_at,
            event.ts,
            event.device,
        ],
    )?;
    Ok(())
}

fn apply_note_delete(tx: &Transaction, event: &Event, id: &str) -> AppResult<()> {
    tx.execute("DELETE FROM notes WHERE id = ?1", params![id])?;
    insert_tombstone(tx, entity::NOTE, id, event.ts)
}

#[derive(Debug)]
struct LegacyWordMarkException {
    location: String,
    excluded: bool,
    created_at: i64,
    updated_at: i64,
    updated_by_device: String,
}

/// Move exception rows that still point at a pre-stable rule id onto the
/// canonical rule entity. The stable id is identity metadata, so changing it
/// must not strand an otherwise newer exception behind an orphaned rule id.
///
/// `preserve_values` is used by the local one-time canonicalization command:
/// it carries the user's exclusions across the identity repair. Replay and
/// snapshot rule updates pass `false`, making the effective rule tuple the
/// usual reset barrier while still retaining any genuinely newer exception.
#[allow(clippy::too_many_arguments)]
pub(crate) fn reconcile_legacy_word_mark_exceptions(
    tx: &Transaction,
    legacy_rule_id: &str,
    canonical_rule_id: &str,
    book_id: &str,
    normalized_word: &str,
    barrier_ts: i64,
    barrier_device: &str,
    preserve_values: bool,
) -> AppResult<Vec<WordMarkExceptionPayload>> {
    if legacy_rule_id == canonical_rule_id {
        return Ok(Vec::new());
    }

    let rows = {
        let mut statement = tx.prepare(
            "SELECT location, excluded, created_at, updated_at, updated_by_device
             FROM word_mark_exceptions
             WHERE rule_id = ?1 OR rule_id = ?2",
        )?;
        let rows = statement
            .query_map(params![legacy_rule_id, canonical_rule_id], |row| {
                Ok(LegacyWordMarkException {
                    location: row.get(0)?,
                    excluded: row.get::<_, i64>(1)? != 0,
                    created_at: row.get(2)?,
                    updated_at: row.get(3)?,
                    updated_by_device: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows
    };
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    // Both ids may have received the same location while sync was catching
    // up. Collapse that pair with the same LWW tuple used everywhere else.
    let mut by_location: BTreeMap<String, LegacyWordMarkException> = BTreeMap::new();
    for row in rows {
        match by_location.entry(row.location.clone()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(row);
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                let current = entry.get_mut();
                let earliest_created_at = current.created_at.min(row.created_at);
                if (current.updated_at, current.updated_by_device.as_str())
                    < (row.updated_at, row.updated_by_device.as_str())
                {
                    *current = row;
                }
                current.created_at = earliest_created_at;
            }
        }
    }

    tx.execute(
        "DELETE FROM word_mark_exceptions WHERE rule_id = ?1 OR rule_id = ?2",
        params![legacy_rule_id, canonical_rule_id],
    )?;

    let barrier = (barrier_ts, barrier_device);
    let mut publishable = Vec::with_capacity(by_location.len());
    for (_, mut row) in by_location {
        let row_tuple = (row.updated_at, row.updated_by_device.as_str());
        if row_tuple < barrier {
            if !preserve_values {
                row.excluded = false;
            }
            row.updated_at = barrier_ts;
            row.updated_by_device = barrier_device.to_string();
        }
        let id = word_mark_exception_id(canonical_rule_id, &row.location);
        tx.execute(
            "INSERT INTO word_mark_exceptions
             (id, rule_id, book_id, normalized_word, location, excluded,
              created_at, updated_at, updated_by_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id,
                canonical_rule_id,
                book_id,
                normalized_word,
                row.location,
                row.excluded as i64,
                row.created_at,
                row.updated_at,
                row.updated_by_device,
            ],
        )?;

        // An event body inherits the command's envelope timestamp. Only rows
        // lifted to that tuple can be faithfully represented in this batch;
        // a newer row is retained locally and will already be present in its
        // originating event stream or the next snapshot.
        if (row.updated_at, row.updated_by_device.as_str()) == barrier {
            publishable.push(WordMarkExceptionPayload {
                id,
                rule_id: canonical_rule_id.to_string(),
                book_id: book_id.to_string(),
                normalized_word: normalized_word.to_string(),
                location: row.location,
                excluded: row.excluded,
                created_at: row.created_at,
            });
        }
    }
    Ok(publishable)
}

fn apply_word_mark_upsert(
    tx: &Transaction,
    event: &Event,
    payload: &WordMarkPayload,
) -> AppResult<()> {
    if parent_tombstoned(tx, &[(entity::BOOK, &payload.book_id)])? {
        return Ok(());
    }
    // Early development builds represented cancellation as a permanent
    // tombstone. A later full upsert supersedes it; an older upsert does not.
    if tombstone_timestamp(tx, entity::WORD_MARK, &payload.id)?
        .is_some_and(|deleted_at| deleted_at >= event.ts)
    {
        return Ok(());
    }
    tx.execute(
        "DELETE FROM _tombstones WHERE entity = ?1 AND id = ?2",
        params![entity::WORD_MARK, payload.id],
    )?;
    let prior_rule_id: Option<String> = tx
        .query_row(
            "SELECT id FROM word_mark_rules
             WHERE book_id = ?1 AND normalized_word = ?2 AND match_mode = ?3",
            params![payload.book_id, payload.normalized_word, payload.match_mode],
            |row| row.get(0),
        )
        .optional()?;
    let changed = tx.execute(
        "INSERT INTO word_mark_rules (id, book_id, normalized_word, display_word, match_mode, color, enabled, created_at, updated_at, updated_by_device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(book_id, normalized_word, match_mode) DO UPDATE SET id = excluded.id,
           display_word = excluded.display_word, color = excluded.color,
           enabled = excluded.enabled, updated_at = excluded.updated_at,
           updated_by_device = excluded.updated_by_device
         WHERE word_mark_rules.updated_at < excluded.updated_at
            OR (word_mark_rules.updated_at = excluded.updated_at AND word_mark_rules.updated_by_device < excluded.updated_by_device)",
        params![
            payload.id,
            payload.book_id,
            payload.normalized_word,
            payload.display_word,
            payload.match_mode,
            payload.color,
            payload.enabled as i64,
            payload.created_at,
            event.ts,
            event.device,
        ],
    )?;
    let repaired_legacy_id = prior_rule_id
        .as_deref()
        .is_some_and(|prior_id| prior_id != payload.id);
    if repaired_legacy_id {
        // Identity repair is independent of LWW content. Even when the
        // incoming payload loses to a newer local tuple, the natural-key row
        // must use the canonical id or its exceptions and snapshots remain
        // invalid forever.
        tx.execute(
            "UPDATE word_mark_rules SET id = ?1
             WHERE book_id = ?2 AND normalized_word = ?3 AND match_mode = ?4",
            params![
                payload.id,
                payload.book_id,
                payload.normalized_word,
                payload.match_mode
            ],
        )?;
        let (effective_ts, effective_device): (i64, String) = tx.query_row(
            "SELECT updated_at, updated_by_device FROM word_mark_rules WHERE id = ?1",
            params![payload.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        reconcile_legacy_word_mark_exceptions(
            tx,
            prior_rule_id.as_deref().expect("legacy id checked above"),
            &payload.id,
            &payload.book_id,
            &payload.normalized_word,
            effective_ts,
            &effective_device,
            false,
        )?;
    } else if changed > 0 {
        // The rule tuple is a reset barrier for its occurrence exceptions.
        // Store disabled rows rather than deleting them so delayed older
        // events cannot resurrect exclusions after an explicit re-mark.
        tx.execute(
            "UPDATE word_mark_exceptions
             SET excluded = 0, updated_at = ?2, updated_by_device = ?3
             WHERE rule_id = ?1
               AND (updated_at < ?2 OR (updated_at = ?2 AND updated_by_device < ?3))",
            params![payload.id, event.ts, event.device],
        )?;
    }
    Ok(())
}

fn apply_word_mark_delete(tx: &Transaction, event: &Event, id: &str) -> AppResult<()> {
    // Compatibility for logs produced by the first development build. New
    // commands publish WordMarkUpsert(enabled=false), but replaying the legacy
    // delete should converge to the same disabled state when the row exists.
    let current_tuple: Option<(i64, String)> = tx
        .query_row(
            "SELECT updated_at, updated_by_device FROM word_mark_rules WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if current_tuple
        .as_ref()
        .is_some_and(|(ts, device)| (*ts, device.as_str()) > (event.ts, event.device.as_str()))
    {
        return Ok(());
    }
    let changed = tx.execute(
        "UPDATE word_mark_rules SET enabled = 0, updated_at = ?2, updated_by_device = ?3
         WHERE id = ?1 AND (updated_at < ?2 OR (updated_at = ?2 AND updated_by_device < ?3))",
        params![id, event.ts, event.device],
    )?;
    if changed > 0 {
        tx.execute(
            "UPDATE word_mark_exceptions
             SET excluded = 0, updated_at = ?2, updated_by_device = ?3
             WHERE rule_id = ?1
               AND (updated_at < ?2 OR (updated_at = ?2 AND updated_by_device < ?3))",
            params![id, event.ts, event.device],
        )?;
    }
    // A compatibility delete may arrive before its older upsert. Retaining a
    // timestamped tombstone is what makes that delivery order converge; a
    // genuinely newer full upsert is still allowed to supersede it above.
    insert_tombstone(tx, entity::WORD_MARK, id, event.ts)
}

fn apply_word_mark_exception_set(
    tx: &Transaction,
    event: &Event,
    payload: &WordMarkExceptionPayload,
) -> AppResult<()> {
    if parent_tombstoned(tx, &[(entity::BOOK, &payload.book_id)])? {
        return Ok(());
    }
    // Keep a validated exception even if its parent rule has not replayed yet.
    // Cross-device clock skew can order a dependent event ahead of its parent;
    // query paths join against an enabled rule, so the temporary orphan stays
    // invisible and becomes effective once the parent arrives.
    let parent_tuple = tx
        .query_row(
            "SELECT updated_at, updated_by_device FROM word_mark_rules WHERE id = ?1",
            params![payload.rule_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    // If a newer parent is already materialized, persist the same disabled
    // barrier row that applying the exception first and the parent second
    // would have produced. Dropping the stale event outright is visually
    // equivalent but breaks byte-for-byte convergence and later snapshots.
    let (excluded, updated_at, updated_by_device) = match parent_tuple {
        Some((ts, device)) if (ts, device.as_str()) > (event.ts, event.device.as_str()) => {
            (false, ts, device)
        }
        _ => (payload.excluded, event.ts, event.device.clone()),
    };
    tx.execute(
        "INSERT INTO word_mark_exceptions
         (id, rule_id, book_id, normalized_word, location, excluded,
          created_at, updated_at, updated_by_device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(rule_id, location) DO UPDATE SET
           id = excluded.id, book_id = excluded.book_id,
           normalized_word = excluded.normalized_word,
           excluded = excluded.excluded, updated_at = excluded.updated_at,
           updated_by_device = excluded.updated_by_device
         WHERE word_mark_exceptions.updated_at < excluded.updated_at
            OR (word_mark_exceptions.updated_at = excluded.updated_at
                AND word_mark_exceptions.updated_by_device < excluded.updated_by_device)",
        params![
            payload.id,
            payload.rule_id,
            payload.book_id,
            payload.normalized_word,
            payload.location,
            excluded as i64,
            payload.created_at,
            updated_at,
            updated_by_device,
        ],
    )?;
    Ok(())
}

fn apply_lookup_occurrence_mark_set(
    tx: &Transaction,
    event: &Event,
    payload: &LookupOccurrenceMarkPayload,
) -> AppResult<()> {
    if parent_tombstoned(tx, &[(entity::BOOK, &payload.book_id)])? {
        return Ok(());
    }
    tx.execute(
        "INSERT INTO lookup_occurrence_marks
         (id, book_id, normalized_word, display_word, location, enabled,
          created_at, updated_at, updated_by_device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(book_id, location) DO UPDATE SET
           id=excluded.id, normalized_word=excluded.normalized_word,
           display_word=excluded.display_word, enabled=excluded.enabled,
           updated_at=excluded.updated_at, updated_by_device=excluded.updated_by_device
         WHERE (lookup_occurrence_marks.updated_at, lookup_occurrence_marks.updated_by_device)
             < (excluded.updated_at, excluded.updated_by_device)",
        params![
            payload.id,
            payload.book_id,
            payload.normalized_word,
            payload.display_word,
            payload.location,
            payload.enabled as i64,
            payload.created_at,
            event.ts,
            event.device,
        ],
    )?;
    Ok(())
}

fn apply_book_summary_upsert(
    tx: &Transaction,
    _event: &Event,
    payload: &BookSummaryPayload,
) -> AppResult<()> {
    if is_tombstoned(tx, entity::BOOK, &payload.book_id)? {
        return Ok(());
    }
    tx.execute(
        "INSERT INTO book_summaries
         (id, book_id, scope, section_index, section_title, content, language, model,
          source_sha256, created_at, updated_at, user_edited)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(book_id, scope, COALESCE(section_index, -1)) DO UPDATE SET
           id=excluded.id, section_title=excluded.section_title, content=excluded.content,
           language=excluded.language, model=excluded.model, source_sha256=excluded.source_sha256,
           updated_at=excluded.updated_at, user_edited=excluded.user_edited
         WHERE book_summaries.updated_at < excluded.updated_at",
        params![
            payload.id,
            payload.book_id,
            payload.scope,
            payload.section_index,
            payload.section_title,
            payload.content,
            payload.language,
            payload.model,
            payload.source_sha256,
            payload.created_at,
            payload.updated_at,
            payload.user_edited as i64,
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// collections + collection_books
// ---------------------------------------------------------------------------

fn apply_collection_create(
    tx: &Transaction,
    event: &Event,
    id: &str,
    name: &str,
    sort_order: i32,
) -> AppResult<()> {
    if is_tombstoned(tx, entity::COLLECTION, id)? {
        return Ok(());
    }
    tx.execute(
        "INSERT OR IGNORE INTO collections
         (id, name, sort_order, created_at, updated_at, updated_by_device)
         VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
        params![id, name, sort_order, event.ts, event.device],
    )?;
    Ok(())
}

fn apply_collection_rename(tx: &Transaction, event: &Event, id: &str, name: &str) -> AppResult<()> {
    tx.execute(
        "UPDATE collections
         SET name = ?1, updated_at = ?2, updated_by_device = ?3
         WHERE id = ?4
           AND (updated_at < ?2 OR (updated_at = ?2 AND updated_by_device < ?3))",
        params![name, event.ts, event.device, id],
    )?;
    Ok(())
}

fn apply_collection_reorder(
    tx: &Transaction,
    event: &Event,
    id: &str,
    sort_order: i32,
) -> AppResult<()> {
    tx.execute(
        "UPDATE collections
         SET sort_order = ?1, updated_at = ?2, updated_by_device = ?3
         WHERE id = ?4
           AND (updated_at < ?2 OR (updated_at = ?2 AND updated_by_device < ?3))",
        params![sort_order, event.ts, event.device, id],
    )?;
    Ok(())
}

fn apply_collection_delete(tx: &Transaction, event: &Event, id: &str) -> AppResult<()> {
    // Tombstone each join row first so a delayed `collection.book.add` for
    // the same pair stays suppressed. cascade_delete then drops the rows.
    let pairs: Vec<String> = {
        let mut stmt =
            tx.prepare("SELECT book_id FROM collection_books WHERE collection_id = ?1")?;
        let collected: Vec<String> = stmt
            .query_map(params![id], |r| r.get::<_, String>(0))?
            .collect::<Result<_, _>>()?;
        collected
    };
    for book_id in pairs {
        let key = format!("{id}:{book_id}");
        insert_tombstone(tx, entity::COLLECTION_BOOK, &key, event.ts)?;
    }
    cascade_delete(tx, entity::COLLECTION, id, event.ts)?;
    insert_tombstone(tx, entity::COLLECTION, id, event.ts)?;
    Ok(())
}

fn apply_collection_book_add(
    tx: &Transaction,
    event: &Event,
    collection: &str,
    book: &str,
) -> AppResult<()> {
    let key = format!("{collection}:{book}");
    if is_tombstoned(tx, entity::COLLECTION_BOOK, &key)?
        || parent_tombstoned(
            tx,
            &[(entity::COLLECTION, collection), (entity::BOOK, book)],
        )?
    {
        return Ok(());
    }
    tx.execute(
        "INSERT OR IGNORE INTO collection_books
         (collection_id, book_id, created_at, updated_at, updated_by_device)
         VALUES (?1, ?2, ?3, ?3, ?4)",
        params![collection, book, event.ts, event.device],
    )?;
    Ok(())
}

fn apply_collection_book_remove(
    tx: &Transaction,
    event: &Event,
    collection: &str,
    book: &str,
) -> AppResult<()> {
    let key = format!("{collection}:{book}");
    cascade_delete(tx, entity::COLLECTION_BOOK, &key, event.ts)?;
    insert_tombstone(tx, entity::COLLECTION_BOOK, &key, event.ts)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// chats + chat_messages
// ---------------------------------------------------------------------------

fn apply_chat_create(
    tx: &Transaction,
    event: &Event,
    id: &str,
    book: &str,
    title: &str,
    model: Option<&str>,
) -> AppResult<()> {
    if is_tombstoned(tx, entity::CHAT, id)? {
        return Ok(());
    }
    if parent_tombstoned(tx, &[(entity::BOOK, book)])? {
        // Parent book is tombstoned — suppress the chat AND leave a chat
        // tombstone so any delayed `chat.message.add` for this id is also
        // dropped. The message arm only consults `('chat', chat_id)`
        // tombstones; without this, a stale (chat.create + chat.message.add)
        // pair from an offline peer would slip the message in as an
        // orphan after the create was silently discarded.
        insert_tombstone(tx, entity::CHAT, id, event.ts)?;
        return Ok(());
    }
    tx.execute(
        "INSERT OR IGNORE INTO chats
         (id, book_id, title, model, pinned, created_at, updated_at, updated_by_device)
         VALUES (?1, ?2, ?3, ?4, 0, ?5, ?5, ?6)",
        params![id, book, title, model, event.ts, event.device],
    )?;
    Ok(())
}

fn apply_chat_rename(tx: &Transaction, event: &Event, id: &str, title: &str) -> AppResult<()> {
    tx.execute(
        "UPDATE chats
         SET title = ?1, updated_at = ?2, updated_by_device = ?3
         WHERE id = ?4
           AND (updated_at < ?2 OR (updated_at = ?2 AND updated_by_device < ?3))",
        params![title, event.ts, event.device, id],
    )?;
    Ok(())
}

fn apply_chat_delete(tx: &Transaction, event: &Event, id: &str) -> AppResult<()> {
    cascade_delete(tx, entity::CHAT, id, event.ts)?;
    insert_tombstone(tx, entity::CHAT, id, event.ts)?;
    Ok(())
}

fn apply_chat_message_add(
    tx: &Transaction,
    event: &Event,
    p: &ChatMessagePayload,
) -> AppResult<()> {
    if is_tombstoned(tx, entity::CHAT_MESSAGE, &p.id)?
        || parent_tombstoned(tx, &[(entity::CHAT, &p.chat_id)])?
    {
        return Ok(());
    }
    tx.execute(
        "INSERT OR IGNORE INTO chat_messages
         (id, chat_id, role, content, context, metadata, created_at, updated_at, updated_by_device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8)",
        params![
            p.id,
            p.chat_id,
            p.role,
            p.content,
            p.context,
            p.metadata,
            event.ts,
            event.device
        ],
    )?;
    // Mirror the live `add_chat_message` command's side effect — the parent
    // chat's recency drives chat-list ordering, so peers must see the bump
    // too. LWW guard prevents an older message event from dragging
    // `updated_at` backwards if the chat has been renamed since.
    tx.execute(
        "UPDATE chats
         SET updated_at = ?1, updated_by_device = ?2
         WHERE id = ?3
           AND (updated_at < ?1 OR (updated_at = ?1 AND updated_by_device < ?2))",
        params![event.ts, event.device, p.chat_id],
    )?;
    Ok(())
}

fn apply_chat_message_replace(
    tx: &Transaction,
    event: &Event,
    p: &ChatMessagePayload,
) -> AppResult<()> {
    if is_tombstoned(tx, entity::CHAT_MESSAGE, &p.id)?
        || parent_tombstoned(tx, &[(entity::CHAT, &p.chat_id)])?
    {
        return Ok(());
    }
    let changed = tx.execute(
        "UPDATE chat_messages
         SET content = ?1, metadata = ?2, updated_at = ?3, updated_by_device = ?4
         WHERE id = ?5 AND chat_id = ?6 AND role = 'assistant'
           AND (updated_at, updated_by_device) < (?3, ?4)",
        params![
            p.content,
            p.metadata,
            event.ts,
            event.device,
            p.id,
            p.chat_id
        ],
    )?;
    if changed == 0 {
        return Ok(());
    }
    tx.execute(
        "UPDATE chats
         SET updated_at = ?1, updated_by_device = ?2
         WHERE id = ?3
           AND (updated_at < ?1 OR (updated_at = ?1 AND updated_by_device < ?2))",
        params![event.ts, event.device, p.chat_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Each test follows the same shape: open an in-memory DB, run migrations
    //! up to 11, build events, apply, assert SQL state. We toggle FK off for
    //! the apply tx because the replay engine does the same — see the module
    //! docstring for the rationale.

    use super::*;
    use crate::db::Db;
    use crate::sync::events::*;
    use rusqlite::Connection;
    use serde_json::json;

    fn open_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        Db::run_migrations_on(&conn).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        conn
    }

    fn ev(ts: i64, device: &str, body: EventBody) -> Event {
        Event {
            id: format!("01HYZX0000000000000000{:04X}", ts as u16),
            ts,
            device: device.to_string(),
            v: EVENT_SCHEMA_VERSION,
            body,
            extra: serde_json::Map::new(),
        }
    }

    fn apply_all(conn: &mut Connection, events: &[Event]) {
        let tx = conn.transaction().unwrap();
        for e in events {
            apply_event(&tx, e).expect("apply_event failed");
        }
        tx.commit().unwrap();
    }

    fn import_book(id: &str) -> EventBody {
        EventBody::BookImport(BookImportPayload {
            id: id.into(),
            title: "T".into(),
            author: "A".into(),
            description: None,
            cover_path: None,
            file_path: format!("books/{id}.epub"),
            format: "epub".into(),
            source_format: None,
            render_format: None,
            source_file_path: None,
            source_sha256: None,
            conversion_version: 0,
            genre: None,
            pages: Some(100),
        })
    }

    fn book_summary(content: &str, updated_at: i64) -> EventBody {
        EventBody::BookSummaryUpsert(BookSummaryPayload {
            id: format!("summary-{updated_at}"),
            book_id: "b1".into(),
            scope: "book".into(),
            section_index: None,
            section_title: None,
            content: content.into(),
            language: "en".into(),
            model: None,
            source_sha256: "hash".into(),
            created_at: updated_at,
            updated_at,
            user_edited: false,
        })
    }

    #[test]
    fn book_summary_upsert_is_idempotent_and_latest_timestamp_wins() {
        let mut conn = open_db();
        apply_all(
            &mut conn,
            &[
                ev(1, "dev-a", import_book("b1")),
                ev(20, "dev-a", book_summary("new", 20)),
                ev(10, "dev-b", book_summary("old", 10)),
                ev(20, "dev-a", book_summary("new", 20)),
            ],
        );
        let summary: String = conn
            .query_row(
                "SELECT content FROM book_summaries WHERE book_id = 'b1' AND scope = 'book'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(summary, "new");
    }

    fn add_highlight(id: &str, book: &str, color: &str) -> EventBody {
        EventBody::HighlightAdd(HighlightPayload {
            id: id.into(),
            book_id: book.into(),
            cfi_range: "epubcfi(/6/4!/2,/1:0,/1:5)".into(),
            color: color.into(),
            note: None,
            text_content: None,
        })
    }

    fn note(id: &str, book: &str, scope: &str, content: &str, created_at: i64) -> EventBody {
        EventBody::NoteUpsert(NotePayload {
            id: id.into(),
            book_id: Some(book.into()),
            anchor_kind: "word".into(),
            normalized_word: Some("term".into()),
            scope: scope.into(),
            location: Some("epubcfi(/6/4!)".into()),
            selected_text: Some("term".into()),
            content: content.into(),
            content_format: "plain_text".into(),
            created_at,
        })
    }

    fn word_mark(book: &str, enabled: bool, color: &str, created_at: i64) -> EventBody {
        let normalized_word = "term".to_string();
        EventBody::WordMarkUpsert(WordMarkPayload {
            id: word_mark_rule_id(book, &normalized_word, "exact"),
            book_id: book.into(),
            normalized_word,
            display_word: "Term".into(),
            match_mode: "exact".into(),
            color: color.into(),
            enabled,
            created_at,
        })
    }

    fn word_mark_exception(
        book: &str,
        location: &str,
        excluded: bool,
        created_at: i64,
    ) -> EventBody {
        let normalized_word = "term".to_string();
        let rule_id = word_mark_rule_id(book, &normalized_word, "exact");
        EventBody::WordMarkExceptionSet(WordMarkExceptionPayload {
            id: word_mark_exception_id(&rule_id, location),
            rule_id,
            book_id: book.into(),
            normalized_word,
            location: location.into(),
            excluded,
            created_at,
        })
    }

    // -----------------------------------------------------------------------
    // book.import / book.delete + tombstone semantics
    // -----------------------------------------------------------------------

    #[test]
    fn book_import_inserts_with_event_metadata() {
        let mut db = open_db();
        apply_all(&mut db, &[ev(1000, "dev-A", import_book("b1"))]);

        let (title, ts, dev): (String, i64, String) = db
            .query_row(
                "SELECT title, updated_at, updated_by_device FROM books WHERE id = 'b1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(title, "T");
        assert_eq!(ts, 1000);
        assert_eq!(dev, "dev-A");
    }

    #[test]
    fn book_delete_removes_row_and_writes_tombstone() {
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(2000, "dev-A", EventBody::BookDelete { id: "b1".into() }),
            ],
        );

        let n: i64 = db
            .query_row("SELECT COUNT(*) FROM books WHERE id = 'b1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 0);
        let tomb: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM _tombstones WHERE entity = 'book' AND id = 'b1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tomb, 1);
    }

    #[test]
    fn book_delete_cascades_learning_tools_and_late_global_note_stays_detached() {
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(
                    1100,
                    "dev-A",
                    note("note-book", "b1", "book", "book note", 1100),
                ),
                ev(
                    1200,
                    "dev-A",
                    note("note-global", "b1", "global", "global note", 1200),
                ),
                ev(1300, "dev-A", word_mark("b1", true, "lookup", 1300)),
                ev(2000, "dev-B", EventBody::BookDelete { id: "b1".into() }),
                // An offline peer may publish a newer edit that still carries
                // the deleted source book. Content should merge, but the
                // parent link must remain detached.
                ev(
                    2100,
                    "dev-A",
                    note("note-global", "b1", "global", "edited later", 1200),
                ),
                ev(
                    2200,
                    "dev-A",
                    note("note-book-late", "b1", "book", "must not return", 2200),
                ),
                ev(2300, "dev-A", word_mark("b1", true, "lookup", 2300)),
            ],
        );

        let notes: Vec<(String, Option<String>, String)> = {
            let mut statement = db
                .prepare("SELECT id, book_id, content FROM notes ORDER BY id")
                .unwrap();
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(
            notes,
            vec![("note-global".to_string(), None, "edited later".to_string())]
        );
        let marker_count: i64 = db
            .query_row("SELECT COUNT(*) FROM word_mark_rules", [], |row| row.get(0))
            .unwrap();
        assert_eq!(marker_count, 0);
    }

    #[test]
    fn concurrent_same_word_rules_converge_to_one_stable_lww_row() {
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(2000, "dev-A", word_mark("b1", true, "lookup", 2000)),
                ev(2000, "dev-B", word_mark("b1", false, "muted", 2000)),
            ],
        );

        let expected_id = word_mark_rule_id("b1", "term", "exact");
        let row: (String, i64, String, String) = db
            .query_row(
                "SELECT id, enabled, color, updated_by_device FROM word_mark_rules",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(row, (expected_id, 0, "muted".into(), "dev-B".into()));
        let count: i64 = db
            .query_row("SELECT COUNT(*) FROM word_mark_rules", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn word_mark_exception_can_arrive_before_its_older_parent_rule() {
        let mut db = open_db();
        apply_all(&mut db, &[ev(1000, "dev-A", import_book("b1"))]);
        db.execute_batch("PRAGMA foreign_keys=ON;").unwrap();

        // Simulate separate sync ticks: the exception's peer is available
        // first, while the causally-earlier rule from another peer arrives
        // later. Keep FK checks on to prove migration 022 does not make the
        // protocol depend on the production connection's FK pragma. The
        // temporary orphan must survive and become effective.
        apply_all(
            &mut db,
            &[ev(
                3000,
                "dev-B",
                word_mark_exception("b1", "epubcfi(/6/4!)", true, 3000),
            )],
        );
        let orphan: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM word_mark_exceptions WHERE excluded = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(orphan, 1);
        db.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();

        apply_all(
            &mut db,
            &[ev(2000, "dev-A", word_mark("b1", true, "lookup", 2000))],
        );
        let effective: i64 = db
            .query_row(
                "SELECT COUNT(*)
                 FROM word_mark_exceptions e
                 JOIN word_mark_rules r ON r.id = e.rule_id
                 WHERE e.excluded = 1 AND r.enabled = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(effective, 1);
    }

    #[test]
    fn word_mark_rule_update_is_a_lww_reset_barrier_for_older_exceptions() {
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(2000, "dev-A", word_mark("b1", true, "lookup", 2000)),
                ev(
                    3000,
                    "dev-A",
                    word_mark_exception("b1", "epubcfi(/6/4!)", true, 3000),
                ),
                ev(4000, "dev-B", word_mark("b1", false, "lookup", 2000)),
            ],
        );
        let row: (i64, i64, String) = db
            .query_row(
                "SELECT excluded, updated_at, updated_by_device
                 FROM word_mark_exceptions",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(row, (0, 4000, "dev-B".into()));

        // A delayed older exception cannot resurrect the occurrence.
        apply_all(
            &mut db,
            &[ev(
                3500,
                "dev-C",
                word_mark_exception("b1", "epubcfi(/6/4!)", true, 3000),
            )],
        );
        let excluded: i64 = db
            .query_row("SELECT excluded FROM word_mark_exceptions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(excluded, 0);
    }

    #[test]
    fn same_timestamp_rule_and_exception_converge_by_device_tiebreaker() {
        let mut rule_then_exception = open_db();
        apply_all(
            &mut rule_then_exception,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(2000, "dev-A", word_mark("b1", true, "lookup", 2000)),
                ev(
                    3000,
                    "dev-A",
                    word_mark_exception("b1", "epubcfi(/6/4!)", true, 3000),
                ),
                ev(3000, "dev-B", word_mark("b1", true, "lookup", 2000)),
            ],
        );

        let mut exception_then_rule = open_db();
        apply_all(
            &mut exception_then_rule,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(2000, "dev-A", word_mark("b1", true, "lookup", 2000)),
                ev(3000, "dev-B", word_mark("b1", true, "lookup", 2000)),
                ev(
                    3000,
                    "dev-A",
                    word_mark_exception("b1", "epubcfi(/6/4!)", true, 3000),
                ),
            ],
        );

        let read = |db: &Connection| -> (i64, i64, String) {
            db.query_row(
                "SELECT excluded, updated_at, updated_by_device
                 FROM word_mark_exceptions",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap()
        };
        assert_eq!(read(&rule_then_exception), (0, 3000, "dev-B".into()));
        assert_eq!(read(&exception_then_rule), read(&rule_then_exception));
    }

    #[test]
    fn legacy_word_mark_delete_blocks_an_older_late_upsert_but_not_a_newer_one() {
        let mut db = open_db();
        let rule_id = word_mark_rule_id("b1", "term", "exact");
        apply_all(
            &mut db,
            &[
                ev(
                    3000,
                    "dev-B",
                    EventBody::WordMarkDelete {
                        id: rule_id.clone(),
                    },
                ),
                ev(1000, "dev-A", import_book("b1")),
                ev(2000, "dev-A", word_mark("b1", true, "lookup", 2000)),
            ],
        );
        let suppressed: i64 = db
            .query_row("SELECT COUNT(*) FROM word_mark_rules", [], |row| row.get(0))
            .unwrap();
        assert_eq!(suppressed, 0);

        apply_all(
            &mut db,
            &[ev(4000, "dev-C", word_mark("b1", true, "lookup", 4000))],
        );
        let restored: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM word_mark_rules WHERE enabled = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(restored, 1);
    }

    #[test]
    fn repeated_tombstones_keep_the_newest_timestamp_independent_of_order() {
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(3000, "dev-B", EventBody::NoteDelete { id: "n1".into() }),
                ev(1000, "dev-A", EventBody::NoteDelete { id: "n1".into() }),
            ],
        );
        let timestamp: i64 = db
            .query_row(
                "SELECT ts FROM _tombstones WHERE entity = 'note' AND id = 'n1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(timestamp, 3000);
    }

    #[test]
    fn tombstone_blocks_resurrection() {
        // delete then add (later ts, same id) → row stays gone.
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(2000, "dev-A", EventBody::BookDelete { id: "b1".into() }),
                ev(3000, "dev-A", import_book("b1")),
            ],
        );
        let n: i64 = db
            .query_row("SELECT COUNT(*) FROM books WHERE id = 'b1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 0, "tombstone should block re-import even at higher ts");
    }

    #[test]
    fn book_delete_cascades_to_children() {
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(1001, "dev-A", add_highlight("h1", "b1", "yellow")),
                ev(
                    1002,
                    "dev-A",
                    EventBody::BookmarkAdd(BookmarkPayload {
                        id: "bm1".into(),
                        book_id: "b1".into(),
                        cfi: "epubcfi(/6/4!)".into(),
                        label: None,
                    }),
                ),
                ev(2000, "dev-A", EventBody::BookDelete { id: "b1".into() }),
            ],
        );
        for table in ["books", "highlights", "bookmarks"] {
            let n: i64 = db
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{table} should be empty after book delete");
        }
    }

    // -----------------------------------------------------------------------
    // LWW correctness
    // -----------------------------------------------------------------------

    #[test]
    fn book_progress_lww_higher_ts_wins_regardless_of_order() {
        // Apply lower-ts last; LWW guard rejects it, so progress stays at the
        // higher-ts value.
        let mut db1 = open_db();
        apply_all(
            &mut db1,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(
                    1500,
                    "dev-A",
                    EventBody::BookProgressSet {
                        book: "b1".into(),
                        progress: 50,
                        cfi: Some("c50".into()),
                    },
                ),
                ev(
                    2000,
                    "dev-A",
                    EventBody::BookProgressSet {
                        book: "b1".into(),
                        progress: 80,
                        cfi: Some("c80".into()),
                    },
                ),
            ],
        );
        let mut db2 = open_db();
        apply_all(
            &mut db2,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(
                    2000,
                    "dev-A",
                    EventBody::BookProgressSet {
                        book: "b1".into(),
                        progress: 80,
                        cfi: Some("c80".into()),
                    },
                ),
                ev(
                    1500,
                    "dev-A",
                    EventBody::BookProgressSet {
                        book: "b1".into(),
                        progress: 50,
                        cfi: Some("c50".into()),
                    },
                ),
            ],
        );

        for db in [&db1, &db2] {
            let (p, cfi): (i32, String) = db
                .query_row(
                    "SELECT progress, current_cfi FROM books WHERE id = 'b1'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(p, 80);
            assert_eq!(cfi, "c80");
        }
    }

    #[test]
    fn same_ms_lww_breaks_tie_by_device_uuid() {
        // Two devices write the same field at the same ms. The lexicographically
        // larger device id wins the tuple compare.
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(
                    2000,
                    "dev-A",
                    EventBody::BookStatusSet {
                        book: "b1".into(),
                        status: "reading".into(),
                    },
                ),
                ev(
                    2000,
                    "dev-B",
                    EventBody::BookStatusSet {
                        book: "b1".into(),
                        status: "finished".into(),
                    },
                ),
            ],
        );
        let (status, dev): (String, String) = db
            .query_row(
                "SELECT status, updated_by_device FROM books WHERE id = 'b1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "finished", "dev-B > dev-A in tuple compare");
        assert_eq!(dev, "dev-B");

        // Reverse order — same outcome.
        let mut db2 = open_db();
        apply_all(
            &mut db2,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(
                    2000,
                    "dev-B",
                    EventBody::BookStatusSet {
                        book: "b1".into(),
                        status: "finished".into(),
                    },
                ),
                ev(
                    2000,
                    "dev-A",
                    EventBody::BookStatusSet {
                        book: "b1".into(),
                        status: "reading".into(),
                    },
                ),
            ],
        );
        let status2: String = db2
            .query_row("SELECT status FROM books WHERE id = 'b1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status2, "finished");
    }

    #[test]
    fn highlight_color_lww_skips_older_event() {
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(1100, "dev-A", add_highlight("h1", "b1", "yellow")),
                ev(
                    1300,
                    "dev-A",
                    EventBody::HighlightColorSet {
                        id: "h1".into(),
                        color: "pink".into(),
                    },
                ),
                ev(
                    1200,
                    "dev-A",
                    EventBody::HighlightColorSet {
                        id: "h1".into(),
                        color: "green".into(),
                    },
                ),
            ],
        );
        let color: String = db
            .query_row("SELECT color FROM highlights WHERE id = 'h1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(color, "pink", "older color event must lose");
    }

    #[test]
    fn vocab_mastery_carries_review_count_idempotently() {
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(
                    1100,
                    "dev-A",
                    EventBody::VocabAdd(VocabPayload {
                        id: "v1".into(),
                        book_id: "b1".into(),
                        word: "serendipity".into(),
                        definition: "fortunate".into(),
                        context_sentence: None,
                        context_explanation: None,
                        cfi: None,
                        mastery: "new".into(),
                        review_count: 0,
                        next_review_at: None,
                        review_interval_days: 0,
                        last_reviewed_at: None,
                        last_review_rating: None,
                        fsrs_stability: None,
                        fsrs_difficulty: None,
                        fsrs_version: 1,
                        created_at: None,
                    }),
                ),
                ev(
                    1200,
                    "dev-A",
                    EventBody::VocabMasterySet {
                        id: "v1".into(),
                        mastery: "learning".into(),
                        next_review_at: Some(2_000_000),
                        review_count: 1,
                        review_interval_days: 1,
                        last_reviewed_at: Some(1200),
                        last_review_rating: Some("hard".into()),
                        fsrs_stability: None,
                        fsrs_difficulty: None,
                        fsrs_version: 1,
                    },
                ),
                ev(
                    1300,
                    "dev-A",
                    EventBody::VocabMasterySet {
                        id: "v1".into(),
                        mastery: "learning".into(),
                        next_review_at: Some(3_000_000),
                        review_count: 2,
                        review_interval_days: 2,
                        last_reviewed_at: Some(1300),
                        last_review_rating: Some("good".into()),
                        fsrs_stability: None,
                        fsrs_difficulty: None,
                        fsrs_version: 1,
                    },
                ),
            ],
        );
        let (m, n): (String, i64) = db
            .query_row(
                "SELECT mastery, review_count FROM vocab_words WHERE id = 'v1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(m, "learning");
        assert_eq!(n, 2, "absolute review_count from later event wins");
    }

    // -----------------------------------------------------------------------
    // Determinism (shuffle property)
    // -----------------------------------------------------------------------

    #[test]
    fn shuffled_apply_yields_identical_state() {
        // Build a fixed event set, apply in two different orders, compare
        // every column on every row. Apply order must not matter once events
        // are sorted by (ts, device).
        let events: Vec<Event> = vec![
            ev(1000, "dev-A", import_book("b1")),
            ev(1000, "dev-B", import_book("b2")),
            ev(1100, "dev-A", add_highlight("h1", "b1", "yellow")),
            ev(1200, "dev-B", add_highlight("h2", "b2", "blue")),
            ev(
                1300,
                "dev-A",
                EventBody::HighlightColorSet {
                    id: "h1".into(),
                    color: "pink".into(),
                },
            ),
            ev(
                1400,
                "dev-A",
                EventBody::BookProgressSet {
                    book: "b1".into(),
                    progress: 25,
                    cfi: Some("c25".into()),
                },
            ),
            ev(
                1500,
                "dev-B",
                EventBody::BookProgressSet {
                    book: "b1".into(),
                    progress: 50,
                    cfi: Some("c50".into()),
                },
            ),
            ev(
                1600,
                "dev-A",
                EventBody::CollectionCreate {
                    id: "c1".into(),
                    name: "Top".into(),
                    sort_order: 0,
                },
            ),
            ev(
                1700,
                "dev-A",
                EventBody::CollectionBookAdd {
                    collection: "c1".into(),
                    book: "b1".into(),
                },
            ),
            ev(
                1800,
                "dev-A",
                EventBody::CollectionRename {
                    id: "c1".into(),
                    name: "Favorites".into(),
                },
            ),
            ev(
                1900,
                "dev-B",
                EventBody::HighlightDelete { id: "h2".into() },
            ),
        ];

        let mut sorted = events.clone();
        sorted.sort_by(|a, b| (a.ts, &a.device).cmp(&(b.ts, &b.device)));

        let mut reverse = sorted.clone();
        reverse.reverse();
        // After reversing we still need (ts, device) order before apply (the
        // determinism rule); the property under test is "any pre-sort
        // permutation produces the same state."
        reverse.sort_by(|a, b| (a.ts, &a.device).cmp(&(b.ts, &b.device)));

        let mut db1 = open_db();
        apply_all(&mut db1, &sorted);
        let mut db2 = open_db();
        apply_all(&mut db2, &reverse);

        let dump = |db: &Connection| -> Vec<(String, String)> {
            let tables = [
                "books",
                "highlights",
                "bookmarks",
                "vocab_words",
                "collections",
                "collection_books",
                "chats",
                "chat_messages",
                "_tombstones",
            ];
            let mut out = Vec::new();
            for t in tables {
                let mut stmt = db
                    .prepare(&format!("SELECT * FROM {t} ORDER BY 1, 2"))
                    .unwrap();
                let cols = stmt.column_count();
                let rows = stmt
                    .query_map([], |r| {
                        let mut s = String::new();
                        for i in 0..cols {
                            let v: rusqlite::types::Value = r.get(i)?;
                            s.push_str(&format!("{v:?}|"));
                        }
                        Ok(s)
                    })
                    .unwrap();
                for row in rows {
                    out.push((t.to_string(), row.unwrap()));
                }
            }
            out
        };
        assert_eq!(dump(&db1), dump(&db2), "shuffle changed final state");
    }

    // -----------------------------------------------------------------------
    // book.metadata.set
    // -----------------------------------------------------------------------

    #[test]
    fn book_metadata_set_updates_string_field_under_lww() {
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(
                    2000,
                    "dev-A",
                    EventBody::BookMetadataSet {
                        book: "b1".into(),
                        field: "author".into(),
                        value: json!("Leo Tolstoy"),
                    },
                ),
            ],
        );
        let author: String = db
            .query_row("SELECT author FROM books WHERE id = 'b1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(author, "Leo Tolstoy");
    }

    #[test]
    fn book_metadata_set_pages_accepts_number_and_null() {
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(
                    2000,
                    "dev-A",
                    EventBody::BookMetadataSet {
                        book: "b1".into(),
                        field: "pages".into(),
                        value: json!(1225),
                    },
                ),
            ],
        );
        let pages: Option<i64> = db
            .query_row("SELECT pages FROM books WHERE id = 'b1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(pages, Some(1225));

        apply_all(
            &mut db,
            &[ev(
                3000,
                "dev-A",
                EventBody::BookMetadataSet {
                    book: "b1".into(),
                    field: "pages".into(),
                    value: Value::Null,
                },
            )],
        );
        let pages: Option<i64> = db
            .query_row("SELECT pages FROM books WHERE id = 'b1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(pages, None);
    }

    #[test]
    fn book_metadata_unknown_field_is_skipped() {
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(
                    2000,
                    "dev-A",
                    EventBody::BookMetadataSet {
                        book: "b1".into(),
                        field: "future_field".into(),
                        value: json!("anything"),
                    },
                ),
            ],
        );
        // No panic, no crash; row's updated_at is unchanged from the import.
        let ts: i64 = db
            .query_row("SELECT updated_at FROM books WHERE id = 'b1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(ts, 1000);
    }

    // -----------------------------------------------------------------------
    // collection_books composite-key tombstone
    // -----------------------------------------------------------------------

    #[test]
    fn collection_book_remove_then_add_blocked_by_composite_tombstone() {
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(
                    1100,
                    "dev-A",
                    EventBody::CollectionCreate {
                        id: "c1".into(),
                        name: "Top".into(),
                        sort_order: 0,
                    },
                ),
                ev(
                    1200,
                    "dev-A",
                    EventBody::CollectionBookAdd {
                        collection: "c1".into(),
                        book: "b1".into(),
                    },
                ),
                ev(
                    1300,
                    "dev-A",
                    EventBody::CollectionBookRemove {
                        collection: "c1".into(),
                        book: "b1".into(),
                    },
                ),
                ev(
                    1400,
                    "dev-A",
                    EventBody::CollectionBookAdd {
                        collection: "c1".into(),
                        book: "b1".into(),
                    },
                ),
            ],
        );
        let n: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM collection_books WHERE collection_id = 'c1' AND book_id = 'b1'",
                [], |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "composite tombstone should suppress re-add");
    }

    // -----------------------------------------------------------------------
    // Idempotency under repeated apply
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // Regression tests for PR #189 review findings.
    // -----------------------------------------------------------------------

    #[test]
    fn chat_message_add_bumps_parent_chat_updated_at() {
        // Mirrors the live `add_chat_message` command's two-table write —
        // the chat's recency drives chat-list ordering on every device, so
        // peers must see the bump.
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(
                    1100,
                    "dev-A",
                    EventBody::ChatCreate {
                        id: "ch1".into(),
                        book: "b1".into(),
                        title: "New chat".into(),
                        model: None,
                    },
                ),
                ev(
                    5000,
                    "dev-A",
                    EventBody::ChatMessageAdd(ChatMessagePayload {
                        id: "m1".into(),
                        chat_id: "ch1".into(),
                        role: "user".into(),
                        content: "hi".into(),
                        context: None,
                        metadata: None,
                    }),
                ),
            ],
        );
        let (chat_ts, by): (i64, String) = db
            .query_row(
                "SELECT updated_at, updated_by_device FROM chats WHERE id = 'ch1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(chat_ts, 5000, "message ts should bump parent chat");
        assert_eq!(by, "dev-A");
    }

    #[test]
    fn chat_message_add_does_not_drag_chat_updated_at_backward() {
        // Rename happens at T=10_000 on dev-A; older message arrives at
        // T=5_000 from dev-B. Chat updated_at must stay at the rename ts.
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(
                    1100,
                    "dev-A",
                    EventBody::ChatCreate {
                        id: "ch1".into(),
                        book: "b1".into(),
                        title: "Old".into(),
                        model: None,
                    },
                ),
                ev(
                    10_000,
                    "dev-A",
                    EventBody::ChatRename {
                        id: "ch1".into(),
                        title: "New".into(),
                    },
                ),
                ev(
                    5_000,
                    "dev-B",
                    EventBody::ChatMessageAdd(ChatMessagePayload {
                        id: "m1".into(),
                        chat_id: "ch1".into(),
                        role: "user".into(),
                        content: "hi".into(),
                        context: None,
                        metadata: None,
                    }),
                ),
            ],
        );
        let chat_ts: i64 = db
            .query_row("SELECT updated_at FROM chats WHERE id = 'ch1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(chat_ts, 10_000, "older message must not drag chat backward");
    }

    #[test]
    fn chat_message_replace_is_lww_and_cannot_create_or_replace_user_messages() {
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(
                    1100,
                    "dev-A",
                    EventBody::ChatCreate {
                        id: "ch1".into(),
                        book: "b1".into(),
                        title: "Chat".into(),
                        model: None,
                    },
                ),
                ev(
                    1200,
                    "dev-A",
                    EventBody::ChatMessageAdd(ChatMessagePayload {
                        id: "assistant".into(),
                        chat_id: "ch1".into(),
                        role: "assistant".into(),
                        content: "old".into(),
                        context: None,
                        metadata: None,
                    }),
                ),
                ev(
                    1300,
                    "dev-A",
                    EventBody::ChatMessageAdd(ChatMessagePayload {
                        id: "user".into(),
                        chat_id: "ch1".into(),
                        role: "user".into(),
                        content: "question".into(),
                        context: None,
                        metadata: None,
                    }),
                ),
                ev(
                    1500,
                    "dev-B",
                    EventBody::ChatMessageReplace(ChatMessagePayload {
                        id: "assistant".into(),
                        chat_id: "ch1".into(),
                        role: "assistant".into(),
                        content: "new".into(),
                        context: None,
                        metadata: Some("{}".into()),
                    }),
                ),
                ev(
                    1400,
                    "dev-C",
                    EventBody::ChatMessageReplace(ChatMessagePayload {
                        id: "assistant".into(),
                        chat_id: "ch1".into(),
                        role: "assistant".into(),
                        content: "stale".into(),
                        context: None,
                        metadata: None,
                    }),
                ),
                ev(
                    1600,
                    "dev-B",
                    EventBody::ChatMessageReplace(ChatMessagePayload {
                        id: "missing".into(),
                        chat_id: "ch1".into(),
                        role: "assistant".into(),
                        content: "must not insert".into(),
                        context: None,
                        metadata: None,
                    }),
                ),
                ev(
                    1700,
                    "dev-B",
                    EventBody::ChatMessageReplace(ChatMessagePayload {
                        id: "user".into(),
                        chat_id: "ch1".into(),
                        role: "assistant".into(),
                        content: "must not replace".into(),
                        context: None,
                        metadata: None,
                    }),
                ),
            ],
        );
        let assistant: (String, Option<String>, i64, String) = db
            .query_row(
                "SELECT content, metadata, updated_at, updated_by_device
                 FROM chat_messages WHERE id = 'assistant'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            assistant,
            ("new".into(), Some("{}".into()), 1500, "dev-B".into())
        );
        let user: String = db
            .query_row(
                "SELECT content FROM chat_messages WHERE id = 'user'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(user, "question");
        let missing: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM chat_messages WHERE id = 'missing'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(missing, 0);
    }

    #[test]
    fn book_metadata_multi_field_same_tx_both_apply() {
        // The live `update_book_metadata` command can rewrite title and
        // author in one transaction, producing two `book.metadata.set`
        // events with identical (ts, device). With strict `<` LWW the
        // second event's field would silently fail to land. This test
        // pins the `<=` relaxation.
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(
                    2000,
                    "dev-A",
                    EventBody::BookMetadataSet {
                        book: "b1".into(),
                        field: "title".into(),
                        value: json!("New Title"),
                    },
                ),
                ev(
                    2000,
                    "dev-A",
                    EventBody::BookMetadataSet {
                        book: "b1".into(),
                        field: "author".into(),
                        value: json!("New Author"),
                    },
                ),
            ],
        );
        let (title, author): (String, String) = db
            .query_row("SELECT title, author FROM books WHERE id = 'b1'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(title, "New Title", "first metadata.set must land");
        assert_eq!(author, "New Author", "second metadata.set must also land");
    }

    // -----------------------------------------------------------------------
    // Late-child-add suppression after a parent delete.
    //
    // Scenario from PR #189 review: device-A creates the join row before
    // going offline, device-B deletes the parent and publishes, devices
    // converge, then device-A comes back and publishes its older event.
    // Without parent-tombstone checks the older event resurrects the join
    // and inflates `list_collections` counts. The same shape applies to
    // every child entity (highlights, bookmarks, vocab, chats, chat
    // messages).
    // -----------------------------------------------------------------------

    #[test]
    fn late_collection_book_add_after_book_delete_is_suppressed() {
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(
                    1100,
                    "dev-A",
                    EventBody::CollectionCreate {
                        id: "c1".into(),
                        name: "Top".into(),
                        sort_order: 0,
                    },
                ),
                // dev-B deletes the book at T=2000.
                ev(2000, "dev-B", EventBody::BookDelete { id: "b1".into() }),
                // dev-A's older `collection.book.add(c1, b1)` arrives late
                // (T=1500 < 2000). Sorted-apply order is delete-then-add,
                // but cross-tick this ordering breaks down — assert the add
                // is suppressed regardless.
                ev(
                    1500,
                    "dev-A",
                    EventBody::CollectionBookAdd {
                        collection: "c1".into(),
                        book: "b1".into(),
                    },
                ),
            ],
        );
        let n: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM collection_books WHERE collection_id = 'c1' AND book_id = 'b1'",
                [], |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            n, 0,
            "collection.book.add must not resurrect a tombstoned book's join row"
        );
    }

    #[test]
    fn late_collection_book_add_suppressed_across_ticks() {
        // The same scenario but across two apply batches — mirrors the
        // multi-tick replay path described in the review (dev-B's delete
        // event applied in tick 1; dev-A's stale add arrives in tick 2).
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(
                    1100,
                    "dev-A",
                    EventBody::CollectionCreate {
                        id: "c1".into(),
                        name: "Top".into(),
                        sort_order: 0,
                    },
                ),
                ev(2000, "dev-B", EventBody::BookDelete { id: "b1".into() }),
            ],
        );
        // Now a second tick brings the late add.
        apply_all(
            &mut db,
            &[ev(
                1500,
                "dev-A",
                EventBody::CollectionBookAdd {
                    collection: "c1".into(),
                    book: "b1".into(),
                },
            )],
        );
        let n: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM collection_books WHERE collection_id = 'c1' AND book_id = 'b1'",
                [], |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "late tick must still see the parent tombstone");
    }

    #[test]
    fn late_highlight_add_after_book_delete_is_suppressed() {
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(2000, "dev-B", EventBody::BookDelete { id: "b1".into() }),
            ],
        );
        apply_all(
            &mut db,
            &[ev(1500, "dev-A", add_highlight("h-late", "b1", "yellow"))],
        );
        let n: i64 = db
            .query_row("SELECT COUNT(*) FROM highlights", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "highlight on a tombstoned book must be suppressed");
    }

    #[test]
    fn late_chat_message_after_book_delete_is_suppressed() {
        // Cascade-deleting a book also tombstones each cascaded chat, so a
        // delayed `chat.message.add` for one of those chats stays out.
        let mut db = open_db();
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(
                    1100,
                    "dev-A",
                    EventBody::ChatCreate {
                        id: "ch1".into(),
                        book: "b1".into(),
                        title: "T".into(),
                        model: None,
                    },
                ),
                ev(2000, "dev-B", EventBody::BookDelete { id: "b1".into() }),
            ],
        );
        apply_all(
            &mut db,
            &[ev(
                1500,
                "dev-A",
                EventBody::ChatMessageAdd(ChatMessagePayload {
                    id: "m1".into(),
                    chat_id: "ch1".into(),
                    role: "user".into(),
                    content: "hi".into(),
                    context: None,
                    metadata: None,
                }),
            )],
        );
        let n: i64 = db
            .query_row("SELECT COUNT(*) FROM chat_messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "message for cascade-deleted chat must be suppressed");
    }

    #[test]
    fn suppressed_chat_create_writes_tombstone_blocking_late_message() {
        // Exact scenario from the second review pass:
        //   tick 1: book.delete(b1) applied — book is tombstoned, no
        //     chat existed locally so cascade_delete_book wrote nothing.
        //   tick 2: stale chat.create(ch1, b1) arrives. Parent book is
        //     tombstoned → suppressed. Without this fix, no chat tombstone
        //     gets written.
        //   tick 3: stale chat.message.add(m1, chat_id=ch1) arrives. The
        //     message arm checks (chat, ch1) tombstone, sees nothing, and
        //     would insert an orphan.
        let mut db = open_db();
        // Tick 1: book imported, then deleted.
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(2000, "dev-B", EventBody::BookDelete { id: "b1".into() }),
            ],
        );
        // Tick 2: late chat.create arrives.
        apply_all(
            &mut db,
            &[ev(
                1500,
                "dev-A",
                EventBody::ChatCreate {
                    id: "ch1".into(),
                    book: "b1".into(),
                    title: "T".into(),
                    model: None,
                },
            )],
        );
        let n_chats: i64 = db
            .query_row("SELECT COUNT(*) FROM chats WHERE id = 'ch1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            n_chats, 0,
            "chat.create must be suppressed by book tombstone"
        );

        let chat_tomb: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM _tombstones WHERE entity = 'chat' AND id = 'ch1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            chat_tomb, 1,
            "suppressed chat.create must leave a chat tombstone"
        );

        // Tick 3: late chat.message.add arrives. The chat tombstone from
        // tick 2 must block the orphan insert.
        apply_all(
            &mut db,
            &[ev(
                1600,
                "dev-A",
                EventBody::ChatMessageAdd(ChatMessagePayload {
                    id: "m1".into(),
                    chat_id: "ch1".into(),
                    role: "user".into(),
                    content: "hi".into(),
                    context: None,
                    metadata: None,
                }),
            )],
        );
        let n_msgs: i64 = db
            .query_row("SELECT COUNT(*) FROM chat_messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            n_msgs, 0,
            "message for suppressed chat must not slip in as an orphan"
        );
    }

    #[test]
    fn cascaded_chat_tombstones_carry_event_ts_not_wall_clock() {
        // Regression for the determinism finding. cascade_delete_book
        // previously stamped per-chat tombstones with `Utc::now()`, which
        // diverges across replay runs and corrupts snapshot equivalence.
        // Pin: the cascaded chat's `_tombstones.ts` must equal the
        // book.delete event's ts.
        let mut db = open_db();
        const DELETE_TS: i64 = 5_000;
        apply_all(
            &mut db,
            &[
                ev(1000, "dev-A", import_book("b1")),
                ev(
                    1100,
                    "dev-A",
                    EventBody::ChatCreate {
                        id: "ch1".into(),
                        book: "b1".into(),
                        title: "T".into(),
                        model: None,
                    },
                ),
                ev(
                    1200,
                    "dev-A",
                    EventBody::ChatCreate {
                        id: "ch2".into(),
                        book: "b1".into(),
                        title: "T2".into(),
                        model: None,
                    },
                ),
                ev(
                    DELETE_TS,
                    "dev-B",
                    EventBody::BookDelete { id: "b1".into() },
                ),
            ],
        );

        let rows: Vec<(String, i64)> = {
            let mut stmt = db
                .prepare("SELECT id, ts FROM _tombstones WHERE entity = 'chat' ORDER BY id")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        assert_eq!(
            rows,
            vec![
                ("ch1".to_string(), DELETE_TS),
                ("ch2".to_string(), DELETE_TS)
            ],
            "cascaded chat tombstones must use the book.delete event ts"
        );
    }

    #[test]
    fn double_apply_is_a_noop() {
        let events = vec![
            ev(1000, "dev-A", import_book("b1")),
            ev(1100, "dev-A", add_highlight("h1", "b1", "yellow")),
            ev(
                1200,
                "dev-A",
                EventBody::HighlightColorSet {
                    id: "h1".into(),
                    color: "pink".into(),
                },
            ),
        ];
        let mut db = open_db();
        apply_all(&mut db, &events);
        // Snapshot the row state, then re-apply.
        let before: (String, i64, String) = db
            .query_row(
                "SELECT color, updated_at, updated_by_device FROM highlights WHERE id = 'h1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        apply_all(&mut db, &events);
        let after: (String, i64, String) = db
            .query_row(
                "SELECT color, updated_at, updated_by_device FROM highlights WHERE id = 'h1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(before, after);
    }
}
