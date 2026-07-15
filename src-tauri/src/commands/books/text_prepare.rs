use super::format::{decode_txt, html_to_text, markdown_to_text};
use super::text_headings::text_document_parts;
use super::*;

#[derive(Debug, Serialize, Clone)]
struct TextPreparationChanged {
    book_id: String,
    state: String,
}

pub(super) fn prepare_text_document(
    source_path: &Path,
    source_format: &str,
    expected_source_sha256: Option<String>,
) -> AppResult<TextBookDocument> {
    let source_bytes = fs::read(source_path)?;
    let actual_source_sha256 = format!("{:x}", Sha256::digest(&source_bytes));
    if expected_source_sha256
        .as_ref()
        .is_some_and(|expected| expected != &actual_source_sha256)
    {
        return Err(AppError::Other("TEXT_SOURCE_HASH_MISMATCH".to_string()));
    }
    let decoded = decode_txt(&source_bytes)?;
    let text = match source_format {
        "markdown" => markdown_to_text(&decoded),
        "html" => html_to_text(&decoded),
        _ => decoded,
    };
    if text.trim().is_empty() {
        return Err(AppError::Other("EMPTY_BOOK".to_string()));
    }
    let (chunks, toc, legacy_locations) = text_document_parts(&text, source_format == "txt");
    Ok(TextBookDocument {
        version: TEXT_DOCUMENT_VERSION,
        source_sha256: Some(actual_source_sha256),
        coordinate_space: "normalized_utf16".to_string(),
        chunks,
        toc,
        legacy_locations,
    })
}

pub(super) fn prepared_document_path(local_dir: &Path, book_id: &str) -> PathBuf {
    local_dir
        .join("prepared")
        .join(format!("{book_id}.v{TEXT_DOCUMENT_VERSION}.json"))
}

pub(super) fn legacy_prepared_document_path(local_dir: &Path, book_id: &str) -> PathBuf {
    local_dir.join("prepared").join(format!("{book_id}.json"))
}

fn cleanup_obsolete_prepared_documents(local_dir: &Path, book_id: &str) {
    let _ = fs::remove_file(legacy_prepared_document_path(local_dir, book_id));
    for version in 1..TEXT_DOCUMENT_VERSION {
        let path = local_dir
            .join("prepared")
            .join(format!("{book_id}.v{version}.json"));
        let _ = fs::remove_file(&path);
        for suffix in ["backup", "tmp"] {
            if let Ok(sidecar) = prepared_document_sidecar_path(&path, suffix) {
                let _ = fs::remove_file(sidecar);
            }
        }
    }
}

pub(super) fn prepared_document_sidecar_path(path: &Path, suffix: &str) -> AppResult<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Other("PREPARATION_PATH_INVALID".to_string()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| AppError::Other("PREPARATION_PATH_INVALID".to_string()))?
        .to_string_lossy();
    Ok(parent.join(format!(".{file_name}.{suffix}")))
}

pub(super) fn prepared_document_backup_path(path: &Path) -> AppResult<PathBuf> {
    prepared_document_sidecar_path(path, "backup")
}

pub(super) fn prepared_document_temporary_path(path: &Path) -> AppResult<PathBuf> {
    prepared_document_sidecar_path(path, "tmp")
}

pub(super) fn read_prepared_document(
    path: &Path,
    expected_source_sha256: Option<&str>,
) -> Option<TextBookDocument> {
    let document = fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<TextBookDocument>(&bytes).ok())?;
    (document.version == TEXT_DOCUMENT_VERSION
        && expected_source_sha256
            .is_none_or(|expected| document.source_sha256.as_deref() == Some(expected)))
    .then_some(document)
}

pub(super) fn load_prepared_document(
    path: &Path,
    expected_source_sha256: Option<&str>,
) -> Option<TextBookDocument> {
    let backup_path = prepared_document_backup_path(path).ok()?;
    let temporary_path = prepared_document_temporary_path(path).ok()?;
    if let Some(document) = read_prepared_document(path, expected_source_sha256) {
        let _ = fs::remove_file(backup_path);
        let _ = fs::remove_file(temporary_path);
        return Some(document);
    }

    for recovery_path in [&temporary_path, &backup_path] {
        let Some(document) = read_prepared_document(recovery_path, expected_source_sha256) else {
            continue;
        };
        if path.exists() {
            let _ = fs::remove_file(path);
        }
        if fs::rename(recovery_path, path).is_err() {
            return None;
        }
        let _ = fs::remove_file(&temporary_path);
        let _ = fs::remove_file(&backup_path);
        log::warn!(
            "recovered interrupted text cache replacement at {}",
            path.display()
        );
        return Some(document);
    }
    None
}

pub(super) fn text_toc_leaf_count(toc: &[TextBookTocEntry]) -> usize {
    toc.iter()
        .enumerate()
        .filter(|(index, entry)| {
            toc.get(index + 1)
                .is_none_or(|next| next.depth <= entry.depth)
        })
        .count()
}

pub(super) fn write_prepared_document(path: &Path, document: &TextBookDocument) -> AppResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Other("PREPARATION_PATH_INVALID".to_string()))?;
    fs::create_dir_all(parent)?;
    let temporary_path = prepared_document_temporary_path(path)?;
    let backup_path = prepared_document_backup_path(path)?;
    let result = (|| -> AppResult<()> {
        let bytes = serde_json::to_vec(document)
            .map_err(|error| AppError::Other(format!("PREPARATION_SERIALIZE_FAILED: {error}")))?;
        match fs::remove_file(&temporary_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let mut temporary = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_path)?;
        temporary.write_all(&bytes)?;
        temporary.sync_all()?;
        drop(temporary);
        if path.exists() {
            match fs::remove_file(&backup_path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            fs::rename(path, &backup_path)?;
            if let Err(error) = fs::rename(&temporary_path, path) {
                let _ = fs::rename(&backup_path, path);
                return Err(error.into());
            }
        } else {
            fs::rename(&temporary_path, path)?;
        }
        let _ = fs::remove_file(&backup_path);
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

#[derive(Clone, Debug)]
pub(super) struct TextPreparationSource {
    pub(super) file_path: Option<String>,
    pub(super) format: Option<String>,
    pub(super) sha256: Option<String>,
    pub(super) conversion_version: i32,
}

pub(super) fn transition_text_preparation_state(
    db: &Db,
    book_id: &str,
    expected_state: &str,
    next_state: &str,
    error: Option<&str>,
) -> AppResult<bool> {
    let conn = db
        .conn
        .lock()
        .map_err(|error| AppError::Other(error.to_string()))?;
    let changed = conn.execute(
        "UPDATE books
         SET preparation_state = ?1,
             preparation_error = ?2
         WHERE id = ?3 AND render_format = 'text' AND preparation_state = ?4",
        params![next_state, error, book_id, expected_state],
    )?;
    Ok(changed == 1)
}

pub(super) fn text_preparation_job_is_current(
    conn: &rusqlite::Connection,
    book_id: &str,
    source: &TextPreparationSource,
) -> AppResult<bool> {
    let current = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM books
             WHERE id = ?1
               AND render_format = 'text'
               AND preparation_state = 'preparing'
               AND source_file_path IS ?2
               AND source_format IS ?3
               AND source_sha256 IS ?4
               AND COALESCE(conversion_version, 0) = ?5
         )",
        params![
            book_id,
            source.file_path.as_deref(),
            source.format.as_deref(),
            source.sha256.as_deref(),
            source.conversion_version,
        ],
        |row| row.get::<_, bool>(0),
    )?;
    Ok(current)
}

pub(super) fn update_current_text_preparation_job(
    db: &Db,
    book_id: &str,
    source: &TextPreparationSource,
    next_state: &str,
    error: Option<&str>,
    pages: Option<i32>,
) -> AppResult<bool> {
    let conn = db
        .conn
        .lock()
        .map_err(|lock_error| AppError::Other(lock_error.to_string()))?;
    let changed = conn.execute(
        "UPDATE books
         SET preparation_state = ?1,
             preparation_error = ?2,
             pages = COALESCE(?3, pages)
         WHERE id = ?4
           AND render_format = 'text'
           AND preparation_state = 'preparing'
           AND source_file_path IS ?5
           AND source_format IS ?6
           AND source_sha256 IS ?7
           AND COALESCE(conversion_version, 0) = ?8",
        params![
            next_state,
            error,
            pages,
            book_id,
            source.file_path.as_deref(),
            source.format.as_deref(),
            source.sha256.as_deref(),
            source.conversion_version,
        ],
    )?;
    Ok(changed == 1)
}

pub(super) fn recover_current_text_preparation_job(
    db: &Db,
    book_id: &str,
    source: &TextPreparationSource,
    prepared_path: &Path,
) -> AppResult<bool> {
    let conn = db
        .conn
        .lock()
        .map_err(|lock_error| AppError::Other(lock_error.to_string()))?;
    if !text_preparation_job_is_current(&conn, book_id, source)? {
        return Ok(false);
    }
    let Some(document) = load_prepared_document(prepared_path, source.sha256.as_deref()) else {
        return Ok(false);
    };
    let pages = i32::try_from(text_toc_leaf_count(&document.toc).max(1))
        .map_err(|_| AppError::Other("TEXT_BOOK_TOO_LARGE".to_string()))?;
    let changed = conn.execute(
        "UPDATE books
         SET preparation_state = 'ready', preparation_error = NULL, pages = ?1
         WHERE id = ?2
           AND render_format = 'text'
           AND preparation_state = 'preparing'
           AND source_file_path IS ?3
           AND source_format IS ?4
           AND source_sha256 IS ?5
           AND COALESCE(conversion_version, 0) = ?6",
        params![
            pages,
            book_id,
            source.file_path.as_deref(),
            source.format.as_deref(),
            source.sha256.as_deref(),
            source.conversion_version,
        ],
    )?;
    Ok(changed == 1)
}

pub(super) fn publish_current_text_preparation_job(
    db: &Db,
    book_id: &str,
    source: &TextPreparationSource,
    prepared_path: &Path,
    document: &TextBookDocument,
    pages: i32,
) -> AppResult<bool> {
    let conn = db
        .conn
        .lock()
        .map_err(|lock_error| AppError::Other(lock_error.to_string()))?;
    if !text_preparation_job_is_current(&conn, book_id, source)? {
        return Ok(false);
    }
    write_prepared_document(prepared_path, document)?;
    let changed = conn.execute(
        "UPDATE books
         SET preparation_state = 'ready', preparation_error = NULL, pages = ?1
         WHERE id = ?2
           AND render_format = 'text'
           AND preparation_state = 'preparing'
           AND source_file_path IS ?3
           AND source_format IS ?4
           AND source_sha256 IS ?5
           AND COALESCE(conversion_version, 0) = ?6",
        params![
            pages,
            book_id,
            source.file_path.as_deref(),
            source.format.as_deref(),
            source.sha256.as_deref(),
            source.conversion_version,
        ],
    )?;
    Ok(changed == 1)
}

pub(super) fn emit_text_preparation_changed(app: &AppHandle, book_id: &str, state: &str) {
    let _ = app.emit(
        "book-preparation-changed",
        TextPreparationChanged {
            book_id: book_id.to_string(),
            state: state.to_string(),
        },
    );
}

pub(super) fn run_text_preparation(app: &AppHandle, book_id: &str) -> AppResult<()> {
    crate::sync::validation::validate_entity_id(book_id)?;
    let db = app.state::<Db>();
    let local_dir = app.state::<LocalDir>();
    let source = {
        let conn = db
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let changed = conn.execute(
            "UPDATE books
             SET preparation_state = 'preparing', preparation_error = NULL
             WHERE id = ?1 AND render_format = 'text' AND preparation_state = 'pending'",
            params![book_id],
        )?;
        if changed == 0 {
            return Ok(());
        }
        conn.query_row(
            "SELECT source_file_path, source_format, source_sha256,
                    COALESCE(conversion_version, 0)
             FROM books WHERE id = ?1",
            params![book_id],
            |row| {
                Ok(TextPreparationSource {
                    file_path: row.get(0)?,
                    format: row.get(1)?,
                    sha256: row.get(2)?,
                    conversion_version: row.get(3)?,
                })
            },
        )?
    };
    emit_text_preparation_changed(app, book_id, "preparing");

    let prepared_path = prepared_document_path(&local_dir.0, book_id);
    if recover_current_text_preparation_job(&db, book_id, &source, &prepared_path)? {
        cleanup_obsolete_prepared_documents(&local_dir.0, book_id);
        emit_text_preparation_changed(app, book_id, "ready");
        return Ok(());
    }
    let Some(source_file_path) = source.file_path.as_deref() else {
        if update_current_text_preparation_job(
            &db,
            book_id,
            &source,
            "failed",
            Some("TEXT_SOURCE_MISSING"),
            None,
        )? {
            emit_text_preparation_changed(app, book_id, "failed");
        }
        return Ok(());
    };
    let source_path = match db.resolve_path(source_file_path) {
        Ok(path) => path,
        Err(error) => {
            let message = error.to_string();
            if update_current_text_preparation_job(
                &db,
                book_id,
                &source,
                "failed",
                Some(&message),
                None,
            )? {
                emit_text_preparation_changed(app, book_id, "failed");
            }
            return Ok(());
        }
    };
    match icloud::file_availability(&source_path) {
        icloud::FileAvailability::Available => {}
        icloud::FileAvailability::ICloudPlaceholder => {
            icloud::trigger_download_file(&source_path);
            if update_current_text_preparation_job(&db, book_id, &source, "pending", None, None)? {
                emit_text_preparation_changed(app, book_id, "pending");
            }
            return Ok(());
        }
        icloud::FileAvailability::Missing => {
            if update_current_text_preparation_job(
                &db,
                book_id,
                &source,
                "failed",
                Some("TEXT_SOURCE_UNAVAILABLE"),
                None,
            )? {
                emit_text_preparation_changed(app, book_id, "failed");
            }
            return Ok(());
        }
    }

    let result = prepare_text_document(
        &source_path,
        source.format.as_deref().unwrap_or("txt"),
        source.sha256.clone(),
    )
    .and_then(|document| {
        let pages = i32::try_from(text_toc_leaf_count(&document.toc).max(1))
            .map_err(|_| AppError::Other("TEXT_BOOK_TOO_LARGE".to_string()))?;
        publish_current_text_preparation_job(
            &db,
            book_id,
            &source,
            &prepared_path,
            &document,
            pages,
        )
    });

    match result {
        Ok(true) => {
            cleanup_obsolete_prepared_documents(&local_dir.0, book_id);
            emit_text_preparation_changed(app, book_id, "ready");
        }
        Ok(false) => {
            log::debug!("discarded stale text preparation task for {book_id}");
        }
        Err(error) => {
            let message = error.to_string();
            if update_current_text_preparation_job(
                &db,
                book_id,
                &source,
                "failed",
                Some(&message),
                None,
            )? {
                emit_text_preparation_changed(app, book_id, "failed");
            }
            log::warn!("text preparation failed for {book_id}: {message}");
        }
    }
    Ok(())
}

pub fn schedule_text_book_preparation(app: AppHandle, book_id: String) {
    let thread_name = format!("text-prep-{}", &book_id[..book_id.len().min(8)]);
    let _ = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            if let Err(error) = run_text_preparation(&app, &book_id) {
                log::warn!("text preparation task failed for {book_id}: {error}");
            }
        });
}

pub(super) fn pending_text_book_ids(db: &Db, recover_interrupted: bool) -> AppResult<Vec<String>> {
    let conn = db
        .conn
        .lock()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if recover_interrupted {
        conn.execute(
            "UPDATE books SET preparation_state = 'pending', preparation_error = NULL
             WHERE render_format = 'text' AND preparation_state = 'preparing'",
            [],
        )?;
    }
    let mut statement = conn.prepare(
        "SELECT id FROM books
         WHERE render_format = 'text' AND preparation_state = 'pending'
         ORDER BY id",
    )?;
    let book_ids = statement
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()
        .map_err(AppError::from)?;
    Ok(book_ids)
}

pub(super) fn schedule_pending_text_book_preparations_inner(
    app: AppHandle,
    recover_interrupted: bool,
) {
    let db = app.state::<Db>();
    let pending = pending_text_book_ids(&db, recover_interrupted);
    match pending {
        Ok(book_ids) => {
            for book_id in book_ids {
                schedule_text_book_preparation(app.clone(), book_id);
            }
        }
        Err(error) => log::warn!("text preparation startup scan failed: {error}"),
    }
}

pub fn resume_interrupted_text_book_preparations(app: AppHandle) {
    schedule_pending_text_book_preparations_inner(app, true);
}

pub fn schedule_pending_text_book_preparations(app: AppHandle) {
    schedule_pending_text_book_preparations_inner(app, false);
}

#[tauri::command]
pub fn get_text_book_document(
    book_id: String,
    db: State<'_, Db>,
    local_dir: State<'_, LocalDir>,
    app: AppHandle,
) -> AppResult<TextBookDocument> {
    crate::sync::validation::validate_entity_id(&book_id)?;
    let state: (String, Option<String>, Option<String>) = {
        let conn = db.reader();
        conn.query_row(
            "SELECT preparation_state, preparation_error, source_sha256
             FROM books WHERE id = ?1 AND render_format = 'text'",
            params![book_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?
    };
    match state.0.as_str() {
        "ready" => {
            let path = prepared_document_path(&local_dir.0, &book_id);
            // Reader-side cache access stays non-mutating. Recovery and
            // sidecar cleanup run only inside the preparation job while it
            // owns the database writer lock.
            match read_prepared_document(&path, state.2.as_deref()) {
                Some(document) => Ok(document),
                _ => {
                    if transition_text_preparation_state(&db, &book_id, "ready", "pending", None)? {
                        schedule_text_book_preparation(app, book_id);
                    }
                    Err(AppError::Other("TEXT_PREPARATION_PENDING".to_string()))
                }
            }
        }
        "pending" => {
            schedule_text_book_preparation(app, book_id);
            Err(AppError::Other("TEXT_PREPARATION_PENDING".to_string()))
        }
        "preparing" => Err(AppError::Other("TEXT_PREPARATION_PENDING".to_string())),
        "failed" => Err(AppError::Other(format!(
            "TEXT_PREPARATION_FAILED:{}",
            state.1.unwrap_or_else(|| "UNKNOWN".to_string())
        ))),
        _ => Err(AppError::Other("TEXT_PREPARATION_PENDING".to_string())),
    }
}

#[tauri::command]
pub fn retry_text_book_preparation(
    book_id: String,
    db: State<'_, Db>,
    app: AppHandle,
) -> AppResult<()> {
    crate::sync::validation::validate_entity_id(&book_id)?;
    if transition_text_preparation_state(&db, &book_id, "failed", "pending", None)? {
        emit_text_preparation_changed(&app, &book_id, "pending");
        schedule_text_book_preparation(app, book_id);
    }
    Ok(())
}
