use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::ai::grounding::index::schedule_index;
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::sync::writer::SyncWriter;

use super::backend::{
    recognize_pdf, CancellationToken, OcrBackend, OcrProgress, OcrmypdfBackend, RecognitionRequest,
};
use super::jobs::{
    create_job, fail_or_cancel_job, finish_job, get_active_job, get_job, get_latest_job,
    mark_interrupted_jobs_failed, update_state_guarded, NewOcrJob, OcrJob, OcrJobUpdate,
};
use super::package::acquire_installed_runtime;
use super::publish::{publish_verified_output, NewAssetRow};
use super::validate::{reject_signed_pdf, validate_output};

const CONVERSION_VERSION: i32 = 1;
const EVENT_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OcrJobView {
    state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pages_done: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pages_total: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OcrAssetItem {
    asset_id: String,
    book_id: String,
    title: String,
    byte_size: i64,
    created_at: i64,
    availability: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OcrAssetsOverview {
    total_bytes: i64,
    items: Vec<OcrAssetItem>,
}

#[derive(Clone, Default)]
pub(crate) struct OcrJobManager {
    inner: Arc<OcrJobManagerInner>,
}

#[derive(Default)]
struct OcrJobManagerInner {
    queue: Mutex<VecDeque<String>>,
    worker_running: AtomicBool,
    cancellations: Mutex<HashMap<String, CancellationToken>>,
    last_event: Mutex<HashMap<String, Instant>>,
}

struct StagingCleanup(PathBuf);

impl Drop for StagingCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl OcrJobManager {
    pub(crate) fn recover_interrupted(&self, db: &Db) -> AppResult<usize> {
        let conn = db
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        mark_interrupted_jobs_failed(&conn, chrono::Utc::now().timestamp_millis())
    }

    fn enqueue(&self, app: AppHandle, job_id: String) {
        let inserted = if let Ok(mut cancellations) = self.inner.cancellations.lock() {
            if cancellations.contains_key(&job_id) {
                false
            } else {
                cancellations.insert(job_id.clone(), CancellationToken::default());
                true
            }
        } else {
            false
        };
        if !inserted {
            return;
        }
        if let Ok(mut queue) = self.inner.queue.lock() {
            queue.push_back(job_id);
        }
        if self
            .inner
            .worker_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let manager = self.clone();
            tauri::async_runtime::spawn_blocking(move || manager.drain_queue(app));
        }
    }

    fn drain_queue(&self, app: AppHandle) {
        loop {
            let job_id = match self.inner.queue.lock() {
                Ok(mut queue) => match queue.pop_front() {
                    Some(job_id) => Some(job_id),
                    None => {
                        self.inner.worker_running.store(false, Ordering::Release);
                        None
                    }
                },
                Err(_) => {
                    self.inner.worker_running.store(false, Ordering::Release);
                    None
                }
            };
            let Some(job_id) = job_id else {
                return;
            };
            if let Err(error) = self.run_job(&app, &job_id) {
                log::warn!("OCR job {job_id} stopped: {error}");
            }
            if let Ok(mut cancellations) = self.inner.cancellations.lock() {
                cancellations.remove(&job_id);
            }
            if let Ok(mut last_event) = self.inner.last_event.lock() {
                last_event.remove(&job_id);
            }
        }
    }

    fn run_job(&self, app: &AppHandle, job_id: &str) -> AppResult<()> {
        let db = app.state::<Db>();
        let job = {
            let conn = db.reader();
            get_job(&conn, job_id)?.ok_or_else(|| ocr_error("OCR_JOB_NOT_FOUND"))?
        };
        if job.state == "cancelled" {
            return Ok(());
        }
        let cancel = self
            .inner
            .cancellations
            .lock()
            .ok()
            .and_then(|tokens| tokens.get(job_id).cloned())
            .ok_or_else(|| ocr_error("OCR_JOB_CANCELLED"))?;

        let result = self.run_job_inner(app, &job, &cancel);
        if let Err(error) = result {
            let cancelled =
                cancel.is_cancelled() || error.to_string().contains("OCR_JOB_CANCELLED");
            let code = (!cancelled).then(|| stable_job_error(&error));
            let detail = (!cancelled).then(|| truncate_detail(&error.to_string()));
            let updated = {
                let conn = db
                    .conn
                    .lock()
                    .map_err(|lock| AppError::Other(lock.to_string()))?;
                fail_or_cancel_job(
                    &conn,
                    &job.id,
                    &job.source_sha256,
                    cancelled,
                    code.as_deref(),
                    detail.as_deref(),
                    chrono::Utc::now().timestamp_millis(),
                )?
            };
            emit_job_changed(app, &updated);
            return Err(error);
        }
        Ok(())
    }

    fn run_job_inner(
        &self,
        app: &AppHandle,
        job: &OcrJob,
        cancel: &CancellationToken,
    ) -> AppResult<()> {
        let db = app.state::<Db>();
        let source = match load_source(&db, &job.book_id, &job.source_sha256) {
            Ok(source) if source.is_file() => source,
            _ => {
                let waiting = transition_job(&db, job, "waiting_source", None, None, None)?;
                emit_job_changed(app, &waiting);
                return Ok(());
            }
        };
        cancel.check()?;

        let runtime =
            acquire_installed_runtime().ok_or_else(|| ocr_error("OCR_RUNTIME_NOT_INSTALLED"))?;
        let backend = OcrmypdfBackend::from_runtime(&runtime);
        let capabilities = backend.probe()?;
        let preparing = transition_job(&db, job, "preparing", Some("analyzing"), None, None)?;
        emit_job_changed(app, &preparing);
        reject_signed_pdf(&source)?;

        let staging_root = db.local_data_dir()?.join("ocr-staging").join(&job.id);
        if staging_root.exists() {
            fs::remove_dir_all(&staging_root)?;
        }
        fs::create_dir_all(&staging_root)?;
        let _cleanup = StagingCleanup(staging_root.clone());
        let recognizing =
            transition_job(&db, job, "recognizing", Some("analyzing"), Some(0), None)?;
        emit_job_changed(app, &recognizing);

        let request = RecognitionRequest {
            source: source.clone(),
            destination: staging_root.join("result.partial.pdf"),
            staging_root,
            source_sha256: job.source_sha256.clone(),
            language_profile: "chi_sim+eng".to_string(),
            quality_profile: "fast".to_string(),
            jobs: job.jobs.unwrap_or(1).clamp(1, 4) as u8,
        };
        let mut progress = |progress: OcrProgress| self.report_progress(app, job, progress);
        let output = recognize_pdf(&backend, &request, &mut progress, cancel)?;
        cancel.check()?;
        let validating = transition_job(
            &db,
            job,
            "validating",
            Some("finalizing"),
            Some(output.page_count),
            Some(output.page_count),
        )?;
        emit_job_changed(app, &validating);
        let verified = validate_output(&source, output)?;
        cancel.check()?;
        let publishing = transition_job(
            &db,
            job,
            "publishing",
            Some("finalizing"),
            Some(verified.page_count),
            Some(verified.page_count),
        )?;
        emit_job_changed(app, &publishing);

        let now = chrono::Utc::now().timestamp_millis();
        let asset_id = publish_verified_output(
            &db,
            NewAssetRow {
                book_id: job.book_id.clone(),
                source_sha256: job.source_sha256.clone(),
                pipeline_version: Some(capabilities.version),
                supersedes_asset_id: None,
                verified: verified.clone(),
                created_at: now,
                updated_by_device: app.state::<SyncWriter>().self_device().to_string(),
            },
        )?;
        let ready = {
            let conn = db
                .conn
                .lock()
                .map_err(|error| AppError::Other(error.to_string()))?;
            finish_job(
                &conn,
                &job.id,
                &job.source_sha256,
                &asset_id,
                &verified,
                now,
            )?
        };
        emit_job_changed(app, &ready);
        let _ = app.emit("book-assets-changed", &job.book_id);
        schedule_index(app.clone(), job.book_id.clone());
        Ok(())
    }

    fn report_progress(&self, app: &AppHandle, job: &OcrJob, progress: OcrProgress) {
        let db = app.state::<Db>();
        let updated = {
            let Ok(conn) = db.conn.lock() else { return };
            update_state_guarded(
                &conn,
                &job.id,
                &job.source_sha256,
                OcrJobUpdate {
                    state: "recognizing",
                    phase: Some(&progress.phase),
                    pages_done: progress.pages_done,
                    pages_total: progress.pages_total,
                    updated_at: chrono::Utc::now().timestamp_millis(),
                },
            )
        };
        let Ok(updated) = updated else { return };
        let should_emit = self
            .inner
            .last_event
            .lock()
            .map(|mut last| {
                let now = Instant::now();
                let emit = last
                    .get(&job.id)
                    .is_none_or(|previous| now.duration_since(*previous) >= EVENT_INTERVAL);
                if emit {
                    last.insert(job.id.clone(), now);
                }
                emit
            })
            .unwrap_or(false);
        if should_emit {
            emit_job_changed(app, &updated);
        }
    }

    fn cancel(&self, app: &AppHandle, book_id: &str) -> AppResult<()> {
        let db = app.state::<Db>();
        let job = {
            let conn = db.reader();
            get_active_job(&conn, book_id)?
        };
        let Some(job) = job else { return Ok(()) };
        if let Ok(tokens) = self.inner.cancellations.lock() {
            if let Some(cancel) = tokens.get(&job.id) {
                cancel.cancel();
            }
        }
        let cancelled = {
            let conn = db
                .conn
                .lock()
                .map_err(|error| AppError::Other(error.to_string()))?;
            fail_or_cancel_job(
                &conn,
                &job.id,
                &job.source_sha256,
                true,
                None,
                None,
                chrono::Utc::now().timestamp_millis(),
            )?
        };
        emit_job_changed(app, &cancelled);
        Ok(())
    }
}

fn transition_job(
    db: &Db,
    job: &OcrJob,
    state: &str,
    phase: Option<&str>,
    pages_done: Option<i32>,
    pages_total: Option<i32>,
) -> AppResult<OcrJob> {
    let conn = db
        .conn
        .lock()
        .map_err(|error| AppError::Other(error.to_string()))?;
    update_state_guarded(
        &conn,
        &job.id,
        &job.source_sha256,
        OcrJobUpdate {
            state,
            phase,
            pages_done,
            pages_total,
            updated_at: chrono::Utc::now().timestamp_millis(),
        },
    )
}

fn load_source(db: &Db, book_id: &str, expected_sha256: &str) -> AppResult<PathBuf> {
    let relative = {
        let conn = db.reader();
        conn.query_row(
            "SELECT file_path FROM books
             WHERE id = ?1 AND COALESCE(source_format, format) = 'pdf'
               AND source_sha256 = ?2",
            params![book_id, expected_sha256],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| ocr_error("OCR_JOB_SOURCE_UNAVAILABLE"))?
    };
    db.resolve_path(&relative)
}

fn automatic_jobs() -> i32 {
    #[cfg(target_os = "macos")]
    {
        let physical = sysctl_u64("hw.physicalcpu");
        let memory = sysctl_u64("hw.memsize");
        if let (Some(physical), Some(memory)) = (physical, memory) {
            let cores = physical.saturating_sub(1).max(1);
            let memory_slots = (memory / (2 * 1024 * 1024 * 1024)).max(1);
            return cores.min(memory_slots).clamp(1, 4) as i32;
        }
    }
    1
}

#[cfg(target_os = "macos")]
fn sysctl_u64(name: &str) -> Option<u64> {
    let output = std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", name])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().parse().ok())
        .flatten()
}

fn emit_job_changed(app: &AppHandle, job: &OcrJob) {
    let _ = app.emit("ocr-job-changed", OcrJobView::from(job));
}

impl From<&OcrJob> for OcrJobView {
    fn from(job: &OcrJob) -> Self {
        Self {
            state: job.state.clone(),
            pages_done: job.pages_done,
            pages_total: job.pages_total,
            error_code: job.error_code.clone(),
        }
    }
}

fn stable_job_error(error: &AppError) -> String {
    match error {
        AppError::Other(code) if code.starts_with("OCR_") => code.clone(),
        _ => "OCR_JOB_FAILED".to_string(),
    }
}

fn truncate_detail(detail: &str) -> String {
    detail.chars().take(8 * 1024).collect()
}

fn ocr_error(code: &str) -> AppError {
    AppError::Other(code.to_string())
}

#[tauri::command]
pub(crate) fn ocr_start(
    book_id: String,
    app: AppHandle,
    manager: State<'_, OcrJobManager>,
) -> AppResult<()> {
    if !super::pipeline_enabled() {
        return Err(ocr_error("OCR_PIPELINE_DISABLED"));
    }
    crate::sync::validation::validate_entity_id(&book_id)?;
    let runtime =
        acquire_installed_runtime().ok_or_else(|| ocr_error("OCR_RUNTIME_NOT_INSTALLED"))?;
    let db = app.state::<Db>();
    let source_sha256 = {
        let conn = db.reader();
        if let Some(active) = get_active_job(&conn, &book_id)? {
            emit_job_changed(&app, &active);
            if active.state == "waiting_source" {
                manager.enqueue(app.clone(), active.id);
            }
            return Ok(());
        }
        conn.query_row(
            "SELECT source_sha256 FROM books
             WHERE id = ?1 AND COALESCE(source_format, format) = 'pdf'",
            params![book_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
        .ok_or_else(|| ocr_error("OCR_JOB_SOURCE_INVALID"))?
    };
    let job = {
        let conn = db
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        create_job(
            &conn,
            NewOcrJob {
                book_id: &book_id,
                source_sha256: &source_sha256,
                backend: Some("ocrmypdf"),
                backend_version: Some(&runtime.version),
                language_profile: "chi_sim+eng",
                quality_profile: "fast",
                jobs: automatic_jobs(),
                conversion_version: CONVERSION_VERSION,
                created_at: chrono::Utc::now().timestamp_millis(),
            },
        )?
    };
    drop(runtime);
    emit_job_changed(&app, &job);
    manager.enqueue(app, job.id);
    Ok(())
}

#[tauri::command]
pub(crate) fn ocr_cancel(
    book_id: String,
    app: AppHandle,
    manager: State<'_, OcrJobManager>,
) -> AppResult<()> {
    crate::sync::validation::validate_entity_id(&book_id)?;
    manager.cancel(&app, &book_id)
}

#[tauri::command]
pub(crate) fn ocr_retry(
    book_id: String,
    app: AppHandle,
    manager: State<'_, OcrJobManager>,
) -> AppResult<()> {
    {
        let db = app.state::<Db>();
        let conn = db.reader();
        if let Some(latest) = get_latest_job(&conn, &book_id)? {
            if !matches!(latest.state.as_str(), "failed" | "cancelled") {
                return Err(ocr_error("OCR_JOB_RETRY_INVALID"));
            }
        }
    }
    ocr_start(book_id, app, manager)
}

#[tauri::command]
pub(crate) fn ocr_job_status(book_id: String, db: State<'_, Db>) -> AppResult<Option<OcrJobView>> {
    crate::sync::validation::validate_entity_id(&book_id)?;
    let conn = db.reader();
    Ok(get_latest_job(&conn, &book_id)?.as_ref().map(Into::into))
}

#[tauri::command]
pub(crate) fn ocr_assets_overview(db: State<'_, Db>) -> AppResult<OcrAssetsOverview> {
    let conn = db.reader();
    let mut statement = conn.prepare(
        "SELECT a.id, a.book_id, b.title, a.byte_size, a.created_at,
                COALESCE(s.availability, 'remote_only')
         FROM book_assets a JOIN books b ON b.id = a.book_id
         LEFT JOIN book_asset_local_state s ON s.asset_id = a.id
         ORDER BY a.created_at DESC, a.id DESC",
    )?;
    let items = statement
        .query_map([], |row| {
            Ok(OcrAssetItem {
                asset_id: row.get(0)?,
                book_id: row.get(1)?,
                title: row.get(2)?,
                byte_size: row.get(3)?,
                created_at: row.get(4)?,
                availability: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let total_bytes = items.iter().map(|item| item.byte_size).sum();
    Ok(OcrAssetsOverview { total_bytes, items })
}

#[tauri::command]
pub(crate) fn ocr_asset_delete(
    asset_id: String,
    all_devices: bool,
    app: AppHandle,
    db: State<'_, Db>,
) -> AppResult<()> {
    crate::sync::validation::validate_entity_id(&asset_id)?;
    if all_devices {
        return Err(ocr_error("OCR_ASSET_DELETE_ALL_UNAVAILABLE"));
    }
    let asset = delete_local_asset(&db, &asset_id)?;
    let _ = app.emit("book-assets-changed", asset.book_id);
    Ok(())
}

fn delete_local_asset(db: &Db, asset_id: &str) -> AppResult<super::assets::BookAsset> {
    let asset = {
        let conn = db.reader();
        super::assets::get_asset(&conn, asset_id)?
            .ok_or_else(|| ocr_error("OCR_ASSET_NOT_FOUND"))?
    };
    {
        let mut conn = db
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        super::assets::delete_local_asset(&mut conn, asset_id)?;
    }
    if let Ok(path) = db.resolve_path(&asset.relative_path) {
        let _ = fs::remove_file(path);
    }
    Ok(asset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_view_uses_frozen_command_contract() {
        let job = OcrJob {
            id: "job".into(),
            book_id: "book".into(),
            source_sha256: "hash".into(),
            state: "recognizing".into(),
            phase: None,
            pages_done: Some(2),
            pages_total: Some(10),
            backend: None,
            backend_version: None,
            language_profile: None,
            quality_profile: None,
            jobs: Some(1),
            conversion_version: 1,
            result_asset_id: None,
            recognized_pages: None,
            skipped_pages: None,
            timed_out_pages: None,
            failed_pages: None,
            temporary_path: None,
            error_code: None,
            error_detail: None,
            created_at: 1,
            started_at: Some(1),
            updated_at: 2,
        };
        assert_eq!(
            serde_json::to_value(OcrJobView::from(&job)).unwrap(),
            serde_json::json!({"state":"recognizing","pagesDone":2,"pagesTotal":10})
        );
    }

    #[test]
    fn automatic_jobs_stays_in_v1_bounds() {
        assert!((1..=4).contains(&automatic_jobs()));
    }

    #[test]
    fn assets_overview_and_local_delete_follow_command_contract() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::init(dir.path()).unwrap();
        fs::create_dir_all(dir.path().join("books")).unwrap();
        {
            let conn = db.conn.lock().unwrap();
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
            let relative = super::super::assets::expected_relative_path("book-1", "asset-1");
            super::super::assets::insert_asset(
                &conn,
                super::super::assets::NewBookAsset {
                    id: "asset-1",
                    book_id: "book-1",
                    relative_path: &relative,
                    content_sha256: "asset-hash",
                    byte_size: 4,
                    source_sha256: "source-hash",
                    pipeline_version: Some("test"),
                    language_profile: "chi_sim+eng",
                    quality_profile: "fast",
                    page_count: 1,
                    supersedes_asset_id: None,
                    created_at: 2,
                    updated_at: 2,
                    updated_by_device: "dev-a",
                },
            )
            .unwrap();
            super::super::assets::set_local_state(
                &conn,
                "asset-1",
                "available_verified",
                Some(3),
                None,
                3,
            )
            .unwrap();
            fs::write(dir.path().join(relative), b"data").unwrap();
        }

        let app = tauri::test::mock_app();
        assert!(app.manage(db));
        let overview = ocr_assets_overview(app.state::<Db>()).unwrap();
        assert_eq!(overview.total_bytes, 4);
        assert_eq!(overview.items.len(), 1);
        assert_eq!(overview.items[0].availability, "available_verified");

        delete_local_asset(&app.state::<Db>(), "asset-1").unwrap();
        assert_eq!(
            ocr_assets_overview(app.state::<Db>()).unwrap().total_bytes,
            0
        );
        assert!(!dir.path().join("books/book-1.ocr.asset-1.pdf").exists());
    }
}
