use std::fs;
use std::io::Cursor;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use base64::Engine;
use image::ImageFormat;
use pdfium_render::prelude::*;
use pulldown_cmark::{
    Event as MarkdownEvent, Options as MarkdownOptions, Parser as MarkdownParser,
    TagEnd as MarkdownTagEnd,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use zip::read::ZipArchive;

use crate::db::Db;
use crate::epub;
use crate::error::{AppError, AppResult};
use crate::icloud;
use crate::pdfium;
use crate::sync::events::{BookImportPayload, EventBody, NotePayload};
use crate::sync::merge::{self, entity};
use crate::sync::writer::SyncWriter;
use crate::LocalDir;

mod convert_prepare;
mod format;
mod import;
mod mutate;
mod pdf;
mod query;
mod text_headings;
mod text_prepare;

#[doc(hidden)]
pub use import::__cmd__import_book_from_dialog;
pub use import::import_book_from_dialog;
// Preserve the historical commands::books::* crate API while implementations
// live in focused child modules.
pub(crate) use format::source_sha256;
#[allow(unused_imports)]
pub(crate) use import::{
    do_import_epub, do_import_from_path, do_import_pdf, do_import_text, import_external_paths,
};
#[doc(hidden)]
pub use mutate::{
    __cmd__delete_book, __cmd__mark_finished, __cmd__update_book_cover,
    __cmd__update_book_metadata, __cmd__update_book_pages, __cmd__update_book_status,
    __cmd__update_reading_progress,
};
pub use mutate::{
    delete_book, mark_finished, update_book_cover, update_book_metadata, update_book_pages,
    update_book_status, update_reading_progress,
};
#[allow(unused_imports)]
pub(crate) use mutate::{do_delete_book, do_delete_book_with_note_policy, do_update_book};
#[doc(hidden)]
pub use query::{
    __cmd__check_book_available, __cmd__get_book, __cmd__get_book_counts, __cmd__list_books,
};
pub use query::{check_book_available, get_book, get_book_counts, list_books};
#[allow(unused_imports)]
pub(crate) use query::{query_book, query_book_exists, query_books, query_books_lite};
pub(crate) use text_prepare::load_prepared_document_for_grounding;
#[doc(hidden)]
pub use text_prepare::{__cmd__get_text_book_document, __cmd__retry_text_book_preparation};
pub use text_prepare::{
    get_text_book_document, resume_interrupted_text_book_preparations, retry_text_book_preparation,
    schedule_pending_text_book_preparations, schedule_text_book_preparation,
};
#[doc(hidden)]
pub use convert_prepare::{__cmd__get_converted_book_path, __cmd__retry_book_conversion};
pub(crate) use convert_prepare::{
    conversion_backend_available, is_conversion_book, schedule_book_conversion,
};
pub use convert_prepare::{
    get_converted_book_path, resume_interrupted_book_conversions, retry_book_conversion,
    schedule_pending_book_conversions,
};

pub(super) const TEXT_DOCUMENT_VERSION: i32 = 3;
/// Bumped whenever the source-format → EPUB conversion output changes in a
/// way that invalidates previously converted artifacts, forcing a per-device
/// re-conversion (the converted EPUB is a local, non-synced derivative).
pub(super) const CONVERSION_VERSION: i32 = 1;
pub(super) const MAX_TEXT_IMPORT_BYTES: u64 = 25 * 1024 * 1024;
pub(super) const TXT_CHAPTER_TARGET_CHARS: usize = 24_000;
pub(super) const IMPORTABLE_BOOK_EXTENSIONS: &[&str] = &[
    "epub", "pdf", "txt", "md", "markdown", "html", "htm", "mobi", "azw", "azw3", "fb2", "fbz",
    "cbz",
];

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TextBookBlockKind {
    Heading,
    Paragraph,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TextBookSourceSpan {
    pub rendered_start: u64,
    pub source_start: u64,
    pub length: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TextBookBlock {
    pub kind: TextBookBlockKind,
    pub text: String,
    pub source_start: u64,
    pub source_end: u64,
    pub source_spans: Vec<TextBookSourceSpan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<u8>,
    /// True for semantic reading-unit headings (volume/book/part/chapter),
    /// independent of their nesting depth in the generated table of contents.
    #[serde(default)]
    pub starts_page: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TextBookChunk {
    pub blocks: Vec<TextBookBlock>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TextBookTocEntry {
    pub title: String,
    pub depth: u8,
    pub source_offset: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TextBookDocument {
    pub version: i32,
    pub source_sha256: Option<String>,
    pub coordinate_space: String,
    pub chunks: Vec<TextBookChunk>,
    pub toc: Vec<TextBookTocEntry>,
    // V1 locations used generated chunk and paragraph indexes. Keeping this
    // compact offset table lets existing progress, bookmarks, and highlights
    // survive a V2 re-parse without retaining the old rendered document.
    pub legacy_locations: Vec<Vec<u64>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Book {
    pub id: String,
    pub title: String,
    pub author: String,
    pub description: Option<String>,
    pub cover_path: Option<String>,
    pub file_path: String,
    pub format: String,
    #[serde(default)]
    pub source_format: Option<String>,
    #[serde(default)]
    pub render_format: Option<String>,
    #[serde(default)]
    pub source_file_path: Option<String>,
    #[serde(default)]
    pub source_sha256: Option<String>,
    #[serde(default)]
    pub conversion_version: i32,
    #[serde(default = "default_preparation_state")]
    pub preparation_state: String,
    #[serde(default)]
    pub preparation_error: Option<String>,
    pub genre: Option<String>,
    pub pages: Option<i32>,
    pub status: String,
    pub progress: i32,
    pub current_cfi: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Whether the book file is locally available (not an iCloud placeholder).
    #[serde(default = "default_true")]
    pub available: bool,
    /// Base64-encoded cover image bytes. Rendered as data URI on the frontend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_data: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct BookAvailability {
    pub status: String,
    pub available: bool,
}

fn default_true() -> bool {
    true
}

fn default_preparation_state() -> String {
    "ready".to_string()
}

#[derive(Debug, serde::Serialize)]
pub struct BookPage {
    pub books: Vec<Book>,
    pub next_cursor: Option<String>,
    pub total: usize,
}

#[derive(Debug, serde::Serialize)]
pub struct BookCounts {
    pub all: usize,
    pub reading: usize,
    pub finished: usize,
}

#[cfg(test)]
use format::*;
#[cfg(test)]
use import::{book_filename, slugify};
#[cfg(test)]
use pdf::*;
#[cfg(test)]
use text_headings::*;
#[cfg(test)]
use text_prepare::*;

#[cfg(test)]
mod tests;
