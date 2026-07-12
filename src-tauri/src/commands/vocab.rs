use chrono::{TimeZone, Utc};
use fsrs::{Card, Rating, State as FsrsState, FSRS};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tauri::State;

use crate::db::Db;
use crate::error::AppResult;
use crate::sync::events::{EventBody, VocabPayload};
use crate::sync::writer::SyncWriter;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct VocabWord {
    pub id: String,
    pub book_id: String,
    pub word: String,
    pub definition: String,
    pub context_sentence: Option<String>,
    pub context_explanation: Option<String>,
    pub cfi: Option<String>,
    pub mastery: String,
    pub review_count: i64,
    pub next_review_at: Option<i64>,
    pub review_interval_days: i64,
    pub last_reviewed_at: Option<i64>,
    pub last_review_rating: Option<String>,
    pub fsrs_stability: Option<f64>,
    pub fsrs_difficulty: Option<f64>,
    pub fsrs_version: i64,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub book_title: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct VocabStats {
    pub total: i64,
    pub new_count: i64,
    pub learning_count: i64,
    pub mastered_count: i64,
    pub due_for_review: i64,
}

fn row_to_vocab(row: &rusqlite::Row) -> rusqlite::Result<VocabWord> {
    Ok(VocabWord {
        id: row.get(0)?,
        book_id: row.get(1)?,
        word: row.get(2)?,
        definition: row.get(3)?,
        context_sentence: row.get(4)?,
        cfi: row.get(5)?,
        mastery: row.get(6)?,
        review_count: row.get(7)?,
        next_review_at: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
        context_explanation: row.get(11)?,
        review_interval_days: row.get(12)?,
        last_reviewed_at: row.get(13)?,
        last_review_rating: row.get(14)?,
        fsrs_stability: row.get(15)?,
        fsrs_difficulty: row.get(16)?,
        fsrs_version: row.get(17)?,
        book_title: None,
    })
}

fn row_to_vocab_with_book(row: &rusqlite::Row) -> rusqlite::Result<VocabWord> {
    Ok(VocabWord {
        id: row.get(0)?,
        book_id: row.get(1)?,
        word: row.get(2)?,
        definition: row.get(3)?,
        context_sentence: row.get(4)?,
        cfi: row.get(5)?,
        mastery: row.get(6)?,
        review_count: row.get(7)?,
        next_review_at: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
        context_explanation: row.get(11)?,
        review_interval_days: row.get(12)?,
        last_reviewed_at: row.get(13)?,
        last_review_rating: row.get(14)?,
        fsrs_stability: row.get(15)?,
        fsrs_difficulty: row.get(16)?,
        fsrs_version: row.get(17)?,
        book_title: row.get(18)?,
    })
}

const SELECT_COLS: &str = "id, book_id, word, definition, context_sentence, cfi, mastery, review_count, next_review_at, created_at, updated_at, context_explanation, review_interval_days, last_reviewed_at, last_review_rating, fsrs_stability, fsrs_difficulty, fsrs_version";

#[cfg(test)]
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
const VOCAB_BACKUP_SCHEMA: &str = "quill-vocabulary";
const VOCAB_BACKUP_VERSION: u32 = 1;
const MAX_VOCAB_IMPORT_BYTES: usize = 10 * 1024 * 1024;
const MAX_VOCAB_IMPORT_WORDS: usize = 50_000;

#[derive(Debug, Serialize, Deserialize)]
pub struct VocabBackup {
    pub schema: String,
    pub version: u32,
    pub exported_at: i64,
    pub words: Vec<VocabBackupWord>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct VocabBackupWord {
    pub id: String,
    pub book_id: String,
    pub word: String,
    pub definition: String,
    #[serde(default)]
    pub context_sentence: Option<String>,
    #[serde(default)]
    pub context_explanation: Option<String>,
    #[serde(default)]
    pub cfi: Option<String>,
    #[serde(default = "default_mastery")]
    pub mastery: String,
    #[serde(default)]
    pub review_count: i64,
    #[serde(default)]
    pub next_review_at: Option<i64>,
    #[serde(default)]
    pub review_interval_days: i64,
    #[serde(default)]
    pub last_reviewed_at: Option<i64>,
    #[serde(default)]
    pub last_review_rating: Option<String>,
    #[serde(default)]
    pub fsrs_stability: Option<f64>,
    #[serde(default)]
    pub fsrs_difficulty: Option<f64>,
    #[serde(default = "default_fsrs_version")]
    pub fsrs_version: i64,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

fn default_mastery() -> String {
    "new".to_string()
}

fn default_fsrs_version() -> i64 {
    1
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VocabImportFormat {
    Json,
    Csv,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VocabImportConflictPolicy {
    Skip,
    Overwrite,
}

#[derive(Debug, Serialize)]
pub struct VocabImportPreview {
    pub valid: usize,
    pub new_words: usize,
    pub conflicts: usize,
    pub missing_books: usize,
    pub duplicate_rows: usize,
    pub invalid_rows: usize,
}

#[derive(Debug, Serialize)]
pub struct VocabImportResult {
    pub preview: VocabImportPreview,
    pub imported: usize,
    pub replaced: usize,
    pub skipped: usize,
    pub dry_run: bool,
}

#[derive(Debug, Clone)]
struct VocabReviewState {
    review_count: i64,
    review_interval_days: i64,
    last_reviewed_at: Option<i64>,
    last_review_rating: Option<String>,
    fsrs_stability: Option<f64>,
    fsrs_difficulty: Option<f64>,
    fsrs_version: i64,
}

fn row_to_review_state(row: &rusqlite::Row) -> rusqlite::Result<VocabReviewState> {
    Ok(VocabReviewState {
        review_count: row.get(0)?,
        review_interval_days: row.get(1)?,
        last_reviewed_at: row.get(2)?,
        last_review_rating: row.get(3)?,
        fsrs_stability: row.get(4)?,
        fsrs_difficulty: row.get(5)?,
        fsrs_version: row.get(6)?,
    })
}

#[derive(Debug, Deserialize)]
struct VocabCsvRow {
    backup_schema: String,
    backup_version: u32,
    id: String,
    book_id: String,
    word: String,
    definition: String,
    #[serde(default)]
    context_sentence: Option<String>,
    #[serde(default)]
    context_explanation: Option<String>,
    #[serde(default)]
    cfi: Option<String>,
    #[serde(default = "default_mastery")]
    mastery: String,
    #[serde(default)]
    review_count: i64,
    #[serde(default)]
    next_review_at: Option<i64>,
    #[serde(default)]
    review_interval_days: i64,
    #[serde(default)]
    last_reviewed_at: Option<i64>,
    #[serde(default)]
    last_review_rating: Option<String>,
    #[serde(default)]
    fsrs_stability: Option<f64>,
    #[serde(default)]
    fsrs_difficulty: Option<f64>,
    #[serde(default = "default_fsrs_version")]
    fsrs_version: i64,
    #[serde(default)]
    created_at: i64,
    #[serde(default)]
    updated_at: i64,
}

impl From<VocabCsvRow> for VocabBackupWord {
    fn from(row: VocabCsvRow) -> Self {
        Self {
            id: row.id,
            book_id: row.book_id,
            word: row.word,
            definition: row.definition,
            context_sentence: row.context_sentence.filter(|value| !value.is_empty()),
            context_explanation: row.context_explanation.filter(|value| !value.is_empty()),
            cfi: row.cfi.filter(|value| !value.is_empty()),
            mastery: row.mastery,
            review_count: row.review_count,
            next_review_at: row.next_review_at,
            review_interval_days: row.review_interval_days,
            last_reviewed_at: row.last_reviewed_at,
            last_review_rating: row.last_review_rating.filter(|value| !value.is_empty()),
            fsrs_stability: row.fsrs_stability,
            fsrs_difficulty: row.fsrs_difficulty,
            fsrs_version: row.fsrs_version,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VocabReviewRating {
    Again,
    Hard,
    Good,
    Easy,
}

impl VocabReviewRating {
    fn as_str(self) -> &'static str {
        match self {
            Self::Again => "again",
            Self::Hard => "hard",
            Self::Good => "good",
            Self::Easy => "easy",
        }
    }
}

fn schedule_review(
    rating: VocabReviewRating,
    stability: Option<f64>,
    difficulty: Option<f64>,
    last_reviewed_at: Option<i64>,
    review_count: i64,
    now: i64,
) -> AppResult<(String, i64, i64, f64, f64)> {
    let now_dt = Utc
        .timestamp_millis_opt(now)
        .single()
        .ok_or_else(|| crate::error::AppError::Other("FSRS_TIME_INVALID".to_string()))?;
    let last_review = last_reviewed_at
        .and_then(|value| Utc.timestamp_millis_opt(value).single())
        .unwrap_or(now_dt);
    let card = Card {
        due: now_dt,
        stability: stability.unwrap_or_default(),
        difficulty: difficulty.unwrap_or_default(),
        elapsed_days: now_dt.signed_duration_since(last_review).num_days().max(0),
        scheduled_days: 0,
        reps: review_count.clamp(0, i32::MAX as i64) as i32,
        lapses: 0,
        state: if stability.is_some() && difficulty.is_some() {
            FsrsState::Review
        } else {
            FsrsState::New
        },
        last_review,
    };
    let fsrs_rating = match rating {
        VocabReviewRating::Again => Rating::Again,
        VocabReviewRating::Hard => Rating::Hard,
        VocabReviewRating::Good => Rating::Good,
        VocabReviewRating::Easy => Rating::Easy,
    };
    let state = FSRS::default().next(card, now_dt, fsrs_rating).card;
    let interval = state.scheduled_days.clamp(0, 36_500);
    let next_review_at = state.due.timestamp_millis();
    let mastery = if interval >= 21 && !matches!(rating, VocabReviewRating::Again) {
        "mastered"
    } else {
        "learning"
    };
    Ok((
        mastery.to_string(),
        interval,
        next_review_at,
        state.stability,
        state.difficulty,
    ))
}

fn validate_mastery(mastery: &str) -> AppResult<()> {
    if matches!(mastery, "new" | "learning" | "mastered") {
        Ok(())
    } else {
        Err(crate::error::AppError::Other(
            "VOCAB_MASTERY_INVALID".to_string(),
        ))
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn add_vocab_word(
    book_id: String,
    word: String,
    definition: String,
    context_sentence: Option<String>,
    context_explanation: Option<String>,
    cfi: Option<String>,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<VocabWord> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp_millis();
    let device = sync.self_device().to_string();

    log::debug!("vocab: add_vocab_word book_id={book_id} word={word:?}");

    // Dedup happens inside the sync transaction so two concurrent adds
    // can't both observe "missing" and insert duplicates. There's no
    // unique index on (book_id, word) — the conn mutex serializes the
    // whole tx, so the second writer's check sees the first writer's
    // committed row.
    let vocab = sync.with_tx(&db, now, |tx, events| {
        let existing: Option<VocabWord> = {
            let mut stmt = tx.prepare(&format!(
                "SELECT {} FROM vocab_words WHERE book_id = ?1 AND word = ?2 COLLATE NOCASE LIMIT 1",
                SELECT_COLS
            ))?;
            let row = stmt
                .query_map(params![book_id, word], row_to_vocab)?
                .next()
                .transpose()?;
            row
        };
        if let Some(existing) = existing {
            // Existing match → no SQL write, no event published. The
            // closure still returns the row so the frontend gets the
            // canonical record.
            return Ok(existing);
        }

        tx.execute(
            "INSERT INTO vocab_words (id, book_id, word, definition, context_sentence, context_explanation, cfi, mastery, review_count, next_review_at, created_at, updated_at, updated_by_device)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'new', 0, NULL, ?8, ?8, ?9)",
            params![id, book_id, word, definition, context_sentence, context_explanation, cfi, now, device],
        )?;
        events.push(EventBody::VocabAdd(VocabPayload {
            id: id.clone(),
            book_id: book_id.clone(),
            word: word.clone(),
            definition: definition.clone(),
            context_sentence: context_sentence.clone(),
            context_explanation: context_explanation.clone(),
            cfi: cfi.clone(),
            mastery: "new".to_string(),
            review_count: 0,
            next_review_at: None,
            review_interval_days: 0,
            last_reviewed_at: None,
            last_review_rating: None,
            fsrs_stability: None,
            fsrs_difficulty: None,
            fsrs_version: 1,
            created_at: Some(now),
        }));
        Ok(VocabWord {
            id: id.clone(),
            book_id: book_id.clone(),
            word: word.clone(),
            definition: definition.clone(),
            context_sentence: context_sentence.clone(),
            context_explanation: context_explanation.clone(),
            cfi: cfi.clone(),
            mastery: "new".to_string(),
            review_count: 0,
            next_review_at: None,
            review_interval_days: 0,
            last_reviewed_at: None,
            last_review_rating: None,
            fsrs_stability: None,
            fsrs_difficulty: None,
            fsrs_version: 1,
            created_at: now,
            updated_at: now,
            book_title: None,
        })
    })?;

    Ok(vocab)
}

#[tauri::command]
pub fn remove_vocab_word(
    id: String,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<()> {
    let now = chrono::Utc::now().timestamp_millis();
    sync.with_tx(&db, now, |tx, events| {
        tx.execute("DELETE FROM vocab_words WHERE id = ?1", params![id])?;
        events.push(EventBody::VocabDelete { id: id.clone() });
        Ok(())
    })
}

pub(crate) fn query_vocab_words(db: &Db, book_id: &str) -> AppResult<Vec<VocabWord>> {
    let conn = db.reader();
    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM vocab_words WHERE book_id = ?1 ORDER BY created_at DESC",
        SELECT_COLS
    ))?;
    let words = stmt
        .query_map(params![book_id], row_to_vocab)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(words)
}

#[tauri::command]
pub fn list_vocab_words(book_id: String, db: State<'_, Db>) -> AppResult<Vec<VocabWord>> {
    query_vocab_words(&db, &book_id)
}

#[tauri::command]
pub fn check_vocab_exists(
    book_id: String,
    word: String,
    db: State<'_, Db>,
) -> AppResult<Option<String>> {
    let conn = db.reader();
    let mut stmt = conn.prepare(
        "SELECT id FROM vocab_words WHERE book_id = ?1 AND word = ?2 COLLATE NOCASE LIMIT 1",
    )?;
    let id: Option<String> = stmt
        .query_map(params![book_id, word], |row| row.get(0))?
        .next()
        .transpose()?;
    Ok(id)
}

#[tauri::command]
pub fn list_all_vocab_words(db: State<'_, Db>) -> AppResult<Vec<VocabWord>> {
    let conn = db.reader();
    let mut stmt = conn.prepare(
        "SELECT v.id, v.book_id, v.word, v.definition, v.context_sentence, v.cfi, v.mastery, v.review_count, v.next_review_at, v.created_at, v.updated_at, v.context_explanation, v.review_interval_days, v.last_reviewed_at, v.last_review_rating, v.fsrs_stability, v.fsrs_difficulty, v.fsrs_version, b.title FROM vocab_words v LEFT JOIN books b ON v.book_id = b.id ORDER BY v.created_at DESC"
    )?;
    let words = stmt
        .query_map([], row_to_vocab_with_book)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(words)
}

#[tauri::command]
pub fn record_vocab_review(
    id: String,
    rating: VocabReviewRating,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<VocabWord> {
    let now = chrono::Utc::now().timestamp_millis();
    let device = sync.self_device().to_string();
    sync.with_tx(&db, now, |tx, events| {
        let current = tx
            .query_row(
                &format!("SELECT {SELECT_COLS} FROM vocab_words WHERE id = ?1"),
                params![id],
                row_to_vocab,
            )
            .map_err(|_| crate::error::AppError::Other("VOCAB_WORD_NOT_FOUND".to_string()))?;
        let (mastery, review_interval_days, next_review_at, stability, difficulty) =
            schedule_review(
                rating,
                current.fsrs_stability,
                current.fsrs_difficulty,
                current.last_reviewed_at,
                current.review_count,
                now,
            )?;
        let review_count = current.review_count.saturating_add(1);
        tx.execute(
            "UPDATE vocab_words
             SET mastery = ?1, review_count = ?2, next_review_at = ?3,
                 review_interval_days = ?4, last_reviewed_at = ?5, last_review_rating = ?6,
                 fsrs_stability = ?7, fsrs_difficulty = ?8, fsrs_version = 1,
                 updated_at = ?5, updated_by_device = ?9
             WHERE id = ?10",
            params![
                mastery,
                review_count,
                next_review_at,
                review_interval_days,
                now,
                rating.as_str(),
                stability,
                difficulty,
                device,
                id,
            ],
        )?;
        events.push(EventBody::VocabMasterySet {
            id: id.clone(),
            mastery,
            next_review_at: Some(next_review_at),
            review_count,
            review_interval_days,
            last_reviewed_at: Some(now),
            last_review_rating: Some(rating.as_str().to_string()),
            fsrs_stability: Some(stability),
            fsrs_difficulty: Some(difficulty),
            fsrs_version: 1,
        });
        tx.query_row(
            &format!("SELECT {SELECT_COLS} FROM vocab_words WHERE id = ?1"),
            params![id],
            row_to_vocab,
        )
        .map_err(Into::into)
    })
}

#[tauri::command]
pub fn update_vocab_mastery(
    id: String,
    mastery: String,
    next_review_at: Option<i64>,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<()> {
    validate_mastery(&mastery)?;
    let now = chrono::Utc::now().timestamp_millis();
    let device = sync.self_device().to_string();
    sync.with_tx(&db, now, |tx, events| {
        let changed = tx.execute(
            "UPDATE vocab_words SET mastery = ?1, next_review_at = ?2, updated_at = ?3, updated_by_device = ?4 WHERE id = ?5",
            params![mastery, next_review_at, now, device, id],
        )?;
        if changed == 0 {
            return Err(crate::error::AppError::Other(
                "VOCAB_WORD_NOT_FOUND".to_string(),
            ));
        }
        // A status change (for example, "start learning") is not a review.
        // Keep the absolute count in the sync event so a future explicit SRS
        // review command can remain idempotent across replay.
        let review = tx
            .query_row(
                "SELECT review_count, review_interval_days, last_reviewed_at, last_review_rating, fsrs_stability, fsrs_difficulty, fsrs_version FROM vocab_words WHERE id = ?1",
                params![id],
                row_to_review_state,
            )
            .map_err(crate::error::AppError::from)?;
        events.push(EventBody::VocabMasterySet {
            id: id.clone(),
            mastery: mastery.clone(),
            next_review_at,
            review_count: review.review_count,
            review_interval_days: review.review_interval_days,
            last_reviewed_at: review.last_reviewed_at,
            last_review_rating: review.last_review_rating,
            fsrs_stability: review.fsrs_stability,
            fsrs_difficulty: review.fsrs_difficulty,
            fsrs_version: review.fsrs_version,
        });
        Ok(())
    })
}

pub(crate) fn query_vocab_due(db: &Db) -> AppResult<Vec<VocabWord>> {
    let conn = db.reader();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM vocab_words WHERE next_review_at IS NOT NULL AND next_review_at <= ?1 ORDER BY next_review_at ASC",
        SELECT_COLS
    ))?;
    let words = stmt
        .query_map(params![now_ms], row_to_vocab)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(words)
}

#[tauri::command]
pub fn list_vocab_due_for_review(db: State<'_, Db>) -> AppResult<Vec<VocabWord>> {
    query_vocab_due(&db)
}

pub(crate) fn query_vocab_stats(db: &Db) -> AppResult<VocabStats> {
    let conn = db.reader();
    let now_ms = chrono::Utc::now().timestamp_millis();
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM vocab_words", [], |r| r.get(0))?;
    let new_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM vocab_words WHERE mastery = 'new'",
        [],
        |r| r.get(0),
    )?;
    let learning_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM vocab_words WHERE mastery = 'learning'",
        [],
        |r| r.get(0),
    )?;
    let mastered_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM vocab_words WHERE mastery = 'mastered'",
        [],
        |r| r.get(0),
    )?;
    let due_for_review: i64 = conn.query_row(
        "SELECT COUNT(*) FROM vocab_words WHERE next_review_at IS NOT NULL AND next_review_at <= ?1",
        params![now_ms],
        |r| r.get(0),
    )?;
    Ok(VocabStats {
        total,
        new_count,
        learning_count,
        mastered_count,
        due_for_review,
    })
}

fn vocab_backup_word(word: VocabWord) -> VocabBackupWord {
    VocabBackupWord {
        id: word.id,
        book_id: word.book_id,
        word: word.word,
        definition: word.definition,
        context_sentence: word.context_sentence,
        context_explanation: word.context_explanation,
        cfi: word.cfi,
        mastery: word.mastery,
        review_count: word.review_count,
        next_review_at: word.next_review_at,
        review_interval_days: word.review_interval_days,
        last_reviewed_at: word.last_reviewed_at,
        last_review_rating: word.last_review_rating,
        fsrs_stability: word.fsrs_stability,
        fsrs_difficulty: word.fsrs_difficulty,
        fsrs_version: word.fsrs_version,
        created_at: word.created_at,
        updated_at: word.updated_at,
    }
}

#[tauri::command]
pub fn export_vocab_backup(db: State<'_, Db>) -> AppResult<VocabBackup> {
    let words = list_all_vocab_words(db)?
        .into_iter()
        .map(vocab_backup_word)
        .collect();
    Ok(VocabBackup {
        schema: VOCAB_BACKUP_SCHEMA.to_string(),
        version: VOCAB_BACKUP_VERSION,
        exported_at: Utc::now().timestamp_millis(),
        words,
    })
}

fn import_error(code: &str) -> crate::error::AppError {
    crate::error::AppError::Other(code.to_string())
}

fn parse_vocab_import(
    data: &str,
    format: VocabImportFormat,
) -> AppResult<(Vec<VocabBackupWord>, usize)> {
    if data.len() > MAX_VOCAB_IMPORT_BYTES {
        return Err(import_error("VOCAB_IMPORT_TOO_LARGE"));
    }
    match format {
        VocabImportFormat::Json => {
            let backup: VocabBackup = serde_json::from_str(data)
                .map_err(|_| import_error("VOCAB_IMPORT_JSON_INVALID"))?;
            if backup.schema != VOCAB_BACKUP_SCHEMA || backup.version != VOCAB_BACKUP_VERSION {
                return Err(import_error("VOCAB_IMPORT_VERSION_UNSUPPORTED"));
            }
            if backup.words.len() > MAX_VOCAB_IMPORT_WORDS {
                return Err(import_error("VOCAB_IMPORT_TOO_MANY_WORDS"));
            }
            Ok((backup.words, 0))
        }
        VocabImportFormat::Csv => {
            let mut reader = csv::ReaderBuilder::new()
                .trim(csv::Trim::All)
                .flexible(false)
                .from_reader(data.as_bytes());
            let mut words = Vec::new();
            let mut invalid_rows = 0;
            for row in reader.deserialize::<VocabCsvRow>() {
                match row {
                    Ok(row)
                        if row.backup_schema == VOCAB_BACKUP_SCHEMA
                            && row.backup_version == VOCAB_BACKUP_VERSION =>
                    {
                        words.push(row.into());
                    }
                    Ok(_) | Err(_) => invalid_rows += 1,
                }
                if words.len().saturating_add(invalid_rows) > MAX_VOCAB_IMPORT_WORDS {
                    return Err(import_error("VOCAB_IMPORT_TOO_MANY_WORDS"));
                }
            }
            Ok((words, invalid_rows))
        }
    }
}

fn validate_import_word(word: &VocabBackupWord) -> bool {
    crate::sync::validation::validate_entity_id(&word.id).is_ok()
        && crate::sync::validation::validate_entity_id(&word.book_id).is_ok()
        && !word.word.trim().is_empty()
        && word.word.len() <= 512
        && word.definition.len() <= 100_000
        && word
            .context_sentence
            .as_ref()
            .is_none_or(|value| value.len() <= 100_000)
        && word
            .context_explanation
            .as_ref()
            .is_none_or(|value| value.len() <= 100_000)
        && word.cfi.as_ref().is_none_or(|value| value.len() <= 16_384)
        && validate_mastery(&word.mastery).is_ok()
        && word.review_count >= 0
        && word.review_interval_days >= 0
        && word.fsrs_version >= 1
        && word
            .fsrs_stability
            .is_none_or(|value| value.is_finite() && value >= 0.0)
        && word
            .fsrs_difficulty
            .is_none_or(|value| value.is_finite() && value >= 0.0)
        && word
            .last_review_rating
            .as_deref()
            .is_none_or(|value| matches!(value, "again" | "hard" | "good" | "easy"))
}

fn preview_vocab_import_words(
    words: &[VocabBackupWord],
    initial_invalid_rows: usize,
    db: &Db,
) -> AppResult<(VocabImportPreview, Vec<VocabBackupWord>)> {
    let conn = db.reader();
    let mut known_books = conn.prepare("SELECT EXISTS(SELECT 1 FROM books WHERE id = ?1)")?;
    let mut known_words = conn.prepare(
        "SELECT id FROM vocab_words WHERE book_id = ?1 AND word = ?2 COLLATE NOCASE LIMIT 1",
    )?;
    let mut seen_ids = HashSet::new();
    let mut seen_words = HashSet::new();
    let mut valid_words = Vec::new();
    let mut preview = VocabImportPreview {
        valid: 0,
        new_words: 0,
        conflicts: 0,
        missing_books: 0,
        duplicate_rows: 0,
        invalid_rows: initial_invalid_rows,
    };

    for word in words {
        if !validate_import_word(word) {
            preview.invalid_rows += 1;
            continue;
        }
        let dedupe_key = format!("{}\u{0}{}", word.book_id, word.word.to_lowercase());
        if !seen_ids.insert(word.id.clone()) || !seen_words.insert(dedupe_key) {
            preview.duplicate_rows += 1;
            continue;
        }
        preview.valid += 1;
        let book_exists: bool = known_books.query_row(params![word.book_id], |row| row.get(0))?;
        if !book_exists {
            preview.missing_books += 1;
            continue;
        }
        let existing: Option<String> = known_words
            .query_row(params![word.book_id, word.word], |row| row.get(0))
            .ok();
        if existing.is_some() {
            preview.conflicts += 1;
        } else {
            preview.new_words += 1;
        }
        valid_words.push(word.clone());
    }
    Ok((preview, valid_words))
}

#[tauri::command]
pub fn preview_vocab_import(
    data: String,
    format: VocabImportFormat,
    db: State<'_, Db>,
) -> AppResult<VocabImportPreview> {
    let (words, invalid_rows) = parse_vocab_import(&data, format)?;
    preview_vocab_import_words(&words, invalid_rows, &db).map(|(preview, _)| preview)
}

pub(crate) fn do_import_vocab_backup(
    data: &str,
    format: VocabImportFormat,
    conflict_policy: VocabImportConflictPolicy,
    dry_run: bool,
    db: &Db,
    sync: &SyncWriter,
) -> AppResult<VocabImportResult> {
    let (words, invalid_rows) = parse_vocab_import(data, format)?;
    let (preview, words) = preview_vocab_import_words(&words, invalid_rows, db)?;
    if dry_run {
        return Ok(VocabImportResult {
            preview,
            imported: 0,
            replaced: 0,
            skipped: 0,
            dry_run: true,
        });
    }

    let timestamp = Utc::now().timestamp_millis();
    let device = sync.self_device().to_string();
    let (imported, replaced, skipped) = sync.with_tx(db, timestamp, |tx, events| {
        let mut imported = 0;
        let mut replaced = 0;
        let mut skipped = preview.invalid_rows + preview.duplicate_rows + preview.missing_books;
        for word in &words {
            let book_exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM books WHERE id = ?1)",
                params![word.book_id],
                |row| row.get(0),
            )?;
            if !book_exists {
                continue;
            }
            let existing: Option<String> = tx
                .query_row(
                    "SELECT id FROM vocab_words WHERE book_id = ?1 AND word = ?2 COLLATE NOCASE LIMIT 1",
                    params![word.book_id, word.word],
                    |row| row.get(0),
                )
                .ok();
            if let Some(existing_id) = existing {
                if matches!(conflict_policy, VocabImportConflictPolicy::Skip) {
                    skipped += 1;
                    continue;
                }
                tx.execute("DELETE FROM vocab_words WHERE id = ?1", params![existing_id])?;
                events.push(EventBody::VocabDelete { id: existing_id });
                replaced += 1;
            }

            let id = uuid::Uuid::new_v4().to_string();
            let created_at = if word.created_at > 0 { word.created_at } else { timestamp };
            tx.execute(
                "INSERT INTO vocab_words (id, book_id, word, definition, context_sentence, context_explanation, cfi, mastery, review_count, next_review_at, review_interval_days, last_reviewed_at, last_review_rating, fsrs_stability, fsrs_difficulty, fsrs_version, created_at, updated_at, updated_by_device)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
                params![
                    id,
                    word.book_id,
                    word.word,
                    word.definition,
                    word.context_sentence,
                    word.context_explanation,
                    word.cfi,
                    word.mastery,
                    word.review_count,
                    word.next_review_at,
                    word.review_interval_days,
                    word.last_reviewed_at,
                    word.last_review_rating,
                    word.fsrs_stability,
                    word.fsrs_difficulty,
                    word.fsrs_version,
                    created_at,
                    timestamp,
                    device,
                ],
            )?;
            events.push(EventBody::VocabAdd(VocabPayload {
                id: id.clone(),
                book_id: word.book_id.clone(),
                word: word.word.clone(),
                definition: word.definition.clone(),
                context_sentence: word.context_sentence.clone(),
                context_explanation: word.context_explanation.clone(),
                cfi: word.cfi.clone(),
                mastery: word.mastery.clone(),
                review_count: word.review_count,
                next_review_at: word.next_review_at,
                review_interval_days: word.review_interval_days,
                last_reviewed_at: word.last_reviewed_at,
                last_review_rating: word.last_review_rating.clone(),
                fsrs_stability: word.fsrs_stability,
                fsrs_difficulty: word.fsrs_difficulty,
                fsrs_version: word.fsrs_version,
                created_at: Some(created_at),
            }));
            imported += 1;
        }
        Ok((imported, replaced, skipped))
    })?;

    Ok(VocabImportResult {
        preview,
        imported,
        replaced,
        skipped,
        dry_run: false,
    })
}

#[tauri::command]
pub fn import_vocab_backup(
    data: String,
    format: VocabImportFormat,
    conflict_policy: VocabImportConflictPolicy,
    dry_run: bool,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<VocabImportResult> {
    do_import_vocab_backup(&data, format, conflict_policy, dry_run, &db, &sync)
}

#[tauri::command]
pub fn bulk_delete_vocab_words(
    ids: Vec<String>,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<usize> {
    let ids: Vec<String> = ids
        .into_iter()
        .filter(|id| crate::sync::validation::validate_entity_id(id).is_ok())
        .collect();
    if ids.is_empty() {
        return Ok(0);
    }
    let timestamp = Utc::now().timestamp_millis();
    sync.with_tx(&db, timestamp, |tx, events| {
        let mut deleted = 0;
        for id in &ids {
            if tx.execute("DELETE FROM vocab_words WHERE id = ?1", params![id])? > 0 {
                events.push(EventBody::VocabDelete { id: id.clone() });
                deleted += 1;
            }
        }
        Ok(deleted)
    })
}

#[tauri::command]
pub fn bulk_update_vocab_mastery(
    ids: Vec<String>,
    mastery: String,
    next_review_at: Option<i64>,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<usize> {
    validate_mastery(&mastery)?;
    let ids: Vec<String> = ids
        .into_iter()
        .filter(|id| crate::sync::validation::validate_entity_id(id).is_ok())
        .collect();
    if ids.is_empty() {
        return Ok(0);
    }
    let timestamp = Utc::now().timestamp_millis();
    let device = sync.self_device().to_string();
    sync.with_tx(&db, timestamp, |tx, events| {
        let mut changed = 0;
        for id in &ids {
            let review = tx
                .query_row(
                    "SELECT review_count, review_interval_days, last_reviewed_at, last_review_rating, fsrs_stability, fsrs_difficulty, fsrs_version FROM vocab_words WHERE id = ?1",
                    params![id],
                    row_to_review_state,
                )
                .ok();
            let Some(review) = review else {
                continue;
            };
            tx.execute(
                "UPDATE vocab_words SET mastery = ?1, next_review_at = ?2, updated_at = ?3, updated_by_device = ?4 WHERE id = ?5",
                params![mastery, next_review_at, timestamp, device, id],
            )?;
            events.push(EventBody::VocabMasterySet {
                id: id.clone(),
                mastery: mastery.clone(),
                next_review_at,
                review_count: review.review_count,
                review_interval_days: review.review_interval_days,
                last_reviewed_at: review.last_reviewed_at,
                last_review_rating: review.last_review_rating,
                fsrs_stability: review.fsrs_stability,
                fsrs_difficulty: review.fsrs_difficulty,
                fsrs_version: review.fsrs_version,
            });
            changed += 1;
        }
        Ok(changed)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::log::EventLog;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn setup_import_db() -> (TempDir, Db, SyncWriter) {
        let dir = TempDir::new().unwrap();
        let db = Db::init(dir.path()).unwrap();
        let writer = SyncWriter::new("dev-import".into());
        writer.set_should_queue(true);
        writer.set_log(Some(Arc::new(
            EventLog::open(
                &dir.path().join("logs/dev-import.jsonl"),
                "dev-import",
                false,
            )
            .unwrap(),
        )));
        writer.set_flush_inline_for_tests(true);
        (dir, db, writer)
    }

    fn insert_import_book(db: &Db, id: &str) {
        let now = 1_700_000_000_000_i64;
        db.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO books (id, title, author, file_path, status, progress, created_at, updated_at)
                 VALUES (?1, 'Import Book', 'Author', 'books/import.epub', 'unread', 0, ?2, ?2)",
                params![id, now],
            )
            .unwrap();
    }

    fn backup_word(id: &str, book_id: &str, word: &str, definition: &str) -> VocabBackupWord {
        VocabBackupWord {
            id: id.to_string(),
            book_id: book_id.to_string(),
            word: word.to_string(),
            definition: definition.to_string(),
            context_sentence: Some("A useful sentence.".to_string()),
            context_explanation: Some("Useful context.".to_string()),
            cfi: Some("epubcfi(/6/2!/4/2)".to_string()),
            mastery: "learning".to_string(),
            review_count: 4,
            next_review_at: Some(1_800_000_000_000),
            review_interval_days: 9,
            last_reviewed_at: Some(1_700_000_000_000),
            last_review_rating: Some("good".to_string()),
            fsrs_stability: Some(12.5),
            fsrs_difficulty: Some(4.3),
            fsrs_version: 1,
            created_at: 1_600_000_000_000,
            updated_at: 1_700_000_000_000,
        }
    }

    fn backup_json(words: Vec<VocabBackupWord>) -> String {
        serde_json::to_string(&VocabBackup {
            schema: VOCAB_BACKUP_SCHEMA.to_string(),
            version: VOCAB_BACKUP_VERSION,
            exported_at: 1_700_000_000_000,
            words,
        })
        .unwrap()
    }

    #[test]
    fn vocab_import_json_preserves_full_srs_state_in_database_and_event() {
        let (dir, db, writer) = setup_import_db();
        insert_import_book(&db, "book-1");
        let word = backup_word(
            "backup-1",
            "book-1",
            "serendipity",
            "A fortunate discovery.",
        );

        let result = do_import_vocab_backup(
            &backup_json(vec![word]),
            VocabImportFormat::Json,
            VocabImportConflictPolicy::Skip,
            false,
            &db,
            &writer,
        )
        .unwrap();

        assert_eq!(result.imported, 1);
        let stored = query_vocab_words(&db, "book-1").unwrap().pop().unwrap();
        assert_eq!(stored.word, "serendipity");
        assert_eq!(stored.mastery, "learning");
        assert_eq!(stored.review_count, 4);
        assert_eq!(stored.next_review_at, Some(1_800_000_000_000));
        assert_eq!(stored.review_interval_days, 9);
        assert_eq!(stored.last_reviewed_at, Some(1_700_000_000_000));
        assert_eq!(stored.last_review_rating.as_deref(), Some("good"));
        assert_eq!(stored.fsrs_stability, Some(12.5));
        assert_eq!(stored.fsrs_difficulty, Some(4.3));
        assert_eq!(stored.fsrs_version, 1);
        assert_eq!(stored.created_at, 1_600_000_000_000);

        let events = EventLog::open(
            &dir.path().join("logs/dev-import.jsonl"),
            "dev-import",
            false,
        )
        .unwrap()
        .read_all()
        .unwrap();
        let EventBody::VocabAdd(payload) = &events[0].body else {
            panic!("expected a VocabAdd event");
        };
        assert_eq!(payload.mastery, stored.mastery);
        assert_eq!(payload.review_count, stored.review_count);
        assert_eq!(payload.next_review_at, stored.next_review_at);
        assert_eq!(payload.review_interval_days, stored.review_interval_days);
        assert_eq!(payload.fsrs_stability, stored.fsrs_stability);
        assert_eq!(payload.fsrs_difficulty, stored.fsrs_difficulty);
        assert_eq!(payload.created_at, Some(stored.created_at));
    }

    #[test]
    fn vocab_import_csv_accepts_a_valid_backup_row() {
        let (_dir, db, writer) = setup_import_db();
        insert_import_book(&db, "book-1");
        let csv = "backup_schema,backup_version,id,book_id,word,definition,context_sentence,context_explanation,cfi,mastery,review_count,next_review_at,review_interval_days,last_reviewed_at,last_review_rating,fsrs_stability,fsrs_difficulty,fsrs_version,created_at,updated_at\nquill-vocabulary,1,backup-1,book-1,ephemeral,Short-lived,A useful sentence.,Useful context.,epubcfi(/6/2!/4/2),learning,4,1800000000000,9,1700000000000,good,12.5,4.3,1,1600000000000,1700000000000\n";

        let result = do_import_vocab_backup(
            csv,
            VocabImportFormat::Csv,
            VocabImportConflictPolicy::Skip,
            false,
            &db,
            &writer,
        )
        .unwrap();

        assert_eq!(result.imported, 1);
        assert_eq!(
            query_vocab_words(&db, "book-1").unwrap()[0].word,
            "ephemeral"
        );
    }

    #[test]
    fn vocab_import_rejects_unsupported_json_schema_version() {
        let (_dir, db, writer) = setup_import_db();
        let invalid = r#"{"schema":"quill-vocabulary","version":99,"exported_at":0,"words":[]}"#;

        let error = do_import_vocab_backup(
            invalid,
            VocabImportFormat::Json,
            VocabImportConflictPolicy::Skip,
            false,
            &db,
            &writer,
        )
        .unwrap_err();

        assert_eq!(error.to_string(), "VOCAB_IMPORT_VERSION_UNSUPPORTED");
    }

    #[test]
    fn vocab_import_reports_missing_books_without_writing_words() {
        let (_dir, db, writer) = setup_import_db();

        let result = do_import_vocab_backup(
            &backup_json(vec![backup_word(
                "backup-1",
                "missing-book",
                "wander",
                "To roam.",
            )]),
            VocabImportFormat::Json,
            VocabImportConflictPolicy::Skip,
            false,
            &db,
            &writer,
        )
        .unwrap();

        assert_eq!(result.preview.missing_books, 1);
        assert_eq!(result.imported, 0);
        assert_eq!(result.skipped, 1);
        let word_count: i64 = db
            .reader()
            .query_row("SELECT COUNT(*) FROM vocab_words", [], |row| row.get(0))
            .unwrap();
        assert_eq!(word_count, 0);
    }

    #[test]
    fn vocab_import_conflict_policy_skips_or_overwrites_existing_word() {
        let (_dir, db, writer) = setup_import_db();
        insert_import_book(&db, "book-1");
        let original = backup_word("backup-1", "book-1", "resolve", "Original definition.");
        do_import_vocab_backup(
            &backup_json(vec![original]),
            VocabImportFormat::Json,
            VocabImportConflictPolicy::Skip,
            false,
            &db,
            &writer,
        )
        .unwrap();

        let replacement = backup_word("backup-2", "book-1", "resolve", "Replacement definition.");
        let skipped = do_import_vocab_backup(
            &backup_json(vec![replacement.clone()]),
            VocabImportFormat::Json,
            VocabImportConflictPolicy::Skip,
            false,
            &db,
            &writer,
        )
        .unwrap();
        assert_eq!(skipped.imported, 0);
        assert_eq!(skipped.skipped, 1);
        assert_eq!(
            query_vocab_words(&db, "book-1").unwrap()[0].definition,
            "Original definition."
        );

        let overwritten = do_import_vocab_backup(
            &backup_json(vec![replacement]),
            VocabImportFormat::Json,
            VocabImportConflictPolicy::Overwrite,
            false,
            &db,
            &writer,
        )
        .unwrap();
        assert_eq!(overwritten.imported, 1);
        assert_eq!(overwritten.replaced, 1);
        let words = query_vocab_words(&db, "book-1").unwrap();
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].definition, "Replacement definition.");
    }

    #[test]
    fn again_returns_word_to_short_learning_interval() {
        let (mastery, interval, due, stability, difficulty) =
            schedule_review(VocabReviewRating::Again, None, None, None, 0, 1_000).unwrap();
        assert_eq!(mastery, "learning");
        assert_eq!(interval, 0);
        assert!(due > 1_000);
        assert!(stability > 0.0 && difficulty > 0.0);
    }

    #[test]
    fn good_grows_interval_and_eventually_marks_mastered() {
        let (_, first, _, stability, difficulty) =
            schedule_review(VocabReviewRating::Good, None, None, None, 0, 1_000).unwrap();
        let (_, second, due, _, _) = schedule_review(
            VocabReviewRating::Good,
            Some(stability),
            Some(difficulty),
            Some(1_000),
            1,
            1_000 + first * DAY_MS,
        )
        .unwrap();
        assert!(second >= first);
        assert!(due > 1_000);
    }
}

#[tauri::command]
pub fn get_vocab_stats(db: State<'_, Db>) -> AppResult<VocabStats> {
    query_vocab_stats(&db)
}
