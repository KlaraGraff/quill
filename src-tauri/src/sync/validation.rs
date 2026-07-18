use std::path::{Component, Path, PathBuf};

use crate::error::{AppError, AppResult};

use super::events::{
    is_supported_event_schema_version, lookup_occurrence_mark_id, normalize_learning_term,
    word_mark_exception_id, word_mark_rule_id, BookSummaryPayload, ChatMessagePayload, Event,
    EventBody, LookupOccurrenceMarkPayload, NotePayload, WordMarkExceptionPayload, WordMarkPayload,
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
const MAX_CHAT_MESSAGE_BYTES: usize = 128 * 1024;
const MAX_CHAT_METADATA_BYTES: usize = 256 * 1024;

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

pub fn validate_tombstone_entity(entity: &str) -> AppResult<()> {
    if matches!(
        entity,
        "book"
            | "highlight"
            | "bookmark"
            | "vocab"
            | "note"
            | "word_mark"
            | "word_mark_exception"
            | "lookup_occurrence_mark"
            | "collection"
            | "collection_book"
            | "chat"
            | "chat_message"
            | "translation"
    ) {
        return Ok(());
    }
    Err(AppError::Other(
        "SYNC_SNAPSHOT_TOMBSTONE_INVALID".to_string(),
    ))
}

pub fn validate_tombstone_id(entity: &str, id: &str) -> AppResult<()> {
    validate_tombstone_entity(entity)?;
    if entity == "collection_book" {
        let Some((collection_id, book_id)) = id.split_once(':') else {
            return Err(AppError::Other(
                "SYNC_SNAPSHOT_TOMBSTONE_INVALID".to_string(),
            ));
        };
        if book_id.contains(':') {
            return Err(AppError::Other(
                "SYNC_SNAPSHOT_TOMBSTONE_INVALID".to_string(),
            ));
        }
        validate_entity_id(collection_id)
            .map_err(|_| AppError::Other("SYNC_SNAPSHOT_TOMBSTONE_INVALID".to_string()))?;
        validate_entity_id(book_id)
            .map_err(|_| AppError::Other("SYNC_SNAPSHOT_TOMBSTONE_INVALID".to_string()))?;
        return Ok(());
    }
    validate_entity_id(id)
        .map_err(|_| AppError::Other("SYNC_SNAPSHOT_TOMBSTONE_INVALID".to_string()))
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

pub(crate) fn validate_word_mark_exception_fields(
    id: &str,
    rule_id: &str,
    book_id: &str,
    normalized_word: &str,
    location: &str,
) -> AppResult<()> {
    validate_entity_id(id)?;
    validate_entity_id(rule_id)?;
    validate_entity_id(book_id)?;
    if normalized_word.is_empty()
        || normalized_word.len() > MAX_LEARNING_TERM_BYTES
        || normalized_word.chars().any(char::is_control)
        || normalize_learning_term(normalized_word) != normalized_word
        || location.trim().is_empty()
        || location.len() > MAX_NOTE_LOCATION_BYTES
        || location.chars().any(char::is_control)
        || rule_id != word_mark_rule_id(book_id, normalized_word, "exact")
        || id != word_mark_exception_id(rule_id, location)
    {
        return Err(AppError::Other(
            "SYNC_WORD_MARK_EXCEPTION_INVALID".to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_word_mark_exception_payload(
    payload: &WordMarkExceptionPayload,
) -> AppResult<()> {
    validate_word_mark_exception_fields(
        &payload.id,
        &payload.rule_id,
        &payload.book_id,
        &payload.normalized_word,
        &payload.location,
    )
}

pub(crate) fn validate_lookup_occurrence_mark_fields(
    id: &str,
    book_id: &str,
    normalized_word: &str,
    display_word: &str,
    location: &str,
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
        || location.trim().is_empty()
        || location.len() > MAX_NOTE_LOCATION_BYTES
        || location.chars().any(char::is_control)
        || id != lookup_occurrence_mark_id(book_id, location)
    {
        return Err(AppError::Other(
            "SYNC_LOOKUP_OCCURRENCE_MARK_INVALID".to_string(),
        ));
    }
    Ok(())
}

fn validate_lookup_occurrence_mark_payload(payload: &LookupOccurrenceMarkPayload) -> AppResult<()> {
    validate_lookup_occurrence_mark_fields(
        &payload.id,
        &payload.book_id,
        &payload.normalized_word,
        &payload.display_word,
        &payload.location,
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
            validate_book_file_path(&payload.file_path)?;
            if let Some(path) = payload.cover_path.as_deref() {
                validate_cover_path(path)?;
            }
            if let Some(path) = payload.source_file_path.as_deref() {
                validate_book_file_path(path)?;
            }
        }
        EventBody::BookDelete { id } => validate_entity_id(id)?,
        EventBody::BookMetadataSet { book, field, value } => {
            validate_entity_id(book)?;
            if field == "file_path" {
                validate_book_file_path(
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
        EventBody::WordMarkExceptionSet(payload) => {
            if event.v < 3 {
                return Err(AppError::Other(
                    "SYNC_WORD_MARK_EXCEPTION_INVALID".to_string(),
                ));
            }
            validate_word_mark_exception_payload(payload)?;
            ensure_not_from_far_future(payload.created_at, "SYNC_WORD_MARK_EXCEPTION_INVALID")?;
        }
        EventBody::LookupOccurrenceMarkSet(payload) => {
            if event.v < 3 {
                return Err(AppError::Other(
                    "SYNC_LOOKUP_OCCURRENCE_MARK_INVALID".to_string(),
                ));
            }
            validate_lookup_occurrence_mark_payload(payload)?;
            ensure_not_from_far_future(payload.created_at, "SYNC_LOOKUP_OCCURRENCE_MARK_INVALID")?;
        }
        EventBody::BookSummaryUpsert(payload) => {
            if event.v < 4 {
                return Err(AppError::Other("SYNC_BOOK_SUMMARY_INVALID".to_string()));
            }
            if event.v < 5 && payload.user_edited {
                return Err(AppError::Other("SYNC_BOOK_SUMMARY_INVALID".to_string()));
            }
            validate_book_summary_payload(payload)?;
        }
        EventBody::ChatMessageReplace(payload) => {
            if event.v < 6 {
                return Err(AppError::Other(
                    "SYNC_CHAT_MESSAGE_REPLACE_INVALID".to_string(),
                ));
            }
            validate_chat_message_replace_payload(payload)?;
        }
        _ => {}
    }
    Ok(())
}

fn validate_chat_message_replace_payload(payload: &ChatMessagePayload) -> AppResult<()> {
    validate_entity_id(&payload.id)?;
    validate_entity_id(&payload.chat_id)?;
    if payload.role != "assistant"
        || payload.content.trim().is_empty()
        || payload.content.len() > MAX_CHAT_MESSAGE_BYTES
        || payload.context.is_some()
        || payload
            .metadata
            .as_ref()
            .is_some_and(|metadata| metadata.len() > MAX_CHAT_METADATA_BYTES)
    {
        return Err(AppError::Other(
            "SYNC_CHAT_MESSAGE_REPLACE_INVALID".to_string(),
        ));
    }
    Ok(())
}

fn validate_book_summary_payload(payload: &BookSummaryPayload) -> AppResult<()> {
    validate_entity_id(&payload.id)?;
    validate_entity_id(&payload.book_id)?;
    if !matches!(payload.scope.as_str(), "book" | "section")
        || (payload.scope == "book" && payload.section_index.is_some())
        || (payload.scope == "section" && payload.section_index.is_none())
        || payload.content.trim().is_empty()
        || payload.content.len() > 200_000
        || payload.language.is_empty()
        || payload.language.len() > 32
        || payload.source_sha256.is_empty()
    {
        return Err(AppError::Other("SYNC_BOOK_SUMMARY_INVALID".to_string()));
    }
    ensure_valid_sync_timestamp(payload.created_at, "SYNC_BOOK_SUMMARY_INVALID")?;
    ensure_valid_sync_timestamp(payload.updated_at, "SYNC_BOOK_SUMMARY_INVALID")
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

pub fn ensure_valid_sync_timestamp(timestamp_ms: i64, code: &str) -> AppResult<()> {
    if timestamp_ms < 0 {
        return Err(AppError::Other(code.to_string()));
    }
    ensure_not_from_far_future(timestamp_ms, code)
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

/// `file_path` and `source_file_path` name a book's blobs by role, not by
/// location: native imports keep one file under `books/` for both roles,
/// while text/converted imports keep the canonical upload under `sources/`
/// (`do_import_text` even points `file_path` there). Either root is valid
/// for either field.
pub fn validate_book_file_path(path: &str) -> AppResult<()> {
    if path.starts_with("sources/") {
        validate_source_path(path)
    } else {
        validate_book_path(path)
    }
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
    use crate::sync::events::{BookImportPayload, EVENT_SCHEMA_VERSION};
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
    fn book_file_paths_accept_both_books_and_sources_roots() {
        assert!(validate_book_file_path("books/example.epub").is_ok());
        assert!(validate_book_file_path("books/example.pdf").is_ok());
        assert!(validate_book_file_path("sources/example.txt").is_ok());
        assert!(validate_book_file_path("sources/example.mobi").is_ok());
        for path in [
            "/tmp/book.epub",
            "books/../../tmp/a.epub",
            "sources/../books/a.epub",
            "covers/a.img",
            "other/a.epub",
        ] {
            assert!(validate_book_file_path(path).is_err(), "accepted {path}");
        }
    }

    #[test]
    fn book_import_events_accept_native_and_text_import_shapes() {
        // Native EPUB/PDF imports reuse the render file for both roles
        // (import.rs `do_import_epub`/`do_import_pdf`); text imports point
        // both fields at `sources/` (`do_import_text`). Both shapes must
        // validate — regression for the contract mismatch that made replay
        // reject whole peer logs.
        let native = Event {
            id: "01HYZX0000000000000000EVT3".into(),
            ts: 1_714_770_000_000,
            device: "dev-A".into(),
            v: EVENT_SCHEMA_VERSION,
            body: EventBody::BookImport(BookImportPayload {
                id: "b1".into(),
                title: "Native".into(),
                author: "Author".into(),
                description: None,
                cover_path: None,
                file_path: "books/b1.epub".into(),
                format: "epub".into(),
                source_format: Some("epub".into()),
                render_format: Some("epub".into()),
                source_file_path: Some("books/b1.epub".into()),
                source_sha256: Some("ab".repeat(32)),
                conversion_version: 0,
                genre: None,
                pages: Some(100),
            }),
            extra: Map::new(),
        };
        assert!(validate_event(&native, "dev-A").is_ok());

        let EventBody::BookImport(native_payload) = &native.body else {
            unreachable!()
        };
        let text = Event {
            body: EventBody::BookImport(BookImportPayload {
                file_path: "sources/b1.txt".into(),
                format: "text".into(),
                source_format: Some("txt".into()),
                render_format: Some("text".into()),
                source_file_path: Some("sources/b1.txt".into()),
                ..native_payload.clone()
            }),
            ..native.clone()
        };
        assert!(validate_event(&text, "dev-A").is_ok());

        let escape = Event {
            body: EventBody::BookImport(BookImportPayload {
                source_file_path: Some("sources/../secrets.pdf".into()),
                ..native_payload.clone()
            }),
            ..native.clone()
        };
        assert!(validate_event(&escape, "dev-A").is_err());
    }

    #[test]
    fn rejects_unsafe_entity_ids() {
        for id in ["", "..", "../../tmp", "a/b", "a\\b"] {
            assert!(validate_entity_id(id).is_err(), "accepted {id}");
        }
    }

    #[test]
    fn validates_snapshot_tombstone_entities_and_ids() {
        for entity in [
            "book",
            "highlight",
            "bookmark",
            "vocab",
            "note",
            "word_mark",
            "word_mark_exception",
            "collection",
            "chat",
            "chat_message",
            "translation",
        ] {
            assert!(
                validate_tombstone_id(entity, "entity-1").is_ok(),
                "expected {entity} to be allowed"
            );
        }
        assert!(validate_tombstone_id("collection_book", "c1:b1").is_ok());
        assert!(validate_tombstone_entity("unknown").is_err());
        for (entity, id) in [
            ("book", ""),
            ("book", "../b1"),
            ("book", "b1:extra"),
            ("collection_book", "c1"),
            ("collection_book", ":b1"),
            ("collection_book", "c1:"),
            ("collection_book", "c1:b1:extra"),
        ] {
            assert!(
                validate_tombstone_id(entity, id).is_err(),
                "accepted invalid tombstone id {entity}:{id}"
            );
        }
    }

    #[test]
    fn validates_snapshot_tombstone_timestamp_bounds() {
        assert!(ensure_valid_sync_timestamp(0, "test").is_ok());
        assert!(ensure_valid_sync_timestamp(-1, "test").is_err());
        let future = chrono::Utc::now().timestamp_millis() + MAX_FUTURE_CLOCK_SKEW_MS + 1;
        assert!(ensure_valid_sync_timestamp(future, "test").is_err());
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

        let exception_id = word_mark_exception_id(&stable_id, "epubcfi(/6/4!)");
        assert!(validate_word_mark_exception_fields(
            &exception_id,
            &stable_id,
            "b1",
            "term",
            "epubcfi(/6/4!)",
        )
        .is_ok());
        assert!(validate_word_mark_exception_fields(
            &exception_id,
            &stable_id,
            "b1",
            "other-term",
            "epubcfi(/6/4!)",
        )
        .is_err());
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

    #[test]
    fn chat_message_replace_requires_v6_and_an_assistant_payload() {
        let event = Event {
            id: "01HYZX0000000000000000EVT2".into(),
            ts: 1_714_770_000_000,
            device: "dev-A".into(),
            v: 6,
            body: EventBody::ChatMessageReplace(ChatMessagePayload {
                id: "message-1".into(),
                chat_id: "chat-1".into(),
                role: "assistant".into(),
                content: "replacement".into(),
                context: None,
                metadata: Some("{}".into()),
            }),
            extra: Map::new(),
        };
        assert!(validate_event(&event, "dev-A").is_ok());
        assert!(validate_event(
            &Event {
                v: 5,
                ..event.clone()
            },
            "dev-A"
        )
        .is_err());
        let EventBody::ChatMessageReplace(mut invalid_payload) = event.body else {
            unreachable!()
        };
        invalid_payload.role = "user".into();
        assert!(validate_event(
            &Event {
                body: EventBody::ChatMessageReplace(invalid_payload),
                ..event
            },
            "dev-A"
        )
        .is_err());
    }
}
