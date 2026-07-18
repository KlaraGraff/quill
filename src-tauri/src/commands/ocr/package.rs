use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::ops::Deref;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use reqwest::header::ACCEPT_ENCODING;
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, State};

use crate::db::Db;
use crate::error::{AppError, AppResult};

const PACKAGE_ID: &str = "lantern-ocr-runtime";
const RELEASE_TAG: &str = "ocr-runtime-v1.0.0";
const RELEASE_ROOT: &str = "https://github.com/KlaraGraff/lantern/releases/download";
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_INSTALLED_BYTES: u64 = 6 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 200_000;
const MAX_ARCHIVE_LISTING_BYTES: usize = 32 * 1024 * 1024;
const DOWNLOAD_ATTEMPTS: usize = 3;
const SELF_TEST_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OcrPackageState {
    NotInstalled,
    Downloading,
    Verifying,
    Installing,
    Installed,
    Uninstalling,
    Failed,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OcrPackageStatus {
    pub state: OcrPackageState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloaded_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

impl OcrPackageStatus {
    fn not_installed() -> Self {
        Self {
            state: OcrPackageState::NotInstalled,
            version: None,
            downloaded_bytes: None,
            total_bytes: None,
            installed_bytes: None,
            error_code: None,
        }
    }

    fn transitional(&self) -> bool {
        matches!(
            self.state,
            OcrPackageState::Downloading
                | OcrPackageState::Verifying
                | OcrPackageState::Installing
                | OcrPackageState::Uninstalling
        )
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct RuntimeManifest {
    pub package_id: String,
    pub version: String,
    pub platform: String,
    pub arch: String,
    pub minimum_os_version: String,
    pub download_size: u64,
    pub installed_size: u64,
    pub sha256: String,
    pub url: String,
}

impl RuntimeManifest {
    fn validate(&self, allow_loopback_http: bool) -> AppResult<()> {
        let (platform, arch) = target_platform_arch()?;
        if self.package_id != PACKAGE_ID
            || self.platform != platform
            || self.arch != arch
            || !safe_path_token(&self.version)
            || self.minimum_os_version.trim().is_empty()
            || self.download_size == 0
            || self.download_size > MAX_DOWNLOAD_BYTES
            || self.installed_size == 0
            || self.installed_size > MAX_INSTALLED_BYTES
            || self.sha256.len() != 64
            || !self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(package_error("OCR_PACKAGE_MANIFEST_INVALID"));
        }
        let url = validate_https_url(&self.url, allow_loopback_http)?;
        if !url.path().ends_with(".tar.zst") {
            return Err(package_error("OCR_PACKAGE_MANIFEST_INVALID"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CurrentPointer {
    version: String,
    directory: String,
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeInfo {
    pub root: PathBuf,
    pub version: String,
}

impl RuntimeInfo {
    pub(crate) fn launcher(&self) -> PathBuf {
        runtime_launcher(&self.root)
    }

    pub(crate) fn tessdata_prefix(&self) -> PathBuf {
        runtime_tessdata(&self.root)
    }
}

/// Keeps an installed version alive while W1 runs an OCR child process.
/// W1 should prefer this over a bare `installed_runtime()` lookup.
pub(crate) struct RuntimeLease {
    info: RuntimeInfo,
}

impl Deref for RuntimeLease {
    type Target = RuntimeInfo;

    fn deref(&self) -> &Self::Target {
        &self.info
    }
}

impl Drop for RuntimeLease {
    fn drop(&mut self) {
        if let Ok(mut state) = RUNTIME_USE.lock() {
            state.active_leases = state.active_leases.saturating_sub(1);
        }
    }
}

#[derive(Default)]
struct RuntimeUseState {
    active_leases: usize,
    uninstalling: bool,
}

struct PackageControl {
    status: OcrPackageStatus,
    cancel: Option<Arc<AtomicBool>>,
}

impl Default for PackageControl {
    fn default() -> Self {
        Self {
            status: OcrPackageStatus::not_installed(),
            cancel: None,
        }
    }
}

static PACKAGE_CONTROL: LazyLock<Mutex<PackageControl>> =
    LazyLock::new(|| Mutex::new(PackageControl::default()));
static PACKAGE_OPERATION: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));
static RUNTIME_USE: LazyLock<Mutex<RuntimeUseState>> =
    LazyLock::new(|| Mutex::new(RuntimeUseState::default()));

#[derive(Clone)]
struct OcrPackageManager {
    root: PathBuf,
    manifest_url: Url,
    client: Client,
    allow_loopback_http: bool,
}

impl OcrPackageManager {
    fn production() -> AppResult<Self> {
        let manifest_url = Url::parse(&release_manifest_url())
            .map_err(|_| package_error("OCR_PACKAGE_MANIFEST_URL_INVALID"))?;
        validate_https_url(manifest_url.as_str(), false)?;
        let client = Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.url().scheme() == "https" {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .connect_timeout(Duration::from_secs(20))
            .timeout(Duration::from_secs(20 * 60))
            .build()
            .map_err(|_| package_error("OCR_PACKAGE_HTTP_CLIENT_FAILED"))?;
        Ok(Self {
            root: package_root(),
            manifest_url,
            client,
            allow_loopback_http: false,
        })
    }

    async fn download_and_install(
        &self,
        app: &AppHandle,
        cancel: &AtomicBool,
    ) -> AppResult<RuntimeInfo> {
        fs::create_dir_all(self.root.join("downloads"))
            .map_err(|_| package_error("OCR_PACKAGE_STORAGE_FAILED"))?;
        fs::create_dir_all(self.root.join("versions"))
            .map_err(|_| package_error("OCR_PACKAGE_STORAGE_FAILED"))?;

        let manifest = fetch_manifest(
            &self.client,
            self.manifest_url.clone(),
            self.allow_loopback_http,
            cancel,
        )
        .await?;
        publish_status(
            app,
            OcrPackageStatus {
                state: OcrPackageState::Downloading,
                version: Some(manifest.version.clone()),
                downloaded_bytes: Some(0),
                total_bytes: Some(manifest.download_size),
                installed_bytes: None,
                error_code: None,
            },
        );

        let archive = self.root.join("downloads").join(format!(
            "{}.{}.partial",
            manifest.version,
            uuid::Uuid::new_v4()
        ));
        let cleanup = FileCleanup::new(archive.clone());
        download_with_retries(
            &self.client,
            Url::parse(&manifest.url).map_err(|_| package_error("OCR_PACKAGE_MANIFEST_INVALID"))?,
            &archive,
            manifest.download_size,
            cancel,
            |downloaded| {
                publish_status(
                    app,
                    OcrPackageStatus {
                        state: OcrPackageState::Downloading,
                        version: Some(manifest.version.clone()),
                        downloaded_bytes: Some(downloaded),
                        total_bytes: Some(manifest.download_size),
                        installed_bytes: None,
                        error_code: None,
                    },
                );
            },
        )
        .await?;

        check_cancelled(cancel)?;
        publish_status(
            app,
            OcrPackageStatus {
                state: OcrPackageState::Verifying,
                version: Some(manifest.version.clone()),
                downloaded_bytes: Some(manifest.download_size),
                total_bytes: Some(manifest.download_size),
                installed_bytes: None,
                error_code: None,
            },
        );
        verify_file_hash(&archive, manifest.download_size, &manifest.sha256)?;

        check_cancelled(cancel)?;
        publish_status(
            app,
            OcrPackageStatus {
                state: OcrPackageState::Installing,
                version: Some(manifest.version.clone()),
                downloaded_bytes: Some(manifest.download_size),
                total_bytes: Some(manifest.download_size),
                installed_bytes: None,
                error_code: None,
            },
        );
        let runtime = install_verified_archive(&self.root, &archive, &manifest, cancel)?;
        drop(cleanup);
        Ok(runtime)
    }
}

struct FileCleanup {
    path: PathBuf,
}

impl FileCleanup {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for FileCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

struct DirectoryCleanup {
    path: PathBuf,
    armed: bool,
}

impl DirectoryCleanup {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for DirectoryCleanup {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

pub(crate) fn installed_runtime() -> Option<RuntimeInfo> {
    installed_runtime_at(&package_root()).ok().flatten()
}

pub(crate) fn acquire_installed_runtime() -> Option<RuntimeLease> {
    let mut state = RUNTIME_USE.lock().ok()?;
    if state.uninstalling {
        return None;
    }
    let info = installed_runtime()?;
    state.active_leases += 1;
    Some(RuntimeLease { info })
}

#[tauri::command]
pub(crate) fn ocr_package_status() -> AppResult<OcrPackageStatus> {
    let transient = PACKAGE_CONTROL
        .lock()
        .map_err(|_| package_error("OCR_PACKAGE_STATE_POISONED"))?
        .status
        .clone();
    if transient.transitional() || transient.state == OcrPackageState::Failed {
        return Ok(transient);
    }
    status_from_disk(&package_root())
}

#[tauri::command]
pub(crate) fn ocr_package_download(app: AppHandle) -> AppResult<()> {
    if installed_runtime().is_some() {
        let status = status_from_disk(&package_root())?;
        publish_status(&app, status);
        return Ok(());
    }
    let cancel = {
        let mut control = PACKAGE_CONTROL
            .lock()
            .map_err(|_| package_error("OCR_PACKAGE_STATE_POISONED"))?;
        if control.status.transitional() || control.status.state == OcrPackageState::Installed {
            return Ok(());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        control.cancel = Some(cancel.clone());
        control.status = OcrPackageStatus {
            state: OcrPackageState::Downloading,
            version: None,
            downloaded_bytes: Some(0),
            total_bytes: None,
            installed_bytes: None,
            error_code: None,
        };
        cancel
    };
    publish_current_status(&app);

    tauri::async_runtime::spawn(async move {
        let _operation = PACKAGE_OPERATION.lock().await;
        let result = match OcrPackageManager::production() {
            Ok(manager) => manager.download_and_install(&app, &cancel).await,
            Err(error) => Err(error),
        };
        let status = match result {
            Ok(info) => status_from_disk(&package_root()).unwrap_or(OcrPackageStatus {
                state: OcrPackageState::Installed,
                version: Some(info.version),
                downloaded_bytes: None,
                total_bytes: None,
                installed_bytes: None,
                error_code: None,
            }),
            Err(error) if is_cancelled_error(&error) => status_from_disk(&package_root())
                .unwrap_or_else(|_| OcrPackageStatus::not_installed()),
            Err(error) => {
                log::error!("ocr package install failed: {error}");
                OcrPackageStatus {
                    state: OcrPackageState::Failed,
                    version: None,
                    downloaded_bytes: None,
                    total_bytes: None,
                    installed_bytes: None,
                    error_code: Some(stable_error_code(&error)),
                }
            }
        };
        if let Ok(mut control) = PACKAGE_CONTROL.lock() {
            control.cancel = None;
            control.status = status.clone();
        }
        let _ = app.emit("ocr-package-changed", status);
    });
    Ok(())
}

#[tauri::command]
pub(crate) fn ocr_package_cancel() -> AppResult<()> {
    let control = PACKAGE_CONTROL
        .lock()
        .map_err(|_| package_error("OCR_PACKAGE_STATE_POISONED"))?;
    if let Some(cancel) = &control.cancel {
        cancel.store(true, Ordering::Release);
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn ocr_package_uninstall(app: AppHandle, db: State<'_, Db>) -> AppResult<()> {
    let _operation = PACKAGE_OPERATION.lock().await;
    if active_ocr_jobs(&db)? {
        return Err(package_error("OCR_PACKAGE_ACTIVE_JOBS"));
    }
    {
        let mut runtime_use = RUNTIME_USE
            .lock()
            .map_err(|_| package_error("OCR_PACKAGE_STATE_POISONED"))?;
        if runtime_use.active_leases != 0 || runtime_use.uninstalling {
            return Err(package_error("OCR_PACKAGE_ACTIVE_JOBS"));
        }
        runtime_use.uninstalling = true;
    }
    let uninstall_guard = UninstallGuard;
    publish_status(
        &app,
        OcrPackageStatus {
            state: OcrPackageState::Uninstalling,
            version: installed_runtime().map(|info| info.version),
            downloaded_bytes: None,
            total_bytes: None,
            installed_bytes: None,
            error_code: None,
        },
    );

    let result = uninstall_runtime_at(&package_root());
    drop(uninstall_guard);
    match result {
        Ok(()) => {
            publish_status(&app, OcrPackageStatus::not_installed());
            Ok(())
        }
        Err(error) => {
            let status = OcrPackageStatus {
                state: OcrPackageState::Failed,
                version: None,
                downloaded_bytes: None,
                total_bytes: None,
                installed_bytes: None,
                error_code: Some(stable_error_code(&error)),
            };
            publish_status(&app, status);
            Err(error)
        }
    }
}

struct UninstallGuard;

impl Drop for UninstallGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = RUNTIME_USE.lock() {
            state.uninstalling = false;
        }
    }
}

fn package_root() -> PathBuf {
    crate::resolve_app_data_dir().join("ocr-runtime")
}

fn release_manifest_url() -> String {
    let target = target_asset_name().unwrap_or("unsupported");
    format!("{RELEASE_ROOT}/{RELEASE_TAG}/manifest-{target}.json")
}

fn target_platform_arch() -> AppResult<(&'static str, &'static str)> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        Ok(("macos", "arm64"))
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        Ok(("windows", "x64"))
    }
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "windows", target_arch = "x86_64")
    )))]
    Err(package_error("OCR_PACKAGE_PLATFORM_UNSUPPORTED"))
}

fn target_asset_name() -> AppResult<&'static str> {
    let (platform, arch) = target_platform_arch()?;
    match (platform, arch) {
        ("macos", "arm64") => Ok("macos-arm64"),
        ("windows", "x64") => Ok("windows-x64"),
        _ => Err(package_error("OCR_PACKAGE_PLATFORM_UNSUPPORTED")),
    }
}

fn status_from_disk(root: &Path) -> AppResult<OcrPackageStatus> {
    let Some(info) = installed_runtime_at(root)? else {
        return Ok(OcrPackageStatus::not_installed());
    };
    let manifest = read_installed_manifest(&info.root)?;
    Ok(OcrPackageStatus {
        state: OcrPackageState::Installed,
        version: Some(info.version),
        downloaded_bytes: None,
        total_bytes: Some(manifest.download_size),
        installed_bytes: Some(directory_size(&info.root)?),
        error_code: None,
    })
}

fn installed_runtime_at(root: &Path) -> AppResult<Option<RuntimeInfo>> {
    let pointer_path = root.join("current.json");
    let bytes = match fs::read(&pointer_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(package_error("OCR_PACKAGE_POINTER_INVALID")),
    };
    if bytes.len() > 4096 {
        return Err(package_error("OCR_PACKAGE_POINTER_INVALID"));
    }
    let pointer: CurrentPointer =
        serde_json::from_slice(&bytes).map_err(|_| package_error("OCR_PACKAGE_POINTER_INVALID"))?;
    if !safe_path_token(&pointer.version) || !safe_path_token(&pointer.directory) {
        return Err(package_error("OCR_PACKAGE_POINTER_INVALID"));
    }
    let versions = root.join("versions");
    let runtime_root = versions.join(&pointer.directory);
    let metadata = match fs::symlink_metadata(&runtime_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(package_error("OCR_PACKAGE_POINTER_INVALID")),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(package_error("OCR_PACKAGE_POINTER_INVALID"));
    }
    let canonical_versions = versions
        .canonicalize()
        .map_err(|_| package_error("OCR_PACKAGE_POINTER_INVALID"))?;
    let canonical_runtime = runtime_root
        .canonicalize()
        .map_err(|_| package_error("OCR_PACKAGE_POINTER_INVALID"))?;
    if !canonical_runtime.starts_with(&canonical_versions) {
        return Err(package_error("OCR_PACKAGE_POINTER_INVALID"));
    }
    let manifest = read_installed_manifest(&canonical_runtime)?;
    if manifest.version != pointer.version || manifest.package_id != PACKAGE_ID {
        return Err(package_error("OCR_PACKAGE_POINTER_INVALID"));
    }
    if !runtime_launcher(&canonical_runtime).is_file()
        || !runtime_tessdata(&canonical_runtime)
            .join("eng.traineddata")
            .is_file()
        || !runtime_tessdata(&canonical_runtime)
            .join("chi_sim.traineddata")
            .is_file()
    {
        return Err(package_error("OCR_PACKAGE_INSTALL_INVALID"));
    }
    Ok(Some(RuntimeInfo {
        root: canonical_runtime,
        version: pointer.version,
    }))
}

async fn fetch_manifest(
    client: &Client,
    url: Url,
    allow_loopback_http: bool,
    cancel: &AtomicBool,
) -> AppResult<RuntimeManifest> {
    validate_https_url(url.as_str(), allow_loopback_http)?;
    let mut last_error = package_error("OCR_PACKAGE_MANIFEST_DOWNLOAD_FAILED");
    for _ in 0..DOWNLOAD_ATTEMPTS {
        check_cancelled(cancel)?;
        let response = match client
            .get(url.clone())
            .header(ACCEPT_ENCODING, "identity")
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => {
                last_error = package_error("OCR_PACKAGE_MANIFEST_DOWNLOAD_FAILED");
                continue;
            }
        };
        if !response.status().is_success() {
            last_error = package_error("OCR_PACKAGE_MANIFEST_DOWNLOAD_FAILED");
            if response.status().is_server_error() {
                continue;
            }
            return Err(last_error);
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_MANIFEST_BYTES)
        {
            return Err(package_error("OCR_PACKAGE_MANIFEST_INVALID"));
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| package_error("OCR_PACKAGE_MANIFEST_DOWNLOAD_FAILED"))?;
            if bytes.len().saturating_add(chunk.len()) > MAX_MANIFEST_BYTES as usize {
                return Err(package_error("OCR_PACKAGE_MANIFEST_INVALID"));
            }
            bytes.extend_from_slice(&chunk);
        }
        let manifest: RuntimeManifest = serde_json::from_slice(&bytes)
            .map_err(|_| package_error("OCR_PACKAGE_MANIFEST_INVALID"))?;
        manifest.validate(allow_loopback_http)?;
        return Ok(manifest);
    }
    Err(last_error)
}

async fn download_with_retries<F>(
    client: &Client,
    url: Url,
    destination: &Path,
    expected_size: u64,
    cancel: &AtomicBool,
    mut progress: F,
) -> AppResult<()>
where
    F: FnMut(u64),
{
    for attempt in 0..DOWNLOAD_ATTEMPTS {
        let _ = fs::remove_file(destination);
        match download_once(
            client,
            url.clone(),
            destination,
            expected_size,
            cancel,
            &mut progress,
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(DownloadAttemptError::Cancelled) => {
                let _ = fs::remove_file(destination);
                return Err(package_error("OCR_PACKAGE_CANCELLED"));
            }
            Err(DownloadAttemptError::Fatal(code)) => {
                let _ = fs::remove_file(destination);
                return Err(package_error(code));
            }
            Err(DownloadAttemptError::Retryable) if attempt + 1 < DOWNLOAD_ATTEMPTS => continue,
            Err(DownloadAttemptError::Retryable) => break,
        }
    }
    let _ = fs::remove_file(destination);
    Err(package_error("OCR_PACKAGE_DOWNLOAD_FAILED"))
}

enum DownloadAttemptError {
    Retryable,
    Fatal(&'static str),
    Cancelled,
}

async fn download_once<F>(
    client: &Client,
    url: Url,
    destination: &Path,
    expected_size: u64,
    cancel: &AtomicBool,
    progress: &mut F,
) -> Result<(), DownloadAttemptError>
where
    F: FnMut(u64),
{
    if cancel.load(Ordering::Acquire) {
        return Err(DownloadAttemptError::Cancelled);
    }
    let response = client
        .get(url)
        .header(ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(|_| DownloadAttemptError::Retryable)?;
    let status = response.status();
    if !status.is_success() {
        return if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
            Err(DownloadAttemptError::Retryable)
        } else {
            Err(DownloadAttemptError::Fatal("OCR_PACKAGE_DOWNLOAD_FAILED"))
        };
    }
    if response
        .content_length()
        .is_some_and(|length| length != expected_size)
    {
        return Err(DownloadAttemptError::Retryable);
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|_| DownloadAttemptError::Fatal("OCR_PACKAGE_STORAGE_FAILED"))?;
    let mut downloaded = 0_u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::Acquire) {
            return Err(DownloadAttemptError::Cancelled);
        }
        let chunk = chunk.map_err(|_| DownloadAttemptError::Retryable)?;
        downloaded = downloaded
            .checked_add(chunk.len() as u64)
            .ok_or(DownloadAttemptError::Fatal("OCR_PACKAGE_LENGTH_MISMATCH"))?;
        if downloaded > expected_size {
            return Err(DownloadAttemptError::Fatal("OCR_PACKAGE_LENGTH_MISMATCH"));
        }
        file.write_all(&chunk)
            .map_err(|_| DownloadAttemptError::Fatal("OCR_PACKAGE_STORAGE_FAILED"))?;
        progress(downloaded);
    }
    file.sync_all()
        .map_err(|_| DownloadAttemptError::Fatal("OCR_PACKAGE_STORAGE_FAILED"))?;
    if downloaded != expected_size {
        return Err(DownloadAttemptError::Retryable);
    }
    Ok(())
}

fn verify_file_hash(path: &Path, expected_size: u64, expected_sha256: &str) -> AppResult<()> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| package_error("OCR_PACKAGE_ARCHIVE_INVALID"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != expected_size {
        return Err(package_error("OCR_PACKAGE_LENGTH_MISMATCH"));
    }
    let mut file = File::open(path).map_err(|_| package_error("OCR_PACKAGE_ARCHIVE_INVALID"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| package_error("OCR_PACKAGE_ARCHIVE_INVALID"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err(package_error("OCR_PACKAGE_HASH_MISMATCH"));
    }
    Ok(())
}

fn install_verified_archive(
    package_root: &Path,
    archive: &Path,
    manifest: &RuntimeManifest,
    cancel: &AtomicBool,
) -> AppResult<RuntimeInfo> {
    check_cancelled(cancel)?;
    let versions = package_root.join("versions");
    fs::create_dir_all(&versions).map_err(|_| package_error("OCR_PACKAGE_STORAGE_FAILED"))?;
    let candidate = versions.join(format!(".install-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&candidate).map_err(|_| package_error("OCR_PACKAGE_STORAGE_FAILED"))?;
    let candidate_cleanup = DirectoryCleanup::new(candidate.clone());
    safe_extract_tar_zst(archive, &candidate, manifest.installed_size)?;
    check_required_runtime_files(&candidate)?;
    check_cancelled(cancel)?;

    let result = activate_candidate(
        package_root,
        &candidate,
        manifest,
        cancel,
        |root, cancel| {
            run_conda_unpack_if_present(root, cancel)?;
            self_test_runtime(root, cancel)
        },
    );
    // `activate_candidate` renames the directory before self-test. Its own
    // guard removes a failed final directory; this guard handles failures that
    // happen before the rename.
    drop(candidate_cleanup);
    result
}

fn activate_candidate<F>(
    package_root: &Path,
    candidate: &Path,
    manifest: &RuntimeManifest,
    cancel: &AtomicBool,
    self_test: F,
) -> AppResult<RuntimeInfo>
where
    F: FnOnce(&Path, &AtomicBool) -> AppResult<()>,
{
    let versions = package_root.join("versions");
    let directory = format!(
        "{}-{}",
        manifest.version,
        manifest.sha256[..12].to_ascii_lowercase()
    );
    if !safe_path_token(&directory) {
        return Err(package_error("OCR_PACKAGE_MANIFEST_INVALID"));
    }
    let final_root = versions.join(&directory);
    if final_root.exists() {
        let current = installed_runtime_at(package_root)?;
        if current.as_ref().is_some_and(|info| info.root == final_root) {
            self_test(&final_root, cancel)?;
            return Ok(current.expect("checked above"));
        }
        fs::remove_dir_all(&final_root).map_err(|_| package_error("OCR_PACKAGE_STORAGE_FAILED"))?;
    }
    fs::rename(candidate, &final_root).map_err(|_| package_error("OCR_PACKAGE_STORAGE_FAILED"))?;
    let mut final_cleanup = DirectoryCleanup::new(final_root.clone());
    check_cancelled(cancel)?;
    self_test(&final_root, cancel)?;
    check_cancelled(cancel)?;
    write_installed_manifest(&final_root, manifest)?;
    let pointer = CurrentPointer {
        version: manifest.version.clone(),
        directory: directory.clone(),
    };
    write_current_pointer(package_root, &pointer)?;
    final_cleanup.disarm();
    cleanup_old_versions(package_root, &directory);
    Ok(RuntimeInfo {
        root: final_root,
        version: manifest.version.clone(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveEntryKind {
    File,
    Directory,
    LinkOrSpecial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ArchiveEntryMetadata {
    kind: ArchiveEntryKind,
    size: u64,
}

fn safe_extract_tar_zst(archive: &Path, destination: &Path, expected_size: u64) -> AppResult<()> {
    let names_output = run_tar(archive, &["-tf"])?;
    let verbose_output = run_tar(archive, &["-tvf"])?;
    if names_output.len() > MAX_ARCHIVE_LISTING_BYTES
        || verbose_output.len() > MAX_ARCHIVE_LISTING_BYTES
    {
        return Err(package_error("OCR_PACKAGE_ARCHIVE_INVALID"));
    }
    let names = String::from_utf8(names_output)
        .map_err(|_| package_error("OCR_PACKAGE_ARCHIVE_INVALID"))?
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let metadata = String::from_utf8(verbose_output)
        .map_err(|_| package_error("OCR_PACKAGE_ARCHIVE_INVALID"))?
        .lines()
        .map(parse_verbose_archive_entry)
        .collect::<AppResult<Vec<_>>>()?;
    if names.is_empty() || names.len() != metadata.len() || names.len() > MAX_ARCHIVE_ENTRIES {
        return Err(package_error("OCR_PACKAGE_ARCHIVE_INVALID"));
    }
    validate_archive_entries(names.iter().zip(metadata.iter().copied()))?;
    let listed_size = metadata.iter().try_fold(0_u64, |total, entry| {
        total
            .checked_add(entry.size)
            .ok_or_else(|| package_error("OCR_PACKAGE_INSTALLED_SIZE_MISMATCH"))
    })?;
    if listed_size != expected_size {
        return Err(package_error("OCR_PACKAGE_INSTALLED_SIZE_MISMATCH"));
    }

    let status = Command::new(tar_program())
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(destination)
        .status()
        .map_err(|_| package_error("OCR_PACKAGE_EXTRACT_FAILED"))?;
    if !status.success() {
        return Err(package_error("OCR_PACKAGE_EXTRACT_FAILED"));
    }
    let actual_size = validate_extracted_tree(destination)?;
    if actual_size != expected_size {
        return Err(package_error("OCR_PACKAGE_INSTALLED_SIZE_MISMATCH"));
    }
    Ok(())
}

fn run_tar(archive: &Path, operation: &[&str]) -> AppResult<Vec<u8>> {
    let mut command = Command::new(tar_program());
    for argument in operation {
        command.arg(argument);
    }
    let output = command
        .env("LC_ALL", "C")
        .arg(archive)
        .output()
        .map_err(|_| package_error("OCR_PACKAGE_ARCHIVE_UNSUPPORTED"))?;
    if !output.status.success() {
        return Err(package_error("OCR_PACKAGE_ARCHIVE_INVALID"));
    }
    Ok(output.stdout)
}

fn tar_program() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Some(root) = std::env::var_os("SystemRoot") {
            return PathBuf::from(root).join("System32").join("tar.exe");
        }
        PathBuf::from("tar.exe")
    }
    #[cfg(not(target_os = "windows"))]
    {
        PathBuf::from("/usr/bin/tar")
    }
}

fn validate_archive_entries<'a>(
    entries: impl IntoIterator<Item = (&'a String, ArchiveEntryMetadata)>,
) -> AppResult<()> {
    let mut seen = HashSet::new();
    for (name, metadata) in entries {
        if metadata.kind == ArchiveEntryKind::LinkOrSpecial {
            return Err(package_error("OCR_PACKAGE_ARCHIVE_ESCAPE"));
        }
        let normalized = match normalize_archive_path(name) {
            Some(path) => path,
            None if metadata.kind == ArchiveEntryKind::Directory
                && matches!(name.as_str(), "." | "./") =>
            {
                continue;
            }
            None => return Err(package_error("OCR_PACKAGE_ARCHIVE_ESCAPE")),
        };
        if !seen.insert(normalized.to_string()) {
            return Err(package_error("OCR_PACKAGE_ARCHIVE_INVALID"));
        }
    }
    Ok(())
}

fn safe_archive_path(value: &str) -> bool {
    normalize_archive_path(value).is_some()
}

fn normalize_archive_path(value: &str) -> Option<&str> {
    if value.is_empty()
        || value.len() > 1024
        || value.starts_with('/')
        || value.starts_with('\\')
        || value.contains('\\')
        || value.contains('\0')
        || value.contains(':')
    {
        return None;
    }
    let value = value.trim_end_matches('/');
    let value = value.strip_prefix("./").unwrap_or(value);
    if value.is_empty() {
        return None;
    }
    Path::new(value)
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
        .then_some(value)
}

fn parse_verbose_archive_entry(line: &str) -> AppResult<ArchiveEntryMetadata> {
    let kind = match line.as_bytes().first().copied() {
        Some(b'-') => ArchiveEntryKind::File,
        Some(b'd') => ArchiveEntryKind::Directory,
        _ => ArchiveEntryKind::LinkOrSpecial,
    };
    // macOS and Windows both ship bsdtar. Its stable verbose prefix is:
    // mode, link-count, owner, group, byte-size. Names and dates may contain
    // spaces, so only the prefix is parsed.
    let size = line
        .split_whitespace()
        .nth(4)
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| package_error("OCR_PACKAGE_ARCHIVE_INVALID"))?;
    Ok(ArchiveEntryMetadata { kind, size })
}

fn validate_extracted_tree(root: &Path) -> AppResult<u64> {
    let canonical_root = root
        .canonicalize()
        .map_err(|_| package_error("OCR_PACKAGE_EXTRACT_FAILED"))?;
    let mut stack = vec![root.to_path_buf()];
    let mut total = 0_u64;
    let mut entries = 0_usize;
    while let Some(directory) = stack.pop() {
        for entry in
            fs::read_dir(&directory).map_err(|_| package_error("OCR_PACKAGE_EXTRACT_FAILED"))?
        {
            let entry = entry.map_err(|_| package_error("OCR_PACKAGE_EXTRACT_FAILED"))?;
            entries += 1;
            if entries > MAX_ARCHIVE_ENTRIES {
                return Err(package_error("OCR_PACKAGE_ARCHIVE_INVALID"));
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)
                .map_err(|_| package_error("OCR_PACKAGE_EXTRACT_FAILED"))?;
            if metadata.file_type().is_symlink() {
                return Err(package_error("OCR_PACKAGE_ARCHIVE_ESCAPE"));
            }
            let canonical = path
                .canonicalize()
                .map_err(|_| package_error("OCR_PACKAGE_EXTRACT_FAILED"))?;
            if !canonical.starts_with(&canonical_root) {
                return Err(package_error("OCR_PACKAGE_ARCHIVE_ESCAPE"));
            }
            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file() {
                total = total
                    .checked_add(metadata.len())
                    .ok_or_else(|| package_error("OCR_PACKAGE_INSTALLED_SIZE_MISMATCH"))?;
                if total > MAX_INSTALLED_BYTES {
                    return Err(package_error("OCR_PACKAGE_INSTALLED_SIZE_MISMATCH"));
                }
            } else {
                return Err(package_error("OCR_PACKAGE_ARCHIVE_ESCAPE"));
            }
        }
    }
    Ok(total)
}

fn check_required_runtime_files(root: &Path) -> AppResult<()> {
    let required = [
        runtime_launcher(root),
        runtime_tessdata(root).join("eng.traineddata"),
        runtime_tessdata(root).join("chi_sim.traineddata"),
        root.join("lib/lantern_progress.py"),
        root.join("lib/lantern_ocr.py"),
        root.join("share/fixtures/scan-fixture.pdf"),
        root.join("THIRD_PARTY_NOTICES.txt"),
        root.join("SBOM.cdx.json"),
    ];
    if required.iter().all(|path| path.is_file()) {
        Ok(())
    } else {
        Err(package_error("OCR_PACKAGE_INSTALL_INVALID"))
    }
}

fn runtime_launcher(root: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        root.join("bin").join("lantern-ocr.exe")
    }
    #[cfg(not(target_os = "windows"))]
    {
        root.join("bin").join("lantern-ocr")
    }
}

fn runtime_tessdata(root: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        root.join("Library").join("share").join("tessdata")
    }
    #[cfg(not(target_os = "windows"))]
    {
        root.join("share").join("tessdata")
    }
}

fn run_conda_unpack_if_present(root: &Path, cancel: &AtomicBool) -> AppResult<()> {
    #[cfg(target_os = "windows")]
    let unpack = root.join("Scripts").join("conda-unpack.exe");
    #[cfg(not(target_os = "windows"))]
    let unpack = root.join("bin").join("conda-unpack");
    if !unpack.is_file() {
        return Ok(());
    }
    let mut command = Command::new(unpack);
    command.current_dir(root);
    run_command_bounded(
        &mut command,
        root,
        "conda-unpack",
        cancel,
        SELF_TEST_TIMEOUT,
    )
}

fn self_test_runtime(root: &Path, cancel: &AtomicBool) -> AppResult<()> {
    check_required_runtime_files(root)?;
    let launcher = runtime_launcher(root);
    let fixture = root.join("share/fixtures/scan-fixture.pdf");
    let output = root
        .parent()
        .ok_or_else(|| package_error("OCR_PACKAGE_SELF_TEST_FAILED"))?
        .join(format!(".self-test-{}.pdf", uuid::Uuid::new_v4()));
    let output_cleanup = FileCleanup::new(output.clone());

    let mut version = Command::new(&launcher);
    configure_runtime_command(&mut version, root);
    version.arg("--version");
    run_command_bounded(
        &mut version,
        root,
        "version",
        cancel,
        Duration::from_secs(30),
    )?;

    let mut smoke = Command::new(&launcher);
    configure_runtime_command(&mut smoke, root);
    smoke.args([
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
        "1",
        "-l",
        "chi_sim+eng",
        "--",
    ]);
    smoke.arg(&fixture).arg(&output);
    run_command_bounded(&mut smoke, root, "ocr", cancel, SELF_TEST_TIMEOUT)?;
    let mut signature = [0_u8; 5];
    let mut file =
        File::open(&output).map_err(|_| package_error("OCR_PACKAGE_SELF_TEST_FAILED"))?;
    file.read_exact(&mut signature)
        .map_err(|_| package_error("OCR_PACKAGE_SELF_TEST_FAILED"))?;
    if &signature != b"%PDF-" {
        return Err(package_error("OCR_PACKAGE_SELF_TEST_FAILED"));
    }
    drop(output_cleanup);
    Ok(())
}

fn configure_runtime_command(command: &mut Command, root: &Path) {
    let mut paths = Vec::new();
    #[cfg(target_os = "windows")]
    {
        paths.push(root.to_path_buf());
        paths.push(root.join("Scripts"));
        paths.push(root.join("Library").join("bin"));
        if let Some(system_root) = std::env::var_os("SystemRoot") {
            paths.push(PathBuf::from(system_root).join("System32"));
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        paths.push(root.join("bin"));
        paths.push(PathBuf::from("/usr/bin"));
        paths.push(PathBuf::from("/bin"));
    }
    let path = std::env::join_paths(paths).unwrap_or_else(|_| OsString::new());
    command
        .env_clear()
        .env("PATH", path)
        .env("TESSDATA_PREFIX", runtime_tessdata(root))
        .env("OMP_THREAD_LIMIT", "1")
        .env("PYTHONNOUSERSITE", "1")
        .env("PYTHONUTF8", "1")
        .current_dir(root);
    for key in [
        "APPDATA",
        "COMSPEC",
        "HOME",
        "LANG",
        "LC_ALL",
        "LOCALAPPDATA",
        "PATHEXT",
        "TEMP",
        "TMP",
        "TMPDIR",
        "USERPROFILE",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    #[cfg(target_os = "windows")]
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        command.env("SystemRoot", system_root);
    }
}

fn run_command_bounded(
    command: &mut Command,
    working_root: &Path,
    label: &str,
    cancel: &AtomicBool,
    timeout: Duration,
) -> AppResult<()> {
    let log_root = working_root
        .parent()
        .ok_or_else(|| package_error("OCR_PACKAGE_SELF_TEST_FAILED"))?;
    let stdout_path = log_root.join(format!(".self-test-{label}-{}.out", uuid::Uuid::new_v4()));
    let stderr_path = log_root.join(format!(".self-test-{label}-{}.err", uuid::Uuid::new_v4()));
    let stdout_cleanup = FileCleanup::new(stdout_path.clone());
    let stderr_cleanup = FileCleanup::new(stderr_path.clone());
    let stdout =
        File::create(&stdout_path).map_err(|_| package_error("OCR_PACKAGE_SELF_TEST_FAILED"))?;
    let stderr =
        File::create(&stderr_path).map_err(|_| package_error("OCR_PACKAGE_SELF_TEST_FAILED"))?;
    let mut child = command
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|_| package_error("OCR_PACKAGE_SELF_TEST_FAILED"))?;
    let started = Instant::now();
    let status = loop {
        if cancel.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(package_error("OCR_PACKAGE_CANCELLED"));
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(package_error("OCR_PACKAGE_SELF_TEST_FAILED"));
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return Err(package_error("OCR_PACKAGE_SELF_TEST_FAILED")),
        }
    };
    if !status.success() {
        let detail = read_limited(&stderr_path, 4096).unwrap_or_default();
        log::error!("ocr runtime self-test {label} failed: {detail}");
        return Err(package_error("OCR_PACKAGE_SELF_TEST_FAILED"));
    }
    drop(stdout_cleanup);
    drop(stderr_cleanup);
    Ok(())
}

fn read_limited(path: &Path, limit: usize) -> std::io::Result<String> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(limit as u64).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn write_installed_manifest(root: &Path, manifest: &RuntimeManifest) -> AppResult<()> {
    let bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|_| package_error("OCR_PACKAGE_MANIFEST_INVALID"))?;
    let path = root.join(".lantern-runtime-manifest.json");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| package_error("OCR_PACKAGE_STORAGE_FAILED"))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| package_error("OCR_PACKAGE_STORAGE_FAILED"))
}

fn read_installed_manifest(root: &Path) -> AppResult<RuntimeManifest> {
    let bytes = fs::read(root.join(".lantern-runtime-manifest.json"))
        .map_err(|_| package_error("OCR_PACKAGE_INSTALL_INVALID"))?;
    if bytes.len() > MAX_MANIFEST_BYTES as usize {
        return Err(package_error("OCR_PACKAGE_INSTALL_INVALID"));
    }
    serde_json::from_slice(&bytes).map_err(|_| package_error("OCR_PACKAGE_INSTALL_INVALID"))
}

fn write_current_pointer(root: &Path, pointer: &CurrentPointer) -> AppResult<()> {
    fs::create_dir_all(root).map_err(|_| package_error("OCR_PACKAGE_STORAGE_FAILED"))?;
    let bytes =
        serde_json::to_vec(pointer).map_err(|_| package_error("OCR_PACKAGE_POINTER_INVALID"))?;
    let temporary = root.join(format!(".current-{}.tmp", uuid::Uuid::new_v4()));
    let destination = root.join("current.json");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|_| package_error("OCR_PACKAGE_STORAGE_FAILED"))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| package_error("OCR_PACKAGE_STORAGE_FAILED"))?;
    atomic_replace_file(&temporary, &destination)
        .map_err(|_| package_error("OCR_PACKAGE_STORAGE_FAILED"))?;
    sync_directory(root);
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn atomic_replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(target_os = "windows")]
fn atomic_replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    #[link(name = "Kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn sync_directory(path: &Path) {
    if let Ok(directory) = File::open(path) {
        let _ = directory.sync_all();
    }
}

fn cleanup_old_versions(root: &Path, current: &str) {
    let can_cleanup = RUNTIME_USE
        .lock()
        .map(|state| state.active_leases == 0)
        .unwrap_or(false);
    if !can_cleanup {
        return;
    }
    let versions = root.join("versions");
    let Ok(entries) = fs::read_dir(&versions) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy() == current {
            continue;
        }
        let path = entry.path();
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                if let Err(error) = fs::remove_dir_all(&path) {
                    log::warn!("ocr package old-version cleanup failed: {error}");
                }
            }
            Ok(_) => {
                let _ = fs::remove_file(&path);
            }
            Err(_) => {}
        }
    }
}

fn uninstall_runtime_at(root: &Path) -> AppResult<()> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(package_error("OCR_PACKAGE_UNINSTALL_FAILED")),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(package_error("OCR_PACKAGE_UNINSTALL_FAILED"));
    }
    fs::remove_dir_all(root).map_err(|_| package_error("OCR_PACKAGE_UNINSTALL_FAILED"))
}

fn active_ocr_jobs(db: &Db) -> AppResult<bool> {
    let conn = db
        .conn
        .lock()
        .map_err(|_| package_error("OCR_PACKAGE_STATE_POISONED"))?;
    conn.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM ocr_jobs
           WHERE state IN (
             'queued','waiting_source','preparing','recognizing','validating','publishing'
           )
         )",
        [],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn directory_size(root: &Path) -> AppResult<u64> {
    validate_extracted_tree(root)
}

fn safe_path_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn validate_https_url(value: &str, allow_loopback_http: bool) -> AppResult<Url> {
    let url = Url::parse(value).map_err(|_| package_error("OCR_PACKAGE_URL_INVALID"))?;
    let loopback_http = allow_loopback_http
        && url.scheme() == "http"
        && url
            .host_str()
            .is_some_and(|host| matches!(host, "127.0.0.1" | "::1" | "localhost"));
    if (url.scheme() != "https" && !loopback_http)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(package_error("OCR_PACKAGE_URL_INVALID"));
    }
    Ok(url)
}

fn publish_status(app: &AppHandle, status: OcrPackageStatus) {
    if let Ok(mut control) = PACKAGE_CONTROL.lock() {
        control.status = status.clone();
    }
    let _ = app.emit("ocr-package-changed", status);
}

fn publish_current_status(app: &AppHandle) {
    if let Ok(control) = PACKAGE_CONTROL.lock() {
        let _ = app.emit("ocr-package-changed", control.status.clone());
    }
}

fn check_cancelled(cancel: &AtomicBool) -> AppResult<()> {
    if cancel.load(Ordering::Acquire) {
        Err(package_error("OCR_PACKAGE_CANCELLED"))
    } else {
        Ok(())
    }
}

fn is_cancelled_error(error: &AppError) -> bool {
    matches!(error, AppError::Other(code) if code == "OCR_PACKAGE_CANCELLED")
}

fn stable_error_code(error: &AppError) -> String {
    match error {
        AppError::Other(code) if code.starts_with("OCR_PACKAGE_") => code.clone(),
        _ => "OCR_PACKAGE_FAILED".to_string(),
    }
}

fn package_error(code: &str) -> AppError {
    AppError::Other(code.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use sha2::Digest;
    use tempfile::TempDir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    #[test]
    fn archive_validation_rejects_traversal_and_links() {
        for path in [
            "../escape",
            "/absolute",
            "C:/windows/system32/file",
            "safe/../../escape",
            "safe\\..\\escape",
        ] {
            let entries = vec![(
                path.to_string(),
                ArchiveEntryMetadata {
                    kind: ArchiveEntryKind::File,
                    size: 1,
                },
            )];
            assert!(validate_archive_entries(entries.iter().map(|(p, k)| (p, *k))).is_err());
        }
        let link = vec![(
            "safe/link".to_string(),
            ArchiveEntryMetadata {
                kind: ArchiveEntryKind::LinkOrSpecial,
                size: 0,
            },
        )];
        assert!(validate_archive_entries(link.iter().map(|(p, k)| (p, *k))).is_err());

        let safe = vec![
            (
                "bin/lantern-ocr".to_string(),
                ArchiveEntryMetadata {
                    kind: ArchiveEntryKind::File,
                    size: 1,
                },
            ),
            (
                "share/tessdata/".to_string(),
                ArchiveEntryMetadata {
                    kind: ArchiveEntryKind::Directory,
                    size: 0,
                },
            ),
            (
                "./share/lantern-ocr/runner.py".to_string(),
                ArchiveEntryMetadata {
                    kind: ArchiveEntryKind::File,
                    size: 1,
                },
            ),
        ];
        assert!(validate_archive_entries(safe.iter().map(|(p, k)| (p, *k))).is_ok());
    }

    #[test]
    fn hash_mismatch_is_rejected() {
        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("runtime.tar.zst");
        fs::write(&archive, b"runtime").unwrap();
        let error = verify_file_hash(&archive, 7, &"0".repeat(64))
            .unwrap_err()
            .to_string();
        assert!(error.contains("OCR_PACKAGE_HASH_MISMATCH"));
    }

    #[tokio::test]
    async fn interrupted_download_retries_from_a_clean_partial() {
        let body = b"complete runtime archive".to_vec();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let accepted = Arc::new(AtomicUsize::new(0));
        let accepted_server = accepted.clone();
        let body_server = body.clone();
        let server = tokio::spawn(async move {
            for request_number in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = vec![0_u8; 2048];
                let _ = socket.read(&mut request).await.unwrap();
                accepted_server.fetch_add(1, Ordering::SeqCst);
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body_server.len()
                );
                socket.write_all(header.as_bytes()).await.unwrap();
                if request_number == 0 {
                    socket
                        .write_all(&body_server[..body_server.len() / 2])
                        .await
                        .unwrap();
                } else {
                    socket.write_all(&body_server).await.unwrap();
                }
            }
        });

        let dir = TempDir::new().unwrap();
        let output = dir.path().join("runtime.partial");
        let cancel = AtomicBool::new(false);
        download_with_retries(
            &Client::new(),
            Url::parse(&format!("http://{address}/runtime.tar.zst")).unwrap(),
            &output,
            body.len() as u64,
            &cancel,
            |_| {},
        )
        .await
        .unwrap();
        server.await.unwrap();
        assert_eq!(accepted.load(Ordering::SeqCst), 2);
        assert_eq!(fs::read(output).unwrap(), body);
    }

    #[test]
    fn self_test_failure_does_not_switch_current_runtime() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("ocr-runtime");
        let versions = root.join("versions");
        fs::create_dir_all(&versions).unwrap();

        let old_manifest = test_manifest("1.0.0", b"old");
        let old_directory = format!("1.0.0-{}", &old_manifest.sha256[..12]);
        let old_root = versions.join(&old_directory);
        seed_runtime(&old_root, &old_manifest);
        write_current_pointer(
            &root,
            &CurrentPointer {
                version: old_manifest.version.clone(),
                directory: old_directory,
            },
        )
        .unwrap();

        let new_manifest = test_manifest("1.1.0", b"new");
        let candidate = versions.join(".candidate");
        seed_runtime_payload(&candidate);
        let cancel = AtomicBool::new(false);
        let error = activate_candidate(&root, &candidate, &new_manifest, &cancel, |_, _| {
            Err(package_error("OCR_PACKAGE_SELF_TEST_FAILED"))
        })
        .unwrap_err()
        .to_string();
        assert!(error.contains("OCR_PACKAGE_SELF_TEST_FAILED"));
        assert_eq!(
            installed_runtime_at(&root).unwrap().unwrap().version,
            "1.0.0"
        );
    }

    #[test]
    fn uninstall_removes_only_runtime_and_keeps_book_assets() {
        let dir = TempDir::new().unwrap();
        let runtime = dir.path().join("app-data/ocr-runtime");
        let asset = dir.path().join("library/books/scanned.ocr.pdf");
        fs::create_dir_all(&runtime).unwrap();
        fs::create_dir_all(asset.parent().unwrap()).unwrap();
        fs::write(runtime.join("current.json"), b"runtime").unwrap();
        fs::write(&asset, b"asset").unwrap();

        uninstall_runtime_at(&runtime).unwrap();
        assert!(!runtime.exists());
        assert_eq!(fs::read(asset).unwrap(), b"asset");
    }

    #[test]
    fn manifest_requires_https_outside_test_loopback_exception() {
        assert!(validate_https_url("http://example.com/runtime.tar.zst", false).is_err());
        assert!(validate_https_url("http://127.0.0.1/runtime.tar.zst", true).is_ok());
        assert!(validate_https_url("https://example.com/runtime.tar.zst", false).is_ok());
        assert!(validate_https_url("https://user@example.com/runtime.tar.zst", false).is_err());
    }

    fn test_manifest(version: &str, bytes: &[u8]) -> RuntimeManifest {
        RuntimeManifest {
            package_id: PACKAGE_ID.to_string(),
            version: version.to_string(),
            platform: target_platform_arch()
                .map(|target| target.0)
                .unwrap_or("unsupported")
                .to_string(),
            arch: target_platform_arch()
                .map(|target| target.1)
                .unwrap_or("unsupported")
                .to_string(),
            minimum_os_version: "test".to_string(),
            download_size: bytes.len() as u64,
            installed_size: 1,
            sha256: format!("{:x}", Sha256::digest(bytes)),
            url: "https://example.com/runtime.tar.zst".to_string(),
        }
    }

    fn seed_runtime(root: &Path, manifest: &RuntimeManifest) {
        seed_runtime_payload(root);
        write_installed_manifest(root, manifest).unwrap();
    }

    fn seed_runtime_payload(root: &Path) {
        let files = [
            runtime_launcher(root),
            runtime_tessdata(root).join("eng.traineddata"),
            runtime_tessdata(root).join("chi_sim.traineddata"),
            root.join("lib/lantern_progress.py"),
            root.join("lib/lantern_ocr.py"),
            root.join("share/fixtures/scan-fixture.pdf"),
            root.join("THIRD_PARTY_NOTICES.txt"),
            root.join("SBOM.cdx.json"),
        ];
        for file in files {
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, b"x").unwrap();
        }
    }
}
