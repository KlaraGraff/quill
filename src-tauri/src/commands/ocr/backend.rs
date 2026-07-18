use std::fs::{self, File};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct BackendCapabilities {
    pub id: String,
    pub version: String,
    pub languages: Vec<String>,
    pub quality_profiles: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct RecognitionRequest {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub staging_root: PathBuf,
    pub source_sha256: String,
    pub language_profile: String,
    pub quality_profile: String,
    pub jobs: u8,
}

/// A request whose source snapshot and staging boundary were verified by the
/// backend wrapper. Implementations receive canonical paths and cannot choose
/// a different output location.
#[derive(Debug, Clone)]
pub(crate) struct ValidatedRecognitionRequest {
    source: PathBuf,
    destination: PathBuf,
    staging_root: PathBuf,
    source_sha256: String,
    language_profile: String,
    quality_profile: String,
    jobs: u8,
}

impl ValidatedRecognitionRequest {
    pub(crate) fn source(&self) -> &Path {
        &self.source
    }

    pub(crate) fn destination(&self) -> &Path {
        &self.destination
    }

    pub(crate) fn staging_root(&self) -> &Path {
        &self.staging_root
    }

    pub(crate) fn source_sha256(&self) -> &str {
        &self.source_sha256
    }

    pub(crate) fn language_profile(&self) -> &str {
        &self.language_profile
    }

    pub(crate) fn quality_profile(&self) -> &str {
        &self.quality_profile
    }

    pub(crate) fn jobs(&self) -> u8 {
        self.jobs
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct OcrProgress {
    pub phase: String,
    pub pages_done: Option<i32>,
    pub pages_total: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct OcrOutput {
    pub output_path: PathBuf,
    pub page_count: i32,
    pub recognized_pages: i32,
    pub skipped_pages: i32,
    pub timed_out_pages: i32,
    pub failed_pages: i32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub(crate) fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub(crate) fn check(&self) -> AppResult<()> {
        if self.is_cancelled() {
            return Err(ocr_error("OCR_JOB_CANCELLED"));
        }
        Ok(())
    }
}

/// Backends only transform bytes in staging. Database changes, sync events,
/// final publication and active-asset selection are intentionally absent from
/// this interface. Implementations cannot construct a validated request; the
/// module-level wrapper below is the only entry point from a raw request.
pub(crate) trait OcrBackend: Send + Sync {
    fn probe(&self) -> AppResult<BackendCapabilities>;

    fn recognize_pdf_in_staging(
        &self,
        request: &ValidatedRecognitionRequest,
        progress: &mut dyn FnMut(OcrProgress),
        cancel: &CancellationToken,
    ) -> AppResult<OcrOutput>;
}

#[derive(Debug, Clone)]
pub(crate) struct OcrmypdfBackend {
    pub runtime_root: PathBuf,
    pub version: String,
}

impl OcrmypdfBackend {
    pub(crate) fn from_runtime(runtime: &super::package::RuntimeInfo) -> Self {
        Self {
            runtime_root: runtime.root.clone(),
            version: runtime.version.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum JsonlEvent {
    Phase {
        phase: String,
    },
    Progress {
        phase: String,
        completed: i32,
        total: i32,
    },
    Warning {
        code: String,
        page: Option<i32>,
    },
    Complete {
        pages: i32,
        ocr_pages: i32,
        skipped_pages: i32,
        #[serde(default)]
        timed_out_pages: i32,
        #[serde(default)]
        failed_pages: i32,
    },
}

impl OcrBackend for OcrmypdfBackend {
    fn probe(&self) -> AppResult<BackendCapabilities> {
        let executable = runtime_executable(&self.runtime_root)?;
        let output = Command::new(executable)
            .arg("--version")
            .env_clear()
            .env("PATH", runtime_path(&self.runtime_root))
            .env("TESSDATA_PREFIX", tessdata_dir(&self.runtime_root))
            .output()
            .map_err(|_| ocr_error("OCR_RUNTIME_EXEC_FAILED"))?;
        if !output.status.success() {
            return Err(ocr_error("OCR_RUNTIME_SELF_TEST_FAILED"));
        }
        Ok(BackendCapabilities {
            id: "ocrmypdf".to_string(),
            version: self.version.clone(),
            languages: vec!["chi_sim+eng".to_string()],
            quality_profiles: vec!["fast".to_string()],
        })
    }

    fn recognize_pdf_in_staging(
        &self,
        request: &ValidatedRecognitionRequest,
        progress: &mut dyn FnMut(OcrProgress),
        cancel: &CancellationToken,
    ) -> AppResult<OcrOutput> {
        cancel.check()?;
        if request.language_profile() != "chi_sim+eng" || request.quality_profile() != "fast" {
            return Err(ocr_error("OCR_BACKEND_REQUEST_INVALID"));
        }
        let executable = runtime_executable(&self.runtime_root)?;
        let mut command = Command::new(executable);
        command
            .args([
                "--mode",
                "skip",
                "--output-type",
                "pdf",
                "--rasterizer",
                "pypdfium",
                "--optimize",
                "0",
                "--fast-web-view",
                "999999",
                "--jobs",
                &request.jobs().to_string(),
                "-l",
                "chi_sim+eng",
                "--",
            ])
            .arg(request.source())
            .arg(request.destination())
            .current_dir(request.staging_root())
            .env_clear()
            .env("PATH", runtime_path(&self.runtime_root))
            .env("TESSDATA_PREFIX", tessdata_dir(&self.runtime_root))
            .env("OMP_THREAD_LIMIT", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_process_group(&mut command);
        let mut child = command
            .spawn()
            .map_err(|_| ocr_error("OCR_RUNTIME_EXEC_FAILED"))?;
        let process_id = child.id();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ocr_error("OCR_RUNTIME_OUTPUT_INVALID"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ocr_error("OCR_RUNTIME_OUTPUT_INVALID"))?;
        let stderr_tail = Arc::new(Mutex::new(Vec::new()));
        let stderr_reader = spawn_stderr_reader(stderr, Arc::clone(&stderr_tail));

        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let stdout_reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Ok(event) = serde_json::from_str::<JsonlEvent>(&line) {
                    if event_tx.send(event).is_err() {
                        break;
                    }
                }
            }
        });

        let mut complete = None;
        let status = loop {
            while let Ok(event) = event_rx.try_recv() {
                match event {
                    JsonlEvent::Phase { phase } => progress(OcrProgress {
                        phase,
                        pages_done: None,
                        pages_total: None,
                    }),
                    JsonlEvent::Progress {
                        phase,
                        completed,
                        total,
                    } if completed >= 0 && total >= completed => progress(OcrProgress {
                        phase,
                        pages_done: Some(completed),
                        pages_total: Some(total),
                    }),
                    JsonlEvent::Warning { code, page } => {
                        let _ = (code, page);
                    }
                    JsonlEvent::Complete {
                        pages,
                        ocr_pages,
                        skipped_pages,
                        timed_out_pages,
                        failed_pages,
                    } => {
                        complete = Some((
                            pages,
                            ocr_pages,
                            skipped_pages,
                            timed_out_pages,
                            failed_pages,
                        ))
                    }
                    JsonlEvent::Progress { .. } => {}
                }
            }
            if cancel.is_cancelled() {
                terminate_process_tree(process_id, &mut child);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(ocr_error("OCR_JOB_CANCELLED"));
            }
            if let Some(status) = child
                .try_wait()
                .map_err(|_| ocr_error("OCR_RUNTIME_EXEC_FAILED"))?
            {
                break status;
            }
            thread::sleep(Duration::from_millis(50));
        };
        let _ = stdout_reader.join();
        let _ = stderr_reader.join();
        while let Ok(event) = event_rx.try_recv() {
            if let JsonlEvent::Complete {
                pages,
                ocr_pages,
                skipped_pages,
                timed_out_pages,
                failed_pages,
            } = event
            {
                complete = Some((
                    pages,
                    ocr_pages,
                    skipped_pages,
                    timed_out_pages,
                    failed_pages,
                ));
            }
        }
        if !status.success() {
            let detail = stderr_tail
                .lock()
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default();
            log::warn!("OCRmyPDF failed: {detail}");
            return Err(ocr_error("OCR_RUNTIME_FAILED"));
        }
        let (pages, recognized_pages, skipped_pages, timed_out_pages, failed_pages) =
            complete.ok_or_else(|| ocr_error("OCR_RUNTIME_OUTPUT_INVALID"))?;
        Ok(OcrOutput {
            output_path: request.destination().to_path_buf(),
            page_count: pages,
            recognized_pages,
            skipped_pages,
            timed_out_pages,
            failed_pages,
        })
    }
}

fn runtime_executable(root: &Path) -> AppResult<PathBuf> {
    let candidates: &[&str] = if cfg!(windows) {
        &[
            "lantern-ocr.cmd",
            "bin/ocrmypdf.exe",
            "Scripts/ocrmypdf.exe",
            "ocrmypdf.exe",
        ]
    } else {
        &["bin/lantern-ocr", "bin/ocrmypdf", "ocrmypdf"]
    };
    candidates
        .iter()
        .map(|candidate| root.join(candidate))
        .find(|path| path.is_file())
        .ok_or_else(|| ocr_error("OCR_RUNTIME_NOT_INSTALLED"))
}

fn tessdata_dir(root: &Path) -> PathBuf {
    let standard = root.join("share/tessdata");
    if standard.is_dir() {
        standard
    } else {
        root.join("Library/share/tessdata")
    }
}

fn runtime_path(root: &Path) -> std::ffi::OsString {
    let separator = if cfg!(windows) { ";" } else { ":" };
    std::env::join_paths([root.join("bin"), root.to_path_buf()])
        .unwrap_or_else(|_| std::ffi::OsString::from(separator))
}

fn spawn_stderr_reader(
    stderr: impl Read + Send + 'static,
    tail: Arc<Mutex<Vec<u8>>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        const LIMIT: usize = 64 * 1024;
        let mut reader = BufReader::new(stderr);
        let mut buffer = [0_u8; 8 * 1024];
        while let Ok(read) = reader.read(&mut buffer) {
            if read == 0 {
                break;
            }
            if let Ok(mut bytes) = tail.lock() {
                bytes.extend_from_slice(&buffer[..read]);
                if bytes.len() > LIMIT {
                    let excess = bytes.len() - LIMIT;
                    bytes.drain(..excess);
                }
            }
        }
    })
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(windows)]
fn configure_process_group(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0000_0200);
}

#[cfg(unix)]
fn terminate_process_tree(process_id: u32, child: &mut std::process::Child) {
    unsafe {
        libc::kill(-(process_id as i32), libc::SIGTERM);
    }
    for _ in 0..20 {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
    unsafe {
        libc::kill(-(process_id as i32), libc::SIGKILL);
    }
    let _ = child.wait();
}

#[cfg(windows)]
fn terminate_process_tree(_process_id: u32, child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

pub(crate) fn recognize_pdf(
    backend: &dyn OcrBackend,
    request: &RecognitionRequest,
    progress: &mut dyn FnMut(OcrProgress),
    cancel: &CancellationToken,
) -> AppResult<OcrOutput> {
    let request = validate_staging_request(request)?;
    cancel.check()?;
    let staged_source = StagedSourceGuard::create(&request)?;
    let backend_request = request.for_backend(staged_source.path().to_path_buf());

    let backend_result = backend.recognize_pdf_in_staging(&backend_request, progress, cancel);
    verify_source_unchanged(&request)?;
    staged_source.verify_unchanged()?;

    let output = backend_result?;
    validate_backend_output(&backend_request, &output)?;
    Ok(output)
}

struct ValidatedWrapperRequest {
    source: PathBuf,
    destination: PathBuf,
    staging_root: PathBuf,
    source_sha256: String,
    language_profile: String,
    quality_profile: String,
    jobs: u8,
}

impl ValidatedWrapperRequest {
    fn for_backend(&self, staged_source: PathBuf) -> ValidatedRecognitionRequest {
        ValidatedRecognitionRequest {
            source: staged_source,
            destination: self.destination.clone(),
            staging_root: self.staging_root.clone(),
            source_sha256: self.source_sha256.clone(),
            language_profile: self.language_profile.clone(),
            quality_profile: self.quality_profile.clone(),
            jobs: self.jobs,
        }
    }
}

struct StagedSourceGuard {
    path: PathBuf,
    expected_sha256: String,
}

impl StagedSourceGuard {
    fn create(request: &ValidatedWrapperRequest) -> AppResult<Self> {
        let path = request
            .staging_root
            .join(format!("source.{}.pdf", Uuid::new_v4()));
        let (_, path) = validate_destination(
            &request.staging_root,
            &path,
            DestinationExpectation::Missing,
        )?;
        let actual_sha256 = copy_with_sha256(&request.source, &path)?;
        if actual_sha256 != request.source_sha256 {
            let _ = fs::remove_file(&path);
            return Err(ocr_error("OCR_SOURCE_HASH_MISMATCH"));
        }
        let mut permissions = fs::metadata(&path)?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&path, permissions)?;
        Ok(Self {
            path,
            expected_sha256: request.source_sha256.clone(),
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn verify_unchanged(&self) -> AppResult<()> {
        let metadata =
            fs::symlink_metadata(&self.path).map_err(|_| ocr_error("OCR_STAGED_SOURCE_CHANGED"))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(ocr_error("OCR_STAGED_SOURCE_CHANGED"));
        }
        let actual = file_sha256(&self.path).map_err(|_| ocr_error("OCR_STAGED_SOURCE_CHANGED"))?;
        if actual != self.expected_sha256 {
            return Err(ocr_error("OCR_STAGED_SOURCE_CHANGED"));
        }
        Ok(())
    }
}

impl Drop for StagedSourceGuard {
    fn drop(&mut self) {
        let _ = make_owner_writable(&self.path);
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(unix)]
fn make_owner_writable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(permissions.mode() | 0o200);
    fs::set_permissions(path, permissions)
}

#[cfg(windows)]
fn make_owner_writable(path: &Path) -> std::io::Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions)
}

fn validate_staging_request(request: &RecognitionRequest) -> AppResult<ValidatedWrapperRequest> {
    if request.language_profile.trim().is_empty()
        || request.quality_profile.trim().is_empty()
        || request.jobs == 0
        || request.jobs > 4
    {
        return Err(ocr_error("OCR_BACKEND_REQUEST_INVALID"));
    }
    if request.source_sha256.len() != 64
        || !request
            .source_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(ocr_error("OCR_SOURCE_HASH_INVALID"));
    }
    if request.source == request.destination {
        return Err(ocr_error("OCR_SOURCE_OVERWRITE_FORBIDDEN"));
    }

    let source_metadata =
        fs::symlink_metadata(&request.source).map_err(|_| ocr_error("OCR_SOURCE_PATH_INVALID"))?;
    if source_metadata.file_type().is_symlink() || !source_metadata.is_file() {
        return Err(ocr_error("OCR_SOURCE_PATH_INVALID"));
    }
    let source = request
        .source
        .canonicalize()
        .map_err(|_| ocr_error("OCR_SOURCE_PATH_INVALID"))?;

    if request.destination.exists()
        && request
            .destination
            .canonicalize()
            .is_ok_and(|destination| destination == source)
    {
        return Err(ocr_error("OCR_SOURCE_OVERWRITE_FORBIDDEN"));
    }

    let (staging_root, destination) = validate_destination(
        &request.staging_root,
        &request.destination,
        DestinationExpectation::Missing,
    )?;
    if destination == source {
        return Err(ocr_error("OCR_SOURCE_OVERWRITE_FORBIDDEN"));
    }

    let expected_source_sha256 = request.source_sha256.to_ascii_lowercase();
    let actual_source_sha256 =
        file_sha256(&source).map_err(|_| ocr_error("OCR_SOURCE_SNAPSHOT_UNREADABLE"))?;
    if actual_source_sha256 != expected_source_sha256 {
        return Err(ocr_error("OCR_SOURCE_HASH_MISMATCH"));
    }

    Ok(ValidatedWrapperRequest {
        source,
        destination,
        staging_root,
        source_sha256: expected_source_sha256,
        language_profile: request.language_profile.clone(),
        quality_profile: request.quality_profile.clone(),
        jobs: request.jobs,
    })
}

#[derive(Clone, Copy)]
enum DestinationExpectation {
    Missing,
    RegularFile,
}

fn validate_destination(
    staging_root: &Path,
    destination: &Path,
    expectation: DestinationExpectation,
) -> AppResult<(PathBuf, PathBuf)> {
    if !is_normal_absolute(staging_root) || !is_normal_absolute(destination) {
        return Err(ocr_error("OCR_BACKEND_PATH_INVALID"));
    }

    let relative = destination
        .strip_prefix(staging_root)
        .map_err(|_| ocr_error("OCR_DESTINATION_OUTSIDE_STAGING"))?;
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(ocr_error("OCR_DESTINATION_OUTSIDE_STAGING"));
    }

    let root_metadata =
        fs::symlink_metadata(staging_root).map_err(|_| ocr_error("OCR_STAGING_ROOT_INVALID"))?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(ocr_error("OCR_STAGING_ROOT_INVALID"));
    }
    let canonical_root = staging_root
        .canonicalize()
        .map_err(|_| ocr_error("OCR_STAGING_ROOT_INVALID"))?;

    let mut parent = staging_root.to_path_buf();
    for component in &components[..components.len() - 1] {
        parent.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&parent)
            .map_err(|_| ocr_error("OCR_DESTINATION_PARENT_INVALID"))?;
        if metadata.file_type().is_symlink() {
            return Err(ocr_error("OCR_DESTINATION_SYMLINK_FORBIDDEN"));
        }
        if !metadata.is_dir() {
            return Err(ocr_error("OCR_DESTINATION_PARENT_INVALID"));
        }
    }

    let canonical_parent = parent
        .canonicalize()
        .map_err(|_| ocr_error("OCR_DESTINATION_PARENT_INVALID"))?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err(ocr_error("OCR_DESTINATION_OUTSIDE_STAGING"));
    }
    let canonical_destination = canonical_parent.join(components.last().unwrap().as_os_str());
    if canonical_destination == canonical_root {
        return Err(ocr_error("OCR_DESTINATION_OUTSIDE_STAGING"));
    }

    match fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(ocr_error("OCR_DESTINATION_SYMLINK_FORBIDDEN"))
        }
        Ok(metadata) => match expectation {
            DestinationExpectation::Missing => Err(ocr_error("OCR_DESTINATION_EXISTS")),
            DestinationExpectation::RegularFile if metadata.is_file() => {
                Ok((canonical_root, canonical_destination))
            }
            DestinationExpectation::RegularFile => Err(ocr_error("OCR_BACKEND_OUTPUT_INVALID")),
        },
        Err(error) if error.kind() == ErrorKind::NotFound => match expectation {
            DestinationExpectation::Missing => Ok((canonical_root, canonical_destination)),
            DestinationExpectation::RegularFile => Err(ocr_error("OCR_BACKEND_OUTPUT_INVALID")),
        },
        Err(_) => Err(ocr_error("OCR_BACKEND_PATH_INVALID")),
    }
}

fn validate_backend_output(
    request: &ValidatedRecognitionRequest,
    output: &OcrOutput,
) -> AppResult<()> {
    if output.output_path != request.destination {
        return Err(ocr_error("OCR_BACKEND_OUTPUT_PATH_INVALID"));
    }
    let (staging_root, destination) = validate_destination(
        &request.staging_root,
        &request.destination,
        DestinationExpectation::RegularFile,
    )?;
    if staging_root != request.staging_root || destination != request.destination {
        return Err(ocr_error("OCR_BACKEND_OUTPUT_PATH_INVALID"));
    }
    Ok(())
}

fn verify_source_unchanged(request: &ValidatedWrapperRequest) -> AppResult<()> {
    let metadata =
        fs::symlink_metadata(&request.source).map_err(|_| ocr_error("OCR_SOURCE_CHANGED"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ocr_error("OCR_SOURCE_CHANGED"));
    }
    let current_source = request
        .source
        .canonicalize()
        .map_err(|_| ocr_error("OCR_SOURCE_CHANGED"))?;
    if current_source != request.source {
        return Err(ocr_error("OCR_SOURCE_CHANGED"));
    }
    let actual_source_sha256 =
        file_sha256(&request.source).map_err(|_| ocr_error("OCR_SOURCE_CHANGED"))?;
    if actual_source_sha256 != request.source_sha256 {
        return Err(ocr_error("OCR_SOURCE_CHANGED"));
    }
    Ok(())
}

fn copy_with_sha256(source: &Path, destination: &Path) -> AppResult<String> {
    let mut input = File::open(source)?;
    if !input.metadata()?.is_file() {
        return Err(ocr_error("OCR_SOURCE_PATH_INVALID"));
    }
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
    }
    output.sync_all()?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn file_sha256(path: &Path) -> AppResult<String> {
    let mut file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(ocr_error("OCR_SOURCE_PATH_INVALID"));
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn is_normal_absolute(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
}

fn ocr_error(code: &str) -> AppError {
    AppError::Other(code.to_string())
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::Write;

    use super::*;

    struct FakeBackend {
        bytes: Vec<u8>,
    }

    impl OcrBackend for FakeBackend {
        fn probe(&self) -> AppResult<BackendCapabilities> {
            Ok(BackendCapabilities {
                id: "fake".to_string(),
                version: "1".to_string(),
                languages: vec!["chi_sim+eng".to_string()],
                quality_profiles: vec!["fast".to_string()],
            })
        }

        fn recognize_pdf_in_staging(
            &self,
            request: &ValidatedRecognitionRequest,
            progress: &mut dyn FnMut(OcrProgress),
            cancel: &CancellationToken,
        ) -> AppResult<OcrOutput> {
            cancel.check()?;
            progress(OcrProgress {
                phase: "recognizing".to_string(),
                pages_done: Some(0),
                pages_total: Some(1),
            });
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(request.destination())?;
            output.write_all(&self.bytes)?;
            output.sync_all()?;
            cancel.check()?;
            progress(OcrProgress {
                phase: "recognizing".to_string(),
                pages_done: Some(1),
                pages_total: Some(1),
            });
            Ok(OcrOutput {
                output_path: request.destination().to_path_buf(),
                page_count: 1,
                recognized_pages: 1,
                skipped_pages: 0,
                timed_out_pages: 0,
                failed_pages: 0,
            })
        }
    }

    struct SourceMutatingBackend;

    impl OcrBackend for SourceMutatingBackend {
        fn probe(&self) -> AppResult<BackendCapabilities> {
            unreachable!()
        }

        fn recognize_pdf_in_staging(
            &self,
            request: &ValidatedRecognitionRequest,
            _progress: &mut dyn FnMut(OcrProgress),
            _cancel: &CancellationToken,
        ) -> AppResult<OcrOutput> {
            make_owner_writable(request.source())?;
            fs::write(request.source(), b"changed after validation")?;
            Ok(OcrOutput {
                output_path: request.destination().to_path_buf(),
                page_count: 1,
                recognized_pages: 1,
                skipped_pages: 0,
                timed_out_pages: 0,
                failed_pages: 0,
            })
        }
    }

    fn request(dir: &Path) -> RecognitionRequest {
        let books = dir.join("books");
        let staging_root = dir.join("staging");
        fs::create_dir_all(&books).unwrap();
        fs::create_dir_all(&staging_root).unwrap();
        let source = books.join("source.pdf");
        fs::write(&source, b"source").unwrap();
        RecognitionRequest {
            source_sha256: file_sha256(&source).unwrap(),
            source,
            destination: staging_root.join("result.partial.pdf"),
            staging_root,
            language_profile: "chi_sim+eng".to_string(),
            quality_profile: "fast".to_string(),
            jobs: 1,
        }
    }

    fn run_fake(request: &RecognitionRequest) -> AppResult<OcrOutput> {
        recognize_pdf(
            &FakeBackend {
                bytes: b"searchable".to_vec(),
            },
            request,
            &mut |_| {},
            &CancellationToken::default(),
        )
    }

    #[test]
    fn fake_backend_writes_only_staging_and_reports_progress() {
        let dir = tempfile::tempdir().unwrap();
        let request = request(dir.path());
        let backend = FakeBackend {
            bytes: b"searchable".to_vec(),
        };
        assert_eq!(backend.probe().unwrap().id, "fake");
        let mut progress_events = Vec::new();
        let output = recognize_pdf(
            &backend,
            &request,
            &mut |event| progress_events.push(event),
            &CancellationToken::default(),
        )
        .unwrap();
        assert_eq!(fs::read(&request.source).unwrap(), b"source");
        assert_eq!(fs::read(&output.output_path).unwrap(), b"searchable");
        assert!(output.output_path.starts_with(
            request
                .staging_root
                .canonicalize()
                .expect("staging root should exist")
        ));
        assert_eq!(progress_events.len(), 2);
    }

    #[test]
    fn cancelled_backend_never_creates_output() {
        let dir = tempfile::tempdir().unwrap();
        let request = request(dir.path());
        let cancel = CancellationToken::default();
        cancel.cancel();
        let error = recognize_pdf(
            &FakeBackend { bytes: vec![1] },
            &request,
            &mut |_| {},
            &cancel,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("OCR_JOB_CANCELLED"));
        assert!(!request.destination.exists());
    }

    #[test]
    fn backend_cannot_overwrite_source() {
        let dir = tempfile::tempdir().unwrap();
        let mut request = request(dir.path());
        request.destination = request.source.clone();
        let error = run_fake(&request).unwrap_err().to_string();
        assert!(error.contains("OCR_SOURCE_OVERWRITE_FORBIDDEN"));
        assert_eq!(fs::read(&request.source).unwrap(), b"source");
    }

    #[test]
    fn backend_cannot_write_another_books_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut request = request(dir.path());
        let other_book = dir.path().join("books/other.pdf");
        fs::write(&other_book, b"other book").unwrap();
        request.destination = other_book.clone();

        let error = run_fake(&request).unwrap_err().to_string();
        assert!(error.contains("OCR_DESTINATION_OUTSIDE_STAGING"));
        assert_eq!(fs::read(other_book).unwrap(), b"other book");
    }

    #[test]
    fn backend_rejects_parent_directory_escape() {
        let dir = tempfile::tempdir().unwrap();
        let mut request = request(dir.path());
        let other_book = dir.path().join("books/other.pdf");
        fs::write(&other_book, b"other book").unwrap();
        request.destination = request.staging_root.join("../books/other.pdf");

        assert!(run_fake(&request).is_err());
        assert_eq!(fs::read(other_book).unwrap(), b"other book");
    }

    #[test]
    fn backend_rejects_existing_staging_target() {
        let dir = tempfile::tempdir().unwrap();
        let request = request(dir.path());
        fs::write(&request.destination, b"existing").unwrap();

        let error = run_fake(&request).unwrap_err().to_string();
        assert!(error.contains("OCR_DESTINATION_EXISTS"));
        assert_eq!(fs::read(&request.destination).unwrap(), b"existing");
    }

    #[cfg(unix)]
    #[test]
    fn backend_rejects_symlink_ancestor() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let mut request = request(dir.path());
        let other_book = dir.path().join("books/other.pdf");
        fs::write(&other_book, b"other book").unwrap();
        let linked_books = request.staging_root.join("linked-books");
        symlink(dir.path().join("books"), &linked_books).unwrap();
        request.destination = linked_books.join("other.pdf");

        let error = run_fake(&request).unwrap_err().to_string();
        assert!(error.contains("OCR_DESTINATION_SYMLINK_FORBIDDEN"));
        assert_eq!(fs::read(other_book).unwrap(), b"other book");
    }

    #[cfg(unix)]
    #[test]
    fn backend_rejects_symlink_target() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let request = request(dir.path());
        let other_book = dir.path().join("books/other.pdf");
        fs::write(&other_book, b"other book").unwrap();
        symlink(&other_book, &request.destination).unwrap();

        let error = run_fake(&request).unwrap_err().to_string();
        assert!(error.contains("OCR_DESTINATION_SYMLINK_FORBIDDEN"));
        assert_eq!(fs::read(other_book).unwrap(), b"other book");
    }

    #[test]
    fn backend_rejects_source_hash_mismatch_before_execution() {
        let dir = tempfile::tempdir().unwrap();
        let mut request = request(dir.path());
        request.source_sha256 = "0".repeat(64);

        let error = run_fake(&request).unwrap_err().to_string();
        assert!(error.contains("OCR_SOURCE_HASH_MISMATCH"));
        assert!(!request.destination.exists());
    }

    #[test]
    fn backend_can_only_mutate_staging_copy() {
        let dir = tempfile::tempdir().unwrap();
        let request = request(dir.path());

        let error = recognize_pdf(
            &SourceMutatingBackend,
            &request,
            &mut |_| {},
            &CancellationToken::default(),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("OCR_STAGED_SOURCE_CHANGED"));
        assert_eq!(fs::read(&request.source).unwrap(), b"source");
        let staging_entries = fs::read_dir(&request.staging_root)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(staging_entries.is_empty(), "staging input must be cleaned");
    }
}
