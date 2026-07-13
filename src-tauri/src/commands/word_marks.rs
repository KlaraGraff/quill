use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::sync::events::{normalize_learning_term, word_mark_rule_id, EventBody, WordMarkPayload};
use crate::sync::validation::{validate_entity_id, validate_word_mark_fields};
use crate::sync::writer::SyncWriter;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordMarkRule {
    pub id: String,
    pub book_id: String,
    pub normalized_word: String,
    pub display_word: String,
    pub match_mode: String,
    pub color: String,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

fn row_to_rule(row: &rusqlite::Row<'_>) -> rusqlite::Result<WordMarkRule> {
    Ok(WordMarkRule {
        id: row.get(0)?,
        book_id: row.get(1)?,
        normalized_word: row.get(2)?,
        display_word: row.get(3)?,
        match_mode: row.get(4)?,
        color: row.get(5)?,
        enabled: row.get::<_, i64>(6)? != 0,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

const RULE_COLUMNS: &str = "id, book_id, normalized_word, display_word, match_mode, color, enabled, created_at, updated_at";

fn require_book(tx: &rusqlite::Transaction<'_>, book_id: &str) -> AppResult<()> {
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM books WHERE id = ?1)",
        params![book_id],
        |row| row.get(0),
    )?;
    if !exists {
        return Err(AppError::Other("WORD_MARK_BOOK_NOT_FOUND".to_string()));
    }
    Ok(())
}

fn prepare_rule(
    book_id: &str,
    word: &str,
    color: Option<&str>,
) -> AppResult<(String, String, String, String, String)> {
    validate_entity_id(book_id)?;
    let normalized_word = normalize_learning_term(word);
    let display_word = word.trim().to_string();
    let match_mode = "exact".to_string();
    let color = color.unwrap_or("lookup").to_string();
    let id = word_mark_rule_id(book_id, &normalized_word, &match_mode);
    validate_word_mark_fields(
        &id,
        book_id,
        &normalized_word,
        &display_word,
        &match_mode,
        &color,
    )?;
    Ok((id, normalized_word, display_word, match_mode, color))
}

fn ensure_word_mark_rule_inner(
    book_id: &str,
    word: &str,
    color: Option<&str>,
    db: &Db,
    sync: &SyncWriter,
) -> AppResult<WordMarkRule> {
    let (id, normalized_word, display_word, match_mode, color) =
        prepare_rule(book_id, word, color)?;
    let timestamp = chrono::Utc::now().timestamp_millis();
    let device = sync.self_device().to_string();

    sync.with_tx(db, timestamp, |tx, events| {
        require_book(tx, book_id)?;
        let existing = tx
            .query_row(
                &format!(
                    "SELECT {RULE_COLUMNS} FROM word_mark_rules
                     WHERE book_id = ?1 AND normalized_word = ?2 AND match_mode = ?3"
                ),
                params![book_id, normalized_word, match_mode],
                row_to_rule,
            )
            .optional()?;

        if let Some(existing) = existing {
            // Migration builds briefly used random UUIDs. Canonicalize that id
            // without changing the user's enabled choice; normal ensure calls
            // on an already-canonical row remain true no-ops.
            if existing.id == id {
                return Ok(existing);
            }
            tx.execute(
                "UPDATE word_mark_rules SET id = ?1, updated_at = ?2, updated_by_device = ?3
                 WHERE book_id = ?4 AND normalized_word = ?5 AND match_mode = ?6",
                params![id, timestamp, device, book_id, normalized_word, match_mode],
            )?;
            let canonical = WordMarkRule {
                id: id.clone(),
                updated_at: timestamp,
                ..existing
            };
            events.push(EventBody::WordMarkUpsert(WordMarkPayload {
                id: canonical.id.clone(),
                book_id: canonical.book_id.clone(),
                normalized_word: canonical.normalized_word.clone(),
                display_word: canonical.display_word.clone(),
                match_mode: canonical.match_mode.clone(),
                color: canonical.color.clone(),
                enabled: canonical.enabled,
                created_at: canonical.created_at,
            }));
            return Ok(canonical);
        }

        tx.execute(
            "INSERT INTO word_mark_rules
             (id, book_id, normalized_word, display_word, match_mode, color, enabled,
              created_at, updated_at, updated_by_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?7, ?8)",
            params![
                id,
                book_id,
                normalized_word,
                display_word,
                match_mode,
                color,
                timestamp,
                device
            ],
        )?;
        events.push(EventBody::WordMarkUpsert(WordMarkPayload {
            id: id.clone(),
            book_id: book_id.to_string(),
            normalized_word: normalized_word.clone(),
            display_word: display_word.clone(),
            match_mode: match_mode.clone(),
            color: color.clone(),
            enabled: true,
            created_at: timestamp,
        }));
        Ok(WordMarkRule {
            id,
            book_id: book_id.to_string(),
            normalized_word,
            display_word,
            match_mode,
            color,
            enabled: true,
            created_at: timestamp,
            updated_at: timestamp,
        })
    })
}

fn set_word_mark_rule_enabled_inner(
    book_id: &str,
    word: &str,
    enabled: bool,
    color: Option<&str>,
    db: &Db,
    sync: &SyncWriter,
) -> AppResult<WordMarkRule> {
    let (id, normalized_word, display_word, match_mode, requested_color) =
        prepare_rule(book_id, word, color)?;
    let timestamp = chrono::Utc::now().timestamp_millis();
    let device = sync.self_device().to_string();

    sync.with_tx(db, timestamp, |tx, events| {
        require_book(tx, book_id)?;
        let existing = tx
            .query_row(
                &format!(
                    "SELECT {RULE_COLUMNS} FROM word_mark_rules
                     WHERE book_id = ?1 AND normalized_word = ?2 AND match_mode = ?3"
                ),
                params![book_id, normalized_word, match_mode],
                row_to_rule,
            )
            .optional()?;
        let created_at = existing
            .as_ref()
            .map(|rule| rule.created_at)
            .unwrap_or(timestamp);
        let color = color
            .map(str::to_string)
            .or_else(|| existing.as_ref().map(|rule| rule.color.clone()))
            .unwrap_or(requested_color);

        tx.execute(
            "INSERT INTO word_mark_rules
             (id, book_id, normalized_word, display_word, match_mode, color, enabled,
              created_at, updated_at, updated_by_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(book_id, normalized_word, match_mode) DO UPDATE SET
               id = excluded.id, display_word = excluded.display_word,
               color = excluded.color, enabled = excluded.enabled,
               updated_at = excluded.updated_at,
               updated_by_device = excluded.updated_by_device",
            params![
                id,
                book_id,
                normalized_word,
                display_word,
                match_mode,
                color,
                enabled as i64,
                created_at,
                timestamp,
                device
            ],
        )?;
        events.push(EventBody::WordMarkUpsert(WordMarkPayload {
            id: id.clone(),
            book_id: book_id.to_string(),
            normalized_word: normalized_word.clone(),
            display_word: display_word.clone(),
            match_mode: match_mode.clone(),
            color: color.clone(),
            enabled,
            created_at,
        }));
        Ok(WordMarkRule {
            id,
            book_id: book_id.to_string(),
            normalized_word,
            display_word,
            match_mode,
            color,
            enabled,
            created_at,
            updated_at: timestamp,
        })
    })
}

/// Insert the rule only when it has never been seen. In particular, a lookup
/// must not turn a rule back on after the user explicitly disabled it.
#[tauri::command]
pub fn ensure_word_mark_rule(
    book_id: String,
    word: String,
    color: Option<String>,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<WordMarkRule> {
    ensure_word_mark_rule_inner(&book_id, &word, color.as_deref(), &db, &sync)
}

#[tauri::command]
pub fn set_word_mark_rule_enabled(
    book_id: String,
    word: String,
    enabled: bool,
    color: Option<String>,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<WordMarkRule> {
    set_word_mark_rule_enabled_inner(&book_id, &word, enabled, color.as_deref(), &db, &sync)
}

/// Compatibility alias for callers that used the first implementation.
#[tauri::command]
pub fn upsert_word_mark(
    book_id: String,
    word: String,
    color: Option<String>,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<WordMarkRule> {
    set_word_mark_rule_enabled_inner(&book_id, &word, true, color.as_deref(), &db, &sync)
}

/// Compatibility wrapper. Cancellation now persists an enabled=false rule so
/// subsequent lookups do not recreate it.
#[tauri::command]
pub fn remove_word_mark(
    book_id: String,
    word: String,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<()> {
    set_word_mark_rule_enabled_inner(&book_id, &word, false, None, &db, &sync)?;
    Ok(())
}

#[tauri::command]
pub fn list_word_marks(book_id: String, db: State<'_, Db>) -> AppResult<Vec<WordMarkRule>> {
    let conn = db.reader();
    let mut statement = conn.prepare(
        "SELECT id, book_id, normalized_word, display_word, match_mode, color, enabled, created_at, updated_at
         FROM word_mark_rules WHERE book_id = ?1 AND enabled = 1 ORDER BY created_at ASC",
    )?;
    let rules = statement
        .query_map(params![book_id], row_to_rule)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::from)?;
    Ok(rules)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_word_normalization_is_case_insensitive() {
        assert_eq!(normalize_learning_term("Running"), "running");
        assert_ne!(
            normalize_learning_term("run"),
            normalize_learning_term("running")
        );
        assert_eq!(
            word_mark_rule_id("book", "running", "exact"),
            word_mark_rule_id("book", &normalize_learning_term("Running"), "exact")
        );
    }
}
