//! Source-format → EPUB conversion pipeline (skeleton).
//!
//! Mirrors the crash-safe state machine in [`super::text_prepare`], but drives
//! books whose reader format is EPUB while their *source* is some other format
//! (MOBI/AZW/AZW3 today; scanned PDF later). The converted EPUB is a **local,
//! non-synced derivative**: it lives under `local_data_dir()/prepared/` exactly
//! like the text pipeline's prepared JSON, so every device re-converts from the
//! synced source and no derived bytes ever enter iCloud.
//!
//! The machine is converter-agnostic behind the [`Converter`] seam. The
//! production backend (route A) shells out to Calibre's `ebook-convert` for
//! MOBI-family sources; import only routes a book through this machine when
//! that backend is detected, so machines without Calibre gracefully keep the
//! current native-reader behaviour for new imports. Scanned-PDF OCR arrives in
//! a later phase as another `Converter`.

use super::*;
use std::ffi::OsStr;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Pluggable conversion backend. Implementations read the synced source file
/// and write a valid EPUB to `dest`. They must not touch the database or the
/// source file; the state machine owns all of that.
pub(super) trait Converter: Send + Sync {
    /// Convert `source` (of `source_format`) into an EPUB written to `dest`.
    /// `dest`'s parent directory is guaranteed to exist. On error the machine
    /// marks the book `failed` and surfaces the message.
    fn convert(&self, source: &Path, dest: &Path, source_format: &str) -> AppResult<()>;
}

/// Fallback for source formats no production backend handles yet: fail
/// cleanly rather than silently producing nothing.
pub(super) struct UnsupportedConverter;

impl Converter for UnsupportedConverter {
    fn convert(&self, _source: &Path, _dest: &Path, source_format: &str) -> AppResult<()> {
        Err(AppError::Other(format!(
            "CONVERSION_UNSUPPORTED:{source_format}"
        )))
    }
}

// ---------------------------------------------------------------------------
// Route A: Calibre's `ebook-convert` as the MOBI-family backend.
// ---------------------------------------------------------------------------

/// Source formats route A can convert today.
const CALIBRE_SOURCE_FORMATS: &[&str] = &["mobi", "azw", "azw3"];

/// Hard cap on a single `ebook-convert` run. Large KF8 books convert in tens
/// of seconds; ten minutes means the tool is wedged, not slow.
const CONVERSION_TIMEOUT: Duration = Duration::from_secs(600);

/// `ebook-convert --version` answers within seconds even on cold start;
/// anything slower is not a usable installation.
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

struct CommandOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

/// Run `executable` with `args`, killing it at `timeout`. Arguments are passed
/// as an array — never through a shell — so book paths cannot smuggle options
/// or commands. Both pipes are drained on threads so a chatty child cannot
/// dead-lock against a full pipe while we poll for exit.
fn run_command_with_timeout(
    executable: &Path,
    args: &[&OsStr],
    timeout: Duration,
) -> AppResult<CommandOutput> {
    let mut child = Command::new(executable)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| AppError::Other(format!("CONVERTER_SPAWN_FAILED:{error}")))?;

    fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> std::thread::JoinHandle<String> {
        std::thread::spawn(move || {
            let mut buffer = String::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_string(&mut buffer);
            }
            buffer
        })
    }
    let stdout_thread = drain(child.stdout.take());
    let stderr_thread = drain(child.stderr.take());

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stdout_thread.join();
                    let _ = stderr_thread.join();
                    return Err(AppError::Other("CONVERSION_TIMEOUT".to_string()));
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AppError::Other(error.to_string()));
            }
        }
    };
    Ok(CommandOutput {
        success: status.success(),
        stdout: stdout_thread.join().unwrap_or_default(),
        stderr: stderr_thread.join().unwrap_or_default(),
    })
}

/// Locate a working `ebook-convert`, validating each candidate with a version
/// probe (a stray non-Calibre binary of the same name is rejected). GUI apps
/// launch with a minimal PATH on macOS, so the standard install locations are
/// probed alongside it.
fn detect_ebook_convert() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(path_var) = std::env::var("PATH") {
        candidates.extend(std::env::split_paths(&path_var).map(|dir| dir.join("ebook-convert")));
    }
    candidates.push(PathBuf::from("/opt/homebrew/bin/ebook-convert"));
    candidates.push(PathBuf::from("/usr/local/bin/ebook-convert"));
    candidates.push(PathBuf::from(
        "/Applications/calibre.app/Contents/MacOS/ebook-convert",
    ));
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(
            Path::new(&home).join("Applications/calibre.app/Contents/MacOS/ebook-convert"),
        );
    }
    candidates.into_iter().find(|path| {
        path.is_file()
            && run_command_with_timeout(path, &[OsStr::new("--version")], VERSION_PROBE_TIMEOUT)
                .map(|output| output.success && output.stdout.contains("calibre"))
                .unwrap_or(false)
    })
}

/// Import-time gate: route a fresh import through the conversion pipeline
/// only when a backend for its format is actually present, so machines
/// without Calibre keep the native read-only path (graceful degradation).
pub(crate) fn conversion_backend_available(source_format: &str) -> bool {
    CALIBRE_SOURCE_FORMATS.contains(&source_format) && detect_ebook_convert().is_some()
}

/// Single line of tool output short enough for `preparation_error` (the UI
/// surfaces it verbatim on failure).
fn tool_error_excerpt(output: &CommandOutput) -> String {
    let text = if output.stderr.trim().is_empty() {
        &output.stdout
    } else {
        &output.stderr
    };
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("no tool output")
        .chars()
        .take(300)
        .collect()
}

struct CalibreConverter {
    executable: PathBuf,
}

impl Converter for CalibreConverter {
    fn convert(&self, source: &Path, dest: &Path, _source_format: &str) -> AppResult<()> {
        // `ebook-convert` infers the output format from `dest`'s extension,
        // which is why the machine's temp sidecar must end in `.epub`.
        let output = run_command_with_timeout(
            &self.executable,
            &[source.as_os_str(), dest.as_os_str()],
            CONVERSION_TIMEOUT,
        )?;
        if !output.success {
            return Err(AppError::Other(format!(
                "CONVERSION_TOOL_FAILED:{}",
                tool_error_excerpt(&output)
            )));
        }
        Ok(())
    }
}

/// Production dispatch by source format. Calibre is re-detected per job, so
/// installing it after a failure makes the retry just work; a job for a book
/// imported while Calibre was present but run after its removal fails with a
/// clear `CALIBRE_MISSING` instead of hanging.
struct ProductionConverter;

impl Converter for ProductionConverter {
    fn convert(&self, source: &Path, dest: &Path, source_format: &str) -> AppResult<()> {
        if CALIBRE_SOURCE_FORMATS.contains(&source_format) {
            let executable = detect_ebook_convert()
                .ok_or_else(|| AppError::Other("CALIBRE_MISSING".to_string()))?;
            return CalibreConverter { executable }.convert(source, dest, source_format);
        }
        UnsupportedConverter.convert(source, dest, source_format)
    }
}

fn production_converter() -> &'static dyn Converter {
    &ProductionConverter
}

#[derive(Debug, Serialize, Clone)]
struct ConversionChanged {
    book_id: String,
    state: String,
}

/// True when a book renders as EPUB but originated from a different source
/// format — i.e. it is a conversion artifact rather than a native EPUB. A
/// missing/`"epub"` source format means native EPUB (not converted).
pub(crate) fn is_conversion_book(render_format: Option<&str>, source_format: Option<&str>) -> bool {
    render_format == Some("epub") && !matches!(source_format, None | Some("epub"))
}

/// Local, non-synced path of the converted EPUB for `book_id`. Versioned so a
/// `CONVERSION_VERSION` bump invalidates stale artifacts without a migration.
pub(crate) fn converted_document_path(local_dir: &Path, book_id: &str) -> PathBuf {
    local_dir
        .join("prepared")
        .join(format!("{book_id}.converted.v{CONVERSION_VERSION}.epub"))
}

fn cleanup_obsolete_converted_documents(local_dir: &Path, book_id: &str) {
    // Empty while CONVERSION_VERSION == 1 (no prior versions to sweep); becomes
    // a real range once the version is bumped and old artifacts must be purged.
    #[allow(clippy::reversed_empty_ranges)]
    for version in 1..CONVERSION_VERSION {
        let path = local_dir
            .join("prepared")
            .join(format!("{book_id}.converted.v{version}.epub"));
        let _ = fs::remove_file(&path);
    }
    // Sweep the in-progress temp sidecar for the current version, if orphaned.
    if let Ok(tmp) = converted_temporary_path(&converted_document_path(local_dir, book_id)) {
        let _ = fs::remove_file(tmp);
    }
}

/// In-progress sidecar next to the final artifact. Hidden (dot-prefixed) so
/// it is clearly not a finished book, but still `.epub`-suffixed because
/// converters like `ebook-convert` choose their output format from the
/// destination extension.
fn converted_temporary_path(path: &Path) -> AppResult<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Other("CONVERSION_PATH_INVALID".to_string()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| AppError::Other("CONVERSION_PATH_INVALID".to_string()))?
        .to_string_lossy();
    Ok(parent.join(format!(".{file_name}.tmp.epub")))
}

/// Whether a usable converted EPUB already exists on disk for this version.
pub(crate) fn converted_artifact_exists(local_dir: &Path, book_id: &str) -> bool {
    converted_document_path(local_dir, book_id)
        .metadata()
        .map(|meta| meta.is_file() && meta.len() > 0)
        .unwrap_or(false)
}

#[derive(Clone, Debug)]
struct ConversionSource {
    file_path: Option<String>,
    format: Option<String>,
    sha256: Option<String>,
    conversion_version: i32,
}

/// The predicate identifying a conversion job at a given state. Kept in one
/// place so every guarded UPDATE stays consistent.
const CONVERSION_PREDICATE: &str =
    "render_format = 'epub' AND source_format IS NOT NULL AND source_format <> 'epub'";

fn emit_conversion_changed(app: &AppHandle, book_id: &str, state: &str) {
    let _ = app.emit(
        "book-preparation-changed",
        ConversionChanged {
            book_id: book_id.to_string(),
            state: state.to_string(),
        },
    );
}

pub(super) fn transition_conversion_state(
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
        &format!(
            "UPDATE books
             SET preparation_state = ?1, preparation_error = ?2
             WHERE id = ?3 AND {CONVERSION_PREDICATE} AND preparation_state = ?4"
        ),
        params![next_state, error, book_id, expected_state],
    )?;
    Ok(changed == 1)
}

/// Confirm the still-preparing job matches the source snapshot we started from,
/// so a concurrent re-import/format change discards this stale worker.
fn conversion_job_is_current(
    conn: &rusqlite::Connection,
    book_id: &str,
    source: &ConversionSource,
) -> AppResult<bool> {
    let current = conn.query_row(
        &format!(
            "SELECT EXISTS(
                 SELECT 1 FROM books
                 WHERE id = ?1 AND {CONVERSION_PREDICATE}
                   AND preparation_state = 'preparing'
                   AND source_file_path IS ?2
                   AND source_format IS ?3
                   AND source_sha256 IS ?4
                   AND COALESCE(conversion_version, 0) = ?5
             )"
        ),
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

fn update_current_conversion_job(
    db: &Db,
    book_id: &str,
    source: &ConversionSource,
    next_state: &str,
    error: Option<&str>,
) -> AppResult<bool> {
    let conn = db
        .conn
        .lock()
        .map_err(|lock_error| AppError::Other(lock_error.to_string()))?;
    let changed = conn.execute(
        &format!(
            "UPDATE books
             SET preparation_state = ?1, preparation_error = ?2
             WHERE id = ?3 AND {CONVERSION_PREDICATE}
               AND preparation_state = 'preparing'
               AND source_file_path IS ?4
               AND source_format IS ?5
               AND source_sha256 IS ?6
               AND COALESCE(conversion_version, 0) = ?7"
        ),
        params![
            next_state,
            error,
            book_id,
            source.file_path.as_deref(),
            source.format.as_deref(),
            source.sha256.as_deref(),
            source.conversion_version,
        ],
    )?;
    Ok(changed == 1)
}

fn publish_current_conversion_job(
    db: &Db,
    book_id: &str,
    source: &ConversionSource,
) -> AppResult<bool> {
    let conn = db
        .conn
        .lock()
        .map_err(|lock_error| AppError::Other(lock_error.to_string()))?;
    if !conversion_job_is_current(&conn, book_id, source)? {
        return Ok(false);
    }
    let changed = conn.execute(
        &format!(
            "UPDATE books
             SET preparation_state = 'ready', preparation_error = NULL
             WHERE id = ?1 AND {CONVERSION_PREDICATE}
               AND preparation_state = 'preparing'
               AND source_file_path IS ?2
               AND source_format IS ?3
               AND source_sha256 IS ?4
               AND COALESCE(conversion_version, 0) = ?5"
        ),
        params![
            book_id,
            source.file_path.as_deref(),
            source.format.as_deref(),
            source.sha256.as_deref(),
            source.conversion_version,
        ],
    )?;
    Ok(changed == 1)
}

/// Core job body, parameterized on the converter so tests can inject a fake.
fn run_conversion_with(
    app: &AppHandle,
    book_id: &str,
    converter: &dyn Converter,
) -> AppResult<()> {
    crate::sync::validation::validate_entity_id(book_id)?;
    let db = app.state::<Db>();
    let local_dir = app.state::<LocalDir>();

    // Claim the job: pending → preparing, capturing the source snapshot.
    let source = {
        let conn = db
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let changed = conn.execute(
            &format!(
                "UPDATE books
                 SET preparation_state = 'preparing', preparation_error = NULL
                 WHERE id = ?1 AND {CONVERSION_PREDICATE} AND preparation_state = 'pending'"
            ),
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
                Ok(ConversionSource {
                    file_path: row.get(0)?,
                    format: row.get(1)?,
                    sha256: row.get(2)?,
                    conversion_version: row.get(3)?,
                })
            },
        )?
    };
    emit_conversion_changed(app, book_id, "preparing");

    let dest = converted_document_path(&local_dir.0, book_id);

    // A usable artifact already on disk (e.g. interrupted after write, before
    // the DB flip) short-circuits to ready.
    if converted_artifact_exists(&local_dir.0, book_id)
        && publish_current_conversion_job(&db, book_id, &source)?
    {
        cleanup_obsolete_converted_documents(&local_dir.0, book_id);
        emit_conversion_changed(app, book_id, "ready");
        crate::ai::grounding::index::schedule_index(app.clone(), book_id.to_string());
        return Ok(());
    }

    let Some(source_file_path) = source.file_path.as_deref() else {
        if update_current_conversion_job(&db, book_id, &source, "failed", Some("SOURCE_MISSING"))? {
            emit_conversion_changed(app, book_id, "failed");
        }
        return Ok(());
    };
    let source_path = match db.resolve_path(source_file_path) {
        Ok(path) => path,
        Err(error) => {
            let message = error.to_string();
            if update_current_conversion_job(&db, book_id, &source, "failed", Some(&message))? {
                emit_conversion_changed(app, book_id, "failed");
            }
            return Ok(());
        }
    };
    match icloud::file_availability(&source_path) {
        icloud::FileAvailability::Available => {}
        icloud::FileAvailability::ICloudPlaceholder => {
            icloud::trigger_download_file(&source_path);
            if update_current_conversion_job(&db, book_id, &source, "pending", None)? {
                emit_conversion_changed(app, book_id, "pending");
            }
            return Ok(());
        }
        icloud::FileAvailability::Missing => {
            if update_current_conversion_job(
                &db,
                book_id,
                &source,
                "failed",
                Some("SOURCE_UNAVAILABLE"),
            )? {
                emit_conversion_changed(app, book_id, "failed");
            }
            return Ok(());
        }
    }

    // Convert into a temp sidecar, then atomically promote to the final path so
    // a crash never leaves a half-written EPUB where a `ready` row points.
    let tmp = converted_temporary_path(&dest)?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let _ = fs::remove_file(&tmp);
    let source_format = source.format.as_deref().unwrap_or("");
    let result = converter
        .convert(&source_path, &tmp, source_format)
        .and_then(|()| {
            if !tmp.is_file() {
                return Err(AppError::Other("CONVERSION_EMPTY_OUTPUT".to_string()));
            }
            let _ = fs::remove_file(&dest);
            fs::rename(&tmp, &dest)?;
            publish_current_conversion_job(&db, book_id, &source)
        });

    match result {
        Ok(true) => {
            cleanup_obsolete_converted_documents(&local_dir.0, book_id);
            emit_conversion_changed(app, book_id, "ready");
            crate::ai::grounding::index::schedule_index(app.clone(), book_id.to_string());
        }
        Ok(false) => {
            // Job went stale mid-flight (re-import/format change). Drop our
            // artifact so the current job re-converts cleanly.
            let _ = fs::remove_file(&dest);
            let _ = fs::remove_file(&tmp);
            log::debug!("discarded stale conversion task for {book_id}");
        }
        Err(error) => {
            let _ = fs::remove_file(&tmp);
            let message = error.to_string();
            if update_current_conversion_job(&db, book_id, &source, "failed", Some(&message))? {
                emit_conversion_changed(app, book_id, "failed");
            }
            log::warn!("conversion failed for {book_id}: {message}");
        }
    }
    Ok(())
}

fn run_conversion(app: &AppHandle, book_id: &str) -> AppResult<()> {
    run_conversion_with(app, book_id, production_converter())
}

pub fn schedule_book_conversion(app: AppHandle, book_id: String) {
    let thread_name = format!("convert-{}", &book_id[..book_id.len().min(8)]);
    let _ = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            if let Err(error) = run_conversion(&app, &book_id) {
                log::warn!("conversion task failed for {book_id}: {error}");
            }
        });
}

fn pending_conversion_book_ids(db: &Db, recover_interrupted: bool) -> AppResult<Vec<String>> {
    let conn = db
        .conn
        .lock()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if recover_interrupted {
        conn.execute(
            &format!(
                "UPDATE books SET preparation_state = 'pending', preparation_error = NULL
                 WHERE {CONVERSION_PREDICATE} AND preparation_state = 'preparing'"
            ),
            [],
        )?;
    }
    let mut statement = conn.prepare(&format!(
        "SELECT id FROM books
         WHERE {CONVERSION_PREDICATE} AND preparation_state = 'pending'
         ORDER BY id"
    ))?;
    let book_ids = statement
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()
        .map_err(AppError::from)?;
    Ok(book_ids)
}

fn schedule_pending_book_conversions_inner(app: AppHandle, recover_interrupted: bool) {
    let db = app.state::<Db>();
    match pending_conversion_book_ids(&db, recover_interrupted) {
        Ok(book_ids) => {
            for book_id in book_ids {
                schedule_book_conversion(app.clone(), book_id);
            }
        }
        Err(error) => log::warn!("conversion startup scan failed: {error}"),
    }
}

pub fn resume_interrupted_book_conversions(app: AppHandle) {
    schedule_pending_book_conversions_inner(app, true);
}

pub fn schedule_pending_book_conversions(app: AppHandle) {
    schedule_pending_book_conversions_inner(app, false);
}

/// Reader-side: return the local converted EPUB path, kicking preparation if
/// needed. Mirrors `get_text_book_document`'s state handling.
#[tauri::command]
pub fn get_converted_book_path(
    book_id: String,
    db: State<'_, Db>,
    local_dir: State<'_, LocalDir>,
    app: AppHandle,
) -> AppResult<String> {
    crate::sync::validation::validate_entity_id(&book_id)?;
    let state: (String, Option<String>) = {
        let conn = db.reader();
        conn.query_row(
            &format!(
                "SELECT preparation_state, preparation_error
                 FROM books WHERE id = ?1 AND {CONVERSION_PREDICATE}"
            ),
            params![book_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?
    };
    match state.0.as_str() {
        "ready" => {
            let path = converted_document_path(&local_dir.0, &book_id);
            if path.is_file() {
                Ok(path.to_string_lossy().to_string())
            } else {
                // Artifact vanished; re-run and report pending.
                if transition_conversion_state(&db, &book_id, "ready", "pending", None)? {
                    schedule_book_conversion(app, book_id);
                }
                Err(AppError::Other("CONVERSION_PENDING".to_string()))
            }
        }
        "pending" => {
            schedule_book_conversion(app, book_id);
            Err(AppError::Other("CONVERSION_PENDING".to_string()))
        }
        "preparing" => Err(AppError::Other("CONVERSION_PENDING".to_string())),
        "failed" => Err(AppError::Other(format!(
            "CONVERSION_FAILED:{}",
            state.1.unwrap_or_else(|| "UNKNOWN".to_string())
        ))),
        _ => Err(AppError::Other("CONVERSION_PENDING".to_string())),
    }
}

#[tauri::command]
pub fn retry_book_conversion(
    book_id: String,
    db: State<'_, Db>,
    app: AppHandle,
) -> AppResult<()> {
    crate::sync::validation::validate_entity_id(&book_id)?;
    if transition_conversion_state(&db, &book_id, "failed", "pending", None)? {
        emit_conversion_changed(&app, &book_id, "pending");
        schedule_book_conversion(app, book_id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct FakeConverter {
        bytes: Vec<u8>,
    }

    impl Converter for FakeConverter {
        fn convert(&self, _source: &Path, dest: &Path, _source_format: &str) -> AppResult<()> {
            fs::write(dest, &self.bytes)?;
            Ok(())
        }
    }

    fn setup() -> (TempDir, Db) {
        let dir = TempDir::new().unwrap();
        let db = Db::init(dir.path()).unwrap();
        (dir, db)
    }

    /// Insert a conversion book (EPUB render format from a MOBI source).
    fn insert_conversion_book(db: &Db, id: &str, state: &str, sha: &str) {
        let conn = db.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO books
             (id, title, author, file_path, format, source_format, render_format,
              source_file_path, source_sha256, conversion_version, preparation_state,
              preparation_error, status, progress, created_at, updated_at)
             VALUES (?1, 'Book', 'Author', ?2, 'mobi', 'mobi', 'epub', ?2, ?3, ?4, ?5,
                     'existing error', 'reading', 0, ?6, ?6)",
            params![id, format!("books/{id}.azw3"), sha, CONVERSION_VERSION, state, now],
        )
        .unwrap();
    }

    fn conversion_source(id: &str, sha: &str) -> ConversionSource {
        ConversionSource {
            file_path: Some(format!("books/{id}.azw3")),
            format: Some("mobi".to_string()),
            sha256: Some(sha.to_string()),
            conversion_version: CONVERSION_VERSION,
        }
    }

    #[test]
    fn is_conversion_book_discriminates_native_epub() {
        assert!(is_conversion_book(Some("epub"), Some("mobi")));
        assert!(is_conversion_book(Some("epub"), Some("azw3")));
        // Native EPUB and text/pdf are not conversion books.
        assert!(!is_conversion_book(Some("epub"), Some("epub")));
        assert!(!is_conversion_book(Some("epub"), None));
        assert!(!is_conversion_book(Some("text"), Some("txt")));
        assert!(!is_conversion_book(Some("pdf"), Some("pdf")));
    }

    #[test]
    fn converted_path_is_versioned() {
        let path = converted_document_path(Path::new("/tmp/local"), "abc");
        assert_eq!(
            path,
            Path::new("/tmp/local")
                .join("prepared")
                .join(format!("abc.converted.v{CONVERSION_VERSION}.epub"))
        );
    }

    #[test]
    fn transition_state_is_guarded_and_scoped() {
        let (_dir, db) = setup();
        insert_conversion_book(&db, "c1", "failed", "hash1");
        // failed → pending succeeds once, then the guard blocks a repeat.
        assert!(transition_conversion_state(&db, "c1", "failed", "pending", None).unwrap());
        assert!(!transition_conversion_state(&db, "c1", "failed", "pending", None).unwrap());
        assert!(!transition_conversion_state(&db, "c1", "ready", "pending", None).unwrap());
    }

    #[test]
    fn native_epub_never_matches_conversion_predicate() {
        let (_dir, db) = setup();
        {
            let conn = db.conn.lock().unwrap();
            let now = chrono::Utc::now().timestamp_millis();
            conn.execute(
                "INSERT INTO books
                 (id, title, author, file_path, format, source_format, render_format,
                  preparation_state, status, progress, created_at, updated_at)
                 VALUES ('native', 'E', 'A', 'books/e.epub', 'epub', 'epub', 'epub',
                         'failed', 'reading', 0, ?1, ?1)",
                params![now],
            )
            .unwrap();
        }
        // A native EPUB in 'failed' must not be reachable by the conversion
        // state machine.
        assert!(!transition_conversion_state(&db, "native", "failed", "pending", None).unwrap());
        assert!(pending_conversion_book_ids(&db, true).unwrap().is_empty());
    }

    #[test]
    fn publish_flips_ready_and_guards_stale_source() {
        let (_dir, db) = setup();
        insert_conversion_book(&db, "c2", "preparing", "hash2");
        let source = conversion_source("c2", "hash2");
        // A stale snapshot (different hash) must not publish.
        let stale = conversion_source("c2", "OTHER");
        assert!(!publish_current_conversion_job(&db, "c2", &stale).unwrap());
        // The matching snapshot flips preparing → ready and clears the error.
        assert!(publish_current_conversion_job(&db, "c2", &source).unwrap());
        let (state, error): (String, Option<String>) = db
            .reader()
            .query_row(
                "SELECT preparation_state, preparation_error FROM books WHERE id = 'c2'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, "ready");
        assert_eq!(error, None);
    }

    #[test]
    fn pending_scan_recovers_interrupted_only_with_flag() {
        let (_dir, db) = setup();
        insert_conversion_book(&db, "active", "preparing", "h-a");
        insert_conversion_book(&db, "queued", "pending", "h-q");
        // Without recovery: only the already-pending job is returned; the
        // in-flight 'preparing' one is left alone.
        assert_eq!(pending_conversion_book_ids(&db, false).unwrap(), ["queued"]);
        let active_state: String = db
            .reader()
            .query_row(
                "SELECT preparation_state FROM books WHERE id = 'active'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(active_state, "preparing");
        // With recovery: the interrupted 'preparing' job is reset to pending.
        let mut ids = pending_conversion_book_ids(&db, true).unwrap();
        ids.sort();
        assert_eq!(ids, ["active", "queued"]);
    }

    #[test]
    fn unsupported_converter_reports_format() {
        let err = UnsupportedConverter
            .convert(Path::new("/a"), Path::new("/b"), "mobi")
            .unwrap_err();
        assert!(err.to_string().contains("CONVERSION_UNSUPPORTED:mobi"));
    }

    #[test]
    fn fake_converter_writes_output() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("out.epub");
        FakeConverter { bytes: b"PK\x03\x04epub".to_vec() }
            .convert(Path::new("/src"), &dest, "mobi")
            .unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"PK\x03\x04epub");
    }
}
