use super::*;

pub(super) fn cover_blob_to_data_uri(bytes: &[u8]) -> String {
    let mime = if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(b"\xFF\xD8\xFF") {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.len() > 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "image/png"
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!("data:{mime};base64,{b64}")
}

/// Resolve relative paths in a Book to absolute using data_dir,
/// and check whether the book file is locally available.
///
/// `app` powers self-healing for converted books (re-scheduling a conversion
/// whose artifact vanished); pass `None` where no `AppHandle` exists and the
/// repair will wait for the next startup resume instead.
pub(super) fn resolve_book_paths(book: &mut Book, db: &Db, app: Option<&AppHandle>) -> AppResult<()> {
    // A converted book (EPUB render format from a non-EPUB source) reads from
    // the local, non-synced converted artifact once preparation is `ready`.
    // The synced `file_path` still points at the source blob and is used by
    // the conversion job; the reader must fetch the local EPUB instead.
    let converted_ready = book.preparation_state == "ready"
        && convert_prepare::is_conversion_book(
            book.render_format.as_deref(),
            book.source_format.as_deref(),
        );
    if converted_ready {
        let local_dir = db.local_data_dir()?;
        let artifact = convert_prepare::converted_document_path(&local_dir, &book.id);
        if artifact.is_file() {
            book.file_path = artifact.to_string_lossy().to_string();
            if let Some(ref cover) = book.cover_path {
                if cover != "none" {
                    book.cover_path = Some(db.resolve_path(cover)?.to_string_lossy().to_string());
                }
            }
            // The converted EPUB lives locally, so it is always available.
            book.available = true;
            return Ok(());
        }
        // Artifact missing despite a `ready` row (cache cleared, or a
        // CONVERSION_VERSION bump moved the expected path): self-heal by
        // re-pending the job so the UI shows the preparing overlay instead of
        // handing foliate the raw source bytes labelled as EPUB.
        if convert_prepare::transition_conversion_state(db, &book.id, "ready", "pending", None)? {
            if let Some(app) = app {
                convert_prepare::schedule_book_conversion(app.clone(), book.id.clone());
            }
        }
        book.preparation_state = "pending".to_string();
        book.preparation_error = None;
    }
    book.file_path = db
        .resolve_path(&book.file_path)?
        .to_string_lossy()
        .to_string();
    if let Some(ref cover) = book.cover_path {
        if cover != "none" {
            book.cover_path = Some(db.resolve_path(cover)?.to_string_lossy().to_string());
        }
    }
    book.available = icloud::is_file_downloaded(std::path::Path::new(&book.file_path));
    Ok(())
}

pub(crate) fn query_books(
    db: &Db,
    filter: Option<&str>,
    search: Option<&str>,
    collection_id: Option<&str>,
    cursor: Option<&str>,
    limit: usize,
) -> AppResult<BookPage> {
    let conn = db.reader();

    let use_collection = collection_id.is_some();
    let from_clause = if use_collection {
        "books INNER JOIN collection_books cb ON cb.book_id = books.id"
    } else {
        "books"
    };

    let mut conditions: Vec<String> = Vec::new();
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(cid) = collection_id {
        conditions.push("cb.collection_id = ?".to_string());
        param_values.push(Box::new(cid.to_string()));
    }

    if let Some(f) = filter {
        match f {
            "reading" | "finished" | "unread" => {
                conditions.push("books.status = ?".to_string());
                param_values.push(Box::new(f.to_string()));
            }
            "all" => {}
            genre => {
                conditions.push("books.genre = ?".to_string());
                param_values.push(Box::new(genre.to_string()));
            }
        }
    }

    if let Some(q) = search {
        if !q.is_empty() {
            conditions.push(
                r"(LOWER(books.title) LIKE ? ESCAPE '\' OR LOWER(books.author) LIKE ? ESCAPE '\')"
                    .to_string(),
            );
            let pattern = crate::db::sqlite_contains_pattern(q);
            param_values.push(Box::new(pattern.clone()));
            param_values.push(Box::new(pattern));
        }
    }

    // Cursor: "updated_at:id" — books older than cursor position.
    if let Some(c) = cursor {
        if let Some((ts_str, cid)) = c.split_once(':') {
            if let Ok(ts) = ts_str.parse::<i64>() {
                conditions.push(
                    "(books.updated_at < ? OR (books.updated_at = ? AND books.id > ?))".to_string(),
                );
                param_values.push(Box::new(ts));
                param_values.push(Box::new(ts));
                param_values.push(Box::new(cid.to_string()));
            }
        }
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    // Count conditions = same as main conditions but without cursor.
    let count_where = {
        let mut cc: Vec<String> = Vec::new();
        if let Some(cid) = collection_id {
            cc.push("cb.collection_id = ?".to_string());
            let _ = cid;
        }
        if let Some(f) = filter {
            match f {
                "reading" | "finished" | "unread" => cc.push("books.status = ?".to_string()),
                "all" => {}
                _ => cc.push("books.genre = ?".to_string()),
            }
        }
        if search.is_some_and(|q| !q.is_empty()) {
            cc.push(
                r"(LOWER(books.title) LIKE ? ESCAPE '\' OR LOWER(books.author) LIKE ? ESCAPE '\')"
                    .to_string(),
            );
        }
        if cc.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", cc.join(" AND "))
        }
    };
    let count_sql = format!("SELECT COUNT(*) FROM {from_clause}{count_where}");
    let mut count_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(cid) = collection_id {
        count_params.push(Box::new(cid.to_string()));
    }
    if let Some(f) = filter {
        match f {
            "reading" | "finished" | "unread" | "all" => {
                if f != "all" {
                    count_params.push(Box::new(f.to_string()));
                }
            }
            _ => {
                count_params.push(Box::new(f.to_string()));
            }
        }
    }
    if let Some(q) = search {
        if !q.is_empty() {
            let pattern = crate::db::sqlite_contains_pattern(q);
            count_params.push(Box::new(pattern.clone()));
            count_params.push(Box::new(pattern));
        }
    }
    let count_refs: Vec<&dyn rusqlite::types::ToSql> =
        count_params.iter().map(|p| p.as_ref()).collect();
    let total: usize = conn.query_row(&count_sql, count_refs.as_slice(), |r| r.get(0))?;

    // Main query with cursor + limit.
    let sql = format!(
        "SELECT books.id, books.title, books.author, books.description, books.cover_path, books.file_path, books.format, books.source_format, books.render_format, books.source_file_path, books.source_sha256, books.conversion_version, books.genre, books.pages, books.status, books.progress, books.current_cfi, books.created_at, books.updated_at, books.cover_data, books.preparation_state, books.preparation_error FROM {from_clause}{where_clause} ORDER BY books.updated_at DESC, books.id ASC LIMIT ?",
    );
    param_values.push(Box::new((limit + 1) as i64));

    let params_refs: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();

    let mut stmt = conn.prepare(&sql)?;
    let mut books: Vec<Book> = stmt
        .query_map(params_refs.as_slice(), |row| {
            let cover_blob: Option<Vec<u8>> = row.get(19)?;
            Ok(Book {
                id: row.get(0)?,
                title: row.get(1)?,
                author: row.get(2)?,
                description: row.get(3)?,
                cover_path: row.get(4)?,
                file_path: row.get(5)?,
                format: row.get(6)?,
                source_format: row.get(7)?,
                render_format: row.get(8)?,
                source_file_path: row.get(9)?,
                source_sha256: row.get(10)?,
                conversion_version: row.get::<_, Option<i32>>(11)?.unwrap_or(0),
                preparation_state: row.get(20)?,
                preparation_error: row.get(21)?,
                genre: row.get(12)?,
                pages: row.get(13)?,
                status: row.get(14)?,
                progress: row.get(15)?,
                current_cfi: row.get(16)?,
                created_at: row.get(17)?,
                updated_at: row.get(18)?,
                available: true,
                cover_data: cover_blob
                    .filter(|b| !b.is_empty())
                    .map(|b| cover_blob_to_data_uri(&b)),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let next_cursor = if books.len() > limit {
        books.truncate(limit);
        let last = &books[limit - 1];
        Some(format!("{}:{}", last.updated_at, last.id))
    } else {
        None
    };

    Ok(BookPage {
        books,
        next_cursor,
        total,
    })
}

/// Shared query helper for the single-book lookup. Same relative-path
/// guarantee as `query_books`.
pub(crate) fn query_book(db: &Db, id: &str) -> AppResult<Book> {
    let conn = db.reader();
    let book = conn.query_row(
        "SELECT id, title, author, description, cover_path, file_path, format, source_format, render_format, source_file_path, source_sha256, conversion_version, genre, pages, status, progress, current_cfi, created_at, updated_at, cover_data, preparation_state, preparation_error FROM books WHERE id = ?1",
        params![id],
        |row| {
            let cover_blob: Option<Vec<u8>> = row.get(19)?;
            Ok(Book {
                id: row.get(0)?,
                title: row.get(1)?,
                author: row.get(2)?,
                description: row.get(3)?,
                cover_path: row.get(4)?,
                file_path: row.get(5)?,
                format: row.get(6)?,
                source_format: row.get(7)?,
                render_format: row.get(8)?,
                source_file_path: row.get(9)?,
                source_sha256: row.get(10)?,
                conversion_version: row.get::<_, Option<i32>>(11)?.unwrap_or(0),
                preparation_state: row.get(20)?,
                preparation_error: row.get(21)?,
                genre: row.get(12)?,
                pages: row.get(13)?,
                status: row.get(14)?,
                progress: row.get(15)?,
                current_cfi: row.get(16)?,
                created_at: row.get(17)?,
                updated_at: row.get(18)?,
                available: true,
                cover_data: cover_blob.filter(|b| !b.is_empty()).map(|b| cover_blob_to_data_uri(&b)),
            })
        },
    )?;
    Ok(book)
}

/// Lightweight book query for MCP — computes `has_cover` from the BLOB
/// without actually loading/encoding cover bytes. Prevents hundreds of
/// MB of wasted DB reads + base64 allocations when MCP lists 1000 books.
pub(crate) fn query_books_lite(
    db: &Db,
    filter: Option<&str>,
    search: Option<&str>,
    collection_id: Option<&str>,
    limit: usize,
) -> AppResult<Vec<Book>> {
    let conn = db.reader();
    let mut conditions: Vec<String> = Vec::new();
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(collection_id) = collection_id {
        conditions.push(
            "EXISTS (SELECT 1 FROM collection_books cb WHERE cb.book_id = books.id AND cb.collection_id = ?)"
                .to_string(),
        );
        param_values.push(Box::new(collection_id.to_string()));
    }

    if let Some(f) = filter {
        match f {
            "reading" | "finished" | "unread" => {
                conditions.push("status = ?".to_string());
                param_values.push(Box::new(f.to_string()));
            }
            "all" => {}
            genre => {
                conditions.push("genre = ?".to_string());
                param_values.push(Box::new(genre.to_string()));
            }
        }
    }
    if let Some(q) = search {
        if !q.is_empty() {
            conditions.push(
                r"(LOWER(title) LIKE ? ESCAPE '\' OR LOWER(author) LIKE ? ESCAPE '\')".to_string(),
            );
            let pattern = crate::db::sqlite_contains_pattern(q);
            param_values.push(Box::new(pattern.clone()));
            param_values.push(Box::new(pattern));
        }
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    let sql = format!(
        "SELECT id, title, author, description, cover_path, file_path, format, source_format, render_format, source_file_path, source_sha256, conversion_version, genre, pages, status, progress, current_cfi, created_at, updated_at, (cover_data IS NOT NULL AND LENGTH(cover_data) > 0) AS has_cover, preparation_state, preparation_error FROM books{where_clause} ORDER BY updated_at DESC LIMIT ?",
    );
    param_values.push(Box::new(limit as i64));
    let params_refs: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();

    let mut stmt = conn.prepare(&sql)?;
    let books = stmt
        .query_map(params_refs.as_slice(), |row| {
            let has_cover: bool = row.get(19)?;
            Ok(Book {
                id: row.get(0)?,
                title: row.get(1)?,
                author: row.get(2)?,
                description: row.get(3)?,
                cover_path: row.get(4)?,
                file_path: row.get(5)?,
                format: row.get(6)?,
                source_format: row.get(7)?,
                render_format: row.get(8)?,
                source_file_path: row.get(9)?,
                source_sha256: row.get(10)?,
                conversion_version: row.get::<_, Option<i32>>(11)?.unwrap_or(0),
                preparation_state: row.get(20)?,
                preparation_error: row.get(21)?,
                genre: row.get(12)?,
                pages: row.get(13)?,
                status: row.get(14)?,
                progress: row.get(15)?,
                current_cfi: row.get(16)?,
                created_at: row.get(17)?,
                updated_at: row.get(18)?,
                available: true,
                cover_data: if has_cover {
                    Some("has_cover".to_string())
                } else {
                    None
                },
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(books)
}

pub(crate) fn query_book_exists(db: &Db, id: &str) -> AppResult<bool> {
    let conn = db.reader();
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM books WHERE id = ?1)",
        params![id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

const DEFAULT_PAGE_SIZE: usize = 20;

#[tauri::command]
pub fn list_books(
    filter: Option<String>,
    search: Option<String>,
    collection_id: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
    db: State<'_, Db>,
    app: AppHandle,
) -> AppResult<BookPage> {
    let page_size = limit.unwrap_or(DEFAULT_PAGE_SIZE);
    let mut page = query_books(
        &db,
        filter.as_deref(),
        search.as_deref(),
        collection_id.as_deref(),
        cursor.as_deref(),
        page_size,
    )?;
    for book in &mut page.books {
        resolve_book_paths(book, &db, Some(&app))?;
    }
    Ok(page)
}

#[tauri::command]
pub fn get_book_counts(db: State<'_, Db>) -> AppResult<BookCounts> {
    let conn = db.reader();
    let all: usize = conn.query_row("SELECT COUNT(*) FROM books", [], |r| r.get(0))?;
    let reading: usize = conn.query_row(
        "SELECT COUNT(*) FROM books WHERE status = 'reading'",
        [],
        |r| r.get(0),
    )?;
    let finished: usize = conn.query_row(
        "SELECT COUNT(*) FROM books WHERE status = 'finished'",
        [],
        |r| r.get(0),
    )?;
    Ok(BookCounts {
        all,
        reading,
        finished,
    })
}

#[tauri::command]
pub fn get_book(id: String, db: State<'_, Db>, app: AppHandle) -> AppResult<Book> {
    let mut book = query_book(&db, &id)?;
    resolve_book_paths(&mut book, &db, Some(&app))?;
    Ok(book)
}

/// Check a book's local file state and trigger iCloud download only for an
/// actual evicted placeholder. A missing local file is not an iCloud retry.
#[tauri::command]
pub fn check_book_available(id: String, db: State<'_, Db>) -> AppResult<BookAvailability> {
    let conn = db.reader();
    let file_path: String = conn.query_row(
        "SELECT file_path FROM books WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )?;

    let abs_path = db.resolve_path(&file_path)?;
    let availability = icloud::file_availability(&abs_path);
    if availability == icloud::FileAvailability::ICloudPlaceholder {
        icloud::trigger_download_file(&abs_path);
    }
    Ok(BookAvailability {
        status: availability.as_str().to_string(),
        available: availability == icloud::FileAvailability::Available,
    })
}
