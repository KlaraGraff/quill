use rusqlite::params;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::Db;
use crate::error::{AppError, AppResult};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LookupRecord {
    pub id: String,
    pub book_id: String,
    pub lookup_text: String,
    pub normalized_text: String,
    pub context_sentence: Option<String>,
    pub chapter: Option<String>,
    pub cfi: Option<String>,
    pub definition: String,
    pub context_explanation: Option<String>,
    pub created_at: i64,
    pub last_looked_up_at: i64,
    pub lookup_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub book_title: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct LookupRecordPage {
    pub records: Vec<LookupRecord>,
    pub next_cursor: Option<String>,
    pub total: usize,
    pub books: Vec<LookupBookFacet>,
}

#[derive(Debug, Serialize)]
pub struct LookupBookFacet {
    pub book_id: String,
    pub book_title: Option<String>,
    pub count: usize,
}

fn row_to_lookup(row: &rusqlite::Row) -> rusqlite::Result<LookupRecord> {
    Ok(LookupRecord {
        id: row.get(0)?,
        book_id: row.get(1)?,
        lookup_text: row.get(2)?,
        normalized_text: row.get(3)?,
        context_sentence: row.get(4)?,
        chapter: row.get(5)?,
        cfi: row.get(6)?,
        definition: row.get(7)?,
        context_explanation: row.get(8)?,
        created_at: row.get(9)?,
        last_looked_up_at: row.get(10)?,
        lookup_count: row.get(11)?,
        book_title: None,
    })
}

const SELECT_COLS: &str = "id, book_id, lookup_text, normalized_text, context_sentence, chapter, cfi, definition, context_explanation, created_at, last_looked_up_at, lookup_count";

fn configured_retention_days(conn: &rusqlite::Connection) -> Option<i64> {
    conn.query_row(
        "SELECT value FROM settings WHERE key = 'lookup_history_retention_days'",
        [],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .and_then(|value| value.parse::<i64>().ok())
    .filter(|days| *days > 0)
}

fn prune_lookup_records_conn(
    conn: &rusqlite::Connection,
    retention_days: Option<i64>,
) -> rusqlite::Result<usize> {
    let Some(days) = retention_days else {
        return Ok(0);
    };
    let cutoff = chrono::Utc::now().timestamp_millis() - days.saturating_mul(24 * 60 * 60 * 1000);
    conn.execute(
        "DELETE FROM lookup_records WHERE last_looked_up_at < ?1",
        params![cutoff],
    )
}

fn row_to_lookup_with_book(row: &rusqlite::Row) -> rusqlite::Result<LookupRecord> {
    Ok(LookupRecord {
        id: row.get(0)?,
        book_id: row.get(1)?,
        lookup_text: row.get(2)?,
        normalized_text: row.get(3)?,
        context_sentence: row.get(4)?,
        chapter: row.get(5)?,
        cfi: row.get(6)?,
        definition: row.get(7)?,
        context_explanation: row.get(8)?,
        created_at: row.get(9)?,
        last_looked_up_at: row.get(10)?,
        lookup_count: row.get(11)?,
        book_title: row.get(12)?,
    })
}

fn normalize(text: &str) -> String {
    text.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
        .to_lowercase()
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn save_lookup_record(
    book_id: String,
    lookup_text: String,
    context_sentence: Option<String>,
    chapter: Option<String>,
    cfi: Option<String>,
    definition: String,
    context_explanation: Option<String>,
    db: State<'_, Db>,
) -> AppResult<LookupRecord> {
    let normalized_text = normalize(&lookup_text);
    if normalized_text.is_empty() {
        return Err(AppError::Other("Lookup text cannot be empty".to_string()));
    }
    let now = chrono::Utc::now().timestamp_millis();
    let id = uuid::Uuid::new_v4().to_string();
    let conn = db.conn.lock().map_err(|e| AppError::Other(e.to_string()))?;

    // CFI is required for exact reader marking. Queries without a stable CFI
    // remain in history but are inserted independently rather than deduped.
    if let Some(ref cfi_value) = cfi {
        let existing: Option<String> = conn
            .query_row(
                "SELECT id FROM lookup_records WHERE book_id = ?1 AND cfi = ?2 AND normalized_text = ?3 LIMIT 1",
                params![book_id, cfi_value, normalized_text],
                |row| row.get(0),
            )
            .ok();
        if let Some(existing_id) = existing {
            conn.execute(
                "UPDATE lookup_records SET lookup_text = ?1, context_sentence = ?2, chapter = ?3, definition = ?4, context_explanation = ?5, last_looked_up_at = ?6, lookup_count = lookup_count + 1 WHERE id = ?7",
                params![lookup_text, context_sentence, chapter, definition, context_explanation, now, existing_id],
            )?;
            prune_lookup_records_conn(&conn, configured_retention_days(&conn))?;
            return conn
                .query_row(
                    &format!("SELECT {SELECT_COLS} FROM lookup_records WHERE id = ?1"),
                    params![existing_id],
                    row_to_lookup,
                )
                .map_err(Into::into);
        }
    }

    conn.execute(
        "INSERT INTO lookup_records (id, book_id, lookup_text, normalized_text, context_sentence, chapter, cfi, definition, context_explanation, created_at, last_looked_up_at, lookup_count) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, 1)",
        params![id, book_id, lookup_text, normalized_text, context_sentence, chapter, cfi, definition, context_explanation, now],
    )?;
    prune_lookup_records_conn(&conn, configured_retention_days(&conn))?;
    conn.query_row(
        &format!("SELECT {SELECT_COLS} FROM lookup_records WHERE id = ?1"),
        params![id],
        row_to_lookup,
    )
    .map_err(Into::into)
}

#[tauri::command]
pub fn list_lookup_records(book_id: String, db: State<'_, Db>) -> AppResult<Vec<LookupRecord>> {
    let conn = db.reader();
    let mut stmt = conn.prepare(&format!(
        "SELECT {SELECT_COLS} FROM lookup_records WHERE book_id = ?1 ORDER BY last_looked_up_at DESC"
    ))?;
    let records = stmt
        .query_map(params![book_id], row_to_lookup)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::from)?;
    Ok(records)
}

#[tauri::command]
pub fn list_all_lookup_records(
    search: Option<String>,
    book_id: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
    db: State<'_, Db>,
) -> AppResult<LookupRecordPage> {
    let conn = db.reader();
    let page_size = limit.unwrap_or(50).clamp(1, 200);
    let search = search.unwrap_or_default().trim().to_string();
    let mut conditions = Vec::new();
    let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(book_id) = book_id.filter(|id| !id.is_empty()) {
        conditions.push("l.book_id = ?".to_string());
        values.push(Box::new(book_id));
    }
    if !search.is_empty() {
        conditions.push("(LOWER(l.lookup_text) LIKE ? OR LOWER(l.definition) LIKE ? OR LOWER(COALESCE(l.context_sentence, '')) LIKE ? OR LOWER(COALESCE(b.title, '')) LIKE ?)".to_string());
        let pattern = format!("%{}%", search.to_lowercase());
        for _ in 0..4 {
            values.push(Box::new(pattern.clone()));
        }
    }
    let base_where = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };
    let total_sql = format!(
        "SELECT COUNT(*) FROM lookup_records l LEFT JOIN books b ON l.book_id = b.id{base_where}"
    );
    let total_refs: Vec<&dyn rusqlite::types::ToSql> =
        values.iter().map(|value| value.as_ref()).collect();
    let total: usize = conn.query_row(&total_sql, total_refs.as_slice(), |row| row.get(0))?;

    let mut facet_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let facet_where = if search.is_empty() {
        String::new()
    } else {
        let pattern = format!("%{}%", search.to_lowercase());
        for _ in 0..4 {
            facet_values.push(Box::new(pattern.clone()));
        }
        " WHERE (LOWER(l.lookup_text) LIKE ? OR LOWER(l.definition) LIKE ? OR LOWER(COALESCE(l.context_sentence, '')) LIKE ? OR LOWER(COALESCE(b.title, '')) LIKE ?)".to_string()
    };
    let facet_sql = format!(
        "SELECT l.book_id, b.title, COUNT(*) FROM lookup_records l LEFT JOIN books b ON l.book_id = b.id{facet_where} GROUP BY l.book_id, b.title ORDER BY LOWER(COALESCE(b.title, '')), l.book_id"
    );
    let facet_refs: Vec<&dyn rusqlite::types::ToSql> =
        facet_values.iter().map(|value| value.as_ref()).collect();
    let mut facet_statement = conn.prepare(&facet_sql)?;
    let books = facet_statement
        .query_map(facet_refs.as_slice(), |row| {
            Ok(LookupBookFacet {
                book_id: row.get(0)?,
                book_title: row.get(1)?,
                count: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    if let Some((timestamp, id)) = cursor.as_deref().and_then(|value| value.split_once(':')) {
        if let Ok(timestamp) = timestamp.parse::<i64>() {
            conditions.push(
                "(l.last_looked_up_at < ? OR (l.last_looked_up_at = ? AND l.id > ?))".to_string(),
            );
            values.push(Box::new(timestamp));
            values.push(Box::new(timestamp));
            values.push(Box::new(id.to_string()));
        }
    }
    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };
    let sql = format!(
        "SELECT l.id, l.book_id, l.lookup_text, l.normalized_text, l.context_sentence, l.chapter, l.cfi, l.definition, l.context_explanation, l.created_at, l.last_looked_up_at, l.lookup_count, b.title FROM lookup_records l LEFT JOIN books b ON l.book_id = b.id{where_clause} ORDER BY l.last_looked_up_at DESC, l.id ASC LIMIT ?"
    );
    values.push(Box::new((page_size + 1) as i64));
    let refs: Vec<&dyn rusqlite::types::ToSql> =
        values.iter().map(|value| value.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let records = stmt
        .query_map(refs.as_slice(), row_to_lookup_with_book)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::from)?;
    let mut records = records;
    let next_cursor = if records.len() > page_size {
        records.truncate(page_size);
        records
            .last()
            .map(|record| format!("{}:{}", record.last_looked_up_at, record.id))
    } else {
        None
    };
    Ok(LookupRecordPage {
        records,
        next_cursor,
        total,
        books,
    })
}

#[tauri::command]
pub fn delete_lookup_record(id: String, db: State<'_, Db>) -> AppResult<()> {
    let conn = db
        .conn
        .lock()
        .map_err(|error| AppError::Other(error.to_string()))?;
    conn.execute("DELETE FROM lookup_records WHERE id = ?1", params![id])?;
    Ok(())
}

#[tauri::command]
pub fn clear_lookup_records(book_id: Option<String>, db: State<'_, Db>) -> AppResult<usize> {
    let conn = db
        .conn
        .lock()
        .map_err(|error| AppError::Other(error.to_string()))?;
    let affected = match book_id.filter(|id| !id.is_empty()) {
        Some(book_id) => conn.execute(
            "DELETE FROM lookup_records WHERE book_id = ?1",
            params![book_id],
        )?,
        None => conn.execute("DELETE FROM lookup_records", [])?,
    };
    Ok(affected)
}

#[tauri::command]
pub fn prune_lookup_records(retention_days: Option<i64>, db: State<'_, Db>) -> AppResult<usize> {
    let conn = db
        .conn
        .lock()
        .map_err(|error| AppError::Other(error.to_string()))?;
    Ok(prune_lookup_records_conn(
        &conn,
        retention_days.filter(|days| *days > 0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Db {
        let dir = tempfile::TempDir::new().unwrap();
        // Keep the temp directory alive for the test by leaking it. The DB
        // owns files beneath it and each test process exits immediately after.
        let path = dir.keep();
        let db = Db::init(&path).unwrap();
        db.conn.lock().unwrap().execute(
            "INSERT INTO books (id, title, author, file_path, status, progress, created_at, updated_at, updated_by_device) VALUES ('book', 'Book', 'Author', 'books/book.epub', 'reading', 0, 1, 1, 'test')",
            [],
        ).unwrap();
        db
    }

    #[test]
    fn same_location_updates_lookup_count() {
        let db = setup();
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO lookup_records (id, book_id, lookup_text, normalized_text, cfi, definition, created_at, last_looked_up_at, lookup_count) VALUES ('one', 'book', 'Wonder', 'wonder', 'epubcfi(/6/2)', 'first', 1, 1, 1)",
            [],
        ).unwrap();
        conn.execute(
            "UPDATE lookup_records SET definition = 'second', lookup_count = lookup_count + 1, last_looked_up_at = 2 WHERE book_id = 'book' AND cfi = 'epubcfi(/6/2)' AND normalized_text = 'wonder'",
            [],
        ).unwrap();
        let (count, definition): (i64, String) = conn
            .query_row(
                "SELECT lookup_count, definition FROM lookup_records WHERE id = 'one'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 2);
        assert_eq!(definition, "second");
    }

    #[test]
    fn records_without_cfi_remain_independent() {
        let db = setup();
        let conn = db.conn.lock().unwrap();
        for id in ["one", "two"] {
            conn.execute(
                "INSERT INTO lookup_records (id, book_id, lookup_text, normalized_text, cfi, definition, created_at, last_looked_up_at, lookup_count) VALUES (?1, 'book', 'Wonder', 'wonder', NULL, '', 1, 1, 1)",
                params![id],
            ).unwrap();
        }
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM lookup_records", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
}
