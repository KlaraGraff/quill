//! Sync settings file — read/write/remove `.sync_setting` containing the
//! user-authorized shared folder and whether the event-log engine should boot.
//!
//! Written by `sync_enable`, removed by `sync_disable`. JSON format
//! for future extensibility.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

const SYNC_SETTINGS_FILE: &str = ".sync_setting";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncSettings {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
}

pub fn sync_settings_path(local_dir: &Path) -> PathBuf {
    local_dir.join(SYNC_SETTINGS_FILE)
}

pub fn is_sync_enabled(local_dir: &Path) -> bool {
    read_sync_settings(local_dir)
        .is_some_and(|s| s.enabled && s.data_dir.as_deref().is_some_and(|path| !path.is_empty()))
}

pub fn read_sync_settings(local_dir: &Path) -> Option<SyncSettings> {
    let bytes = fs::read(sync_settings_path(local_dir)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn recorded_data_dir(local_dir: &Path) -> Option<PathBuf> {
    let settings = read_sync_settings(local_dir)?;
    let dir = settings.data_dir?;
    if dir.is_empty() {
        return None;
    }
    Some(PathBuf::from(dir))
}

/// Return the selected folder only when it is still an accessible iCloud Drive
/// directory. The marker is user-editable local state, so callers that touch
/// blobs must not treat its raw absolute path as authority.
pub fn recorded_usable_icloud_dir(local_dir: &Path) -> Option<PathBuf> {
    recorded_data_dir(local_dir).filter(|dir| is_usable_icloud_dir(dir))
}

pub fn is_usable_icloud_dir(path: &Path) -> bool {
    is_icloud_drive_dir(path) && is_writable_dir(path)
}

pub fn is_icloud_drive_dir(path: &Path) -> bool {
    let Some(home) = std::env::var_os("HOME") else {
        return false;
    };
    let Ok(root) = PathBuf::from(home)
        .join("Library/Mobile Documents")
        .canonicalize()
    else {
        return false;
    };
    let Ok(selected) = path.canonicalize() else {
        return false;
    };
    selected.starts_with(root)
}

pub fn is_writable_dir(path: &Path) -> bool {
    let probe = path.join(format!(
        ".quill-personal-write-probe-{}",
        uuid::Uuid::new_v4()
    ));
    match fs::write(&probe, []) {
        Ok(()) => {
            let _ = fs::remove_file(probe);
            true
        }
        Err(_) => false,
    }
}

fn write_settings(local_dir: &Path, settings: &SyncSettings) -> AppResult<()> {
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|e| AppError::Other(format!("serialize sync settings: {e}")))?;
    fs::write(sync_settings_path(local_dir), bytes)?;
    Ok(())
}

pub fn set_shared_dir(local_dir: &Path, data_dir: &Path) -> AppResult<()> {
    let existing = read_sync_settings(local_dir);
    let settings = SyncSettings {
        enabled: existing.is_some_and(|settings| settings.enabled),
        data_dir: Some(data_dir.to_string_lossy().into_owned()),
    };
    write_settings(local_dir, &settings)
}

pub fn set_sync_enabled(local_dir: &Path, enabled: bool) -> AppResult<()> {
    let mut settings = read_sync_settings(local_dir)
        .ok_or_else(|| AppError::Other("SYNC_FOLDER_NOT_CONFIGURED".to_string()))?;
    if settings.data_dir.as_deref().is_none_or(str::is_empty) {
        return Err(AppError::Other("SYNC_FOLDER_NOT_CONFIGURED".to_string()));
    }
    settings.enabled = enabled;
    write_settings(local_dir, &settings)
}

pub fn write_sync_settings(local_dir: &Path, data_dir: Option<&Path>) -> AppResult<()> {
    let data_dir =
        data_dir.ok_or_else(|| AppError::Other("SYNC_FOLDER_NOT_CONFIGURED".to_string()))?;
    set_shared_dir(local_dir, data_dir)?;
    set_sync_enabled(local_dir, true)
}

pub fn remove_sync_settings(local_dir: &Path) -> AppResult<()> {
    let path = sync_settings_path(local_dir);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AppError::Io(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn not_enabled_by_default() {
        let local = TempDir::new().unwrap();
        assert!(!is_sync_enabled(local.path()));
        assert_eq!(recorded_data_dir(local.path()), None);
    }

    #[test]
    fn write_then_read_round_trips() {
        let local = TempDir::new().unwrap();
        let data_dir = Path::new("/tmp/some/icloud/path");
        write_sync_settings(local.path(), Some(data_dir)).unwrap();
        assert!(is_sync_enabled(local.path()));
        assert_eq!(recorded_data_dir(local.path()).as_deref(), Some(data_dir));
    }

    #[test]
    fn remove_clears_settings() {
        let local = TempDir::new().unwrap();
        write_sync_settings(local.path(), Some(Path::new("/tmp"))).unwrap();
        assert!(is_sync_enabled(local.path()));
        remove_sync_settings(local.path()).unwrap();
        assert!(!is_sync_enabled(local.path()));
    }

    #[test]
    fn selected_dir_stays_disabled_until_explicitly_enabled() {
        let local = TempDir::new().unwrap();
        let data_dir = local.path().join("shared");
        set_shared_dir(local.path(), &data_dir).unwrap();
        assert!(!is_sync_enabled(local.path()));
        assert_eq!(
            recorded_data_dir(local.path()).as_deref(),
            Some(data_dir.as_path())
        );
        set_sync_enabled(local.path(), true).unwrap();
        assert!(is_sync_enabled(local.path()));
    }
}
