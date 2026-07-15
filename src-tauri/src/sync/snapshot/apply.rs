use rusqlite::{params, Connection, OptionalExtension, Transaction};
use ulid::Ulid;

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::sync::{merge, validation};

use super::rows::*;
use super::{MIN_SUPPORTED_SNAPSHOT_SCHEMA_VERSION, SNAPSHOT_SCHEMA_VERSION};

use crate::sync::events::Event;

fn is_supported_snapshot_schema_version(version: u32) -> bool {
    (MIN_SUPPORTED_SNAPSHOT_SCHEMA_VERSION..=SNAPSHOT_SCHEMA_VERSION).contains(&version)
}

impl Snapshot {
    /// Build a snapshot from an event sequence. Used by compaction (own log
    /// → own snapshot) and by tests. Folds events into an in-memory DB via
    /// `merge::apply_event`, then dumps the materialized rows back out as
    /// the snapshot state.
    ///
    /// `events` should already be in `(ts, device)` order — the same order
    /// `ReplayEngine::tick()` uses. The snapshot's `id` and
    /// `truncated_before` are set to the lexicographically largest event id
    /// seen (callers that want a more conservative truncation point can
    /// override `truncated_before` on the returned struct).
    pub fn from_events(device: &str, events: &[Event]) -> AppResult<Self> {
        let mut conn = Connection::open_in_memory()?;
        Db::run_migrations_on(&conn)?;
        {
            let tx = conn.transaction()?;
            for ev in events {
                merge::apply_event(&tx, ev)?;
            }
            tx.commit()?;
        }

        let state = dump_state(&conn)?;
        let max_id = events
            .iter()
            .map(|e| e.id.clone())
            .max()
            .unwrap_or_else(|| Ulid::nil().to_string());

        Ok(Snapshot {
            v: SNAPSHOT_SCHEMA_VERSION,
            device: device.to_string(),
            id: max_id.clone(),
            generated_at: chrono::Utc::now().timestamp_millis(),
            truncated_before: Some(max_id),
            state,
        })
    }

    /// Build a snapshot directly from an open quill.db (legacy file-sync
    /// or freshly-migrated local DB). Skips the merge-engine roundtrip
    /// because the DB already holds the materialized state — we just dump
    /// every synced table.
    ///
    /// `id` is freshly minted (no log exists yet — peers will treat this
    /// as a brand-new snapshot id in their `_replay_state` watermarks).
    /// `truncated_before` is `None` so peers don't try to truncate a tail
    /// that doesn't exist on this device.
    ///
    /// Used by `migration::run_migration` to bootstrap the per-device log
    /// from a legacy DB. Caller is responsible for ensuring `conn` has
    /// already been migrated to the current schema (Db::init does this).
    pub fn from_legacy_db(conn: &Connection, device: &str) -> AppResult<Self> {
        let state = dump_state(conn)?;
        let id = Ulid::new().to_string();
        Ok(Snapshot {
            v: SNAPSHOT_SCHEMA_VERSION,
            device: device.to_string(),
            id,
            generated_at: chrono::Utc::now().timestamp_millis(),
            truncated_before: None,
            state,
        })
    }
}

impl Snapshot {
    /// Apply this snapshot into local SQLite. Idempotent under repeated
    /// application; tombstones in `state.tombstones` are written first so
    /// the entity rows that follow can short-circuit on the local-tombstone
    /// check. Watermarks are advanced per Step 6 of the spec.
    ///
    /// `peer_device` is the keyed `_replay_state.peer_device` — usually
    /// `self.device`, but the caller passes it explicitly so this works for
    /// the migration apply-back case (where the snapshot's `device` is the
    /// migrating device but `_replay_state` still treats it as a peer).
    pub fn apply_peer(&self, tx: &Transaction, peer_device: &str) -> AppResult<ApplyOutcome> {
        validation::validate_peer_device(peer_device)?;
        if self.device != peer_device
            || !is_supported_snapshot_schema_version(self.v)
            || self.id.parse::<Ulid>().is_err()
        {
            return Err(AppError::Other(
                "SYNC_SNAPSHOT_ENVELOPE_INVALID".to_string(),
            ));
        }
        // A lower envelope version must not smuggle state introduced by a
        // newer schema. Otherwise an older client can accept the envelope,
        // ignore the unknown field, and advance its watermark past data it
        // never materialized.
        if self.v < 2 && (!self.state.notes.is_empty() || !self.state.word_mark_rules.is_empty()) {
            return Err(AppError::Other(
                "SYNC_SNAPSHOT_ENVELOPE_INVALID".to_string(),
            ));
        }
        if self.v < 3
            && (!self.state.word_mark_exceptions.is_empty()
                || !self.state.lookup_occurrence_marks.is_empty())
        {
            return Err(AppError::Other(
                "SYNC_SNAPSHOT_WORD_MARK_EXCEPTION_INVALID".to_string(),
            ));
        }
        if self.v < 4 && !self.state.book_summaries.is_empty() {
            return Err(AppError::Other(
                "SYNC_SNAPSHOT_BOOK_SUMMARY_INVALID".to_string(),
            ));
        }
        if self.v < 5
            && self
                .state
                .book_summaries
                .values()
                .any(|row| row.user_edited)
        {
            return Err(AppError::Other(
                "SYNC_SNAPSHOT_BOOK_SUMMARY_INVALID".to_string(),
            ));
        }
        let snapshot_id = self
            .id
            .parse::<Ulid>()
            .map_err(|_| AppError::Other("SYNC_SNAPSHOT_ENVELOPE_INVALID".to_string()))?;
        validation::ensure_not_from_far_future(
            self.generated_at,
            "SYNC_SNAPSHOT_ENVELOPE_INVALID",
        )?;
        validation::ensure_not_from_far_future(
            i64::try_from(snapshot_id.timestamp_ms())
                .map_err(|_| AppError::Other("SYNC_SNAPSHOT_ENVELOPE_INVALID".to_string()))?,
            "SYNC_SNAPSHOT_ENVELOPE_INVALID",
        )?;
        for (id, book) in &self.state.books {
            validation::validate_entity_id(id)?;
            validation::validate_book_path(&book.file_path)?;
            if let Some(path) = book.cover_path.as_deref() {
                validation::validate_cover_path(path)?;
            }
        }
        for (id, note) in &self.state.notes {
            validation::validate_note_fields(
                id,
                note.book_id.as_deref(),
                &note.anchor_kind,
                note.normalized_word.as_deref(),
                &note.scope,
                note.location.as_deref(),
                note.selected_text.as_deref(),
                &note.content,
                &note.content_format,
            )?;
            validation::ensure_not_from_far_future(note.created_at, "SYNC_SNAPSHOT_NOTE_INVALID")?;
            validation::ensure_not_from_far_future(note.updated_at, "SYNC_SNAPSHOT_NOTE_INVALID")?;
        }
        for (id, rule) in &self.state.word_mark_rules {
            validation::validate_word_mark_fields(
                id,
                &rule.book_id,
                &rule.normalized_word,
                &rule.display_word,
                &rule.match_mode,
                &rule.color,
            )?;
            validation::ensure_not_from_far_future(
                rule.created_at,
                "SYNC_SNAPSHOT_WORD_MARK_INVALID",
            )?;
            validation::ensure_not_from_far_future(
                rule.updated_at,
                "SYNC_SNAPSHOT_WORD_MARK_INVALID",
            )?;
        }
        for (id, exception) in &self.state.word_mark_exceptions {
            validation::validate_word_mark_exception_fields(
                id,
                &exception.rule_id,
                &exception.book_id,
                &exception.normalized_word,
                &exception.location,
            )?;
            validation::ensure_not_from_far_future(
                exception.created_at,
                "SYNC_SNAPSHOT_WORD_MARK_EXCEPTION_INVALID",
            )?;
            validation::ensure_not_from_far_future(
                exception.updated_at,
                "SYNC_SNAPSHOT_WORD_MARK_EXCEPTION_INVALID",
            )?;
        }
        for (id, mark) in &self.state.lookup_occurrence_marks {
            validation::validate_lookup_occurrence_mark_fields(
                id,
                &mark.book_id,
                &mark.normalized_word,
                &mark.display_word,
                &mark.location,
            )?;
            validation::ensure_not_from_far_future(
                mark.created_at,
                "SYNC_SNAPSHOT_LOOKUP_OCCURRENCE_MARK_INVALID",
            )?;
            validation::ensure_not_from_far_future(
                mark.updated_at,
                "SYNC_SNAPSHOT_LOOKUP_OCCURRENCE_MARK_INVALID",
            )?;
        }
        for (entity, tombstones) in &self.state.tombstones {
            validation::validate_tombstone_entity(entity)?;
            for tombstone in tombstones {
                validation::validate_tombstone_id(entity, &tombstone.id)?;
                validation::ensure_valid_sync_timestamp(
                    tombstone.ts,
                    "SYNC_SNAPSHOT_TOMBSTONE_INVALID",
                )?;
            }
        }
        let prior: Option<(Option<String>, Option<String>)> = tx
            .query_row(
                "SELECT last_snapshot_id, last_event_id
                 FROM _replay_state WHERE peer_device = ?1",
                params![peer_device],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;

        let prior_snap = prior.as_ref().and_then(|(s, _)| s.clone());
        let prior_event = prior.as_ref().and_then(|(_, e)| e.clone());

        if prior_snap.as_deref() == Some(self.id.as_str()) {
            return Ok(ApplyOutcome::AlreadyApplied);
        }

        // Spec step 4b: snapshot summarises events we've already individually
        // applied — skip rows but bump last_snapshot_id so we don't re-parse.
        if prior_event
            .as_deref()
            .is_some_and(|e| e >= self.id.as_str())
        {
            upsert_replay_state(tx, peer_device, Some(&self.id), prior_event.as_deref())?;
            return Ok(ApplyOutcome::HeaderOnly);
        }

        // Spec step 5: tombstones first. Route every entry through
        // `merge::cascade_delete` so a snapshot ingest scrubs the same
        // child rows that the corresponding event-path delete would have
        // — otherwise applying a peer snapshot leaves orphan highlights
        // for a deleted book, stranded `collection_books` joins for a
        // removed pair, etc.
        for (entity, list) in &self.state.tombstones {
            for t in list {
                if entity == merge::entity::WORD_MARK {
                    let newer_rule_exists: bool = tx.query_row(
                        "SELECT EXISTS(
                           SELECT 1 FROM word_mark_rules
                           WHERE id = ?1 AND updated_at > ?2
                         )",
                        params![t.id, t.ts],
                        |row| row.get(0),
                    )?;
                    if newer_rule_exists {
                        // Unlike permanent entity deletion, the legacy word
                        // marker tombstone can be superseded by a newer full
                        // upsert. Do not let an old peer snapshot disable that
                        // already-materialized newer state.
                        continue;
                    }
                }
                merge::cascade_delete(tx, entity, &t.id, t.ts)?;
                merge::insert_tombstone(tx, entity, &t.id, t.ts)?;
            }
        }

        for (id, row) in &self.state.books {
            if merge::is_tombstoned(tx, merge::entity::BOOK, id)? {
                continue;
            }
            upsert_book(tx, id, row)?;
        }
        for (id, row) in &self.state.highlights {
            if merge::is_tombstoned(tx, merge::entity::HIGHLIGHT, id)?
                || merge::is_tombstoned(tx, merge::entity::BOOK, &row.book_id)?
            {
                continue;
            }
            upsert_highlight(tx, id, row)?;
        }
        for (id, row) in &self.state.bookmarks {
            if merge::is_tombstoned(tx, merge::entity::BOOKMARK, id)?
                || merge::is_tombstoned(tx, merge::entity::BOOK, &row.book_id)?
            {
                continue;
            }
            insert_bookmark(tx, id, row)?;
        }
        for (id, row) in &self.state.vocab_words {
            if merge::is_tombstoned(tx, merge::entity::VOCAB, id)?
                || merge::is_tombstoned(tx, merge::entity::BOOK, &row.book_id)?
            {
                continue;
            }
            upsert_vocab(tx, id, row)?;
        }
        for (id, row) in &self.state.notes {
            if merge::is_tombstoned(tx, merge::entity::NOTE, id)? {
                continue;
            }
            if row.scope == "book" {
                if let Some(book_id) = row.book_id.as_deref() {
                    if merge::is_tombstoned(tx, merge::entity::BOOK, book_id)? {
                        continue;
                    }
                }
            }
            upsert_note(tx, id, row)?;
        }
        for (id, row) in &self.state.word_mark_rules {
            if merge::is_tombstoned(tx, merge::entity::BOOK, &row.book_id)? {
                continue;
            }
            upsert_word_mark(tx, id, row)?;
        }
        for (id, row) in &self.state.word_mark_exceptions {
            if merge::is_tombstoned(tx, merge::entity::BOOK, &row.book_id)? {
                continue;
            }
            upsert_word_mark_exception(tx, id, row)?;
        }
        for (id, row) in &self.state.lookup_occurrence_marks {
            if merge::is_tombstoned(tx, merge::entity::BOOK, &row.book_id)? {
                continue;
            }
            upsert_lookup_occurrence_mark(tx, id, row)?;
        }
        for (id, row) in &self.state.book_summaries {
            if merge::is_tombstoned(tx, merge::entity::BOOK, &row.book_id)? {
                continue;
            }
            upsert_book_summary(tx, id, row)?;
        }
        for (id, row) in &self.state.collections {
            if merge::is_tombstoned(tx, merge::entity::COLLECTION, id)? {
                continue;
            }
            upsert_collection(tx, id, row)?;
        }
        for (key, row) in &self.state.collection_books {
            if merge::is_tombstoned(tx, merge::entity::COLLECTION_BOOK, key)?
                || merge::is_tombstoned(tx, merge::entity::COLLECTION, &row.collection_id)?
                || merge::is_tombstoned(tx, merge::entity::BOOK, &row.book_id)?
            {
                continue;
            }
            upsert_collection_book(tx, row)?;
        }
        for (id, row) in &self.state.chats {
            if merge::is_tombstoned(tx, merge::entity::CHAT, id)? {
                continue;
            }
            if merge::is_tombstoned(tx, merge::entity::BOOK, &row.book_id)? {
                // Mirror apply_chat_create: a suppressed chat needs its own
                // tombstone because chat messages only carry chat_id and
                // cannot consult the deleted parent book. `created_at` is the
                // original chat.create event time represented by the row.
                merge::insert_tombstone(tx, merge::entity::CHAT, id, row.created_at)?;
                continue;
            }
            upsert_chat(tx, id, row)?;
        }
        for (id, row) in &self.state.chat_messages {
            if merge::is_tombstoned(tx, merge::entity::CHAT_MESSAGE, id)?
                || merge::is_tombstoned(tx, merge::entity::CHAT, &row.chat_id)?
            {
                continue;
            }
            insert_chat_message(tx, id, row)?;
        }

        // Watermarks: last_snapshot_id moves to this snapshot's id;
        // last_event_id is monotonic — never decrease.
        let new_event_id = match prior_event.as_deref() {
            Some(prev) if prev > self.id.as_str() => prior_event.clone(),
            _ => Some(self.id.clone()),
        };
        upsert_replay_state(tx, peer_device, Some(&self.id), new_event_id.as_deref())?;
        Ok(ApplyOutcome::Applied)
    }
}
// ---------------------------------------------------------------------------
// Per-table upserts. INSERT … ON CONFLICT … DO UPDATE WHERE pattern: insert if
// new, otherwise let LWW decide. Append-only tables use INSERT OR IGNORE
// (their rows are immutable post-creation).
// ---------------------------------------------------------------------------

pub(super) fn upsert_book(tx: &Transaction, id: &str, r: &BookRow) -> AppResult<()> {
    tx.execute(
        "INSERT INTO books
         (id, title, author, description, cover_path, file_path, genre, pages,
          format, source_format, render_format, source_file_path, source_sha256, conversion_version, preparation_state, preparation_error, status, progress, current_cfi, created_at, updated_at, updated_by_device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                 CASE WHEN COALESCE(?11, ?9) = 'text' THEN 'pending' ELSE 'ready' END, NULL,
                 ?15, ?16, ?17, ?18, ?19, ?20)
         ON CONFLICT(id) DO UPDATE SET
           title=excluded.title,
           author=excluded.author,
           description=excluded.description,
           cover_path=excluded.cover_path,
           file_path=excluded.file_path,
           genre=excluded.genre,
           pages=excluded.pages,
           format=excluded.format,
           source_format=excluded.source_format,
           render_format=excluded.render_format,
           source_file_path=excluded.source_file_path,
           source_sha256=excluded.source_sha256,
           conversion_version=excluded.conversion_version,
           preparation_state=CASE
             WHEN excluded.render_format IS NOT 'text' THEN 'ready'
             WHEN excluded.source_format IS NOT books.source_format
               OR excluded.render_format IS NOT books.render_format
               OR excluded.source_file_path IS NOT books.source_file_path
               OR excluded.source_sha256 IS NOT books.source_sha256
               OR excluded.conversion_version IS NOT books.conversion_version
             THEN 'pending'
             ELSE books.preparation_state
           END,
           preparation_error=CASE
             WHEN excluded.render_format IS NOT 'text' THEN NULL
             WHEN excluded.source_format IS NOT books.source_format
               OR excluded.render_format IS NOT books.render_format
               OR excluded.source_file_path IS NOT books.source_file_path
               OR excluded.source_sha256 IS NOT books.source_sha256
               OR excluded.conversion_version IS NOT books.conversion_version
             THEN NULL
             ELSE books.preparation_error
           END,
           status=excluded.status,
           progress=excluded.progress,
           current_cfi=excluded.current_cfi,
           updated_at=excluded.updated_at,
           updated_by_device=excluded.updated_by_device
         WHERE (books.updated_at, books.updated_by_device)
             < (excluded.updated_at, excluded.updated_by_device)",
        params![
            id, r.title, r.author, r.description, r.cover_path, r.file_path,
            r.genre, r.pages, r.format, r.source_format, r.render_format,
            r.source_file_path, r.source_sha256, r.conversion_version,
            r.status, r.progress, r.current_cfi, r.created_at, r.updated_at, r.updated_by_device,
        ],
    )?;
    Ok(())
}

fn upsert_highlight(tx: &Transaction, id: &str, r: &HighlightRow) -> AppResult<()> {
    tx.execute(
        "INSERT INTO highlights
         (id, book_id, cfi_range, color, note, text_content,
          created_at, updated_at, updated_by_device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET
           color=excluded.color,
           note=excluded.note,
           text_content=excluded.text_content,
           updated_at=excluded.updated_at,
           updated_by_device=excluded.updated_by_device
         WHERE (highlights.updated_at, highlights.updated_by_device)
             < (excluded.updated_at, excluded.updated_by_device)",
        params![
            id,
            r.book_id,
            r.cfi_range,
            r.color,
            r.note,
            r.text_content,
            r.created_at,
            r.updated_at,
            r.updated_by_device,
        ],
    )?;
    Ok(())
}

fn insert_bookmark(tx: &Transaction, id: &str, r: &BookmarkRow) -> AppResult<()> {
    tx.execute(
        "INSERT OR IGNORE INTO bookmarks
         (id, book_id, cfi, label, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, r.book_id, r.cfi, r.label, r.created_at, r.updated_at],
    )?;
    Ok(())
}

fn upsert_vocab(tx: &Transaction, id: &str, r: &VocabRow) -> AppResult<()> {
    tx.execute(
        "INSERT INTO vocab_words
         (id, book_id, word, definition, context_sentence, context_explanation, cfi,
          mastery, review_count, next_review_at,
          review_interval_days, last_reviewed_at, last_review_rating,
          fsrs_stability, fsrs_difficulty, fsrs_version,
          created_at, updated_at, updated_by_device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
         ON CONFLICT(id) DO UPDATE SET
           mastery=excluded.mastery,
           review_count=excluded.review_count,
           next_review_at=excluded.next_review_at,
           review_interval_days=excluded.review_interval_days,
           last_reviewed_at=excluded.last_reviewed_at,
           last_review_rating=excluded.last_review_rating,
           fsrs_stability=excluded.fsrs_stability,
           fsrs_difficulty=excluded.fsrs_difficulty,
           fsrs_version=excluded.fsrs_version,
           updated_at=excluded.updated_at,
           updated_by_device=excluded.updated_by_device
         WHERE (vocab_words.updated_at, vocab_words.updated_by_device)
             < (excluded.updated_at, excluded.updated_by_device)",
        params![
            id,
            r.book_id,
            r.word,
            r.definition,
            r.context_sentence,
            r.context_explanation,
            r.cfi,
            r.mastery,
            r.review_count,
            r.next_review_at,
            r.review_interval_days,
            r.last_reviewed_at,
            r.last_review_rating,
            r.fsrs_stability,
            r.fsrs_difficulty,
            r.fsrs_version,
            r.created_at,
            r.updated_at,
            r.updated_by_device,
        ],
    )?;
    Ok(())
}

fn upsert_note(tx: &Transaction, id: &str, r: &NoteRow) -> AppResult<()> {
    let book_id = match r.book_id.as_deref() {
        Some(book_id) if merge::is_tombstoned(tx, merge::entity::BOOK, book_id)? => None,
        value => value,
    };
    if book_id.is_none() && r.book_id.is_some() {
        // Detachment is an invariant imposed by the parent tombstone, not a
        // competing note edit. Enforce it even when the local note has a newer
        // LWW tuple (for example after replaying data from an older client).
        tx.execute("UPDATE notes SET book_id = NULL WHERE id = ?1", params![id])?;
    }
    tx.execute(
        "INSERT INTO notes
         (id, book_id, anchor_kind, normalized_word, scope, location, selected_text,
          content, content_format, created_at, updated_at, updated_by_device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(id) DO UPDATE SET
           book_id=excluded.book_id,
           anchor_kind=excluded.anchor_kind,
           normalized_word=excluded.normalized_word,
           scope=excluded.scope,
           location=excluded.location,
           selected_text=excluded.selected_text,
           content=excluded.content,
           content_format=excluded.content_format,
           updated_at=excluded.updated_at,
           updated_by_device=excluded.updated_by_device
         WHERE (notes.updated_at, notes.updated_by_device)
             < (excluded.updated_at, excluded.updated_by_device)",
        params![
            id,
            book_id,
            r.anchor_kind,
            r.normalized_word,
            r.scope,
            r.location,
            r.selected_text,
            r.content,
            r.content_format,
            r.created_at,
            r.updated_at,
            r.updated_by_device,
        ],
    )?;
    Ok(())
}

fn upsert_word_mark(tx: &Transaction, id: &str, r: &WordMarkRow) -> AppResult<()> {
    if merge::tombstone_timestamp(tx, merge::entity::WORD_MARK, id)?
        .is_some_and(|deleted_at| deleted_at >= r.updated_at)
    {
        return Ok(());
    }
    tx.execute(
        "DELETE FROM _tombstones WHERE entity = ?1 AND id = ?2",
        params![merge::entity::WORD_MARK, id],
    )?;
    let prior_rule_id: Option<String> = tx
        .query_row(
            "SELECT id FROM word_mark_rules
             WHERE book_id = ?1 AND normalized_word = ?2 AND match_mode = ?3",
            params![r.book_id, r.normalized_word, r.match_mode],
            |row| row.get(0),
        )
        .optional()?;
    let changed = tx.execute(
        "INSERT INTO word_mark_rules
         (id, book_id, normalized_word, display_word, match_mode, color, enabled,
          created_at, updated_at, updated_by_device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(book_id, normalized_word, match_mode) DO UPDATE SET
           id=excluded.id,
           display_word=excluded.display_word,
           color=excluded.color,
           enabled=excluded.enabled,
           updated_at=excluded.updated_at,
           updated_by_device=excluded.updated_by_device
         WHERE (word_mark_rules.updated_at, word_mark_rules.updated_by_device)
             < (excluded.updated_at, excluded.updated_by_device)",
        params![
            id,
            r.book_id,
            r.normalized_word,
            r.display_word,
            r.match_mode,
            r.color,
            r.enabled as i64,
            r.created_at,
            r.updated_at,
            r.updated_by_device,
        ],
    )?;
    let repaired_legacy_id = prior_rule_id
        .as_deref()
        .is_some_and(|prior_id| prior_id != id);
    if repaired_legacy_id {
        tx.execute(
            "UPDATE word_mark_rules SET id = ?1
             WHERE book_id = ?2 AND normalized_word = ?3 AND match_mode = ?4",
            params![id, r.book_id, r.normalized_word, r.match_mode],
        )?;
        let (effective_ts, effective_device): (i64, String) = tx.query_row(
            "SELECT updated_at, updated_by_device FROM word_mark_rules WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        merge::reconcile_legacy_word_mark_exceptions(
            tx,
            prior_rule_id.as_deref().expect("legacy id checked above"),
            id,
            &r.book_id,
            &r.normalized_word,
            effective_ts,
            &effective_device,
            false,
        )?;
    } else if changed > 0 {
        tx.execute(
            "UPDATE word_mark_exceptions
             SET excluded = 0, updated_at = ?2, updated_by_device = ?3
             WHERE rule_id = ?1
               AND (updated_at < ?2 OR (updated_at = ?2 AND updated_by_device < ?3))",
            params![id, r.updated_at, r.updated_by_device],
        )?;
    }
    Ok(())
}

fn upsert_word_mark_exception(
    tx: &Transaction,
    id: &str,
    r: &WordMarkExceptionRow,
) -> AppResult<()> {
    let parent_tuple = tx
        .query_row(
            "SELECT updated_at, updated_by_device FROM word_mark_rules WHERE id = ?1",
            params![r.rule_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let (excluded, updated_at, updated_by_device) = match parent_tuple {
        Some((ts, device))
            if (ts, device.as_str()) > (r.updated_at, r.updated_by_device.as_str()) =>
        {
            (false, ts, device)
        }
        _ => (r.excluded, r.updated_at, r.updated_by_device.clone()),
    };
    tx.execute(
        "INSERT INTO word_mark_exceptions
         (id, rule_id, book_id, normalized_word, location, excluded,
          created_at, updated_at, updated_by_device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(rule_id, location) DO UPDATE SET
           id=excluded.id, book_id=excluded.book_id,
           normalized_word=excluded.normalized_word,
           excluded=excluded.excluded, updated_at=excluded.updated_at,
           updated_by_device=excluded.updated_by_device
         WHERE (word_mark_exceptions.updated_at, word_mark_exceptions.updated_by_device)
             < (excluded.updated_at, excluded.updated_by_device)",
        params![
            id,
            r.rule_id,
            r.book_id,
            r.normalized_word,
            r.location,
            excluded as i64,
            r.created_at,
            updated_at,
            updated_by_device,
        ],
    )?;
    Ok(())
}

fn upsert_lookup_occurrence_mark(
    tx: &Transaction,
    id: &str,
    row: &LookupOccurrenceMarkRow,
) -> AppResult<()> {
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
            id,
            row.book_id,
            row.normalized_word,
            row.display_word,
            row.location,
            row.enabled as i64,
            row.created_at,
            row.updated_at,
            row.updated_by_device,
        ],
    )?;
    Ok(())
}

fn upsert_collection(tx: &Transaction, id: &str, r: &CollectionRow) -> AppResult<()> {
    tx.execute(
        "INSERT INTO collections
         (id, name, sort_order, created_at, updated_at, updated_by_device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET
           name=excluded.name,
           sort_order=excluded.sort_order,
           updated_at=excluded.updated_at,
           updated_by_device=excluded.updated_by_device
         WHERE (collections.updated_at, collections.updated_by_device)
             < (excluded.updated_at, excluded.updated_by_device)",
        params![
            id,
            r.name,
            r.sort_order,
            r.created_at,
            r.updated_at,
            r.updated_by_device
        ],
    )?;
    Ok(())
}

fn upsert_collection_book(tx: &Transaction, r: &CollectionBookRow) -> AppResult<()> {
    tx.execute(
        "INSERT INTO collection_books
         (collection_id, book_id, created_at, updated_at, updated_by_device)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(collection_id, book_id) DO UPDATE SET
           updated_at=excluded.updated_at,
           updated_by_device=excluded.updated_by_device
         WHERE (collection_books.updated_at, collection_books.updated_by_device)
             < (excluded.updated_at, excluded.updated_by_device)",
        params![
            r.collection_id,
            r.book_id,
            r.created_at,
            r.updated_at,
            r.updated_by_device
        ],
    )?;
    Ok(())
}

fn upsert_book_summary(tx: &Transaction, id: &str, r: &BookSummaryRow) -> AppResult<()> {
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
            id,
            r.book_id,
            r.scope,
            r.section_index,
            r.section_title,
            r.content,
            r.language,
            r.model,
            r.source_sha256,
            r.created_at,
            r.updated_at,
            r.user_edited as i64,
        ],
    )?;
    Ok(())
}

fn upsert_chat(tx: &Transaction, id: &str, r: &ChatRow) -> AppResult<()> {
    tx.execute(
        "INSERT INTO chats
         (id, book_id, title, model, pinned, metadata, created_at, updated_at, updated_by_device)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET
           title=excluded.title,
           model=excluded.model,
           pinned=excluded.pinned,
           metadata=excluded.metadata,
           updated_at=excluded.updated_at,
           updated_by_device=excluded.updated_by_device
         WHERE (chats.updated_at, chats.updated_by_device)
             < (excluded.updated_at, excluded.updated_by_device)",
        params![
            id,
            r.book_id,
            r.title,
            r.model,
            r.pinned as i64,
            r.metadata,
            r.created_at,
            r.updated_at,
            r.updated_by_device,
        ],
    )?;
    Ok(())
}

fn insert_chat_message(tx: &Transaction, id: &str, r: &ChatMessageRow) -> AppResult<()> {
    tx.execute(
        "INSERT OR IGNORE INTO chat_messages
         (id, chat_id, role, content, context, metadata, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            id,
            r.chat_id,
            r.role,
            r.content,
            r.context,
            r.metadata,
            r.created_at,
            r.updated_at
        ],
    )?;
    Ok(())
}

fn upsert_replay_state(
    tx: &Transaction,
    peer: &str,
    last_snapshot: Option<&str>,
    last_event: Option<&str>,
) -> AppResult<()> {
    let now = chrono::Utc::now().timestamp_millis();
    tx.execute(
        "INSERT INTO _replay_state (peer_device, last_snapshot_id, last_event_id, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(peer_device) DO UPDATE SET
           last_snapshot_id = excluded.last_snapshot_id,
           last_event_id = excluded.last_event_id,
           updated_at = excluded.updated_at",
        params![peer, last_snapshot, last_event, now],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// State dump — read every synced table out of `conn` into a SnapshotState.
// Used by `Snapshot::from_events`.
// ---------------------------------------------------------------------------

pub(super) fn dump_state(conn: &Connection) -> AppResult<SnapshotState> {
    let mut state = SnapshotState::default();

    // books — cover_data excluded from snapshots; covers sync via .img files
    let mut stmt = conn.prepare(
        "SELECT id, title, author, description, cover_path, file_path, genre, pages,
                format, source_format, render_format, source_file_path, source_sha256, conversion_version, status, progress, current_cfi,
                created_at, updated_at, updated_by_device
         FROM books",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            BookRow {
                title: r.get(1)?,
                author: r.get(2)?,
                description: r.get(3)?,
                cover_path: r.get(4)?,
                file_path: r.get(5)?,
                genre: r.get(6)?,
                pages: r.get(7)?,
                format: r.get(8)?,
                source_format: r.get(9)?,
                render_format: r.get(10)?,
                source_file_path: r.get(11)?,
                source_sha256: r.get(12)?,
                conversion_version: r.get::<_, Option<i32>>(13)?.unwrap_or(0),
                status: r.get(14)?,
                progress: r.get(15)?,
                current_cfi: r.get(16)?,
                created_at: r.get(17)?,
                updated_at: r.get(18)?,
                updated_by_device: r.get(19)?,
                cover_data: None,
            },
        ))
    })?;
    for row in rows {
        let (id, b) = row?;
        state.books.insert(id, b);
    }
    drop(stmt);

    // highlights
    let mut stmt = conn.prepare(
        "SELECT id, book_id, cfi_range, color, note, text_content,
                created_at, updated_at, updated_by_device FROM highlights",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            HighlightRow {
                book_id: r.get(1)?,
                cfi_range: r.get(2)?,
                color: r.get(3)?,
                note: r.get(4)?,
                text_content: r.get(5)?,
                created_at: r.get(6)?,
                updated_at: r.get(7)?,
                updated_by_device: r.get(8)?,
            },
        ))
    })?;
    for row in rows {
        let (id, h) = row?;
        state.highlights.insert(id, h);
    }
    drop(stmt);

    // bookmarks
    let mut stmt =
        conn.prepare("SELECT id, book_id, cfi, label, created_at, updated_at FROM bookmarks")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            BookmarkRow {
                book_id: r.get(1)?,
                cfi: r.get(2)?,
                label: r.get(3)?,
                created_at: r.get(4)?,
                updated_at: r.get(5)?,
            },
        ))
    })?;
    for row in rows {
        let (id, b) = row?;
        state.bookmarks.insert(id, b);
    }
    drop(stmt);

    // vocab_words
    let mut stmt = conn.prepare(
        "SELECT id, book_id, word, definition, context_sentence, cfi,
                mastery, review_count, next_review_at, review_interval_days,
                last_reviewed_at, last_review_rating,
                fsrs_stability, fsrs_difficulty, fsrs_version,
                created_at, updated_at, updated_by_device, context_explanation FROM vocab_words",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            VocabRow {
                book_id: r.get(1)?,
                word: r.get(2)?,
                definition: r.get(3)?,
                context_sentence: r.get(4)?,
                cfi: r.get(5)?,
                mastery: r.get(6)?,
                review_count: r.get(7)?,
                next_review_at: r.get(8)?,
                review_interval_days: r.get(9)?,
                last_reviewed_at: r.get(10)?,
                last_review_rating: r.get(11)?,
                fsrs_stability: r.get(12)?,
                fsrs_difficulty: r.get(13)?,
                fsrs_version: r.get(14)?,
                created_at: r.get(15)?,
                updated_at: r.get(16)?,
                updated_by_device: r.get(17)?,
                context_explanation: r.get(18)?,
            },
        ))
    })?;
    for row in rows {
        let (id, v) = row?;
        state.vocab_words.insert(id, v);
    }
    drop(stmt);

    // notes
    let mut stmt = conn.prepare(
        "SELECT id, book_id, anchor_kind, normalized_word, scope, location,
                selected_text, content, content_format, created_at, updated_at,
                updated_by_device FROM notes",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            NoteRow {
                book_id: r.get(1)?,
                anchor_kind: r.get(2)?,
                normalized_word: r.get(3)?,
                scope: r.get(4)?,
                location: r.get(5)?,
                selected_text: r.get(6)?,
                content: r.get(7)?,
                content_format: r.get(8)?,
                created_at: r.get(9)?,
                updated_at: r.get(10)?,
                updated_by_device: r.get(11)?,
            },
        ))
    })?;
    for row in rows {
        let (id, note) = row?;
        state.notes.insert(id, note);
    }
    drop(stmt);

    // whole-book automatic word markers
    let mut stmt = conn.prepare(
        "SELECT id, book_id, normalized_word, display_word, match_mode, color,
                enabled, created_at, updated_at, updated_by_device
         FROM word_mark_rules",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            WordMarkRow {
                book_id: r.get(1)?,
                normalized_word: r.get(2)?,
                display_word: r.get(3)?,
                match_mode: r.get(4)?,
                color: r.get(5)?,
                enabled: r.get::<_, i64>(6)? != 0,
                created_at: r.get(7)?,
                updated_at: r.get(8)?,
                updated_by_device: r.get(9)?,
            },
        ))
    })?;
    for row in rows {
        let (id, mark) = row?;
        state.word_mark_rules.insert(id, mark);
    }
    drop(stmt);

    // per-location exclusions from automatic whole-book markers
    let mut stmt = conn.prepare(
        "SELECT id, rule_id, book_id, normalized_word, location, excluded,
                created_at, updated_at, updated_by_device
         FROM word_mark_exceptions",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            WordMarkExceptionRow {
                rule_id: r.get(1)?,
                book_id: r.get(2)?,
                normalized_word: r.get(3)?,
                location: r.get(4)?,
                excluded: r.get::<_, i64>(5)? != 0,
                created_at: r.get(6)?,
                updated_at: r.get(7)?,
                updated_by_device: r.get(8)?,
            },
        ))
    })?;
    for row in rows {
        let (id, exception) = row?;
        state.word_mark_exceptions.insert(id, exception);
    }
    drop(stmt);

    // one-location automatic marks created by successful lookups
    let mut stmt = conn.prepare(
        "SELECT id, book_id, normalized_word, display_word, location, enabled,
                created_at, updated_at, updated_by_device
         FROM lookup_occurrence_marks",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            LookupOccurrenceMarkRow {
                book_id: r.get(1)?,
                normalized_word: r.get(2)?,
                display_word: r.get(3)?,
                location: r.get(4)?,
                enabled: r.get::<_, i64>(5)? != 0,
                created_at: r.get(6)?,
                updated_at: r.get(7)?,
                updated_by_device: r.get(8)?,
            },
        ))
    })?;
    for row in rows {
        let (id, mark) = row?;
        state.lookup_occurrence_marks.insert(id, mark);
    }
    drop(stmt);

    // Synced summaries. Device-local chunks/index state are intentionally not
    // enumerated here; see docs/impls/1-grounded-book-chat-overview.md D2.
    let mut stmt = conn.prepare(
        "SELECT id, book_id, scope, section_index, section_title, content, language, model,
                source_sha256, created_at, updated_at, user_edited FROM book_summaries",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            BookSummaryRow {
                book_id: r.get(1)?,
                scope: r.get(2)?,
                section_index: r.get(3)?,
                section_title: r.get(4)?,
                content: r.get(5)?,
                language: r.get(6)?,
                model: r.get(7)?,
                source_sha256: r.get(8)?,
                created_at: r.get(9)?,
                updated_at: r.get(10)?,
                user_edited: r.get::<_, i64>(11)? != 0,
            },
        ))
    })?;
    for row in rows {
        let (id, summary) = row?;
        state.book_summaries.insert(id, summary);
    }
    drop(stmt);

    // collections
    let mut stmt = conn.prepare(
        "SELECT id, name, sort_order, created_at, updated_at, updated_by_device FROM collections",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            CollectionRow {
                name: r.get(1)?,
                sort_order: r.get(2)?,
                created_at: r.get(3)?,
                updated_at: r.get(4)?,
                updated_by_device: r.get(5)?,
            },
        ))
    })?;
    for row in rows {
        let (id, c) = row?;
        state.collections.insert(id, c);
    }
    drop(stmt);

    // collection_books — composite key
    let mut stmt = conn.prepare(
        "SELECT collection_id, book_id, created_at, updated_at, updated_by_device
         FROM collection_books",
    )?;
    let rows = stmt.query_map([], |r| {
        let col: String = r.get(0)?;
        let book: String = r.get(1)?;
        Ok((
            format!("{col}:{book}"),
            CollectionBookRow {
                collection_id: col,
                book_id: book,
                created_at: r.get(2)?,
                updated_at: r.get(3)?,
                updated_by_device: r.get(4)?,
            },
        ))
    })?;
    for row in rows {
        let (key, cb) = row?;
        state.collection_books.insert(key, cb);
    }
    drop(stmt);

    // chats
    let mut stmt = conn.prepare(
        "SELECT id, book_id, title, model, pinned, metadata,
                created_at, updated_at, updated_by_device FROM chats",
    )?;
    let rows = stmt.query_map([], |r| {
        let pinned: i64 = r.get(4)?;
        Ok((
            r.get::<_, String>(0)?,
            ChatRow {
                book_id: r.get(1)?,
                title: r.get(2)?,
                model: r.get(3)?,
                pinned: pinned != 0,
                metadata: r.get(5)?,
                created_at: r.get(6)?,
                updated_at: r.get(7)?,
                updated_by_device: r.get(8)?,
            },
        ))
    })?;
    for row in rows {
        let (id, c) = row?;
        state.chats.insert(id, c);
    }
    drop(stmt);

    // chat_messages
    let mut stmt = conn.prepare(
        "SELECT id, chat_id, role, content, context, metadata, created_at, updated_at
         FROM chat_messages",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            ChatMessageRow {
                chat_id: r.get(1)?,
                role: r.get(2)?,
                content: r.get(3)?,
                context: r.get(4)?,
                metadata: r.get(5)?,
                created_at: r.get(6)?,
                updated_at: r.get(7)?,
            },
        ))
    })?;
    for row in rows {
        let (id, m) = row?;
        state.chat_messages.insert(id, m);
    }
    drop(stmt);

    // _tombstones
    let mut stmt = conn.prepare("SELECT entity, id, ts FROM _tombstones")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            TombstoneRow {
                id: r.get(1)?,
                ts: r.get(2)?,
            },
        ))
    })?;
    for row in rows {
        let (entity, t) = row?;
        state.tombstones.entry(entity).or_default().push(t);
    }

    Ok(state)
}
