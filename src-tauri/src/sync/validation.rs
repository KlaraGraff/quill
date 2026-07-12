use std::path::{Component, Path, PathBuf};

use crate::error::{AppError, AppResult};

use super::events::{Event, EventBody, EVENT_SCHEMA_VERSION};

const BOOK_EXTENSIONS: &[&str] = &[
    "epub", "pdf", "txt", "md", "markdown", "html", "htm", "mobi", "azw", "azw3", "fb2", "fbz",
    "cbz",
];
const COVER_EXTENSIONS: &[&str] = &["img", "jpg", "jpeg", "png", "webp"];
const MAX_FUTURE_CLOCK_SKEW_MS: i64 = 24 * 60 * 60 * 1_000;

pub fn validate_entity_id(id: &str) -> AppResult<()> {
    if id.is_empty()
        || id.len() > 128
        || id == "."
        || id == ".."
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(AppError::Other("SYNC_ENTITY_ID_INVALID".to_string()));
    }
    Ok(())
}

pub fn validate_peer_device(device: &str) -> AppResult<()> {
    if uuid::Uuid::parse_str(device).is_ok() {
        return Ok(());
    }
    #[cfg(test)]
    if device.starts_with("dev-") || device.starts_with("peer-") || device == "self" {
        return Ok(());
    }
    Err(AppError::Other("SYNC_DEVICE_ID_INVALID".to_string()))
}

pub fn validate_event(event: &Event, expected_device: &str) -> AppResult<()> {
    validate_peer_device(expected_device)?;
    if event.device != expected_device {
        return Err(AppError::Other("SYNC_EVENT_DEVICE_MISMATCH".to_string()));
    }
    let ulid = event
        .id
        .parse::<ulid::Ulid>()
        .map_err(|_| AppError::Other("SYNC_EVENT_ENVELOPE_INVALID".to_string()))?;
    if event.v != EVENT_SCHEMA_VERSION {
        return Err(AppError::Other("SYNC_EVENT_ENVELOPE_INVALID".to_string()));
    }
    ensure_not_from_far_future(event.ts, "SYNC_EVENT_TIMESTAMP_INVALID")?;
    ensure_not_from_far_future(
        i64::try_from(ulid.timestamp_ms())
            .map_err(|_| AppError::Other("SYNC_EVENT_ENVELOPE_INVALID".to_string()))?,
        "SYNC_EVENT_ENVELOPE_INVALID",
    )?;
    match &event.body {
        EventBody::BookImport(payload) => {
            validate_entity_id(&payload.id)?;
            validate_book_path(&payload.file_path)?;
            if let Some(path) = payload.cover_path.as_deref() {
                validate_cover_path(path)?;
            }
            if let Some(path) = payload.source_file_path.as_deref() {
                validate_source_path(path)?;
            }
        }
        EventBody::BookDelete { id } => validate_entity_id(id)?,
        EventBody::BookMetadataSet { book, field, value } => {
            validate_entity_id(book)?;
            if field == "file_path" {
                validate_book_path(
                    value
                        .as_str()
                        .ok_or_else(|| AppError::Other("SYNC_BLOB_PATH_INVALID".to_string()))?,
                )?;
            } else if field == "cover_path" {
                match value.as_str() {
                    Some(path) => validate_cover_path(path)?,
                    None if value.is_null() => {}
                    None => return Err(AppError::Other("SYNC_BLOB_PATH_INVALID".to_string())),
                }
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn ensure_not_from_far_future(timestamp_ms: i64, code: &str) -> AppResult<()> {
    let latest = chrono::Utc::now()
        .timestamp_millis()
        .saturating_add(MAX_FUTURE_CLOCK_SKEW_MS);
    if timestamp_ms > latest {
        return Err(AppError::Other(code.to_string()));
    }
    Ok(())
}

fn validate_relative_blob_path(path: &str, root: &str, extensions: &[&str]) -> AppResult<()> {
    let path = Path::new(path);
    if path.is_absolute() {
        return Err(AppError::Other("SYNC_BLOB_PATH_INVALID".to_string()));
    }
    let components: Vec<_> = path.components().collect();
    if components.len() != 2
        || components[0] != Component::Normal(root.as_ref())
        || !matches!(components[1], Component::Normal(_))
    {
        return Err(AppError::Other("SYNC_BLOB_PATH_INVALID".to_string()));
    }
    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| AppError::Other("SYNC_BLOB_PATH_INVALID".to_string()))?;
    if filename.is_empty() || filename.starts_with('.') || filename.contains(['/', '\\']) {
        return Err(AppError::Other("SYNC_BLOB_PATH_INVALID".to_string()));
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| AppError::Other("SYNC_BLOB_PATH_INVALID".to_string()))?;
    if !extensions.contains(&extension.as_str()) {
        return Err(AppError::Other("SYNC_BLOB_PATH_INVALID".to_string()));
    }
    Ok(())
}

pub fn validate_book_path(path: &str) -> AppResult<()> {
    validate_relative_blob_path(path, "books", BOOK_EXTENSIONS)
}

pub fn validate_cover_path(path: &str) -> AppResult<()> {
    if path == "none" {
        return Ok(());
    }
    validate_relative_blob_path(path, "covers", COVER_EXTENSIONS)
}

pub fn validate_source_path(path: &str) -> AppResult<()> {
    validate_relative_blob_path(path, "sources", BOOK_EXTENSIONS)
}

pub fn resolve_book_path(data_dir: &Path, path: &str) -> AppResult<PathBuf> {
    validate_book_path(path)?;
    Ok(data_dir.join(path))
}

pub fn resolve_cover_path(data_dir: &Path, path: &str) -> AppResult<PathBuf> {
    validate_cover_path(path)?;
    if path == "none" {
        return Err(AppError::Other("SYNC_BLOB_PATH_INVALID".to_string()));
    }
    Ok(data_dir.join(path))
}

pub fn resolve_blob_path(data_dir: &Path, path: &str) -> AppResult<PathBuf> {
    if path.starts_with("books/") {
        resolve_book_path(data_dir, path)
    } else if path.starts_with("covers/") {
        resolve_cover_path(data_dir, path)
    } else if path.starts_with("sources/") {
        validate_source_path(path)?;
        Ok(data_dir.join(path))
    } else {
        Err(AppError::Other("SYNC_BLOB_PATH_INVALID".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_expected_blob_paths() {
        assert!(validate_book_path("books/example.epub").is_ok());
        assert!(validate_cover_path("covers/id.img").is_ok());
    }

    #[test]
    fn rejects_paths_outside_blob_roots() {
        for path in ["/tmp/book.epub", "books/../../tmp/a.epub", "other/a.epub"] {
            assert!(validate_book_path(path).is_err(), "accepted {path}");
        }
    }

    #[test]
    fn rejects_unsafe_entity_ids() {
        for id in ["", "..", "../../tmp", "a/b", "a\\b"] {
            assert!(validate_entity_id(id).is_err(), "accepted {id}");
        }
    }

    #[test]
    fn rejects_timestamps_far_in_the_future() {
        let future = chrono::Utc::now().timestamp_millis() + MAX_FUTURE_CLOCK_SKEW_MS + 1;
        assert!(ensure_not_from_far_future(future, "test").is_err());
    }
}
