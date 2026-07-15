use super::text_prepare::{
    legacy_prepared_document_path, prepared_document_backup_path, prepared_document_path,
    prepared_document_temporary_path,
};
use super::*;

const MAX_CUSTOM_COVER_BYTES: u64 = 10 * 1024 * 1024;

fn validated_cover_bytes(path: &Path) -> AppResult<Vec<u8>> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_CUSTOM_COVER_BYTES {
        return Err(AppError::Other("BOOK_COVER_SIZE_INVALID".to_string()));
    }
    let bytes = fs::read(path)?;
    let supported = bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(b"\xFF\xD8\xFF")
        || (bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP");
    if !supported {
        return Err(AppError::Other("BOOK_COVER_FORMAT_INVALID".to_string()));
    }
    image::load_from_memory(&bytes)
        .map_err(|_| AppError::Other("BOOK_COVER_FORMAT_INVALID".to_string()))?;
    Ok(bytes)
}

pub(crate) fn do_delete_book(id: &str, db: &Db, sync: &SyncWriter) -> AppResult<()> {
    do_delete_book_with_note_policy(id, false, db, sync)
}

pub(crate) fn do_delete_book_with_note_policy(
    id: &str,
    preserve_book_notes: bool,
    db: &Db,
    sync: &SyncWriter,
) -> AppResult<()> {
    crate::sync::validation::validate_entity_id(id)?;
    let (file_path, source_file_path): (String, Option<String>) = {
        let conn = db.conn.lock().map_err(|e| AppError::Other(e.to_string()))?;
        conn.query_row(
            "SELECT file_path, source_file_path FROM books WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?
    };

    let now = chrono::Utc::now().timestamp_millis();
    let device = sync.self_device().to_string();
    sync.with_tx(db, now, |tx, events| {
        if preserve_book_notes {
            let detached_notes = {
                let mut statement = tx.prepare(
                    "SELECT id, anchor_kind, normalized_word, selected_text, content,
                            content_format, created_at
                     FROM notes WHERE book_id = ?1 AND scope = 'book'",
                )?;
                let notes = statement
                    .query_map(params![id], |row| {
                        Ok(NotePayload {
                            id: row.get(0)?,
                            book_id: None,
                            anchor_kind: row.get(1)?,
                            normalized_word: row.get(2)?,
                            scope: "detached".to_string(),
                            location: None,
                            selected_text: row.get(3)?,
                            content: row.get(4)?,
                            content_format: row.get(5)?,
                            created_at: row.get(6)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                notes
            };
            for note in detached_notes {
                tx.execute(
                    "UPDATE notes
                     SET book_id = NULL, scope = 'detached', location = NULL,
                         updated_at = ?2, updated_by_device = ?3
                     WHERE id = ?1",
                    params![note.id, now, device],
                )?;
                events.push(EventBody::NoteUpsert(note));
            }
        }
        // Keep the local command path byte-equivalent to replaying the
        // published BookDelete event. In particular, cascade_delete records
        // chat tombstones before removing chats so delayed messages cannot
        // materialize as orphans on this device or in its next snapshot.
        merge::cascade_delete(tx, entity::BOOK, id, now)?;
        merge::insert_tombstone(tx, entity::BOOK, id, now)?;
        events.push(EventBody::BookDelete { id: id.to_string() });
        Ok(())
    })?;

    let abs_file = db.resolve_path(&file_path)?;
    let _ = fs::remove_file(&abs_file);
    if let Some(source_path) = source_file_path.filter(|path| path != &file_path) {
        let abs_source = db.resolve_path(&source_path)?;
        let _ = fs::remove_file(abs_source);
    }
    let cover_file = db.resolve_path(&format!("covers/{id}.img"))?;
    let _ = fs::remove_file(&cover_file);

    Ok(())
}

#[tauri::command]
pub fn delete_book(
    id: String,
    preserve_notes: Option<bool>,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
    local_dir: State<'_, LocalDir>,
) -> AppResult<()> {
    do_delete_book_with_note_policy(&id, preserve_notes.unwrap_or(false), &db, &sync)?;
    let prepared_path = prepared_document_path(&local_dir.0, &id);
    let _ = fs::remove_file(&prepared_path);
    if let Ok(backup_path) = prepared_document_backup_path(&prepared_path) {
        let _ = fs::remove_file(backup_path);
    }
    if let Ok(temporary_path) = prepared_document_temporary_path(&prepared_path) {
        let _ = fs::remove_file(temporary_path);
    }
    let _ = fs::remove_file(legacy_prepared_document_path(&local_dir.0, &id));
    Ok(())
}

#[tauri::command]
pub fn update_reading_progress(
    id: String,
    progress: i32,
    cfi: Option<String>,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<()> {
    let now = chrono::Utc::now().timestamp_millis();
    let device = sync.self_device().to_string();
    // Page-turn rate is dominated by this command; gate the event push on
    // the per-book throttle so a reading session doesn't balloon the log.
    // The SQL write always lands so the local UI stays current — only the
    // event publication is coalesced. Semantic transitions like
    // `mark_finished` deliberately do NOT consult the throttle.
    let emit = sync.should_emit_progress(&id);
    sync.with_tx(&db, now, |tx, events| {
        tx.execute(
            "UPDATE books SET progress = ?1, current_cfi = ?2, updated_at = ?3, updated_by_device = ?4 WHERE id = ?5",
            params![progress, cfi, now, device, id],
        )?;
        if emit {
            events.push(EventBody::BookProgressSet {
                book: id.clone(),
                progress,
                cfi: cfi.clone(),
            });
        }
        Ok(())
    })
}

#[tauri::command]
pub fn update_book_pages(id: String, pages: i32, db: State<'_, Db>) -> AppResult<()> {
    // Local-only — `pages` is derived from the book file on this device and
    // not part of the sync contract. Plain DB write, no SyncWriter.
    let conn = db.conn.lock().map_err(|e| AppError::Other(e.to_string()))?;
    conn.execute(
        "UPDATE books SET pages = ?1 WHERE id = ?2",
        params![pages, id],
    )?;
    Ok(())
}

#[tauri::command]
pub fn mark_finished(id: String, db: State<'_, Db>, sync: State<'_, SyncWriter>) -> AppResult<()> {
    let now = chrono::Utc::now().timestamp_millis();
    let device = sync.self_device().to_string();
    sync.with_tx(&db, now, |tx, events| {
        // Read the current cfi BEFORE the UPDATE so the synthesized
        // `book.progress.set` carries the resume position the local row
        // keeps. Local SQL doesn't touch `current_cfi` here, so emitting
        // `cfi: None` would silently null the column on every peer while
        // this device still has it — a snapshot-equivalence violation.
        let current_cfi: Option<String> = tx
            .query_row(
                "SELECT current_cfi FROM books WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        tx.execute(
            "UPDATE books SET status = 'finished', progress = 100, updated_at = ?1, updated_by_device = ?2 WHERE id = ?3",
            params![now, device, id],
        )?;
        // Mark-finished is two LWW columns moving in lockstep; the merge
        // engine has no `book.finished` event, so we publish the same pair
        // of events the user could have produced manually. The progress
        // event is published unconditionally — the throttle is for noisy
        // page-turn updates only, never for semantic transitions.
        events.push(EventBody::BookStatusSet {
            book: id.clone(),
            status: "finished".into(),
        });
        events.push(EventBody::BookProgressSet {
            book: id.clone(),
            progress: 100,
            cfi: current_cfi,
        });
        Ok(())
    })
}

pub(crate) fn do_update_book(
    id: &str,
    title: Option<&str>,
    author: Option<&str>,
    genre: Option<&str>,
    status: Option<&str>,
    db: &Db,
    sync: &SyncWriter,
) -> AppResult<Book> {
    let now = chrono::Utc::now().timestamp_millis();
    let device = sync.self_device().to_string();
    sync.with_tx(db, now, |tx, events| {
        if let Some(t) = title {
            tx.execute(
                "UPDATE books SET title = ?1, updated_at = ?2, updated_by_device = ?3 WHERE id = ?4",
                params![t, now, device, id],
            )?;
            events.push(EventBody::BookMetadataSet {
                book: id.to_string(),
                field: "title".into(),
                value: serde_json::Value::String(t.to_string()),
            });
        }
        if let Some(a) = author {
            tx.execute(
                "UPDATE books SET author = ?1, updated_at = ?2, updated_by_device = ?3 WHERE id = ?4",
                params![a, now, device, id],
            )?;
            events.push(EventBody::BookMetadataSet {
                book: id.to_string(),
                field: "author".into(),
                value: serde_json::Value::String(a.to_string()),
            });
        }
        if let Some(g) = genre {
            tx.execute(
                "UPDATE books SET genre = ?1, updated_at = ?2, updated_by_device = ?3 WHERE id = ?4",
                params![g, now, device, id],
            )?;
            events.push(EventBody::BookMetadataSet {
                book: id.to_string(),
                field: "genre".into(),
                value: serde_json::Value::String(g.to_string()),
            });
        }
        if let Some(s) = status {
            tx.execute(
                "UPDATE books SET status = ?1, updated_at = ?2, updated_by_device = ?3 WHERE id = ?4",
                params![s, now, device, id],
            )?;
            events.push(EventBody::BookStatusSet {
                book: id.to_string(),
                status: s.to_string(),
            });
        }
        Ok(())
    })?;
    query_book(db, id)
}

#[tauri::command]
pub fn update_book_status(
    id: String,
    status: String,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<()> {
    do_update_book(&id, None, None, None, Some(&status), &db, &sync)?;
    Ok(())
}

#[tauri::command]
pub fn update_book_metadata(
    id: String,
    title: String,
    author: String,
    app: AppHandle,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<()> {
    do_update_book(&id, Some(&title), Some(&author), None, None, &db, &sync)?;
    if let Err(error) = app.emit(
        "book-metadata-changed",
        serde_json::json!({ "id": id, "title": title, "author": author }),
    ) {
        log::warn!("failed to notify readers about metadata update: {error}");
    }
    Ok(())
}

#[tauri::command]
pub fn update_book_cover(
    id: String,
    image_path: String,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<()> {
    crate::sync::validation::validate_entity_id(&id)?;
    let bytes = validated_cover_bytes(Path::new(&image_path))?;
    let relative_path = format!("covers/{id}.img");
    let destination = db.resolve_path(&relative_path)?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = destination.with_extension("img.tmp");
    let previous = fs::read(&destination).ok();
    fs::write(&temporary, &bytes)?;
    fs::rename(&temporary, &destination)?;

    let now = chrono::Utc::now().timestamp_millis();
    let device = sync.self_device().to_string();
    let result = sync.with_tx(&db, now, |tx, events| {
        let changed = tx.execute(
            "UPDATE books
             SET cover_path = ?1, cover_data = ?2, updated_at = ?3, updated_by_device = ?4
             WHERE id = ?5",
            params![relative_path, bytes, now, device, id],
        )?;
        if changed == 0 {
            return Err(AppError::Other("BOOK_NOT_FOUND".to_string()));
        }
        events.push(EventBody::BookMetadataSet {
            book: id.clone(),
            field: "cover_path".to_string(),
            value: serde_json::Value::String(relative_path.clone()),
        });
        Ok(())
    });
    if let Err(error) = result {
        if let Some(previous) = previous {
            let _ = fs::write(&destination, previous);
        } else {
            let _ = fs::remove_file(&destination);
        }
        return Err(error);
    }
    sync.queue_cover_write(&db, &id, &bytes);
    Ok(())
}

#[cfg(test)]
mod cover_tests {
    use super::*;

    #[test]
    fn custom_cover_rejects_non_image_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cover.png");
        fs::write(&path, b"not an image").unwrap();
        assert!(validated_cover_bytes(&path).is_err());
    }
}
