use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub v: u32,
    pub device: String,
    /// ULID of the last event included in `state`. For a from-log snapshot
    /// this is the highest event id from the source log; for migration
    /// snapshots it's a freshly-minted ULID.
    pub id: String,
    pub generated_at: i64,
    /// Compaction watermark — events with id `<= truncated_before` in the
    /// source log can safely be discarded after the snapshot lands. `None`
    /// for migration snapshots, which are written before any log exists.
    pub truncated_before: Option<String>,
    pub state: SnapshotState,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SnapshotState {
    #[serde(default)]
    pub books: BTreeMap<String, BookRow>,
    #[serde(default)]
    pub highlights: BTreeMap<String, HighlightRow>,
    #[serde(default)]
    pub bookmarks: BTreeMap<String, BookmarkRow>,
    #[serde(default)]
    pub vocab_words: BTreeMap<String, VocabRow>,
    #[serde(default)]
    pub notes: BTreeMap<String, NoteRow>,
    #[serde(default)]
    pub word_mark_rules: BTreeMap<String, WordMarkRow>,
    #[serde(default)]
    pub word_mark_exceptions: BTreeMap<String, WordMarkExceptionRow>,
    #[serde(default)]
    pub lookup_occurrence_marks: BTreeMap<String, LookupOccurrenceMarkRow>,
    #[serde(default)]
    pub book_summaries: BTreeMap<String, BookSummaryRow>,
    #[serde(default)]
    pub collections: BTreeMap<String, CollectionRow>,
    /// Keyed by `"<collection_id>:<book_id>"` — the same composite key the
    /// merge engine uses for tombstones.
    #[serde(default)]
    pub collection_books: BTreeMap<String, CollectionBookRow>,
    #[serde(default)]
    pub chats: BTreeMap<String, ChatRow>,
    #[serde(default)]
    pub chat_messages: BTreeMap<String, ChatMessageRow>,
    /// `entity` (the same string in `_tombstones.entity`) → list of ids.
    #[serde(default)]
    pub tombstones: BTreeMap<String, Vec<TombstoneRow>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BookRow {
    pub title: String,
    pub author: String,
    pub description: Option<String>,
    pub cover_path: Option<String>,
    pub file_path: String,
    pub genre: Option<String>,
    pub pages: Option<i64>,
    pub format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_sha256: Option<String>,
    #[serde(default)]
    pub conversion_version: i32,
    pub status: String,
    pub progress: i32,
    pub current_cfi: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub updated_by_device: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_data: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HighlightRow {
    pub book_id: String,
    pub cfi_range: String,
    pub color: String,
    pub note: Option<String>,
    pub text_content: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub updated_by_device: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BookmarkRow {
    pub book_id: String,
    pub cfi: String,
    pub label: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VocabRow {
    pub book_id: String,
    pub word: String,
    pub definition: String,
    pub context_sentence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_explanation: Option<String>,
    pub cfi: Option<String>,
    pub mastery: String,
    pub review_count: i64,
    pub next_review_at: Option<i64>,
    #[serde(default)]
    pub review_interval_days: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reviewed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_review_rating: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fsrs_stability: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fsrs_difficulty: Option<f64>,
    #[serde(default = "default_fsrs_version")]
    pub fsrs_version: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub updated_by_device: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoteRow {
    pub book_id: Option<String>,
    pub anchor_kind: String,
    pub normalized_word: Option<String>,
    pub scope: String,
    pub location: Option<String>,
    pub selected_text: Option<String>,
    pub content: String,
    pub content_format: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub updated_by_device: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WordMarkRow {
    pub book_id: String,
    pub normalized_word: String,
    pub display_word: String,
    pub match_mode: String,
    pub color: String,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub updated_by_device: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WordMarkExceptionRow {
    pub rule_id: String,
    pub book_id: String,
    pub normalized_word: String,
    pub location: String,
    pub excluded: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub updated_by_device: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LookupOccurrenceMarkRow {
    pub book_id: String,
    pub normalized_word: String,
    pub display_word: String,
    pub location: String,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub updated_by_device: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BookSummaryRow {
    pub book_id: String,
    pub scope: String,
    pub section_index: Option<i64>,
    pub section_title: Option<String>,
    pub content: String,
    pub language: String,
    pub model: Option<String>,
    pub source_sha256: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub user_edited: bool,
}

fn default_fsrs_version() -> i64 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CollectionRow {
    pub name: String,
    pub sort_order: i32,
    pub created_at: i64,
    pub updated_at: i64,
    pub updated_by_device: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CollectionBookRow {
    pub collection_id: String,
    pub book_id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub updated_by_device: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatRow {
    pub book_id: String,
    pub title: String,
    pub model: Option<String>,
    pub pinned: bool,
    pub metadata: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub updated_by_device: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessageRow {
    pub chat_id: String,
    pub role: String,
    pub content: String,
    pub context: Option<String>,
    pub metadata: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TombstoneRow {
    pub id: String,
    pub ts: i64,
}

/// What `compact_own_log` did. Surfaced so the replay tick can log
/// it and the "Compact log" button can show feedback.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CompactReport {
    /// Number of log events folded into the new snapshot. Zero when
    /// the log was already empty (no-op).
    pub events_folded: usize,
    /// True when a fresh snapshot file replaced the previous one.
    pub snapshot_written: bool,
    /// Bytes the log shrank by minus bytes the snapshot grew by. Can
    /// be negative on the first compaction (snapshot is brand new and
    /// larger than the log it replaces).
    pub bytes_freed: i64,
}

/// Outcome reported back to the replay engine after a peer snapshot is
/// processed. Mirrors the watermark advance written into `_replay_state`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// Snapshot id matches `_replay_state.last_snapshot_id` — no-op.
    AlreadyApplied,
    /// Snapshot id `<=` `_replay_state.last_event_id`; we've already seen
    /// every event this snapshot summarises individually. Watermarks are
    /// advanced to `last_snapshot_id = snapshot.id` so we don't re-parse it.
    HeaderOnly,
    /// Snapshot rows applied; `last_snapshot_id` set, `last_event_id` bumped
    /// to `MAX(prev, snapshot.id)`.
    Applied,
}
