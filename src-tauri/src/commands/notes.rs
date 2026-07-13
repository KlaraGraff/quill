use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::sync::events::{normalize_learning_term, EventBody, NotePayload};
use crate::sync::validation::{validate_entity_id, validate_note_fields};
use crate::sync::writer::SyncWriter;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Note {
    pub id: String,
    pub book_id: Option<String>,
    pub book_title: Option<String>,
    pub anchor_kind: String,
    pub normalized_word: Option<String>,
    pub scope: String,
    pub location: Option<String>,
    pub selected_text: Option<String>,
    pub content: String,
    pub content_format: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize)]
pub struct NotePage {
    pub notes: Vec<Note>,
    pub next_cursor: Option<String>,
    pub total: usize,
}

fn row_to_note(row: &rusqlite::Row<'_>) -> rusqlite::Result<Note> {
    Ok(Note {
        id: row.get(0)?,
        book_id: row.get(1)?,
        book_title: row.get(2)?,
        anchor_kind: row.get(3)?,
        normalized_word: row.get(4)?,
        scope: row.get(5)?,
        location: row.get(6)?,
        selected_text: row.get(7)?,
        content: row.get(8)?,
        content_format: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

const NOTE_COLUMNS: &str = "n.id, n.book_id, b.title, n.anchor_kind, n.normalized_word, n.scope, n.location, n.selected_text, n.content, n.content_format, n.created_at, n.updated_at";

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn save_note(
    id: Option<String>,
    book_id: Option<String>,
    anchor_kind: String,
    word: Option<String>,
    scope: String,
    location: Option<String>,
    selected_text: Option<String>,
    content: String,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<Note> {
    let normalized_word = word
        .as_deref()
        .map(normalize_learning_term)
        .filter(|value| !value.is_empty());

    let id = id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let content_format = "plain_text".to_string();
    validate_note_fields(
        &id,
        book_id.as_deref(),
        &anchor_kind,
        normalized_word.as_deref(),
        &scope,
        location.as_deref(),
        selected_text.as_deref(),
        &content,
        &content_format,
    )?;
    let timestamp = chrono::Utc::now().timestamp_millis();
    let device = sync.self_device().to_string();
    sync.with_tx(&db, timestamp, |tx, events| {
        let effective_book_id = match book_id.as_deref() {
            Some(candidate) => {
                let exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM books WHERE id = ?1)",
                    params![candidate],
                    |row| row.get(0),
                )?;
                if !exists && scope == "book" {
                    return Err(AppError::Other("NOTE_BOOK_NOT_FOUND".to_string()));
                }
                exists.then_some(candidate)
            }
            None => None,
        };
        let created_at = tx
            .query_row(
                "SELECT created_at FROM notes WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(timestamp);
        tx.execute(
            "INSERT INTO notes (id, book_id, anchor_kind, normalized_word, scope, location, selected_text, content, content_format, created_at, updated_at, updated_by_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(id) DO UPDATE SET book_id = excluded.book_id, anchor_kind = excluded.anchor_kind,
               normalized_word = excluded.normalized_word, scope = excluded.scope, location = excluded.location,
               selected_text = excluded.selected_text, content = excluded.content,
               content_format = excluded.content_format, updated_at = excluded.updated_at,
               updated_by_device = excluded.updated_by_device",
            params![id, effective_book_id, anchor_kind, normalized_word, scope, location, selected_text, content, content_format, created_at, timestamp, device],
        )?;
        events.push(EventBody::NoteUpsert(NotePayload {
            id: id.clone(),
            book_id: effective_book_id.map(str::to_string),
            anchor_kind: anchor_kind.clone(),
            normalized_word: normalized_word.clone(),
            scope: scope.clone(),
            location: location.clone(),
            selected_text: selected_text.clone(),
            content: content.clone(),
            content_format: content_format.clone(),
            created_at,
        }));
        Ok(())
    })?;

    let conn = db.reader();
    conn.query_row(
        &format!("SELECT {NOTE_COLUMNS} FROM notes n LEFT JOIN books b ON b.id = n.book_id WHERE n.id = ?1"),
        params![id],
        row_to_note,
    )
    .map_err(Into::into)
}

#[tauri::command]
pub fn delete_note(id: String, db: State<'_, Db>, sync: State<'_, SyncWriter>) -> AppResult<()> {
    validate_entity_id(&id)?;
    let timestamp = chrono::Utc::now().timestamp_millis();
    sync.with_tx(&db, timestamp, |tx, events| {
        tx.execute("DELETE FROM notes WHERE id = ?1", params![id])?;
        events.push(EventBody::NoteDelete { id: id.clone() });
        Ok(())
    })
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn list_notes(
    book_id: Option<String>,
    anchor_kind: Option<String>,
    search: Option<String>,
    updated_after: Option<i64>,
    updated_before: Option<i64>,
    cursor: Option<String>,
    limit: Option<usize>,
    db: State<'_, Db>,
) -> AppResult<NotePage> {
    let search = search.filter(|value| !value.trim().is_empty());
    let pattern = search
        .as_ref()
        .map(|value| format!("%{}%", value.trim().to_lowercase()));
    let conn = db.reader();
    let total: usize = conn.query_row(
        "SELECT COUNT(*) FROM notes n LEFT JOIN books b ON b.id = n.book_id
         WHERE (?1 IS NULL OR n.book_id = ?1)
           AND (?2 IS NULL OR n.anchor_kind = ?2)
           AND (?3 IS NULL OR LOWER(n.content) LIKE ?3 OR LOWER(COALESCE(n.selected_text, '')) LIKE ?3 OR LOWER(COALESCE(n.normalized_word, '')) LIKE ?3 OR LOWER(COALESCE(b.title, '')) LIKE ?3)
           AND (?4 IS NULL OR n.updated_at >= ?4)
           AND (?5 IS NULL OR n.updated_at <= ?5)",
        params![book_id, anchor_kind, pattern, updated_after, updated_before],
        |row| row.get(0),
    )?;
    let page_limit = limit.unwrap_or(100).clamp(1, 500);
    let fetch_limit = page_limit + 1;
    let mut statement = conn.prepare(&format!(
        "SELECT {NOTE_COLUMNS} FROM notes n LEFT JOIN books b ON b.id = n.book_id
         WHERE (?1 IS NULL OR n.book_id = ?1)
           AND (?2 IS NULL OR n.anchor_kind = ?2)
           AND (?3 IS NULL OR LOWER(n.content) LIKE ?3 OR LOWER(COALESCE(n.selected_text, '')) LIKE ?3 OR LOWER(COALESCE(n.normalized_word, '')) LIKE ?3 OR LOWER(COALESCE(b.title, '')) LIKE ?3)
           AND (?4 IS NULL OR n.updated_at >= ?4)
           AND (?5 IS NULL OR n.updated_at <= ?5)
           AND (?6 IS NULL OR printf('%020lld:%s', n.updated_at, n.id) < ?6)
         ORDER BY n.updated_at DESC, n.id DESC LIMIT ?7"
    ))?;
    let mut notes = statement
        .query_map(
            params![
                book_id,
                anchor_kind,
                pattern,
                updated_after,
                updated_before,
                cursor,
                fetch_limit
            ],
            row_to_note,
        )?
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::from)?;
    let has_more = notes.len() > page_limit;
    notes.truncate(page_limit);
    let next_cursor = has_more.then(|| {
        let last = notes.last().expect("non-empty page with continuation");
        format!("{:020}:{}", last.updated_at, last.id)
    });
    Ok(NotePage {
        notes,
        next_cursor,
        total,
    })
}

#[tauri::command]
pub fn list_context_notes(
    book_id: String,
    word: Option<String>,
    location: Option<String>,
    db: State<'_, Db>,
) -> AppResult<Vec<Note>> {
    let normalized_word = word
        .as_deref()
        .map(normalize_learning_term)
        .filter(|value| !value.is_empty());
    let conn = db.reader();
    let mut statement = conn.prepare(&format!(
        "SELECT {NOTE_COLUMNS} FROM notes n LEFT JOIN books b ON b.id = n.book_id
         WHERE ((?2 IS NOT NULL AND n.anchor_kind = 'word' AND n.normalized_word = ?2 AND (n.scope = 'global' OR n.book_id = ?1))
            OR (?3 IS NOT NULL AND n.anchor_kind = 'selection' AND n.book_id = ?1 AND n.location = ?3))
         ORDER BY n.updated_at DESC, n.id ASC"
    ))?;
    let notes = statement
        .query_map(params![book_id, normalized_word, location], row_to_note)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::from)?;
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn normalizes_lookup_words_without_substring_matching() {
        assert_eq!(normalize_learning_term("  Interfaces, "), "interfaces");
        assert_eq!(normalize_learning_term("don't"), "don't");
    }

    #[test]
    fn migration_preserves_legacy_highlight_note_in_both_tables() {
        let conn = Connection::open_in_memory().unwrap();
        Db::run_migrations_up_to(&conn, 20).unwrap();
        conn.execute(
            "INSERT INTO books
             (id, title, author, file_path, format, status, progress, created_at, updated_at)
             VALUES ('b1', 'Book', 'Author', 'books/b1.epub', 'epub', 'reading', 0, 1000, 1000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO highlights
             (id, book_id, cfi_range, color, note, text_content, created_at, updated_at)
             VALUES ('h1', 'b1', 'epubcfi(/6/4!)', 'yellow', 'legacy note',
                     'quoted text', 1100, 1200)",
            [],
        )
        .unwrap();

        Db::run_migrations_up_to(&conn, 21).unwrap();

        let migrated: (String, String, String, String, i64, i64) = conn
            .query_row(
                "SELECT book_id, anchor_kind, location, content, created_at, updated_at
                 FROM notes WHERE id = 'legacy-highlight-note-h1'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            migrated,
            (
                "b1".into(),
                "selection".into(),
                "epubcfi(/6/4!)".into(),
                "legacy note".into(),
                1100,
                1200
            )
        );
        let original: String = conn
            .query_row("SELECT note FROM highlights WHERE id = 'h1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(original, "legacy note");
    }
}
