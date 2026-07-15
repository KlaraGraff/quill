use super::format::{detect_import_format, source_sha256, unsupported_format, ImportFormat};
use super::pdf::extract_pdf;
use super::query::{cover_blob_to_data_uri, resolve_book_paths};
use super::*;

/// Sanitize a book title into a safe filename slug.
/// Keeps alphanumeric, spaces (→ hyphens), and common punctuation, then truncates.
pub(super) fn slugify(title: &str) -> String {
    let slug: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .to_lowercase();
    // Truncate to ~60 bytes at a word boundary, but never slice into
    // a multi-byte UTF-8 character. Naive `slug[..60]` panics on
    // non-ASCII titles (e.g. CJK) where byte 60 lands mid-codepoint —
    // which surfaces as `import_book` returning a command-runtime
    // panic the UI sees as "spinner forever". `floor_char_boundary`
    // walks back to the previous char start.
    if slug.len() <= 60 {
        slug
    } else {
        let cut = floor_char_boundary(&slug, 60);
        let head = &slug[..cut];
        head.rfind('-').map_or(head, |i| &head[..i]).to_string()
    }
}

/// Largest valid char-boundary `<= max_bytes`. Stable equivalent of
/// `str::floor_char_boundary` (which is still nightly-only as of
/// rustc 1.85). Walks at most 3 bytes back since UTF-8 codepoints are
/// at most 4 bytes wide.
pub(super) fn floor_char_boundary(s: &str, max_bytes: usize) -> usize {
    let mut i = max_bytes.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Build a human-readable filename: `{slug}_{short-id}.{ext}`
pub(super) fn book_filename(title: &str, book_id: &str, ext: &str) -> String {
    let slug = slugify(title);
    let short_id = &book_id[..8]; // first 8 chars of UUID
    if slug.is_empty() {
        format!("{}.{}", book_id, ext)
    } else {
        format!("{}_{}.{}", slug, short_id, ext)
    }
}

struct ImportFileCleanup {
    paths: Vec<PathBuf>,
    committed: bool,
}

impl ImportFileCleanup {
    fn new(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            paths: paths.into_iter().collect(),
            committed: false,
        }
    }

    fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for ImportFileCleanup {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for path in &self.paths {
            match fs::remove_file(path) {
                Ok(()) => log::info!("import_book: rolled back file {}", path.display()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => log::warn!(
                    "import_book: failed to roll back file {}: {error}",
                    path.display()
                ),
            }
        }
    }
}

pub(crate) fn do_import_epub(file_path: &str, db: &Db, sync: &SyncWriter) -> AppResult<Book> {
    let data_dir = db
        .data_dir
        .lock()
        .map_err(|e| AppError::Other(e.to_string()))?
        .clone();
    let books_dir = data_dir.join("books");

    let book_id = uuid::Uuid::new_v4().to_string();
    let src = std::path::Path::new(file_path);
    let source_sha256 = source_sha256(src)?;

    let metadata = epub::extract_metadata(src).inspect_err(|e| {
        log::error!("import_book: extract_metadata failed for {file_path}: {e}")
    })?;
    let pages = epub::count_chapters(src)
        .inspect_err(|e| log::error!("import_book: count_chapters failed for {file_path}: {e}"))?
        as i32;

    let filename = book_filename(&metadata.title, &book_id, "epub");
    let dest = books_dir.join(&filename);
    let cleanup = ImportFileCleanup::new([dest.clone()]);
    fs::copy(src, &dest)?;

    let now = chrono::Utc::now().timestamp_millis();
    let rel_file_path = format!("books/{}", filename);
    let cover_data_b64 = metadata.cover_data.as_deref().map(cover_blob_to_data_uri);

    let book = Book {
        id: book_id,
        title: metadata.title,
        author: metadata.author,
        description: metadata.description,
        cover_path: None,
        file_path: rel_file_path.clone(),
        format: "epub".to_string(),
        source_format: Some("epub".to_string()),
        render_format: Some("epub".to_string()),
        source_file_path: Some(rel_file_path.clone()),
        source_sha256: Some(source_sha256),
        conversion_version: 0,
        preparation_state: default_preparation_state(),
        preparation_error: None,
        genre: None,
        pages: Some(pages),
        status: "unread".to_string(),
        progress: 0,
        current_cfi: None,
        created_at: now,
        updated_at: now,
        available: true,
        cover_data: cover_data_b64,
    };

    do_insert_book(&book, metadata.cover_data.as_deref(), db, sync, now)?;
    cleanup.commit();

    log::info!(
        "import_book: complete id={} title={:?}",
        book.id,
        book.title
    );
    Ok(book)
}

pub(crate) fn do_import_text(
    file_path: &str,
    source_format: &str,
    db: &Db,
    sync: &SyncWriter,
) -> AppResult<Book> {
    let source = Path::new(file_path);
    let source_hash = source_sha256(source)?;
    let title = source
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Untitled")
        .trim();
    let title = if title.is_empty() { "Untitled" } else { title };
    let data_dir = db
        .data_dir
        .lock()
        .map_err(|e| AppError::Other(e.to_string()))?
        .clone();
    let sources_dir = data_dir.join("sources");
    fs::create_dir_all(&sources_dir)?;
    let book_id = uuid::Uuid::new_v4().to_string();
    let source_extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("txt")
        .to_ascii_lowercase();
    let source_filename = book_filename(title, &book_id, &source_extension);
    let source_path = sources_dir.join(&source_filename);
    let cleanup = ImportFileCleanup::new([source_path.clone()]);
    fs::copy(source, &source_path)?;
    let now = chrono::Utc::now().timestamp_millis();
    let book = Book {
        id: book_id,
        title: title.to_string(),
        author: "Unknown Author".to_string(),
        description: None,
        cover_path: None,
        file_path: format!("sources/{source_filename}"),
        format: "text".to_string(),
        source_format: Some(source_format.to_string()),
        render_format: Some("text".to_string()),
        source_file_path: Some(format!("sources/{source_filename}")),
        source_sha256: Some(source_hash),
        conversion_version: TEXT_DOCUMENT_VERSION,
        preparation_state: "pending".to_string(),
        preparation_error: None,
        genre: None,
        pages: None,
        status: "unread".to_string(),
        progress: 0,
        current_cfi: None,
        created_at: now,
        updated_at: now,
        available: true,
        cover_data: None,
    };
    do_insert_book(&book, None, db, sync, now)?;
    cleanup.commit();
    Ok(book)
}

fn do_import_native(
    file_path: &str,
    format: ImportFormat,
    db: &Db,
    sync: &SyncWriter,
) -> AppResult<Book> {
    let source = Path::new(file_path);
    let source_sha256 = source_sha256(source)?;
    let extension = format
        .native_extension(source)
        .ok_or_else(unsupported_format)?;
    // AZW/AZW3 share Foliate's MOBI parser but keep their source extension so
    // the stored filename and the File handed to the reader remain faithful.
    let source_format = if format == ImportFormat::Mobi {
        extension.clone()
    } else {
        format.source_name().to_string()
    };
    let title = source
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Untitled")
        .trim();
    let title = if title.is_empty() { "Untitled" } else { title };
    let data_dir = db
        .data_dir
        .lock()
        .map_err(|error| AppError::Other(error.to_string()))?
        .clone();
    let books_dir = data_dir.join("books");
    fs::create_dir_all(&books_dir)?;
    let book_id = uuid::Uuid::new_v4().to_string();
    let filename = book_filename(title, &book_id, &extension);
    let final_path = books_dir.join(&filename);
    let cleanup = ImportFileCleanup::new([final_path.clone()]);
    fs::copy(source, &final_path)?;
    let now = chrono::Utc::now().timestamp_millis();
    let book = Book {
        id: book_id,
        title: title.to_string(),
        author: "Unknown Author".to_string(),
        description: None,
        cover_path: None,
        file_path: format!("books/{filename}"),
        format: source_format.clone(),
        source_format: Some(source_format.clone()),
        render_format: Some(source_format),
        source_file_path: Some(format!("books/{filename}")),
        source_sha256: Some(source_sha256),
        conversion_version: 0,
        preparation_state: default_preparation_state(),
        preparation_error: None,
        genre: None,
        pages: None,
        status: "unread".to_string(),
        progress: 0,
        current_cfi: None,
        created_at: now,
        updated_at: now,
        available: true,
        cover_data: None,
    };
    do_insert_book(&book, None, db, sync, now)?;
    cleanup.commit();
    Ok(book)
}

pub(crate) fn do_import_pdf(file_path: &str, db: &Db, sync: &SyncWriter) -> AppResult<Book> {
    let data_dir = db
        .data_dir
        .lock()
        .map_err(|e| AppError::Other(e.to_string()))?
        .clone();
    let books_dir = data_dir.join("books");
    fs::create_dir_all(&books_dir)?;

    let book_id = uuid::Uuid::new_v4().to_string();
    let src = Path::new(file_path);
    let source_sha256 = source_sha256(src)?;

    let fallback_title = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled");
    // pdfium streams via Read+Seek so memory stays bounded regardless of
    // PDF size; fs::copy then streams the file to its destination. We
    // give up the "read once" optimization on purpose — for a 500MB
    // magazine the byte-buffer pattern would spike RAM, and the OS page
    // cache covers the cost of the second read (fs::copy) anyway.
    let t1 = std::time::Instant::now();
    let extracted = extract_pdf(src, fallback_title);
    let t_extract = t1.elapsed();

    let filename = book_filename(&extracted.title, &book_id, "pdf");
    let dest = books_dir.join(&filename);
    let cleanup = ImportFileCleanup::new([dest.clone()]);
    let t2 = std::time::Instant::now();
    fs::copy(src, &dest)?;
    let t_copy = t2.elapsed();

    let now = chrono::Utc::now().timestamp_millis();
    let rel_file_path = format!("books/{}", filename);
    let cover_data_b64 = extracted.cover.as_deref().map(cover_blob_to_data_uri);

    let book = Book {
        id: book_id,
        title: extracted.title,
        author: extracted.author,
        description: extracted.description,
        cover_path: None,
        file_path: rel_file_path.clone(),
        format: "pdf".to_string(),
        source_format: Some("pdf".to_string()),
        render_format: Some("pdf".to_string()),
        source_file_path: Some(rel_file_path.clone()),
        source_sha256: Some(source_sha256),
        conversion_version: 0,
        preparation_state: default_preparation_state(),
        preparation_error: None,
        genre: None,
        pages: Some(extracted.pages),
        status: "unread".to_string(),
        progress: 0,
        current_cfi: None,
        created_at: now,
        updated_at: now,
        available: true,
        cover_data: cover_data_b64,
    };

    do_insert_book(&book, extracted.cover.as_deref(), db, sync, now)?;
    cleanup.commit();

    log::info!(
        "import_book: complete id={} title={:?} format=pdf cover={} | extract={:?} copy={:?}",
        book.id,
        book.title,
        extracted.cover.is_some(),
        t_extract,
        t_copy,
    );
    Ok(book)
}

fn do_insert_book(
    book: &Book,
    cover_bytes: Option<&[u8]>,
    db: &Db,
    sync: &SyncWriter,
    now: i64,
) -> AppResult<()> {
    let device = sync.self_device().to_string();
    sync.with_tx(db, now, |tx, events| {
        tx.execute(
            "INSERT INTO books (id, title, author, description, cover_path, file_path, format, source_format, render_format, source_file_path, source_sha256, conversion_version, preparation_state, preparation_error, genre, pages, status, progress, current_cfi, created_at, updated_at, updated_by_device, cover_data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
            params![
                book.id,
                book.title,
                book.author,
                book.description,
                book.cover_path,
                book.file_path,
                book.format,
                book.source_format.as_deref().unwrap_or(&book.format),
                book.render_format.as_deref().unwrap_or(&book.format),
                book.source_file_path,
                book.source_sha256,
                book.conversion_version,
                book.preparation_state,
                book.preparation_error,
                book.genre,
                book.pages,
                book.status,
                book.progress,
                book.current_cfi,
                book.created_at,
                book.updated_at,
                device,
                cover_bytes,
            ],
        )?;
        events.push(EventBody::BookImport(BookImportPayload {
            id: book.id.clone(),
            title: book.title.clone(),
            author: book.author.clone(),
            description: book.description.clone(),
            cover_path: book.cover_path.clone(),
            file_path: book.file_path.clone(),
            format: book.format.clone(),
            source_format: book.source_format.clone(),
            render_format: book.render_format.clone(),
            source_file_path: book.source_file_path.clone(),
            source_sha256: book.source_sha256.clone(),
            conversion_version: book.conversion_version,
            genre: book.genre.clone(),
            pages: book.pages,
        }));
        Ok(())
    })?;
    if let Some(bytes) = cover_bytes {
        sync.queue_cover_write(db, &book.id, bytes);
    }
    Ok(())
}

fn import_user_selected_path(
    path: &Path,
    db: &Db,
    sync: &SyncWriter,
    app: &AppHandle,
) -> AppResult<Book> {
    let file_path = path
        .to_str()
        .ok_or_else(|| AppError::Other("BOOK_IMPORT_PATH_INVALID".to_string()))?;
    let mut book = do_import_from_path(file_path, db, sync)?;
    if book.render_format.as_deref() == Some("text") {
        schedule_text_book_preparation(app.clone(), book.id.clone());
    }
    resolve_book_paths(&mut book, db)?;
    Ok(book)
}

/// Import through a native file chooser. The webview never supplies an
/// arbitrary path, which keeps this command within Tauri's file-scope model.
#[tauri::command]
pub async fn import_book_from_dialog(
    app: AppHandle,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<Option<Book>> {
    let Some(selected) = app
        .dialog()
        .file()
        .add_filter("Books", IMPORTABLE_BOOK_EXTENSIONS)
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = selected
        .into_path()
        .map_err(|_| AppError::Other("BOOK_IMPORT_PATH_INVALID".to_string()))?;
    import_user_selected_path(&path, &db, &sync, &app).map(Some)
}

/// Native drag/drop and OS file-association events have already been approved
/// by the operating system. They are handled in Rust instead of forwarding a
/// path through a webview command.
pub(crate) fn import_external_paths(
    paths: Vec<PathBuf>,
    db: &Db,
    sync: &SyncWriter,
    app: &AppHandle,
) {
    for path in paths {
        if !path.is_file() || !is_importable_book_path(&path) {
            continue;
        }
        match import_user_selected_path(path.as_path(), db, sync, app) {
            Ok(book) => {
                let _ = app.emit("book-imported", book);
            }
            Err(error) => {
                log::warn!(
                    "import_book: native import failed for {}: {error}",
                    path.display()
                );
                let _ = app.emit("book-import-failed", error.to_string());
            }
        }
    }
}

fn is_importable_book_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            IMPORTABLE_BOOK_EXTENSIONS
                .iter()
                .any(|allowed| extension.eq_ignore_ascii_case(allowed))
        })
}

pub(crate) fn do_import_from_path(file_path: &str, db: &Db, sync: &SyncWriter) -> AppResult<Book> {
    let _mutation = sync.mutation_guard()?;
    let format = detect_import_format(Path::new(&file_path))?;
    log::info!(
        "import_book: start file={file_path} format={}",
        format.source_name()
    );
    match format {
        ImportFormat::Pdf => do_import_pdf(file_path, db, sync),
        ImportFormat::Epub => do_import_epub(file_path, db, sync),
        ImportFormat::Txt => do_import_text(file_path, "txt", db, sync),
        ImportFormat::Markdown => do_import_text(file_path, "markdown", db, sync),
        ImportFormat::Html => do_import_text(file_path, "html", db, sync),
        ImportFormat::Mobi | ImportFormat::Fb2 | ImportFormat::Fbz | ImportFormat::Cbz => {
            do_import_native(file_path, format, db, sync)
        }
    }
}
