use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::sync::events::{
    lookup_occurrence_mark_id, normalize_learning_term, word_mark_exception_id, word_mark_rule_id,
    EventBody, LookupOccurrenceMarkPayload, WordMarkExceptionPayload, WordMarkPayload,
};
use crate::sync::merge;
use crate::sync::validation::{
    validate_entity_id, validate_lookup_occurrence_mark_fields,
    validate_word_mark_exception_fields, validate_word_mark_fields,
};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordMarkException {
    pub id: String,
    pub rule_id: String,
    pub book_id: String,
    pub normalized_word: String,
    pub location: String,
    pub excluded: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookupOccurrenceMark {
    pub id: String,
    pub book_id: String,
    pub normalized_word: String,
    pub display_word: String,
    pub location: String,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Serialize)]
pub struct WordFormsEntry {
    pub normalized_word: String,
    pub display_word: String,
    pub forms: Vec<String>,
    pub source: Option<String>,
    pub created_at: i64,
    pub updated_at: Option<i64>,
}

fn normalized_forms(word: &str, forms: Vec<String>) -> Vec<String> {
    let normalized_word = normalize_learning_term(word);
    let mut values = forms
        .into_iter()
        .map(|value| normalize_learning_term(&value))
        .filter(|value| !value.is_empty() && value != &normalized_word)
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

#[tauri::command]
pub fn list_word_forms(db: State<'_, Db>) -> AppResult<Vec<WordFormsEntry>> {
    let conn = db.reader();
    let mut statement = conn.prepare(
        "SELECT r.normalized_word, MAX(r.display_word), f.forms, f.source,
                MAX(r.created_at), f.updated_at
         FROM word_mark_rules r
         LEFT JOIN word_forms f ON f.normalized_word = r.normalized_word
         WHERE r.enabled = 1
         GROUP BY r.normalized_word
         ORDER BY CASE WHEN f.normalized_word IS NULL THEN 0 ELSE 1 END,
                  MAX(r.created_at) DESC",
    )?;
    let values = statement
        .query_map([], |row| {
            let forms_json: Option<String> = row.get(2)?;
            Ok(WordFormsEntry {
                normalized_word: row.get(0)?,
                display_word: row.get(1)?,
                forms: forms_json
                    .and_then(|value| serde_json::from_str(&value).ok())
                    .unwrap_or_default(),
                source: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(values)
}

#[tauri::command]
pub fn set_word_forms(
    word: String,
    forms: Vec<String>,
    source: Option<String>,
    db: State<'_, Db>,
) -> AppResult<Vec<String>> {
    let normalized_word = normalize_learning_term(&word);
    if normalized_word.is_empty() || normalized_word.chars().count() > 256 {
        return Err(AppError::Other("WORD_FORMS_WORD_INVALID".to_string()));
    }
    let forms = normalized_forms(&normalized_word, forms);
    let forms_json =
        serde_json::to_string(&forms).map_err(|error| AppError::Other(error.to_string()))?;
    let source = if source.as_deref() == Some("model") {
        "model"
    } else {
        "user"
    };
    let timestamp = chrono::Utc::now().timestamp_millis();
    let conn = db
        .conn
        .lock()
        .map_err(|error| AppError::Other(error.to_string()))?;
    conn.execute(
        "INSERT INTO word_forms(normalized_word, forms, source, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(normalized_word) DO UPDATE SET
           forms=excluded.forms, source=excluded.source, updated_at=excluded.updated_at",
        params![normalized_word, forms_json, source, timestamp],
    )?;
    Ok(forms)
}

#[tauri::command]
pub fn get_word_forms(words: Vec<String>, db: State<'_, Db>) -> AppResult<Vec<WordFormsEntry>> {
    let conn = db.reader();
    let mut result = Vec::new();
    for word in words.into_iter().map(|word| normalize_learning_term(&word)) {
        let row = conn.query_row(
            "SELECT normalized_word, normalized_word, forms, source, updated_at, updated_at
             FROM word_forms WHERE normalized_word = ?1",
            params![word],
            |row| {
                let forms_json: String = row.get(2)?;
                Ok(WordFormsEntry {
                    normalized_word: row.get(0)?,
                    display_word: row.get(1)?,
                    forms: serde_json::from_str(&forms_json).unwrap_or_default(),
                    source: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            },
        );
        if let Ok(entry) = row {
            result.push(entry);
        }
    }
    Ok(result)
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
    let timestamp = sync.next_logical_timestamp();
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
            let preserved_exceptions = merge::reconcile_legacy_word_mark_exceptions(
                tx,
                &existing.id,
                &id,
                book_id,
                &normalized_word,
                timestamp,
                &device,
                true,
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
            // The canonicalizing upsert is a reset barrier on peers. Republish
            // the migrated exclusions at that same tuple so an identity-only
            // repair does not silently clear the user's per-location choices.
            events.extend(
                preserved_exceptions
                    .into_iter()
                    .map(EventBody::WordMarkExceptionSet),
            );
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
    let timestamp = sync.next_logical_timestamp();
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

        let occurrence_resets = if enabled {
            Vec::new()
        } else {
            let mut statement = tx.prepare(
                "SELECT id, display_word, location, created_at
                 FROM lookup_occurrence_marks
                 WHERE book_id = ?1 AND normalized_word = ?2 AND enabled = 1",
            )?;
            let rows = statement
                .query_map(params![book_id, normalized_word], |row| {
                    Ok(LookupOccurrenceMarkPayload {
                        id: row.get(0)?,
                        book_id: book_id.to_string(),
                        normalized_word: normalized_word.clone(),
                        display_word: row.get(1)?,
                        location: row.get(2)?,
                        enabled: false,
                        created_at: row.get(3)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };

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
        if let Some(existing) = existing.as_ref().filter(|rule| rule.id != id) {
            merge::reconcile_legacy_word_mark_exceptions(
                tx,
                &existing.id,
                &id,
                book_id,
                &normalized_word,
                timestamp,
                &device,
                false,
            )?;
        }
        // Any explicit rule toggle starts a new whole-book marking baseline.
        // The WordMarkUpsert event applies the same reset on peers, so a
        // separate event per exception would only duplicate that state.
        tx.execute(
            "UPDATE word_mark_exceptions
             SET excluded = 0, updated_at = ?2, updated_by_device = ?3
             WHERE rule_id = ?1
               AND (updated_at < ?2 OR (updated_at = ?2 AND updated_by_device < ?3))",
            params![id, timestamp, device],
        )?;
        if !enabled {
            tx.execute(
                "UPDATE lookup_occurrence_marks
                 SET enabled = 0, updated_at = ?3, updated_by_device = ?4
                 WHERE book_id = ?1 AND normalized_word = ?2 AND enabled = 1",
                params![book_id, normalized_word, timestamp, device],
            )?;
        }
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
        events.extend(
            occurrence_resets
                .into_iter()
                .map(EventBody::LookupOccurrenceMarkSet),
        );
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

fn set_word_mark_exception_inner(
    book_id: &str,
    word: &str,
    location: &str,
    excluded: bool,
    db: &Db,
    sync: &SyncWriter,
) -> AppResult<WordMarkException> {
    validate_entity_id(book_id)?;
    let normalized_word = normalize_learning_term(word);
    let rule_id = word_mark_rule_id(book_id, &normalized_word, "exact");
    let id = word_mark_exception_id(&rule_id, location);
    validate_word_mark_exception_fields(&id, &rule_id, book_id, &normalized_word, location)?;
    let timestamp = sync.next_logical_timestamp();
    let device = sync.self_device().to_string();
    let mut created_at = timestamp;
    sync.with_tx(db, timestamp, |tx, events| {
        require_book(tx, book_id)?;
        let rule_enabled: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM word_mark_rules
             WHERE id = ?1 AND book_id = ?2 AND normalized_word = ?3 AND enabled = 1)",
            params![rule_id, book_id, normalized_word],
            |row| row.get(0),
        )?;
        if !rule_enabled {
            return Err(AppError::Other("WORD_MARK_RULE_NOT_ACTIVE".to_string()));
        }
        created_at = tx
            .query_row(
                "SELECT created_at FROM word_mark_exceptions
                 WHERE rule_id = ?1 AND location = ?2",
                params![rule_id, location],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(timestamp);
        tx.execute(
            "INSERT INTO word_mark_exceptions
             (id, rule_id, book_id, normalized_word, location, excluded,
              created_at, updated_at, updated_by_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(rule_id, location) DO UPDATE SET
               id=excluded.id, book_id=excluded.book_id,
               normalized_word=excluded.normalized_word,
               excluded=excluded.excluded, updated_at=excluded.updated_at,
               updated_by_device=excluded.updated_by_device",
            params![
                id,
                rule_id,
                book_id,
                normalized_word,
                location,
                excluded as i64,
                created_at,
                timestamp,
                device,
            ],
        )?;
        events.push(EventBody::WordMarkExceptionSet(WordMarkExceptionPayload {
            id: id.clone(),
            rule_id: rule_id.clone(),
            book_id: book_id.to_string(),
            normalized_word: normalized_word.clone(),
            location: location.to_string(),
            excluded,
            created_at,
        }));
        Ok(())
    })?;
    Ok(WordMarkException {
        id,
        rule_id,
        book_id: book_id.to_string(),
        normalized_word,
        location: location.to_string(),
        excluded,
        created_at,
        updated_at: timestamp,
    })
}

#[tauri::command]
pub fn set_word_mark_exception(
    book_id: String,
    word: String,
    location: String,
    excluded: bool,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<WordMarkException> {
    set_word_mark_exception_inner(&book_id, &word, &location, excluded, &db, &sync)
}

#[tauri::command]
pub fn list_word_mark_exceptions(
    book_id: String,
    db: State<'_, Db>,
) -> AppResult<Vec<WordMarkException>> {
    validate_entity_id(&book_id)?;
    let conn = db.reader();
    let mut statement = conn.prepare(
        "SELECT id, rule_id, book_id, normalized_word, location, excluded,
                created_at, updated_at
         FROM word_mark_exceptions e
         WHERE e.book_id = ?1 AND e.excluded = 1
           AND EXISTS(SELECT 1 FROM word_mark_rules r
                      WHERE r.id = e.rule_id AND r.enabled = 1)
         ORDER BY created_at ASC, id ASC",
    )?;
    let exceptions = statement
        .query_map(params![book_id], |row| {
            Ok(WordMarkException {
                id: row.get(0)?,
                rule_id: row.get(1)?,
                book_id: row.get(2)?,
                normalized_word: row.get(3)?,
                location: row.get(4)?,
                excluded: row.get::<_, i64>(5)? != 0,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(exceptions)
}

fn set_lookup_occurrence_mark_inner(
    book_id: &str,
    word: &str,
    location: &str,
    enabled: bool,
    db: &Db,
    sync: &SyncWriter,
) -> AppResult<LookupOccurrenceMark> {
    validate_entity_id(book_id)?;
    let normalized_word = normalize_learning_term(word);
    let display_word = word.trim().to_string();
    let id = lookup_occurrence_mark_id(book_id, location);
    validate_lookup_occurrence_mark_fields(
        &id,
        book_id,
        &normalized_word,
        &display_word,
        location,
    )?;
    let timestamp = sync.next_logical_timestamp();
    let device = sync.self_device().to_string();
    let mut created_at = timestamp;
    sync.with_tx(db, timestamp, |tx, events| {
        require_book(tx, book_id)?;
        created_at = tx
            .query_row(
                "SELECT created_at FROM lookup_occurrence_marks
                 WHERE book_id = ?1 AND location = ?2",
                params![book_id, location],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(timestamp);
        tx.execute(
            "INSERT INTO lookup_occurrence_marks
             (id, book_id, normalized_word, display_word, location, enabled,
              created_at, updated_at, updated_by_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(book_id, location) DO UPDATE SET
               id=excluded.id, normalized_word=excluded.normalized_word,
               display_word=excluded.display_word, enabled=excluded.enabled,
               updated_at=excluded.updated_at, updated_by_device=excluded.updated_by_device",
            params![
                id,
                book_id,
                normalized_word,
                display_word,
                location,
                enabled as i64,
                created_at,
                timestamp,
                device,
            ],
        )?;
        events.push(EventBody::LookupOccurrenceMarkSet(
            LookupOccurrenceMarkPayload {
                id: id.clone(),
                book_id: book_id.to_string(),
                normalized_word: normalized_word.clone(),
                display_word: display_word.clone(),
                location: location.to_string(),
                enabled,
                created_at,
            },
        ));
        Ok(())
    })?;
    Ok(LookupOccurrenceMark {
        id,
        book_id: book_id.to_string(),
        normalized_word,
        display_word,
        location: location.to_string(),
        enabled,
        created_at,
        updated_at: timestamp,
    })
}

fn ensure_lookup_occurrence_mark_inner(
    book_id: &str,
    word: &str,
    location: &str,
    db: &Db,
    sync: &SyncWriter,
) -> AppResult<LookupOccurrenceMark> {
    validate_entity_id(book_id)?;
    let existing = {
        let conn = db.reader();
        conn.query_row(
            "SELECT id, book_id, normalized_word, display_word, location, enabled,
                    created_at, updated_at
             FROM lookup_occurrence_marks
             WHERE book_id = ?1 AND location = ?2",
            params![book_id, location],
            |row| {
                Ok(LookupOccurrenceMark {
                    id: row.get(0)?,
                    book_id: row.get(1)?,
                    normalized_word: row.get(2)?,
                    display_word: row.get(3)?,
                    location: row.get(4)?,
                    enabled: row.get::<_, i64>(5)? != 0,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                })
            },
        )
        .optional()?
    };
    if let Some(existing) = existing {
        return Ok(existing);
    }
    set_lookup_occurrence_mark_inner(book_id, word, location, true, db, sync)
}

/// Creates an automatic mark only when this occurrence has never been seen.
/// A successful lookup must not silently restore a mark the user removed.
#[tauri::command]
pub fn ensure_lookup_occurrence_mark(
    book_id: String,
    word: String,
    location: String,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<LookupOccurrenceMark> {
    ensure_lookup_occurrence_mark_inner(&book_id, &word, &location, &db, &sync)
}

#[tauri::command]
pub fn set_lookup_occurrence_mark_enabled(
    book_id: String,
    word: String,
    location: String,
    enabled: bool,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<LookupOccurrenceMark> {
    set_lookup_occurrence_mark_inner(&book_id, &word, &location, enabled, &db, &sync)
}

#[tauri::command]
pub fn list_lookup_occurrence_marks(
    book_id: String,
    db: State<'_, Db>,
) -> AppResult<Vec<LookupOccurrenceMark>> {
    validate_entity_id(&book_id)?;
    let conn = db.reader();
    let mut statement = conn.prepare(
        "SELECT id, book_id, normalized_word, display_word, location, enabled,
                created_at, updated_at
         FROM lookup_occurrence_marks
         WHERE book_id = ?1 AND enabled = 1
         ORDER BY created_at, id",
    )?;
    let rows = statement
        .query_map(params![book_id], |row| {
            Ok(LookupOccurrenceMark {
                id: row.get(0)?,
                book_id: row.get(1)?,
                normalized_word: row.get(2)?,
                display_word: row.get(3)?,
                location: row.get(4)?,
                enabled: row.get::<_, i64>(5)? != 0,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn clear_lookup_marks_for_book_inner(book_id: &str, db: &Db, sync: &SyncWriter) -> AppResult<()> {
    validate_entity_id(book_id)?;
    let timestamp = sync.next_logical_timestamp();
    let device = sync.self_device().to_string();
    sync.with_tx(db, timestamp, |tx, events| {
        require_book(tx, book_id)?;
        let legacy_disabled_rule_resets = {
            let mut statement = tx.prepare(&format!(
                "SELECT {RULE_COLUMNS} FROM word_mark_rules WHERE book_id = ?1"
            ))?;
            let rows = statement
                .query_map(params![book_id], row_to_rule)?
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);

            let mut disabled = Vec::new();
            for rule in rows {
                let canonical_id =
                    word_mark_rule_id(book_id, &rule.normalized_word, &rule.match_mode);
                if rule.id == canonical_id {
                    continue;
                }
                tx.execute(
                    "UPDATE word_mark_rules
                     SET id = ?1, updated_at = ?2, updated_by_device = ?3
                     WHERE id = ?4",
                    params![canonical_id, timestamp, device, rule.id],
                )?;
                merge::reconcile_legacy_word_mark_exceptions(
                    tx,
                    &rule.id,
                    &canonical_id,
                    book_id,
                    &rule.normalized_word,
                    timestamp,
                    &device,
                    false,
                )?;
                if !rule.enabled {
                    disabled.push(WordMarkPayload {
                        id: canonical_id,
                        book_id: book_id.to_string(),
                        normalized_word: rule.normalized_word,
                        display_word: rule.display_word,
                        match_mode: rule.match_mode,
                        color: rule.color,
                        enabled: false,
                        created_at: rule.created_at,
                    });
                }
            }
            disabled
        };
        let rules = {
            let mut statement = tx.prepare(
                "SELECT id, normalized_word, display_word, match_mode, color, created_at
                 FROM word_mark_rules WHERE book_id = ?1 AND enabled = 1",
            )?;
            let rows = statement
                .query_map(params![book_id], |row| {
                    Ok(WordMarkPayload {
                        id: row.get(0)?,
                        book_id: book_id.to_string(),
                        normalized_word: row.get(1)?,
                        display_word: row.get(2)?,
                        match_mode: row.get(3)?,
                        color: row.get(4)?,
                        enabled: false,
                        created_at: row.get(5)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        let occurrences = {
            let mut statement = tx.prepare(
                "SELECT id, normalized_word, display_word, location, created_at
                 FROM lookup_occurrence_marks WHERE book_id = ?1 AND enabled = 1",
            )?;
            let rows = statement
                .query_map(params![book_id], |row| {
                    Ok(LookupOccurrenceMarkPayload {
                        id: row.get(0)?,
                        book_id: book_id.to_string(),
                        normalized_word: row.get(1)?,
                        display_word: row.get(2)?,
                        location: row.get(3)?,
                        enabled: false,
                        created_at: row.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        let orphan_exception_resets = {
            let mut statement = tx.prepare(
                "SELECT e.id, e.rule_id, e.normalized_word, e.location, e.created_at
                 FROM word_mark_exceptions e
                 WHERE e.book_id = ?1 AND e.excluded = 1
                   AND NOT EXISTS(
                     SELECT 1 FROM word_mark_rules r
                     WHERE r.id = e.rule_id AND r.enabled = 1
                   )",
            )?;
            let rows = statement
                .query_map(params![book_id], |row| {
                    Ok(WordMarkExceptionPayload {
                        id: row.get(0)?,
                        rule_id: row.get(1)?,
                        book_id: book_id.to_string(),
                        normalized_word: row.get(2)?,
                        location: row.get(3)?,
                        excluded: false,
                        created_at: row.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            rows
        };
        tx.execute(
            "UPDATE word_mark_rules SET enabled = 0, updated_at = ?2, updated_by_device = ?3
             WHERE book_id = ?1 AND enabled = 1",
            params![book_id, timestamp, device],
        )?;
        tx.execute(
            "UPDATE word_mark_exceptions SET excluded = 0, updated_at = ?2, updated_by_device = ?3
             WHERE book_id = ?1",
            params![book_id, timestamp, device],
        )?;
        tx.execute(
            "UPDATE lookup_occurrence_marks SET enabled = 0, updated_at = ?2, updated_by_device = ?3
             WHERE book_id = ?1 AND enabled = 1",
            params![book_id, timestamp, device],
        )?;
        events.extend(
            legacy_disabled_rule_resets
                .into_iter()
                .map(EventBody::WordMarkUpsert),
        );
        events.extend(rules.into_iter().map(EventBody::WordMarkUpsert));
        events.extend(
            occurrences
                .into_iter()
                .map(EventBody::LookupOccurrenceMarkSet),
        );
        // Active parent rules act as reset barriers for their exceptions.
        // Orphaned exceptions have no parent event, so publish their disabled
        // state explicitly to keep a book-wide clear convergent on peers.
        events.extend(
            orphan_exception_resets
                .into_iter()
                .map(EventBody::WordMarkExceptionSet),
        );
        Ok(())
    })
}

#[tauri::command]
pub fn clear_lookup_marks_for_book(
    book_id: String,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<()> {
    clear_lookup_marks_for_book_inner(&book_id, &db, &sync)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, Db, SyncWriter) {
        let dir = TempDir::new().unwrap();
        let db = Db::init(dir.path()).unwrap();
        let sync = SyncWriter::new("dev-A".into());
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO books
                 (id, title, author, file_path, format, status, progress,
                  created_at, updated_at, updated_by_device)
                 VALUES ('book', 'Book', 'Author', 'books/book.epub', 'epub',
                         'unread', 0, 1, 1, 'dev-A')",
                [],
            )
            .unwrap();
        }
        (dir, db, sync)
    }

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

    #[test]
    fn rapid_rule_toggles_keep_the_second_action_newer() {
        let (_dir, db, sync) = setup();

        let enabled =
            set_word_mark_rule_enabled_inner("book", "Running", true, Some("lookup"), &db, &sync)
                .unwrap();
        let disabled =
            set_word_mark_rule_enabled_inner("book", "Running", false, None, &db, &sync).unwrap();

        assert!(enabled.enabled);
        assert!(!disabled.enabled);
        assert!(disabled.updated_at > enabled.updated_at);
    }

    #[test]
    fn canonicalizing_a_legacy_rule_preserves_its_canonical_orphan_exception() {
        let (_dir, db, sync) = setup();
        let rule =
            set_word_mark_rule_enabled_inner("book", "Running", true, Some("lookup"), &db, &sync)
                .unwrap();
        let exception =
            set_word_mark_exception_inner("book", "Running", "textloc:v2:10:17", true, &db, &sync)
                .unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "UPDATE word_mark_rules SET id = 'legacy-random-id' WHERE id = ?1",
                params![rule.id],
            )
            .unwrap();
        }
        sync.set_should_queue(true);

        let canonical =
            ensure_word_mark_rule_inner("book", "Running", Some("lookup"), &db, &sync).unwrap();

        let conn = db.reader();
        let stored: (String, String, i64) = conn
            .query_row(
                "SELECT id, rule_id, excluded FROM word_mark_exceptions WHERE location = ?1",
                params![exception.location],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            stored.0,
            word_mark_exception_id(&canonical.id, &exception.location)
        );
        assert_eq!(stored.1, canonical.id);
        assert_eq!(stored.2, 1, "identity repair must retain the exclusion");
        let pending: i64 = conn
            .query_row("SELECT COUNT(*) FROM _pending_publish", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            pending, 2,
            "the rule and preserved exception must both publish"
        );
    }

    #[test]
    fn rapid_exception_toggles_keep_the_second_action_newer() {
        let (_dir, db, sync) = setup();
        set_word_mark_rule_enabled_inner("book", "Running", true, Some("lookup"), &db, &sync)
            .unwrap();

        let excluded =
            set_word_mark_exception_inner("book", "Running", "epubcfi(/6/4!)", true, &db, &sync)
                .unwrap();
        let restored =
            set_word_mark_exception_inner("book", "Running", "epubcfi(/6/4!)", false, &db, &sync)
                .unwrap();

        assert!(excluded.excluded);
        assert!(!restored.excluded);
        assert!(restored.updated_at > excluded.updated_at);
        assert_eq!(restored.created_at, excluded.created_at);
    }

    #[test]
    fn lookup_occurrence_ensure_preserves_an_explicit_removal() {
        let (_dir, db, sync) = setup();
        let first =
            ensure_lookup_occurrence_mark_inner("book", "Running", "textloc:v2:10:17", &db, &sync)
                .unwrap();

        let unchanged =
            ensure_lookup_occurrence_mark_inner("book", "Running", "textloc:v2:10:17", &db, &sync)
                .unwrap();
        assert_eq!(unchanged.id, first.id);
        assert_eq!(unchanged.created_at, first.created_at);
        assert_eq!(unchanged.updated_at, first.updated_at);

        let disabled = set_lookup_occurrence_mark_inner(
            "book",
            "Running",
            "textloc:v2:10:17",
            false,
            &db,
            &sync,
        )
        .unwrap();
        assert!(!disabled.enabled);
        assert!(disabled.updated_at > first.updated_at);
        assert_eq!(disabled.created_at, first.created_at);

        let still_disabled =
            ensure_lookup_occurrence_mark_inner("book", "Running", "textloc:v2:10:17", &db, &sync)
                .unwrap();
        assert!(!still_disabled.enabled);
        assert_eq!(still_disabled.updated_at, disabled.updated_at);
        assert_eq!(still_disabled.id, first.id);
        assert_eq!(still_disabled.created_at, first.created_at);
    }

    #[test]
    fn clear_lookup_marks_disables_rules_occurrences_and_exceptions() {
        let (_dir, db, sync) = setup();
        set_word_mark_rule_enabled_inner("book", "Running", true, Some("lookup"), &db, &sync)
            .unwrap();
        set_word_mark_exception_inner("book", "Running", "textloc:v2:10:17", true, &db, &sync)
            .unwrap();
        set_lookup_occurrence_mark_inner("book", "Elsewhere", "textloc:v2:30:39", true, &db, &sync)
            .unwrap();
        sync.set_should_queue(true);
        clear_lookup_marks_for_book_inner("book", &db, &sync).unwrap();

        let conn = db.reader();
        for (table, predicate) in [
            ("word_mark_rules", "enabled = 1"),
            ("word_mark_exceptions", "excluded = 1"),
            ("lookup_occurrence_marks", "enabled = 1"),
        ] {
            let count: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE book_id = 'book' AND {predicate}"),
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 0, "{table} should have no active lookup marks");
        }
        let pending_events: i64 = conn
            .query_row("SELECT COUNT(*) FROM _pending_publish", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(pending_events, 2);
    }

    #[test]
    fn removing_a_whole_book_rule_also_clears_same_term_occurrences() {
        let (_dir, db, sync) = setup();
        set_lookup_occurrence_mark_inner("book", "Running", "textloc:v2:10:17", true, &db, &sync)
            .unwrap();
        set_word_mark_rule_enabled_inner("book", "Running", true, Some("lookup"), &db, &sync)
            .unwrap();

        set_word_mark_rule_enabled_inner("book", "Running", false, None, &db, &sync).unwrap();

        let conn = db.reader();
        let active_occurrences: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM lookup_occurrence_marks
                 WHERE book_id = 'book' AND normalized_word = 'running' AND enabled = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(active_occurrences, 0);
    }

    #[test]
    fn word_forms_are_normalized_deduplicated_and_exclude_the_source_word() {
        assert_eq!(
            normalized_forms(
                "Run",
                vec![
                    " Running ".to_string(),
                    "runs".to_string(),
                    "RUNS".to_string(),
                    "run".to_string(),
                    "".to_string(),
                ],
            ),
            vec!["running".to_string(), "runs".to_string()],
        );
    }
}
