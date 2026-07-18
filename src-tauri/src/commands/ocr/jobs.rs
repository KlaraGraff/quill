use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

pub(crate) const ACTIVE_STATES: &[&str] = &[
    "queued",
    "waiting_source",
    "preparing",
    "recognizing",
    "validating",
    "publishing",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct OcrJob {
    pub id: String,
    pub book_id: String,
    pub source_sha256: String,
    pub state: String,
    pub phase: Option<String>,
    pub pages_done: Option<i32>,
    pub pages_total: Option<i32>,
    pub backend: Option<String>,
    pub backend_version: Option<String>,
    pub language_profile: Option<String>,
    pub quality_profile: Option<String>,
    pub jobs: Option<i32>,
    pub conversion_version: i32,
    pub result_asset_id: Option<String>,
    pub recognized_pages: Option<i32>,
    pub skipped_pages: Option<i32>,
    pub timed_out_pages: Option<i32>,
    pub failed_pages: Option<i32>,
    pub temporary_path: Option<String>,
    pub error_code: Option<String>,
    pub error_detail: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct NewOcrJob<'a> {
    pub book_id: &'a str,
    pub source_sha256: &'a str,
    pub backend: Option<&'a str>,
    pub backend_version: Option<&'a str>,
    pub language_profile: &'a str,
    pub quality_profile: &'a str,
    pub jobs: i32,
    pub conversion_version: i32,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub(crate) struct OcrJobUpdate<'a> {
    pub state: &'a str,
    pub phase: Option<&'a str>,
    pub pages_done: Option<i32>,
    pub pages_total: Option<i32>,
    pub updated_at: i64,
}

fn job_error(code: &str) -> AppError {
    AppError::Other(code.to_string())
}

fn valid_state(state: &str) -> bool {
    ACTIVE_STATES.contains(&state) || matches!(state, "ready" | "failed" | "cancelled")
}

fn transition_allowed(current: &str, next: &str) -> bool {
    current == next
        || matches!(
            (current, next),
            (
                "queued",
                "waiting_source" | "preparing" | "failed" | "cancelled"
            ) | ("waiting_source", "preparing" | "failed" | "cancelled")
                | ("preparing", "recognizing" | "failed" | "cancelled")
                | ("recognizing", "validating" | "failed" | "cancelled")
                | ("validating", "publishing" | "failed" | "cancelled")
                | ("publishing", "ready" | "failed")
        )
}

fn requires_current_source(state: &str) -> bool {
    matches!(
        state,
        "preparing" | "recognizing" | "validating" | "publishing" | "ready"
    )
}

fn validate_merged_progress(current: &OcrJob, update: &OcrJobUpdate<'_>) -> AppResult<()> {
    let done = update.pages_done.or(current.pages_done);
    let total = update.pages_total.or(current.pages_total);
    let negative = done.is_some_and(|value| value < 0) || total.is_some_and(|value| value < 0);
    let exceeds = matches!((done, total), (Some(done), Some(total)) if done > total);
    let regresses = matches!((current.pages_done, done), (Some(old), Some(new)) if new < old)
        || matches!((current.pages_total, total), (Some(old), Some(new)) if new < old);
    if negative || exceeds || regresses {
        return Err(job_error("OCR_JOB_PROGRESS_INVALID"));
    }
    if update.updated_at < current.updated_at {
        return Err(job_error("OCR_JOB_UPDATED_AT_INVALID"));
    }
    Ok(())
}

fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<OcrJob> {
    Ok(OcrJob {
        id: row.get(0)?,
        book_id: row.get(1)?,
        source_sha256: row.get(2)?,
        state: row.get(3)?,
        phase: row.get(4)?,
        pages_done: row.get(5)?,
        pages_total: row.get(6)?,
        backend: row.get(7)?,
        backend_version: row.get(8)?,
        language_profile: row.get(9)?,
        quality_profile: row.get(10)?,
        jobs: row.get(11)?,
        conversion_version: row.get(12)?,
        result_asset_id: row.get(13)?,
        recognized_pages: row.get(14)?,
        skipped_pages: row.get(15)?,
        timed_out_pages: row.get(16)?,
        failed_pages: row.get(17)?,
        temporary_path: row.get(18)?,
        error_code: row.get(19)?,
        error_detail: row.get(20)?,
        created_at: row.get(21)?,
        started_at: row.get(22)?,
        updated_at: row.get(23)?,
    })
}

const JOB_COLUMNS: &str = "id, book_id, source_sha256, state, phase,
    pages_done, pages_total, backend, backend_version, language_profile,
    quality_profile, jobs, conversion_version, result_asset_id,
    recognized_pages, skipped_pages, timed_out_pages, failed_pages,
    temporary_path, error_code, error_detail, created_at, started_at, updated_at";

pub(crate) fn create_job(conn: &Connection, request: NewOcrJob<'_>) -> AppResult<OcrJob> {
    crate::sync::validation::validate_entity_id(request.book_id)?;
    if request.source_sha256.is_empty()
        || request.jobs < 1
        || request.jobs > 4
        || request.conversion_version < 1
        || request.language_profile != "chi_sim+eng"
        || request.quality_profile != "fast"
    {
        return Err(job_error("OCR_JOB_REQUEST_INVALID"));
    }

    let id = Uuid::new_v4().to_string();
    let changed = conn.execute(
        "INSERT INTO ocr_jobs (
             id, book_id, source_sha256, state, pages_done, backend,
             backend_version, language_profile, quality_profile, jobs,
             conversion_version, created_at, updated_at
         )
         SELECT ?1, id, ?3, 'queued', 0, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10
         FROM books
         WHERE id = ?2
           AND COALESCE(source_format, format) = 'pdf'
           AND source_sha256 = ?3",
        params![
            id,
            request.book_id,
            request.source_sha256,
            request.backend,
            request.backend_version,
            request.language_profile,
            request.quality_profile,
            request.jobs,
            request.conversion_version,
            request.created_at,
        ],
    );
    match changed {
        Ok(1) => get_job(conn, &id)?.ok_or_else(|| job_error("OCR_JOB_NOT_FOUND")),
        Ok(_) => Err(job_error("OCR_JOB_SOURCE_INVALID")),
        Err(error)
            if error.to_string().contains("ocr_jobs_one_active")
                || error.to_string().contains("UNIQUE constraint failed") =>
        {
            Err(job_error("OCR_JOB_ALREADY_ACTIVE"))
        }
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn get_job(conn: &Connection, id: &str) -> AppResult<Option<OcrJob>> {
    let sql = format!("SELECT {JOB_COLUMNS} FROM ocr_jobs WHERE id = ?1");
    conn.query_row(&sql, params![id], row_to_job)
        .optional()
        .map_err(Into::into)
}

pub(crate) fn get_active_job(conn: &Connection, book_id: &str) -> AppResult<Option<OcrJob>> {
    let placeholders = ACTIVE_STATES
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT {JOB_COLUMNS} FROM ocr_jobs
         WHERE book_id = ?1 AND state IN ({placeholders})
         ORDER BY updated_at DESC LIMIT 1"
    );
    let mut values: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(ACTIVE_STATES.len() + 1);
    values.push(&book_id);
    for state in ACTIVE_STATES {
        values.push(state);
    }
    conn.query_row(&sql, values.as_slice(), row_to_job)
        .optional()
        .map_err(Into::into)
}

pub(crate) fn get_latest_job(conn: &Connection, book_id: &str) -> AppResult<Option<OcrJob>> {
    let sql = format!(
        "SELECT {JOB_COLUMNS} FROM ocr_jobs
         WHERE book_id = ?1 ORDER BY updated_at DESC, id DESC LIMIT 1"
    );
    conn.query_row(&sql, params![book_id], row_to_job)
        .optional()
        .map_err(Into::into)
}

pub(crate) fn update_state_guarded(
    conn: &Connection,
    id: &str,
    expected_source_sha256: &str,
    update: OcrJobUpdate<'_>,
) -> AppResult<OcrJob> {
    if !valid_state(update.state) {
        return Err(job_error("OCR_JOB_STATE_INVALID"));
    }
    let current = get_job(conn, id)?.ok_or_else(|| job_error("OCR_JOB_NOT_FOUND"))?;
    if current.source_sha256 != expected_source_sha256 {
        return Err(job_error("OCR_JOB_STALE"));
    }
    if !transition_allowed(&current.state, update.state) {
        return Err(job_error("OCR_JOB_TRANSITION_INVALID"));
    }
    validate_merged_progress(&current, &update)?;

    let guard_source = requires_current_source(update.state);
    let started_at =
        matches!(update.state, "preparing" | "recognizing").then_some(update.updated_at);
    let changed = conn.execute(
        "UPDATE ocr_jobs
         SET state = ?1,
             phase = ?2,
             pages_done = COALESCE(?3, pages_done),
             pages_total = COALESCE(?4, pages_total),
             started_at = COALESCE(started_at, ?5),
             updated_at = ?6
         WHERE id = ?7
           AND source_sha256 = ?8
           AND state = ?9
           AND (?10 = 0 OR EXISTS (
               SELECT 1 FROM books
               WHERE books.id = ocr_jobs.book_id
                 AND COALESCE(books.source_format, books.format) = 'pdf'
                 AND books.source_sha256 = ?8
           ))",
        params![
            update.state,
            update.phase,
            update.pages_done,
            update.pages_total,
            started_at,
            update.updated_at,
            id,
            expected_source_sha256,
            current.state,
            guard_source,
        ],
    )?;
    if changed == 0 {
        if guard_source {
            return Err(job_error("OCR_JOB_SOURCE_UNAVAILABLE"));
        }
        return Err(job_error("OCR_JOB_STALE"));
    }
    get_job(conn, id)?.ok_or_else(|| job_error("OCR_JOB_NOT_FOUND"))
}

pub(crate) fn mark_interrupted_jobs_failed(conn: &Connection, updated_at: i64) -> AppResult<usize> {
    conn.execute(
        "UPDATE ocr_jobs
         SET state = 'failed', phase = NULL,
             error_code = 'OCR_JOB_INTERRUPTED', error_detail = NULL,
             updated_at = MAX(updated_at, ?1)
         WHERE state IN (
             'queued', 'waiting_source', 'preparing', 'recognizing',
             'validating', 'publishing'
         )",
        params![updated_at],
    )
    .map_err(Into::into)
}

pub(crate) fn finish_job(
    conn: &Connection,
    id: &str,
    expected_source_sha256: &str,
    asset_id: &str,
    output: &super::validate::VerifiedOutput,
    updated_at: i64,
) -> AppResult<OcrJob> {
    crate::sync::validation::validate_entity_id(asset_id)?;
    let current = get_job(conn, id)?.ok_or_else(|| job_error("OCR_JOB_NOT_FOUND"))?;
    if current.state != "publishing" || current.source_sha256 != expected_source_sha256 {
        return Err(job_error("OCR_JOB_TRANSITION_INVALID"));
    }
    let changed = conn.execute(
        "UPDATE ocr_jobs
         SET state = 'ready', phase = NULL, pages_done = ?1, pages_total = ?1,
             result_asset_id = ?2, recognized_pages = ?3, skipped_pages = ?4,
             timed_out_pages = ?5, failed_pages = ?6, temporary_path = NULL,
             error_code = NULL, error_detail = NULL, updated_at = ?7
         WHERE id = ?8 AND source_sha256 = ?9 AND state = 'publishing'
           AND EXISTS (
             SELECT 1 FROM books
             WHERE books.id = ocr_jobs.book_id
               AND COALESCE(books.source_format, books.format) = 'pdf'
               AND books.source_sha256 = ?9
           )",
        params![
            output.page_count,
            asset_id,
            output.recognized_pages,
            output.skipped_pages,
            output.timed_out_pages,
            output.failed_pages,
            updated_at,
            id,
            expected_source_sha256,
        ],
    )?;
    if changed == 0 {
        return Err(job_error("OCR_JOB_SOURCE_UNAVAILABLE"));
    }
    get_job(conn, id)?.ok_or_else(|| job_error("OCR_JOB_NOT_FOUND"))
}

pub(crate) fn fail_or_cancel_job(
    conn: &Connection,
    id: &str,
    expected_source_sha256: &str,
    cancelled: bool,
    error_code: Option<&str>,
    error_detail: Option<&str>,
    updated_at: i64,
) -> AppResult<OcrJob> {
    let state = if cancelled { "cancelled" } else { "failed" };
    let changed = conn.execute(
        "UPDATE ocr_jobs
         SET state = ?1, phase = NULL, error_code = ?2, error_detail = ?3,
             temporary_path = NULL, updated_at = MAX(updated_at, ?4)
         WHERE id = ?5 AND source_sha256 = ?6
           AND state IN (
             'queued', 'waiting_source', 'preparing', 'recognizing',
             'validating', 'publishing'
           )",
        params![
            state,
            error_code,
            error_detail,
            updated_at,
            id,
            expected_source_sha256,
        ],
    )?;
    if changed == 0 {
        let current = get_job(conn, id)?.ok_or_else(|| job_error("OCR_JOB_NOT_FOUND"))?;
        if current.state == state {
            return Ok(current);
        }
        return Err(job_error("OCR_JOB_STALE"));
    }
    get_job(conn, id)?.ok_or_else(|| job_error("OCR_JOB_NOT_FOUND"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::Db::run_migrations_on(&conn).unwrap();
        conn.execute(
            "INSERT INTO books (
                 id, title, author, file_path, format, source_format,
                 source_file_path, source_sha256, status, progress,
                 created_at, updated_at
             ) VALUES (
                 'book-1', 'Scanned', 'Author', 'books/source.pdf', 'pdf',
                 'pdf', 'books/source.pdf', 'source-hash', 'unread', 0, 1, 1
             )",
            [],
        )
        .unwrap();
        conn
    }

    fn create(conn: &Connection) -> OcrJob {
        create_job(
            conn,
            NewOcrJob {
                book_id: "book-1",
                source_sha256: "source-hash",
                backend: Some("fake"),
                backend_version: Some("1"),
                language_profile: "chi_sim+eng",
                quality_profile: "fast",
                jobs: 1,
                conversion_version: 1,
                created_at: 2,
            },
        )
        .unwrap()
    }

    fn update<'a>(state: &'a str, at: i64) -> OcrJobUpdate<'a> {
        OcrJobUpdate {
            state,
            phase: None,
            pages_done: None,
            pages_total: None,
            updated_at: at,
        }
    }

    #[test]
    fn one_active_job_per_book() {
        let conn = open_db();
        create(&conn);
        let error = create_job(
            &conn,
            NewOcrJob {
                book_id: "book-1",
                source_sha256: "source-hash",
                backend: None,
                backend_version: None,
                language_profile: "chi_sim+eng",
                quality_profile: "fast",
                jobs: 1,
                conversion_version: 1,
                created_at: 3,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("OCR_JOB_ALREADY_ACTIVE"));
    }

    #[test]
    fn stale_source_can_park_or_finish_job_and_release_slot() {
        for terminal in ["waiting_source", "failed", "cancelled"] {
            let conn = open_db();
            let job = create(&conn);
            conn.execute(
                "UPDATE books SET source_sha256 = 'replacement' WHERE id = 'book-1'",
                [],
            )
            .unwrap();
            let updated =
                update_state_guarded(&conn, &job.id, "source-hash", update(terminal, 3)).unwrap();
            assert_eq!(updated.state, terminal);
            if terminal != "waiting_source" {
                conn.execute(
                    "UPDATE books SET source_sha256 = 'source-hash' WHERE id = 'book-1'",
                    [],
                )
                .unwrap();
                assert!(create_job(
                    &conn,
                    NewOcrJob {
                        book_id: "book-1",
                        source_sha256: "source-hash",
                        backend: None,
                        backend_version: None,
                        language_profile: "chi_sim+eng",
                        quality_profile: "fast",
                        jobs: 1,
                        conversion_version: 1,
                        created_at: 4,
                    },
                )
                .is_ok());
            }
        }
    }

    #[test]
    fn stale_source_cannot_continue_or_publish_ready() {
        let conn = open_db();
        let job = create(&conn);
        conn.execute(
            "UPDATE books SET source_sha256 = 'replacement' WHERE id = 'book-1'",
            [],
        )
        .unwrap();
        assert!(
            update_state_guarded(&conn, &job.id, "source-hash", update("preparing", 3),)
                .unwrap_err()
                .to_string()
                .contains("OCR_JOB_SOURCE_UNAVAILABLE")
        );

        conn.execute(
            "UPDATE books SET source_sha256 = 'source-hash' WHERE id = 'book-1'",
            [],
        )
        .unwrap();
        let mut current =
            update_state_guarded(&conn, &job.id, "source-hash", update("preparing", 4)).unwrap();
        for (state, at) in [("recognizing", 5), ("validating", 6), ("publishing", 7)] {
            current =
                update_state_guarded(&conn, &current.id, "source-hash", update(state, at)).unwrap();
        }
        conn.execute(
            "UPDATE books SET source_sha256 = 'replacement' WHERE id = 'book-1'",
            [],
        )
        .unwrap();
        assert!(
            update_state_guarded(&conn, &current.id, "source-hash", update("ready", 8),).is_err()
        );
    }

    #[test]
    fn progress_and_timestamp_cannot_regress() {
        let conn = open_db();
        let job = create(&conn);
        update_state_guarded(
            &conn,
            &job.id,
            "source-hash",
            OcrJobUpdate {
                state: "preparing",
                phase: Some("analyzing"),
                pages_done: Some(2),
                pages_total: Some(10),
                updated_at: 4,
            },
        )
        .unwrap();
        assert!(update_state_guarded(
            &conn,
            &job.id,
            "source-hash",
            OcrJobUpdate {
                state: "preparing",
                phase: None,
                pages_done: Some(1),
                pages_total: None,
                updated_at: 5,
            },
        )
        .is_err());
    }

    #[test]
    fn startup_marks_active_jobs_failed_without_source_guard() {
        let conn = open_db();
        create(&conn);
        conn.execute(
            "UPDATE books SET source_sha256 = NULL WHERE id = 'book-1'",
            [],
        )
        .unwrap();
        assert_eq!(mark_interrupted_jobs_failed(&conn, 9).unwrap(), 1);
        assert!(get_active_job(&conn, "book-1").unwrap().is_none());
    }
}
