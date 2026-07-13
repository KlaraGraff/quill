use std::path::{Component, Path, PathBuf};

use crate::error::{AppError, AppResult};

use super::events::{
    is_supported_event_schema_version, normalize_learning_term, word_mark_rule_id, Event,
    EventBody, NotePayload, WordMarkPayload,
};

const BOOK_EXTENSIONS: &[&str] = &[
    "epub", "pdf", "txt", "md", "markdown", "html", "htm", "mobi", "azw", "azw3", "fb2", "fbz",
    "cbz",
];
const COVER_EXTENSIONS: &[&str] = &["img", "jpg", "jpeg", "png", "webp"];
const MAX_FUTURE_CLOCK_SKEW_MS: i64 = 24 * 60 * 60 * 1_000;
const MAX_NOTE_CONTENT_BYTES: usize = 100_000;
const MAX_NOTE_SELECTED_TEXT_BYTES: usize = 100_000;
const MAX_NOTE_LOCATION_BYTES: usize = 16 * 1024;
const MAX_LEARNING_TERM_BYTES: usize = 256;
const MAX_WORD_MARK_DISPLAY_BYTES: usize = 1_024;
const MAX_WORD_MARK_COLOR_BYTES: usize = 64;

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

#[allow(clippy::too_many_arguments)]
pub(crate) fn validate_note_fields(
    id: &str,
    book_id: Option<&str>,
    anchor_kind: &str,
    normalized_word: Option<&str>,
    scope: &str,
    location: Option<&str>,
    selected_text: Option<&str>,
    content: &str,
    content_format: &str,
) -> AppResult<()> {
    validate_entity_id(id)?;
    if let Some(book_id) = book_id {
        validate_entity_id(book_id)?;
    }

    let normalized_word_is_valid = normalized_word.is_none_or(|word| {
        !word.is_empty()
            && word.len() <= MAX_LEARNING_TERM_BYTES
            && !word.chars().any(char::is_control)
            && normalize_learning_term(word) == word
    });
    if !matches!(anchor_kind, "word" | "selection")
        || !matches!(scope, "book" | "global" | "detached")
        || (scope == "book" && book_id.is_none())
        || (scope == "detached" && book_id.is_some())
        || (anchor_kind == "word" && normalized_word.is_none())
        || !normalized_word_is_valid
        || content.len() > MAX_NOTE_CONTENT_BYTES
        || selected_text.is_some_and(|text| text.len() > MAX_NOTE_SELECTED_TEXT_BYTES)
        || location.is_some_and(|value| value.len() > MAX_NOTE_LOCATION_BYTES)
        || content_format != "plain_text"
    {
        return Err(AppError::Other("SYNC_NOTE_INVALID".to_string()));
    }
    Ok(())
}

pub(crate) fn validate_note_payload(payload: &NotePayload) -> AppResult<()> {
    validate_note_fields(
        &payload.id,
        payload.book_id.as_deref(),
        &payload.anchor_kind,
        payload.normalized_word.as_deref(),
        &payload.scope,
        payload.location.as_deref(),
        payload.selected_text.as_deref(),
        &payload.content,
        &payload.content_format,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn validate_word_mark_fields(
    id: &str,
    book_id: &str,
    normalized_word: &str,
    display_word: &str,
    match_mode: &str,
    color: &str,
) -> AppResult<()> {
    validate_entity_id(id)?;
    validate_entity_id(book_id)?;
    if normalized_word.is_empty()
        || normalized_word.len() > MAX_LEARNING_TERM_BYTES
        || normalized_word.chars().any(char::is_control)
        || normalize_learning_term(normalized_word) != normalized_word
        || display_word.trim().is_empty()
        || display_word.len() > MAX_WORD_MARK_DISPLAY_BYTES
        || display_word.chars().any(char::is_control)
        || match_mode != "exact"
        || color.is_empty()
        || color.len() > MAX_WORD_MARK_COLOR_BYTES
        || color.chars().any(char::is_control)
        || id != word_mark_rule_id(book_id, normalized_word, match_mode)
    {
        return Err(AppError::Other("SYNC_WORD_MARK_INVALID".to_string()));
    }
    Ok(())
}

pub(crate) fn validate_word_mark_payload(payload: &WordMarkPayload) -> AppResult<()> {
    validate_word_mark_fields(
        &payload.id,
        &payload.book_id,
        &payload.normalized_word,
        &payload.display_word,
        &payload.match_mode,
        &payload.color,
    )
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
    if !is_supported_event_schema_version(event.v) {
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
        EventBody::NoteUpsert(payload) => {
            if event.v < 2 {
                return Err(AppError::Other("SYNC_NOTE_INVALID".to_string()));
            }
            validate_note_payload(payload)?;
            ensure_not_from_far_future(payload.created_at, "SYNC_NOTE_INVALID")?;
        }
        EventBody::NoteDelete { id } => {
            if event.v < 2 {
                return Err(AppError::Other("SYNC_NOTE_INVALID".to_string()));
            }
            validate_entity_id(id)?;
        }
        EventBody::WordMarkUpsert(payload) => {
            if event.v < 2 {
                return Err(AppError::Other("SYNC_WORD_MARK_INVALID".to_string()));
            }
            validate_word_mark_payload(payload)?;
            ensure_not_from_far_future(payload.created_at, "SYNC_WORD_MARK_INVALID")?;
        }
        EventBody::WordMarkDelete { id } => {
            if event.v < 2 {
                return Err(AppError::Other("SYNC_WORD_MARK_INVALID".to_string()));
            }
            validate_entity_id(id)?;
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
    use serde_json::Map;

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

    #[test]
    fn validates_learning_entity_invariants() {
        assert!(validate_note_fields(
            "note-1",
            None,
            "word",
            Some("term"),
            "book",
            None,
            Some("term"),
            "content",
            "plain_text",
        )
        .is_err());
        assert!(validate_note_fields(
            "note-detached",
            None,
            "selection",
            None,
            "detached",
            None,
            Some("quoted passage"),
            "content",
            "plain_text",
        )
        .is_ok());
        assert!(validate_note_fields(
            "note-1",
            Some("b1"),
            "word",
            None,
            "book",
            None,
            Some("term"),
            "content",
            "plain_text",
        )
        .is_err());

        let stable_id = word_mark_rule_id("b1", "term", "exact");
        assert!(
            validate_word_mark_fields(&stable_id, "b1", "term", "Term", "exact", "lookup").is_ok()
        );
        assert!(
            validate_word_mark_fields("random-id", "b1", "term", "Term", "exact", "lookup")
                .is_err()
        );
    }

    #[test]
    fn accepts_legacy_v1_events_but_reserves_learning_events_for_v2() {
        let legacy = Event {
            id: "01HYZX0000000000000000EVT1".into(),
            ts: 1_714_770_000_000,
            device: "dev-A".into(),
            v: 1,
            body: EventBody::BookDelete { id: "b1".into() },
            extra: Map::new(),
        };
        assert!(validate_event(&legacy, "dev-A").is_ok());

        let invalid_v1_learning = Event {
            body: EventBody::NoteUpsert(NotePayload {
                id: "note-1".into(),
                book_id: Some("b1".into()),
                anchor_kind: "word".into(),
                normalized_word: Some("term".into()),
                scope: "book".into(),
                location: None,
                selected_text: Some("term".into()),
                content: "content".into(),
                content_format: "plain_text".into(),
                created_at: 1_714_770_000_000,
            }),
            ..legacy
        };
        assert!(validate_event(&invalid_v1_learning, "dev-A").is_err());
    }
}
