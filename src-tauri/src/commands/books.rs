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
use zip::read::ZipArchive;

use crate::db::Db;
use crate::epub;
use crate::error::{AppError, AppResult};
use crate::icloud;
use crate::pdfium;
use crate::sync::events::{BookImportPayload, EventBody, NotePayload};
use crate::sync::writer::SyncWriter;
use crate::LocalDir;

fn cover_blob_to_data_uri(bytes: &[u8]) -> String {
    let mime = if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(b"\xFF\xD8\xFF") {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.len() > 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "image/png"
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!("data:{mime};base64,{b64}")
}

struct PdfExtracted {
    title: String,
    author: String,
    description: Option<String>,
    pages: i32,
    cover: Option<Vec<u8>>,
}

const TEXT_DOCUMENT_VERSION: i32 = 2;
const MAX_TEXT_IMPORT_BYTES: u64 = 25 * 1024 * 1024;
const TXT_CHAPTER_TARGET_CHARS: usize = 24_000;

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

#[derive(Debug, Serialize, Clone)]
struct TextPreparationChanged {
    book_id: String,
    state: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImportFormat {
    Epub,
    Pdf,
    Txt,
    Markdown,
    Html,
    Mobi,
    Fb2,
    Fbz,
    Cbz,
}

impl ImportFormat {
    fn source_name(self) -> &'static str {
        match self {
            Self::Epub => "epub",
            Self::Pdf => "pdf",
            Self::Txt => "txt",
            Self::Markdown => "markdown",
            Self::Html => "html",
            Self::Mobi => "mobi",
            Self::Fb2 => "fb2",
            Self::Fbz => "fbz",
            Self::Cbz => "cbz",
        }
    }

    fn native_extension(self, path: &Path) -> Option<String> {
        match self {
            Self::Mobi => path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| extension.to_ascii_lowercase()),
            Self::Fb2 => Some("fb2".to_string()),
            Self::Fbz => Some("fbz".to_string()),
            Self::Cbz => Some("cbz".to_string()),
            Self::Epub | Self::Pdf | Self::Txt | Self::Markdown | Self::Html => None,
        }
    }
}

fn unsupported_format() -> AppError {
    AppError::Other("UNSUPPORTED_FORMAT".to_string())
}

fn detect_import_format(path: &Path) -> AppResult<ImportFormat> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file()
        || metadata.len() > MAX_TEXT_IMPORT_BYTES
            && matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some(ext) if ext.eq_ignore_ascii_case("txt")
                    || ext.eq_ignore_ascii_case("md")
                    || ext.eq_ignore_ascii_case("markdown")
                    || ext.eq_ignore_ascii_case("html")
                    || ext.eq_ignore_ascii_case("htm")
            )
    {
        return Err(unsupported_format());
    }
    let mut file = fs::File::open(path)?;
    let mut header = [0_u8; 8 * 1024];
    let read = file.read(&mut header)?;
    let header = &header[..read];
    if header.starts_with(b"%PDF-") {
        return Ok(ImportFormat::Pdf);
    }
    if header.starts_with(b"PK\x03\x04") {
        let file = fs::File::open(path)?;
        let mut archive =
            ZipArchive::new(file).map_err(|_| AppError::Other("INVALID_CONTAINER".to_string()))?;
        let has_epub_mimetype = if let Ok(mut mimetype) = archive.by_name("mimetype") {
            let mut content = String::new();
            let _ = mimetype.read_to_string(&mut content);
            content.trim() == "application/epub+zip"
        } else {
            false
        };
        if has_epub_mimetype && archive.by_name("META-INF/container.xml").is_ok() {
            return Ok(ImportFormat::Epub);
        }
        let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
        let entries = archive
            .file_names()
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>();
        if ext.eq_ignore_ascii_case("fbz") && entries.iter().any(|name| name.ends_with(".fb2")) {
            return Ok(ImportFormat::Fbz);
        }
        if ext.eq_ignore_ascii_case("cbz")
            && entries.iter().any(|name| {
                name.ends_with(".jpg")
                    || name.ends_with(".jpeg")
                    || name.ends_with(".png")
                    || name.ends_with(".gif")
                    || name.ends_with(".webp")
            })
        {
            return Ok(ImportFormat::Cbz);
        }
        return Err(AppError::Other("INVALID_CONTAINER".to_string()));
    }

    let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    if matches!(ext.to_ascii_lowercase().as_str(), "mobi" | "azw" | "azw3")
        && header.len() >= 68
        && &header[60..68] == b"BOOKMOBI"
    {
        return Ok(ImportFormat::Mobi);
    }
    if ext.eq_ignore_ascii_case("fb2")
        && std::str::from_utf8(header)
            .ok()
            .is_some_and(|text| text.to_ascii_lowercase().contains("<fictionbook"))
    {
        return Ok(ImportFormat::Fb2);
    }
    let is_text_extension = || {
        ext.eq_ignore_ascii_case("txt")
            || ext.eq_ignore_ascii_case("md")
            || ext.eq_ignore_ascii_case("markdown")
            || ext.eq_ignore_ascii_case("html")
            || ext.eq_ignore_ascii_case("htm")
    };
    // UTF-16 text contains NUL bytes by design. Inspect the BOM before the
    // generic binary guard so decode_txt can handle UTF-16 LE/BE correctly.
    if encoding_rs::Encoding::for_bom(header).is_some() && is_text_extension() {
        return match ext.to_ascii_lowercase().as_str() {
            "txt" => Ok(ImportFormat::Txt),
            "md" | "markdown" => Ok(ImportFormat::Markdown),
            "html" | "htm" => Ok(ImportFormat::Html),
            _ => Err(unsupported_format()),
        };
    }
    if !header.contains(&0) {
        if ext.eq_ignore_ascii_case("txt") {
            return Ok(ImportFormat::Txt);
        }
        if ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown") {
            return Ok(ImportFormat::Markdown);
        }
        if ext.eq_ignore_ascii_case("html") || ext.eq_ignore_ascii_case("htm") {
            return Ok(ImportFormat::Html);
        }
    }
    Err(unsupported_format())
}

fn decode_txt(bytes: &[u8]) -> AppResult<String> {
    if bytes.len() as u64 > MAX_TEXT_IMPORT_BYTES {
        return Err(AppError::Other("TEXT_FILE_TOO_LARGE".to_string()));
    }
    if let Some((encoding, bom_len)) = encoding_rs::Encoding::for_bom(bytes) {
        let (text, _, had_errors) = encoding.decode(&bytes[bom_len..]);
        if had_errors {
            return Err(AppError::Other("ENCODING_UNCERTAIN".to_string()));
        }
        return Ok(text.into_owned());
    }
    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(bytes, true);
    let encoding = detector.guess(None, true);
    let (text, _, had_errors) = encoding.decode(bytes);
    if had_errors {
        return Err(AppError::Other("ENCODING_UNCERTAIN".to_string()));
    }
    Ok(text.into_owned())
}

fn source_sha256(path: &Path) -> AppResult<String> {
    let mut file = fs::File::open(path)?;
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

fn markdown_to_text(markdown: &str) -> String {
    let options = MarkdownOptions::ENABLE_STRIKETHROUGH
        | MarkdownOptions::ENABLE_TABLES
        | MarkdownOptions::ENABLE_TASKLISTS;
    let mut text = String::new();
    for event in MarkdownParser::new_ext(markdown, options) {
        match event {
            MarkdownEvent::Text(value) | MarkdownEvent::Code(value) => text.push_str(&value),
            MarkdownEvent::SoftBreak | MarkdownEvent::HardBreak => text.push('\n'),
            MarkdownEvent::End(
                MarkdownTagEnd::Paragraph
                | MarkdownTagEnd::Heading(_)
                | MarkdownTagEnd::BlockQuote(_)
                | MarkdownTagEnd::CodeBlock
                | MarkdownTagEnd::Item
                | MarkdownTagEnd::TableRow,
            ) => text.push('\n'),
            _ => {}
        }
    }
    text
}

fn html_to_text(html: &str) -> String {
    let cleaned = ammonia::clean(html);
    scraper::Html::parse_fragment(&cleaned)
        .root_element()
        .text()
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, Clone)]
struct TextLine {
    text: String,
    source_start: u64,
    source_end: u64,
    separator_before: Option<u64>,
    paragraph_break_before: bool,
    leading_whitespace: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum HeadingRank {
    Volume,
    Book,
    Division,
    Chapter,
    Section,
}

#[derive(Clone)]
enum ParsedHeading {
    Parent {
        title: String,
        rank: HeadingRank,
        child: Option<(String, HeadingRank)>,
    },
    Child {
        title: String,
        rank: HeadingRank,
    },
    TopLevel(String),
}

#[derive(Clone)]
struct HeadingContext {
    rank: HeadingRank,
    identity: String,
}

fn utf16_len(value: &str) -> u64 {
    value.encode_utf16().count() as u64
}

fn normalized_text_lines(text: &str) -> Vec<TextLine> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut result = Vec::new();
    let mut source_offset = 0_u64;
    let mut separator_before = None;
    let mut paragraph_break_before = true;
    for segment in normalized.split_inclusive('\n') {
        let raw = segment.strip_suffix('\n').unwrap_or(segment);
        let trimmed_start = raw.trim_start();
        let trimmed = trimmed_start.trim_end();
        if !trimmed.is_empty() {
            let leading_bytes = raw.len() - trimmed_start.len();
            let start = source_offset + utf16_len(&raw[..leading_bytes]);
            result.push(TextLine {
                text: trimmed.to_string(),
                source_start: start,
                source_end: start + utf16_len(trimmed),
                separator_before,
                paragraph_break_before,
                leading_whitespace: raw[..leading_bytes].chars().count(),
            });
            paragraph_break_before = false;
        } else if !result.is_empty() {
            paragraph_break_before = true;
        }
        separator_before = segment
            .ends_with('\n')
            .then(|| source_offset + utf16_len(raw));
        source_offset += utf16_len(segment);
    }
    result
}

fn legacy_chapter_title(line: &str) -> bool {
    line.len() < 100
        && (line.to_ascii_lowercase().starts_with("chapter ")
            || line.to_ascii_lowercase().starts_with("chapter\t")
            || (line.starts_with('第') && (line.contains('章') || line.contains('节'))))
}

fn legacy_text_locations(lines: &[TextLine]) -> Vec<Vec<u64>> {
    let mut chapters = Vec::new();
    let mut current = Vec::new();
    let mut current_chars = 0_usize;
    let flush = |chapters: &mut Vec<Vec<u64>>, current: &mut Vec<u64>| {
        if !current.is_empty() {
            chapters.push(std::mem::take(current));
        }
    };

    for line in lines {
        if legacy_chapter_title(&line.text) {
            flush(&mut chapters, &mut current);
            current_chars = 0;
            continue;
        }
        current.push(line.source_start);
        current_chars += line.text.len();
        if current_chars >= TXT_CHAPTER_TARGET_CHARS {
            flush(&mut chapters, &mut current);
            current_chars = 0;
        }
    }
    flush(&mut chapters, &mut current);
    if chapters.is_empty() {
        chapters.push(vec![0]);
    }
    chapters
}

fn trim_heading_separator(value: &str) -> &str {
    value.trim_matches(|character: char| {
        character.is_whitespace()
            || matches!(
                character,
                ':' | '.' | '-' | '\u{2013}' | '\u{2014}' | '\u{00b7}'
            )
    })
}

fn canonical_roman_number(value: &str) -> bool {
    let upper = value.to_ascii_uppercase();
    if upper.is_empty() || upper.len() > 15 {
        return false;
    }
    let mut total = 0_u32;
    let mut previous = 0_u32;
    for character in upper.chars().rev() {
        let current = match character {
            'I' => 1,
            'V' => 5,
            'X' => 10,
            'L' => 50,
            'C' => 100,
            'D' => 500,
            'M' => 1_000,
            _ => return false,
        };
        if current < previous {
            total = total.saturating_sub(current);
        } else {
            total += current;
            previous = current;
        }
    }
    if !(1..=3_999).contains(&total) {
        return false;
    }
    let mut remaining = total;
    let mut canonical = String::new();
    for (number, numeral) in [
        (1_000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ] {
        while remaining >= number {
            canonical.push_str(numeral);
            remaining -= number;
        }
    }
    canonical == upper
}

fn number_word_kind(value: &str) -> Option<u8> {
    match trim_heading_separator(value).to_ascii_uppercase().as_str() {
        "ZERO" | "TEN" | "ELEVEN" | "TWELVE" | "THIRTEEN" | "FOURTEEN" | "FIFTEEN" | "SIXTEEN"
        | "SEVENTEEN" | "EIGHTEEN" | "NINETEEN" => Some(1),
        "ONE" | "TWO" | "THREE" | "FOUR" | "FIVE" | "SIX" | "SEVEN" | "EIGHT" | "NINE" => Some(2),
        "TWENTY" | "THIRTY" | "FORTY" | "FIFTY" | "SIXTY" | "SEVENTY" | "EIGHTY" | "NINETY" => {
            Some(3)
        }
        "HUNDRED" => Some(4),
        _ => None,
    }
}

fn english_number_phrase_len(words: &[&str]) -> usize {
    let Some(first) = words.first() else {
        return 0;
    };
    let cleaned = trim_heading_separator(first);
    if cleaned.chars().all(|character| character.is_ascii_digit())
        || canonical_roman_number(cleaned)
    {
        return 1;
    }
    match number_word_kind(cleaned) {
        Some(2)
            if words
                .get(1)
                .is_some_and(|word| number_word_kind(word) == Some(4)) =>
        {
            let mut length = 2;
            if words
                .get(length)
                .is_some_and(|word| word.eq_ignore_ascii_case("and"))
            {
                length += 1;
            }
            match words.get(length).and_then(|word| number_word_kind(word)) {
                Some(3) => {
                    length += 1;
                    if words.get(length).and_then(|word| number_word_kind(word)) == Some(2) {
                        length += 1;
                    }
                }
                Some(1 | 2) => length += 1,
                _ => {}
            }
            length
        }
        Some(3) => 1 + usize::from(words.get(1).and_then(|word| number_word_kind(word)) == Some(2)),
        Some(1 | 2 | 4) => 1,
        _ => 0,
    }
}

fn is_heading_number(value: &str) -> bool {
    let value = trim_heading_separator(value);
    if value.is_empty() {
        return false;
    }
    if value.chars().all(|character| character.is_ascii_digit()) {
        return true;
    }
    if canonical_roman_number(value) {
        return true;
    }
    number_word_kind(value).is_some()
}

fn english_heading_rank(keyword: &str) -> Option<HeadingRank> {
    match trim_heading_separator(keyword)
        .to_ascii_uppercase()
        .as_str()
    {
        "VOLUME" => Some(HeadingRank::Volume),
        "BOOK" => Some(HeadingRank::Book),
        "PART" | "ACT" => Some(HeadingRank::Division),
        "CHAPTER" | "SCENE" => Some(HeadingRank::Chapter),
        "SECTION" => Some(HeadingRank::Section),
        _ => None,
    }
}

fn english_parent_heading(line: &str) -> Option<ParsedHeading> {
    let words = line.split_whitespace().collect::<Vec<_>>();
    if words.len() < 2 {
        return None;
    }
    let keyword = trim_heading_separator(words[0]).to_ascii_uppercase();
    let rank = english_heading_rank(&keyword)?;
    if rank > HeadingRank::Division {
        return None;
    }

    let number_length = english_number_phrase_len(&words[1..]);
    if number_length == 0 {
        return None;
    }
    let parent_end = 1 + number_length;

    let parent_title = words[..parent_end]
        .iter()
        .map(|word| trim_heading_separator(word))
        .collect::<Vec<_>>()
        .join(" ");
    let mut child_index = parent_end;
    while child_index < words.len() && trim_heading_separator(words[child_index]).is_empty() {
        child_index += 1;
    }
    let child = words.get(child_index).and_then(|first| {
        let first = trim_heading_separator(first);
        let child_rank = if is_heading_number(first) {
            Some(HeadingRank::Chapter)
        } else {
            english_heading_rank(first)
        }?;
        Some((
            trim_heading_separator(&words[child_index..].join(" ")).to_string(),
            child_rank,
        ))
    });

    Some(ParsedHeading::Parent {
        title: if child.is_some() {
            parent_title
        } else {
            line.to_string()
        },
        rank,
        child,
    })
}

fn english_child_heading(line: &str) -> Option<HeadingRank> {
    let words = line.split_whitespace().collect::<Vec<_>>();
    let rank = english_heading_rank(words.first()?)?;
    (english_number_phrase_len(&words[1..]) > 0).then_some(rank)
}

fn prefixed_chinese_heading_unit(line: &str) -> Option<char> {
    let rest = line.strip_prefix('第')?;
    let mut numeral_end = 0_usize;
    for (index, character) in rest.char_indices() {
        if character.is_ascii_digit()
            || "零〇一二三四五六七八九十百千万两壹贰叁肆伍陆柒捌玖拾佰仟".contains(character)
        {
            numeral_end = index + character.len_utf8();
        } else {
            break;
        }
    }
    if numeral_end == 0 {
        return None;
    }

    let mut suffix = rest[numeral_end..].chars();
    let unit = suffix.next()?;
    if !"章节回卷部篇集".contains(unit) {
        return None;
    }
    Some(unit)
}

fn chinese_heading(line: &str) -> Option<ParsedHeading> {
    if let Some(unit) = prefixed_chinese_heading_unit(line) {
        if matches!(unit, '章' | '节' | '回') {
            return Some(ParsedHeading::Child {
                title: line.to_string(),
                rank: if unit == '节' {
                    HeadingRank::Section
                } else {
                    HeadingRank::Chapter
                },
            });
        }
        if matches!(unit, '卷' | '部' | '篇' | '集') {
            return Some(ParsedHeading::Parent {
                title: line.to_string(),
                rank: if unit == '卷' {
                    HeadingRank::Volume
                } else {
                    HeadingRank::Division
                },
                child: None,
            });
        }
    }
    let compact = line.split_whitespace().next().unwrap_or(line);
    let mut characters = compact.chars();
    let first = characters.next();
    let second = characters.next();
    if matches!(first, Some('上' | '中' | '下')) && matches!(second, Some('卷' | '部' | '篇'))
        || matches!(first, Some('卷' | '部' | '篇'))
            && second.is_some_and(|character| {
                character.is_ascii_digit() || "零〇一二三四五六七八九十百千两".contains(character)
            })
    {
        return Some(ParsedHeading::Parent {
            title: line.to_string(),
            rank: if matches!(second, Some('卷')) || matches!(first, Some('卷')) {
                HeadingRank::Volume
            } else {
                HeadingRank::Division
            },
            child: None,
        });
    }
    None
}

fn title_case_heading_word(value: &str) -> bool {
    let value = trim_heading_separator(value);
    if value.is_empty()
        || value.chars().all(|character| character.is_ascii_digit())
        || canonical_roman_number(value) && value == value.to_ascii_uppercase()
    {
        return true;
    }
    if matches!(
        value.to_ascii_lowercase().as_str(),
        "a" | "an"
            | "and"
            | "as"
            | "at"
            | "by"
            | "for"
            | "from"
            | "in"
            | "of"
            | "on"
            | "or"
            | "the"
            | "to"
            | "with"
    ) {
        return true;
    }
    value.chars().next().is_some_and(char::is_uppercase)
}

fn english_heading_shape(line: &str) -> bool {
    let words = line.split_whitespace().collect::<Vec<_>>();
    let Some(keyword) = words.first() else {
        return false;
    };
    let keyword = trim_heading_separator(keyword).to_ascii_uppercase();
    let number_length = if matches!(keyword.as_str(), "PART" | "BOOK" | "VOLUME")
        || matches!(keyword.as_str(), "CHAPTER" | "SECTION" | "SCENE" | "ACT")
    {
        english_number_phrase_len(&words[1..])
    } else {
        0
    };
    if number_length == 0 {
        return false;
    }
    let suffix_start = 1 + number_length;
    if suffix_start == words.len()
        || line == line.to_ascii_uppercase()
        || line.contains(':')
        || line.contains(" - ")
        || line.contains(" \u{2013} ")
        || line.contains(" \u{2014} ")
    {
        return true;
    }
    let numbered_words_are_title_case = words[1..suffix_start].iter().all(|word| {
        let word = trim_heading_separator(word);
        word.chars().all(|character| character.is_ascii_digit())
            || canonical_roman_number(word) && word == word.to_ascii_uppercase()
            || word.chars().next().is_some_and(char::is_uppercase)
    });
    numbered_words_are_title_case
        && words[suffix_start..]
            .iter()
            .all(|word| title_case_heading_word(word))
}

fn parse_heading(
    line: &str,
    has_parent: bool,
    paragraph_break_before: bool,
) -> Option<ParsedHeading> {
    if line.chars().count() > 120 {
        return None;
    }
    if english_heading_shape(line) {
        if let Some(heading) = english_parent_heading(line) {
            return Some(heading);
        }
        if let Some(rank) = english_child_heading(line) {
            return Some(ParsedHeading::Child {
                title: line.to_string(),
                rank,
            });
        }
    }
    if let Some(heading) = chinese_heading(line) {
        return Some(heading);
    }

    let upper = line.to_ascii_uppercase();
    const TOP_LEVEL_HEADINGS: [&str; 10] = [
        "PROLOGUE",
        "EPILOGUE",
        "INTRODUCTION",
        "PREFACE",
        "FOREWORD",
        "AFTERWORD",
        "ACKNOWLEDGMENTS",
        "ACKNOWLEDGEMENTS",
        "CONCLUSION",
        "POSTSCRIPT",
    ];
    if TOP_LEVEL_HEADINGS.iter().any(|heading| {
        if upper == *heading {
            return true;
        }
        let Some(rest) = upper.strip_prefix(heading) else {
            return false;
        };
        rest.starts_with([':', '-', '\u{2013}', '\u{2014}'])
            || rest.starts_with(' ')
                && (line == upper
                    || line.contains(':')
                    || line.split_whitespace().skip(1).all(title_case_heading_word))
    }) {
        return Some(ParsedHeading::TopLevel(line.to_string()));
    }
    let bare_number = line.split_whitespace().count() == 1 && is_heading_number(line);
    let bare_number_shape = line.chars().all(|character| character.is_ascii_digit())
        || line == line.to_ascii_uppercase()
        || paragraph_break_before && number_word_kind(line).is_some();
    if bare_number && bare_number_shape && (has_parent || paragraph_break_before) {
        return Some(ParsedHeading::Child {
            title: line.to_string(),
            rank: HeadingRank::Chapter,
        });
    }
    None
}

fn has_sentence_ending(value: &str) -> bool {
    value
        .trim_end_matches(|character: char| {
            matches!(
                character,
                '"' | '\'' | ')' | ']' | '}' | '\u{2019}' | '\u{201d}'
            )
        })
        .ends_with([
            '.', '?', '!', '\u{2026}', '\u{3002}', '\u{ff1f}', '\u{ff01}',
        ])
}

fn is_list_item_line(value: &str) -> bool {
    let value = value.trim_start();
    if [
        "- ",
        "* ",
        "+ ",
        "\u{2022} ",
        "\u{2023} ",
        "\u{25e6} ",
        "[ ] ",
        "[x] ",
        "[X] ",
    ]
    .iter()
    .any(|prefix| value.starts_with(prefix))
    {
        return true;
    }

    let Some(marker_end) = value.find(char::is_whitespace) else {
        return false;
    };
    let marker = &value[..marker_end];
    let marker = marker
        .strip_suffix('.')
        .or_else(|| marker.strip_suffix(')'));
    let Some(marker) = marker else {
        return false;
    };
    let marker = marker.strip_prefix('(').unwrap_or(marker);
    if marker.is_empty() || marker.len() > 15 {
        return false;
    }
    marker.chars().all(|character| character.is_ascii_digit())
        || marker.len() == 1
            && marker
                .chars()
                .all(|character| character.is_ascii_alphabetic())
        || canonical_roman_number(marker)
}

fn starts_with_uppercase_letter(value: &str) -> bool {
    value
        .chars()
        .find(|character| character.is_alphabetic())
        .is_some_and(char::is_uppercase)
}

fn starts_with_lowercase_letter(value: &str) -> bool {
    value
        .chars()
        .find(|character| character.is_alphabetic())
        .is_some_and(char::is_lowercase)
}

fn hard_wrap_width(lines: &[TextLine], depths: &[Option<u8>]) -> Option<usize> {
    let content_lines = lines
        .iter()
        .zip(depths)
        .filter(|(_, depth)| depth.is_none())
        .map(|(line, _)| line)
        .collect::<Vec<_>>();
    let content = content_lines
        .iter()
        .map(|line| line.text.chars().count())
        .filter(|length| *length >= 8)
        .collect::<Vec<_>>();
    if content.len() < 12 {
        return None;
    }

    let list_items = content_lines
        .iter()
        .filter(|line| is_list_item_line(&line.text))
        .count();
    if list_items >= 3 && list_items * 100 >= content_lines.len() * 20 {
        return None;
    }
    let uppercase_starts = content_lines
        .iter()
        .filter(|line| starts_with_uppercase_letter(&line.text))
        .count();
    let soft_lines = content_lines
        .iter()
        .filter(|line| !has_sentence_ending(&line.text))
        .count();
    if content_lines.len() >= 8
        && uppercase_starts * 100 >= content_lines.len() * 75
        && soft_lines * 100 >= content_lines.len() * 35
    {
        return None;
    }

    let mut sorted = content.clone();
    sorted.sort_unstable();
    let median = sorted[sorted.len() / 2];
    if !(24..=120).contains(&median) {
        return None;
    }
    let regular = content
        .iter()
        .filter(|length| **length * 100 >= median * 65 && **length * 100 <= median * 140)
        .count();
    if regular * 100 < content.len() * 65 {
        return None;
    }

    let mut adjacent = 0_usize;
    let mut soft_endings = 0_usize;
    for (index, line) in lines.iter().enumerate().take(lines.len().saturating_sub(1)) {
        if depths[index].is_some()
            || depths[index + 1].is_some()
            || lines[index + 1].paragraph_break_before
        {
            continue;
        }
        adjacent += 1;
        if !has_sentence_ending(&line.text) {
            soft_endings += 1;
        }
    }
    (adjacent >= 8 && soft_endings * 100 >= adjacent * 35).then_some(median)
}

fn intentional_line_breaks(
    lines: &[TextLine],
    depths: &[Option<u8>],
    wrap_width: usize,
) -> Vec<bool> {
    let mut protected = vec![false; lines.len()];
    let mut start = 0_usize;
    while start < lines.len() {
        if depths[start].is_some() {
            start += 1;
            continue;
        }

        let mut end = start + 1;
        while end < lines.len() && depths[end].is_none() && !lines[end].paragraph_break_before {
            end += 1;
        }
        let run = &lines[start..end];
        if run.len() >= 3 {
            let soft_lines = run
                .iter()
                .filter(|line| !has_sentence_ending(&line.text))
                .count();
            let uppercase_starts = run
                .iter()
                .filter(|line| starts_with_uppercase_letter(&line.text))
                .count();
            let lowercase_starts = run
                .iter()
                .filter(|line| starts_with_lowercase_letter(&line.text))
                .count();
            let shorter_lines = run
                .iter()
                .filter(|line| line.text.chars().count() * 100 <= wrap_width * 85)
                .count();
            let verse_like = soft_lines * 100 >= run.len() * 75
                && (uppercase_starts * 100 >= run.len() * 75
                    || lowercase_starts == run.len()
                    || shorter_lines * 100 >= run.len() * 75);
            if verse_like {
                protected[start..end].fill(true);
            }
        }
        start = end;
    }
    protected
}

fn block_from_line(line: &TextLine, kind: TextBookBlockKind, depth: Option<u8>) -> TextBookBlock {
    TextBookBlock {
        kind,
        text: line.text.clone(),
        source_start: line.source_start,
        source_end: line.source_end,
        source_spans: vec![TextBookSourceSpan {
            rendered_start: 0,
            source_start: line.source_start,
            length: utf16_len(&line.text),
        }],
        depth,
    }
}

fn append_reflowed_line(block: &mut TextBookBlock, line: &TextLine) {
    let separator_rendered_start = utf16_len(&block.text);
    let previous = block.text.chars().next_back();
    let next = line.text.chars().next();
    let cjk = |character: char| {
        matches!(
            character as u32,
            0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xac00..=0xd7af
        )
    };
    let cjk_punctuation = |character: char| "，。！？；：、）》」』】…".contains(character);
    let insert_space = !matches!(previous, Some('-' | '\u{2010}' | '\u{2011}'))
        && !matches!((previous, next), (Some(left), Some(right)) if cjk(right) && (cjk(left) || cjk_punctuation(left)));
    if insert_space {
        block.text.push(' ');
        block.source_spans.push(TextBookSourceSpan {
            rendered_start: separator_rendered_start,
            source_start: line.separator_before.unwrap_or(block.source_end),
            length: 1,
        });
    }
    block.source_spans.push(TextBookSourceSpan {
        rendered_start: separator_rendered_start + u64::from(insert_space),
        source_start: line.source_start,
        length: utf16_len(&line.text),
    });
    block.text.push_str(&line.text);
    block.source_end = line.source_end;
}

fn chunk_text_blocks(blocks: Vec<TextBookBlock>) -> Vec<TextBookChunk> {
    let mut chunks = Vec::new();
    let mut current = Vec::new();
    let mut current_chars = 0_usize;
    for block in blocks {
        current_chars += block.text.len();
        current.push(block);
        if current_chars >= TXT_CHAPTER_TARGET_CHARS {
            chunks.push(TextBookChunk {
                blocks: std::mem::take(&mut current),
            });
            current_chars = 0;
        }
    }
    if !current.is_empty() {
        chunks.push(TextBookChunk { blocks: current });
    }
    chunks
}

fn enter_heading_context(
    stack: &mut Vec<HeadingContext>,
    rank: HeadingRank,
    title: &str,
) -> (u8, bool) {
    while stack.last().is_some_and(|context| context.rank > rank) {
        stack.pop();
    }

    let identity = trim_heading_separator(title).to_ascii_uppercase();
    if stack.last().is_some_and(|context| context.rank == rank) {
        let same = stack
            .last()
            .is_some_and(|context| context.identity == identity);
        if same {
            return ((stack.len() - 1) as u8, true);
        }
        stack.pop();
    }
    stack.push(HeadingContext { rank, identity });
    ((stack.len() - 1) as u8, false)
}

fn text_document_parts(
    text: &str,
    reflow_hard_wraps: bool,
) -> (Vec<TextBookChunk>, Vec<TextBookTocEntry>, Vec<Vec<u64>>) {
    let lines = normalized_text_lines(text);
    let legacy_locations = legacy_text_locations(&lines);
    let mut toc = Vec::new();
    let mut heading_stack = Vec::<HeadingContext>::new();
    let mut depths = Vec::with_capacity(lines.len());

    for line in &lines {
        let heading = parse_heading(
            &line.text,
            heading_stack
                .iter()
                .any(|context| context.rank < HeadingRank::Chapter),
            line.paragraph_break_before,
        );
        let depth = match heading {
            Some(ParsedHeading::Parent { title, rank, child }) => {
                let (parent_depth, repeated_parent) =
                    enter_heading_context(&mut heading_stack, rank, &title);
                if child.is_none() || !repeated_parent {
                    toc.push(TextBookTocEntry {
                        title: title.clone(),
                        depth: parent_depth,
                        source_offset: line.source_start,
                    });
                }
                if let Some((child, child_rank)) = child {
                    let (child_depth, _) =
                        enter_heading_context(&mut heading_stack, child_rank, &child);
                    toc.push(TextBookTocEntry {
                        title: child,
                        depth: child_depth,
                        source_offset: line.source_start,
                    });
                    Some(child_depth)
                } else {
                    Some(parent_depth)
                }
            }
            Some(ParsedHeading::Child { title, rank }) => {
                let (depth, _) = enter_heading_context(&mut heading_stack, rank, &title);
                toc.push(TextBookTocEntry {
                    title,
                    depth,
                    source_offset: line.source_start,
                });
                Some(depth)
            }
            Some(ParsedHeading::TopLevel(title)) => {
                heading_stack.clear();
                toc.push(TextBookTocEntry {
                    title,
                    depth: 0,
                    source_offset: line.source_start,
                });
                Some(0)
            }
            None => None,
        };
        depths.push(depth);
    }

    let wrap_width = reflow_hard_wraps
        .then(|| hard_wrap_width(&lines, &depths))
        .flatten();
    let mut blocks = Vec::new();
    let mut paragraph: Option<TextBookBlock> = None;
    let mut previous_line: Option<&TextLine> = None;
    let protected_line_breaks = wrap_width
        .map(|width| intentional_line_breaks(&lines, &depths, width))
        .unwrap_or_else(|| vec![false; lines.len()]);
    for (index, (line, depth)) in lines.iter().zip(depths).enumerate() {
        if let Some(depth) = depth {
            if let Some(paragraph) = paragraph.take() {
                blocks.push(paragraph);
            }
            blocks.push(block_from_line(
                line,
                TextBookBlockKind::Heading,
                Some(depth),
            ));
            previous_line = Some(line);
            continue;
        }

        let starts_paragraph = paragraph.is_none()
            || wrap_width.is_none()
            || line.paragraph_break_before
            || line.leading_whitespace > 0
            || previous_line.is_some_and(|previous| previous.leading_whitespace > 0)
            || is_list_item_line(&line.text)
            || previous_line.is_some_and(|previous| is_list_item_line(&previous.text))
            || protected_line_breaks[index]
            || index
                .checked_sub(1)
                .is_some_and(|previous| protected_line_breaks[previous])
            || previous_line.is_some_and(|previous| {
                previous.text.chars().count() * 100 < wrap_width.unwrap_or_default() * 60
            });
        if starts_paragraph {
            if let Some(paragraph) =
                paragraph.replace(block_from_line(line, TextBookBlockKind::Paragraph, None))
            {
                blocks.push(paragraph);
            }
        } else if let Some(paragraph) = &mut paragraph {
            append_reflowed_line(paragraph, line);
        }
        previous_line = Some(line);
    }
    if let Some(paragraph) = paragraph {
        blocks.push(paragraph);
    }
    let chunks = chunk_text_blocks(blocks);

    if let Some(first_block) = chunks.first().and_then(|chunk| chunk.blocks.first()) {
        if toc
            .first()
            .is_none_or(|entry| entry.source_offset > first_block.source_start)
        {
            toc.insert(
                0,
                TextBookTocEntry {
                    title: "Reading".to_string(),
                    depth: 0,
                    source_offset: first_block.source_start,
                },
            );
        }
    }
    (chunks, toc, legacy_locations)
}

fn prepare_text_document(
    source_path: &Path,
    source_format: &str,
    expected_source_sha256: Option<String>,
) -> AppResult<TextBookDocument> {
    let source_bytes = fs::read(source_path)?;
    let actual_source_sha256 = format!("{:x}", Sha256::digest(&source_bytes));
    if expected_source_sha256
        .as_ref()
        .is_some_and(|expected| expected != &actual_source_sha256)
    {
        return Err(AppError::Other("TEXT_SOURCE_HASH_MISMATCH".to_string()));
    }
    let decoded = decode_txt(&source_bytes)?;
    let text = match source_format {
        "markdown" => markdown_to_text(&decoded),
        "html" => html_to_text(&decoded),
        _ => decoded,
    };
    if text.trim().is_empty() {
        return Err(AppError::Other("EMPTY_BOOK".to_string()));
    }
    let (chunks, toc, legacy_locations) = text_document_parts(&text, source_format == "txt");
    Ok(TextBookDocument {
        version: TEXT_DOCUMENT_VERSION,
        source_sha256: Some(actual_source_sha256),
        coordinate_space: "normalized_utf16".to_string(),
        chunks,
        toc,
        legacy_locations,
    })
}

fn prepared_document_path(local_dir: &Path, book_id: &str) -> PathBuf {
    local_dir
        .join("prepared")
        .join(format!("{book_id}.v{TEXT_DOCUMENT_VERSION}.json"))
}

fn legacy_prepared_document_path(local_dir: &Path, book_id: &str) -> PathBuf {
    local_dir.join("prepared").join(format!("{book_id}.json"))
}

fn prepared_document_sidecar_path(path: &Path, suffix: &str) -> AppResult<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Other("PREPARATION_PATH_INVALID".to_string()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| AppError::Other("PREPARATION_PATH_INVALID".to_string()))?
        .to_string_lossy();
    Ok(parent.join(format!(".{file_name}.{suffix}")))
}

fn prepared_document_backup_path(path: &Path) -> AppResult<PathBuf> {
    prepared_document_sidecar_path(path, "backup")
}

fn prepared_document_temporary_path(path: &Path) -> AppResult<PathBuf> {
    prepared_document_sidecar_path(path, "tmp")
}

fn read_prepared_document(
    path: &Path,
    expected_source_sha256: Option<&str>,
) -> Option<TextBookDocument> {
    let document = fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<TextBookDocument>(&bytes).ok())?;
    (document.version == TEXT_DOCUMENT_VERSION
        && expected_source_sha256
            .is_none_or(|expected| document.source_sha256.as_deref() == Some(expected)))
    .then_some(document)
}

fn load_prepared_document(
    path: &Path,
    expected_source_sha256: Option<&str>,
) -> Option<TextBookDocument> {
    let backup_path = prepared_document_backup_path(path).ok()?;
    let temporary_path = prepared_document_temporary_path(path).ok()?;
    if let Some(document) = read_prepared_document(path, expected_source_sha256) {
        let _ = fs::remove_file(backup_path);
        let _ = fs::remove_file(temporary_path);
        return Some(document);
    }

    for recovery_path in [&temporary_path, &backup_path] {
        let Some(document) = read_prepared_document(recovery_path, expected_source_sha256) else {
            continue;
        };
        if path.exists() {
            let _ = fs::remove_file(path);
        }
        if fs::rename(recovery_path, path).is_ok() {
            let _ = fs::remove_file(&temporary_path);
            let _ = fs::remove_file(&backup_path);
            log::warn!(
                "recovered interrupted text cache replacement at {}",
                path.display()
            );
        }
        return Some(document);
    }
    None
}

fn text_toc_leaf_count(toc: &[TextBookTocEntry]) -> usize {
    toc.iter()
        .enumerate()
        .filter(|(index, entry)| {
            toc.get(index + 1)
                .is_none_or(|next| next.depth <= entry.depth)
        })
        .count()
}

fn write_prepared_document(path: &Path, document: &TextBookDocument) -> AppResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Other("PREPARATION_PATH_INVALID".to_string()))?;
    fs::create_dir_all(parent)?;
    let temporary_path = prepared_document_temporary_path(path)?;
    let backup_path = prepared_document_backup_path(path)?;
    let result = (|| -> AppResult<()> {
        let bytes = serde_json::to_vec(document)
            .map_err(|error| AppError::Other(format!("PREPARATION_SERIALIZE_FAILED: {error}")))?;
        match fs::remove_file(&temporary_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let mut temporary = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_path)?;
        temporary.write_all(&bytes)?;
        temporary.sync_all()?;
        drop(temporary);
        if path.exists() {
            match fs::remove_file(&backup_path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            fs::rename(path, &backup_path)?;
            if let Err(error) = fs::rename(&temporary_path, path) {
                let _ = fs::rename(&backup_path, path);
                return Err(error.into());
            }
        } else {
            fs::rename(&temporary_path, path)?;
        }
        let _ = fs::remove_file(&backup_path);
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

/// Single pdfium pass: load the doc once (streaming via `Read + Seek`
/// internally — bounded memory regardless of PDF size), pull
/// title/author/page-count from its metadata, and render page 1 to JPEG.
/// Avoids the multi-second `lopdf::Document::load_mem` parse on large
/// PDFs (lopdf walks the full document up front; pdfium is lazy).
///
/// Best-effort throughout: any failure falls back to the filename-derived
/// title + "Unknown Author" + 0 pages + no cover, so the import still
/// succeeds. The cover sub-step has its own fallback inside.
fn extract_pdf(path: &Path, fallback_title: &str) -> PdfExtracted {
    let fallback = || PdfExtracted {
        title: fallback_title.to_string(),
        author: "Unknown Author".into(),
        description: None,
        pages: 0,
        cover: None,
    };

    let Ok(pdfium) =
        pdfium::pdfium().inspect_err(|e| log::warn!("extract_pdf: pdfium unavailable: {e}"))
    else {
        return fallback();
    };
    let Ok(doc) = pdfium
        .load_pdf_from_file(path, None)
        .inspect_err(|e| log::warn!("extract_pdf: load_pdf_from_file({}): {e}", path.display()))
    else {
        return fallback();
    };

    // Read from the PDF Info dictionary via pdfium (`FPDF_GetMetaText`).
    // The old frontend pdf.js path also tried the XMP `/Metadata` stream
    // (dc:title / dc:creator / dc:description) before falling back to
    // Info, but in practice almost every PDF that has XMP also fills in
    // Info with matching values — the XMP-only population is dominated
    // by PDF/A archival and PDF/X print-prep files, which aren't typical
    // reading material. Falling back to filename for that tail keeps
    // this code path simple; revisit if a real-world bug surfaces.
    let metadata = doc.metadata();
    let info = |tag: PdfDocumentMetadataTagType| {
        metadata
            .get(tag)
            .map(|t| t.value().to_string())
            .filter(|s| !s.trim().is_empty())
    };

    let title =
        info(PdfDocumentMetadataTagType::Title).unwrap_or_else(|| fallback_title.to_string());
    let author =
        info(PdfDocumentMetadataTagType::Author).unwrap_or_else(|| "Unknown Author".into());
    let description = info(PdfDocumentMetadataTagType::Subject);
    let pages = doc.pages().len();
    let cover = render_first_page(&doc);

    PdfExtracted {
        title,
        author,
        description,
        pages,
        cover,
    }
}

/// Render page 1 of an already-loaded PDF to JPEG bytes.
/// Returns `None` on any rendering or encoding failure.
fn render_first_page(doc: &PdfDocument) -> Option<Vec<u8>> {
    let page = doc.pages().first().ok()?;
    let bitmap = page
        .render_with_config(
            &PdfRenderConfig::new()
                .set_target_width(600)
                .render_form_data(false)
                .render_annotations(false),
        )
        .ok()?;
    let mut buf = Vec::new();
    bitmap
        .as_image()
        .ok()?
        .into_rgb8()
        .write_to(&mut Cursor::new(&mut buf), ImageFormat::Jpeg)
        .ok()?;
    Some(buf)
}

/// Sanitize a book title into a safe filename slug.
/// Keeps alphanumeric, spaces (→ hyphens), and common punctuation, then truncates.
fn slugify(title: &str) -> String {
    let slug: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .to_lowercase();
    // Truncate to ~60 bytes at a word boundary, but never slice into
    // a multi-byte UTF-8 character. Naive `slug[..60]` panics on
    // non-ASCII titles (e.g. CJK) where byte 60 lands mid-codepoint —
    // which surfaces as `import_book` returning a command-runtime
    // panic the UI sees as "spinner forever". `floor_char_boundary`
    // walks back to the previous char start.
    if slug.len() <= 60 {
        slug
    } else {
        let cut = floor_char_boundary(&slug, 60);
        let head = &slug[..cut];
        head.rfind('-').map_or(head, |i| &head[..i]).to_string()
    }
}

/// Largest valid char-boundary `<= max_bytes`. Stable equivalent of
/// `str::floor_char_boundary` (which is still nightly-only as of
/// rustc 1.85). Walks at most 3 bytes back since UTF-8 codepoints are
/// at most 4 bytes wide.
fn floor_char_boundary(s: &str, max_bytes: usize) -> usize {
    let mut i = max_bytes.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Build a human-readable filename: `{slug}_{short-id}.{ext}`
fn book_filename(title: &str, book_id: &str, ext: &str) -> String {
    let slug = slugify(title);
    let short_id = &book_id[..8]; // first 8 chars of UUID
    if slug.is_empty() {
        format!("{}.{}", book_id, ext)
    } else {
        format!("{}_{}.{}", slug, short_id, ext)
    }
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

struct ImportFileCleanup {
    paths: Vec<PathBuf>,
    committed: bool,
}

impl ImportFileCleanup {
    fn new(paths: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            paths: paths.into_iter().collect(),
            committed: false,
        }
    }

    fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for ImportFileCleanup {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for path in &self.paths {
            match fs::remove_file(path) {
                Ok(()) => log::info!("import_book: rolled back file {}", path.display()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => log::warn!(
                    "import_book: failed to roll back file {}: {error}",
                    path.display()
                ),
            }
        }
    }
}

/// Resolve relative paths in a Book to absolute using data_dir,
/// and check whether the book file is locally available.
fn resolve_book_paths(book: &mut Book, db: &Db) -> AppResult<()> {
    book.file_path = db
        .resolve_path(&book.file_path)?
        .to_string_lossy()
        .to_string();
    if let Some(ref cover) = book.cover_path {
        if cover != "none" {
            book.cover_path = Some(db.resolve_path(cover)?.to_string_lossy().to_string());
        }
    }
    book.available = icloud::is_file_downloaded(std::path::Path::new(&book.file_path));
    Ok(())
}

#[derive(Clone, Debug)]
struct TextPreparationSource {
    file_path: Option<String>,
    format: Option<String>,
    sha256: Option<String>,
    conversion_version: i32,
}

fn transition_text_preparation_state(
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
        "UPDATE books
         SET preparation_state = ?1,
             preparation_error = ?2
         WHERE id = ?3 AND render_format = 'text' AND preparation_state = ?4",
        params![next_state, error, book_id, expected_state],
    )?;
    Ok(changed == 1)
}

fn text_preparation_job_is_current(
    conn: &rusqlite::Connection,
    book_id: &str,
    source: &TextPreparationSource,
) -> AppResult<bool> {
    let current = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM books
             WHERE id = ?1
               AND render_format = 'text'
               AND preparation_state = 'preparing'
               AND source_file_path IS ?2
               AND source_format IS ?3
               AND source_sha256 IS ?4
               AND COALESCE(conversion_version, 0) = ?5
         )",
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

fn update_current_text_preparation_job(
    db: &Db,
    book_id: &str,
    source: &TextPreparationSource,
    next_state: &str,
    error: Option<&str>,
    pages: Option<i32>,
) -> AppResult<bool> {
    let conn = db
        .conn
        .lock()
        .map_err(|lock_error| AppError::Other(lock_error.to_string()))?;
    let changed = conn.execute(
        "UPDATE books
         SET preparation_state = ?1,
             preparation_error = ?2,
             pages = COALESCE(?3, pages)
         WHERE id = ?4
           AND render_format = 'text'
           AND preparation_state = 'preparing'
           AND source_file_path IS ?5
           AND source_format IS ?6
           AND source_sha256 IS ?7
           AND COALESCE(conversion_version, 0) = ?8",
        params![
            next_state,
            error,
            pages,
            book_id,
            source.file_path.as_deref(),
            source.format.as_deref(),
            source.sha256.as_deref(),
            source.conversion_version,
        ],
    )?;
    Ok(changed == 1)
}

fn recover_current_text_preparation_job(
    db: &Db,
    book_id: &str,
    source: &TextPreparationSource,
    prepared_path: &Path,
) -> AppResult<bool> {
    let conn = db
        .conn
        .lock()
        .map_err(|lock_error| AppError::Other(lock_error.to_string()))?;
    if !text_preparation_job_is_current(&conn, book_id, source)? {
        return Ok(false);
    }
    let Some(document) = load_prepared_document(prepared_path, source.sha256.as_deref()) else {
        return Ok(false);
    };
    let pages = i32::try_from(text_toc_leaf_count(&document.toc).max(1))
        .map_err(|_| AppError::Other("TEXT_BOOK_TOO_LARGE".to_string()))?;
    let changed = conn.execute(
        "UPDATE books
         SET preparation_state = 'ready', preparation_error = NULL, pages = ?1
         WHERE id = ?2
           AND render_format = 'text'
           AND preparation_state = 'preparing'
           AND source_file_path IS ?3
           AND source_format IS ?4
           AND source_sha256 IS ?5
           AND COALESCE(conversion_version, 0) = ?6",
        params![
            pages,
            book_id,
            source.file_path.as_deref(),
            source.format.as_deref(),
            source.sha256.as_deref(),
            source.conversion_version,
        ],
    )?;
    Ok(changed == 1)
}

fn publish_current_text_preparation_job(
    db: &Db,
    book_id: &str,
    source: &TextPreparationSource,
    prepared_path: &Path,
    document: &TextBookDocument,
    pages: i32,
) -> AppResult<bool> {
    let conn = db
        .conn
        .lock()
        .map_err(|lock_error| AppError::Other(lock_error.to_string()))?;
    if !text_preparation_job_is_current(&conn, book_id, source)? {
        return Ok(false);
    }
    write_prepared_document(prepared_path, document)?;
    let changed = conn.execute(
        "UPDATE books
         SET preparation_state = 'ready', preparation_error = NULL, pages = ?1
         WHERE id = ?2
           AND render_format = 'text'
           AND preparation_state = 'preparing'
           AND source_file_path IS ?3
           AND source_format IS ?4
           AND source_sha256 IS ?5
           AND COALESCE(conversion_version, 0) = ?6",
        params![
            pages,
            book_id,
            source.file_path.as_deref(),
            source.format.as_deref(),
            source.sha256.as_deref(),
            source.conversion_version,
        ],
    )?;
    Ok(changed == 1)
}

fn emit_text_preparation_changed(app: &AppHandle, book_id: &str, state: &str) {
    let _ = app.emit(
        "book-preparation-changed",
        TextPreparationChanged {
            book_id: book_id.to_string(),
            state: state.to_string(),
        },
    );
}

fn run_text_preparation(app: &AppHandle, book_id: &str) -> AppResult<()> {
    crate::sync::validation::validate_entity_id(book_id)?;
    let db = app.state::<Db>();
    let local_dir = app.state::<LocalDir>();
    let source = {
        let conn = db
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let changed = conn.execute(
            "UPDATE books
             SET preparation_state = 'preparing', preparation_error = NULL
             WHERE id = ?1 AND render_format = 'text' AND preparation_state = 'pending'",
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
                Ok(TextPreparationSource {
                    file_path: row.get(0)?,
                    format: row.get(1)?,
                    sha256: row.get(2)?,
                    conversion_version: row.get(3)?,
                })
            },
        )?
    };
    emit_text_preparation_changed(app, book_id, "preparing");

    let prepared_path = prepared_document_path(&local_dir.0, book_id);
    if recover_current_text_preparation_job(&db, book_id, &source, &prepared_path)? {
        let _ = fs::remove_file(legacy_prepared_document_path(&local_dir.0, book_id));
        emit_text_preparation_changed(app, book_id, "ready");
        return Ok(());
    }
    let Some(source_file_path) = source.file_path.as_deref() else {
        if update_current_text_preparation_job(
            &db,
            book_id,
            &source,
            "failed",
            Some("TEXT_SOURCE_MISSING"),
            None,
        )? {
            emit_text_preparation_changed(app, book_id, "failed");
        }
        return Ok(());
    };
    let source_path = match db.resolve_path(source_file_path) {
        Ok(path) => path,
        Err(error) => {
            let message = error.to_string();
            if update_current_text_preparation_job(
                &db,
                book_id,
                &source,
                "failed",
                Some(&message),
                None,
            )? {
                emit_text_preparation_changed(app, book_id, "failed");
            }
            return Ok(());
        }
    };
    match icloud::file_availability(&source_path) {
        icloud::FileAvailability::Available => {}
        icloud::FileAvailability::ICloudPlaceholder => {
            icloud::trigger_download_file(&source_path);
            if update_current_text_preparation_job(&db, book_id, &source, "pending", None, None)? {
                emit_text_preparation_changed(app, book_id, "pending");
            }
            return Ok(());
        }
        icloud::FileAvailability::Missing => {
            if update_current_text_preparation_job(
                &db,
                book_id,
                &source,
                "failed",
                Some("TEXT_SOURCE_UNAVAILABLE"),
                None,
            )? {
                emit_text_preparation_changed(app, book_id, "failed");
            }
            return Ok(());
        }
    }

    let result = prepare_text_document(
        &source_path,
        source.format.as_deref().unwrap_or("txt"),
        source.sha256.clone(),
    )
    .and_then(|document| {
        let pages = i32::try_from(text_toc_leaf_count(&document.toc).max(1))
            .map_err(|_| AppError::Other("TEXT_BOOK_TOO_LARGE".to_string()))?;
        publish_current_text_preparation_job(
            &db,
            book_id,
            &source,
            &prepared_path,
            &document,
            pages,
        )
    });

    match result {
        Ok(true) => {
            let _ = fs::remove_file(legacy_prepared_document_path(&local_dir.0, book_id));
            emit_text_preparation_changed(app, book_id, "ready");
        }
        Ok(false) => {
            log::debug!("discarded stale text preparation task for {book_id}");
        }
        Err(error) => {
            let message = error.to_string();
            if update_current_text_preparation_job(
                &db,
                book_id,
                &source,
                "failed",
                Some(&message),
                None,
            )? {
                emit_text_preparation_changed(app, book_id, "failed");
            }
            log::warn!("text preparation failed for {book_id}: {message}");
        }
    }
    Ok(())
}

pub fn schedule_text_book_preparation(app: AppHandle, book_id: String) {
    let thread_name = format!("text-prep-{}", &book_id[..book_id.len().min(8)]);
    let _ = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            if let Err(error) = run_text_preparation(&app, &book_id) {
                log::warn!("text preparation task failed for {book_id}: {error}");
            }
        });
}

fn pending_text_book_ids(db: &Db, recover_interrupted: bool) -> AppResult<Vec<String>> {
    let conn = db
        .conn
        .lock()
        .map_err(|error| AppError::Other(error.to_string()))?;
    if recover_interrupted {
        conn.execute(
            "UPDATE books SET preparation_state = 'pending', preparation_error = NULL
             WHERE render_format = 'text' AND preparation_state = 'preparing'",
            [],
        )?;
    }
    let mut statement = conn.prepare(
        "SELECT id FROM books
         WHERE render_format = 'text' AND preparation_state = 'pending'
         ORDER BY id",
    )?;
    let book_ids = statement
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()
        .map_err(AppError::from)?;
    Ok(book_ids)
}

fn schedule_pending_text_book_preparations_inner(app: AppHandle, recover_interrupted: bool) {
    let db = app.state::<Db>();
    let pending = pending_text_book_ids(&db, recover_interrupted);
    match pending {
        Ok(book_ids) => {
            for book_id in book_ids {
                schedule_text_book_preparation(app.clone(), book_id);
            }
        }
        Err(error) => log::warn!("text preparation startup scan failed: {error}"),
    }
}

pub fn resume_interrupted_text_book_preparations(app: AppHandle) {
    schedule_pending_text_book_preparations_inner(app, true);
}

pub fn schedule_pending_text_book_preparations(app: AppHandle) {
    schedule_pending_text_book_preparations_inner(app, false);
}

pub(crate) fn do_import_epub(file_path: &str, db: &Db, sync: &SyncWriter) -> AppResult<Book> {
    let data_dir = db
        .data_dir
        .lock()
        .map_err(|e| AppError::Other(e.to_string()))?
        .clone();
    let books_dir = data_dir.join("books");

    let book_id = uuid::Uuid::new_v4().to_string();
    let src = std::path::Path::new(file_path);
    let source_sha256 = source_sha256(src)?;

    let metadata = epub::extract_metadata(src).inspect_err(|e| {
        log::error!("import_book: extract_metadata failed for {file_path}: {e}")
    })?;
    let pages = epub::count_chapters(src)
        .inspect_err(|e| log::error!("import_book: count_chapters failed for {file_path}: {e}"))?
        as i32;

    let filename = book_filename(&metadata.title, &book_id, "epub");
    let dest = books_dir.join(&filename);
    let cleanup = ImportFileCleanup::new([dest.clone()]);
    fs::copy(src, &dest)?;

    let now = chrono::Utc::now().timestamp_millis();
    let rel_file_path = format!("books/{}", filename);
    let cover_data_b64 = metadata.cover_data.as_deref().map(cover_blob_to_data_uri);

    let book = Book {
        id: book_id,
        title: metadata.title,
        author: metadata.author,
        description: metadata.description,
        cover_path: None,
        file_path: rel_file_path.clone(),
        format: "epub".to_string(),
        source_format: Some("epub".to_string()),
        render_format: Some("epub".to_string()),
        source_file_path: Some(rel_file_path.clone()),
        source_sha256: Some(source_sha256),
        conversion_version: 0,
        preparation_state: default_preparation_state(),
        preparation_error: None,
        genre: None,
        pages: Some(pages),
        status: "unread".to_string(),
        progress: 0,
        current_cfi: None,
        created_at: now,
        updated_at: now,
        available: true,
        cover_data: cover_data_b64,
    };

    do_insert_book(&book, metadata.cover_data.as_deref(), db, sync, now)?;
    cleanup.commit();

    log::info!(
        "import_book: complete id={} title={:?}",
        book.id,
        book.title
    );
    Ok(book)
}

pub(crate) fn do_import_text(
    file_path: &str,
    source_format: &str,
    db: &Db,
    sync: &SyncWriter,
) -> AppResult<Book> {
    let source = Path::new(file_path);
    let source_hash = source_sha256(source)?;
    let title = source
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Untitled")
        .trim();
    let title = if title.is_empty() { "Untitled" } else { title };
    let data_dir = db
        .data_dir
        .lock()
        .map_err(|e| AppError::Other(e.to_string()))?
        .clone();
    let sources_dir = data_dir.join("sources");
    fs::create_dir_all(&sources_dir)?;
    let book_id = uuid::Uuid::new_v4().to_string();
    let source_extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("txt")
        .to_ascii_lowercase();
    let source_filename = book_filename(title, &book_id, &source_extension);
    let source_path = sources_dir.join(&source_filename);
    let cleanup = ImportFileCleanup::new([source_path.clone()]);
    fs::copy(source, &source_path)?;
    let now = chrono::Utc::now().timestamp_millis();
    let book = Book {
        id: book_id,
        title: title.to_string(),
        author: "Unknown Author".to_string(),
        description: None,
        cover_path: None,
        file_path: format!("sources/{source_filename}"),
        format: "text".to_string(),
        source_format: Some(source_format.to_string()),
        render_format: Some("text".to_string()),
        source_file_path: Some(format!("sources/{source_filename}")),
        source_sha256: Some(source_hash),
        conversion_version: TEXT_DOCUMENT_VERSION,
        preparation_state: "pending".to_string(),
        preparation_error: None,
        genre: None,
        pages: None,
        status: "unread".to_string(),
        progress: 0,
        current_cfi: None,
        created_at: now,
        updated_at: now,
        available: true,
        cover_data: None,
    };
    do_insert_book(&book, None, db, sync, now)?;
    cleanup.commit();
    Ok(book)
}

fn do_import_native(
    file_path: &str,
    format: ImportFormat,
    db: &Db,
    sync: &SyncWriter,
) -> AppResult<Book> {
    let source = Path::new(file_path);
    let source_sha256 = source_sha256(source)?;
    let extension = format
        .native_extension(source)
        .ok_or_else(unsupported_format)?;
    // AZW/AZW3 share Foliate's MOBI parser but keep their source extension so
    // the stored filename and the File handed to the reader remain faithful.
    let source_format = if format == ImportFormat::Mobi {
        extension.clone()
    } else {
        format.source_name().to_string()
    };
    let title = source
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Untitled")
        .trim();
    let title = if title.is_empty() { "Untitled" } else { title };
    let data_dir = db
        .data_dir
        .lock()
        .map_err(|error| AppError::Other(error.to_string()))?
        .clone();
    let books_dir = data_dir.join("books");
    fs::create_dir_all(&books_dir)?;
    let book_id = uuid::Uuid::new_v4().to_string();
    let filename = book_filename(title, &book_id, &extension);
    let final_path = books_dir.join(&filename);
    let cleanup = ImportFileCleanup::new([final_path.clone()]);
    fs::copy(source, &final_path)?;
    let now = chrono::Utc::now().timestamp_millis();
    let book = Book {
        id: book_id,
        title: title.to_string(),
        author: "Unknown Author".to_string(),
        description: None,
        cover_path: None,
        file_path: format!("books/{filename}"),
        format: source_format.clone(),
        source_format: Some(source_format.clone()),
        render_format: Some(source_format),
        source_file_path: Some(format!("books/{filename}")),
        source_sha256: Some(source_sha256),
        conversion_version: 0,
        preparation_state: default_preparation_state(),
        preparation_error: None,
        genre: None,
        pages: None,
        status: "unread".to_string(),
        progress: 0,
        current_cfi: None,
        created_at: now,
        updated_at: now,
        available: true,
        cover_data: None,
    };
    do_insert_book(&book, None, db, sync, now)?;
    cleanup.commit();
    Ok(book)
}

pub(crate) fn do_import_pdf(file_path: &str, db: &Db, sync: &SyncWriter) -> AppResult<Book> {
    let data_dir = db
        .data_dir
        .lock()
        .map_err(|e| AppError::Other(e.to_string()))?
        .clone();
    let books_dir = data_dir.join("books");
    fs::create_dir_all(&books_dir)?;

    let book_id = uuid::Uuid::new_v4().to_string();
    let src = Path::new(file_path);
    let source_sha256 = source_sha256(src)?;

    let fallback_title = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled");
    // pdfium streams via Read+Seek so memory stays bounded regardless of
    // PDF size; fs::copy then streams the file to its destination. We
    // give up the "read once" optimization on purpose — for a 500MB
    // magazine the byte-buffer pattern would spike RAM, and the OS page
    // cache covers the cost of the second read (fs::copy) anyway.
    let t1 = std::time::Instant::now();
    let extracted = extract_pdf(src, fallback_title);
    let t_extract = t1.elapsed();

    let filename = book_filename(&extracted.title, &book_id, "pdf");
    let dest = books_dir.join(&filename);
    let cleanup = ImportFileCleanup::new([dest.clone()]);
    let t2 = std::time::Instant::now();
    fs::copy(src, &dest)?;
    let t_copy = t2.elapsed();

    let now = chrono::Utc::now().timestamp_millis();
    let rel_file_path = format!("books/{}", filename);
    let cover_data_b64 = extracted.cover.as_deref().map(cover_blob_to_data_uri);

    let book = Book {
        id: book_id,
        title: extracted.title,
        author: extracted.author,
        description: extracted.description,
        cover_path: None,
        file_path: rel_file_path.clone(),
        format: "pdf".to_string(),
        source_format: Some("pdf".to_string()),
        render_format: Some("pdf".to_string()),
        source_file_path: Some(rel_file_path.clone()),
        source_sha256: Some(source_sha256),
        conversion_version: 0,
        preparation_state: default_preparation_state(),
        preparation_error: None,
        genre: None,
        pages: Some(extracted.pages),
        status: "unread".to_string(),
        progress: 0,
        current_cfi: None,
        created_at: now,
        updated_at: now,
        available: true,
        cover_data: cover_data_b64,
    };

    do_insert_book(&book, extracted.cover.as_deref(), db, sync, now)?;
    cleanup.commit();

    log::info!(
        "import_book: complete id={} title={:?} format=pdf cover={} | extract={:?} copy={:?}",
        book.id,
        book.title,
        extracted.cover.is_some(),
        t_extract,
        t_copy,
    );
    Ok(book)
}

fn do_insert_book(
    book: &Book,
    cover_bytes: Option<&[u8]>,
    db: &Db,
    sync: &SyncWriter,
    now: i64,
) -> AppResult<()> {
    let device = sync.self_device().to_string();
    sync.with_tx(db, now, |tx, events| {
        tx.execute(
            "INSERT INTO books (id, title, author, description, cover_path, file_path, format, source_format, render_format, source_file_path, source_sha256, conversion_version, preparation_state, preparation_error, genre, pages, status, progress, current_cfi, created_at, updated_at, updated_by_device, cover_data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
            params![
                book.id,
                book.title,
                book.author,
                book.description,
                book.cover_path,
                book.file_path,
                book.format,
                book.source_format.as_deref().unwrap_or(&book.format),
                book.render_format.as_deref().unwrap_or(&book.format),
                book.source_file_path,
                book.source_sha256,
                book.conversion_version,
                book.preparation_state,
                book.preparation_error,
                book.genre,
                book.pages,
                book.status,
                book.progress,
                book.current_cfi,
                book.created_at,
                book.updated_at,
                device,
                cover_bytes,
            ],
        )?;
        events.push(EventBody::BookImport(BookImportPayload {
            id: book.id.clone(),
            title: book.title.clone(),
            author: book.author.clone(),
            description: book.description.clone(),
            cover_path: book.cover_path.clone(),
            file_path: book.file_path.clone(),
            format: book.format.clone(),
            source_format: book.source_format.clone(),
            render_format: book.render_format.clone(),
            source_file_path: book.source_file_path.clone(),
            source_sha256: book.source_sha256.clone(),
            conversion_version: book.conversion_version,
            genre: book.genre.clone(),
            pages: book.pages,
        }));
        Ok(())
    })?;
    if let Some(bytes) = cover_bytes {
        sync.queue_cover_write(db, &book.id, bytes);
    }
    Ok(())
}

#[tauri::command]
pub async fn import_book(
    file_path: String,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
    app: AppHandle,
) -> AppResult<Book> {
    let mut book = do_import_from_path(&file_path, &db, &sync)?;
    if book.render_format.as_deref() == Some("text") {
        schedule_text_book_preparation(app, book.id.clone());
    }
    resolve_book_paths(&mut book, &db)?;
    Ok(book)
}

pub(crate) fn do_import_from_path(file_path: &str, db: &Db, sync: &SyncWriter) -> AppResult<Book> {
    let _mutation = sync.mutation_guard()?;
    let format = detect_import_format(Path::new(&file_path))?;
    log::info!(
        "import_book: start file={file_path} format={}",
        format.source_name()
    );
    match format {
        ImportFormat::Pdf => do_import_pdf(file_path, db, sync),
        ImportFormat::Epub => do_import_epub(file_path, db, sync),
        ImportFormat::Txt => do_import_text(file_path, "txt", db, sync),
        ImportFormat::Markdown => do_import_text(file_path, "markdown", db, sync),
        ImportFormat::Html => do_import_text(file_path, "html", db, sync),
        ImportFormat::Mobi | ImportFormat::Fb2 | ImportFormat::Fbz | ImportFormat::Cbz => {
            do_import_native(file_path, format, db, sync)
        }
    }
}

/// Shared query helper. Returns books with the **relative** `file_path`
/// and `cover_path` as stored in SQLite (`books/<slug>.epub`,
/// `covers/<id>.jpg`). The Tauri `list_books` wrapper resolves these to
/// absolute paths for the frontend; the MCP `list_books` tool returns
/// them as-is so the response doesn't leak this user's home directory
/// layout to AI clients.
/// Paginated response for `list_books`.
#[derive(Debug, serde::Serialize)]
pub struct BookPage {
    pub books: Vec<Book>,
    pub next_cursor: Option<String>,
    pub total: usize,
}

pub(crate) fn query_books(
    db: &Db,
    filter: Option<&str>,
    search: Option<&str>,
    collection_id: Option<&str>,
    cursor: Option<&str>,
    limit: usize,
) -> AppResult<BookPage> {
    let conn = db.reader();

    let use_collection = collection_id.is_some();
    let from_clause = if use_collection {
        "books INNER JOIN collection_books cb ON cb.book_id = books.id"
    } else {
        "books"
    };

    let mut conditions: Vec<String> = Vec::new();
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(cid) = collection_id {
        conditions.push("cb.collection_id = ?".to_string());
        param_values.push(Box::new(cid.to_string()));
    }

    if let Some(f) = filter {
        match f {
            "reading" | "finished" | "unread" => {
                conditions.push("books.status = ?".to_string());
                param_values.push(Box::new(f.to_string()));
            }
            "all" => {}
            genre => {
                conditions.push("books.genre = ?".to_string());
                param_values.push(Box::new(genre.to_string()));
            }
        }
    }

    if let Some(q) = search {
        if !q.is_empty() {
            conditions
                .push("(LOWER(books.title) LIKE ? OR LOWER(books.author) LIKE ?)".to_string());
            let pattern = format!("%{}%", q.to_lowercase());
            param_values.push(Box::new(pattern.clone()));
            param_values.push(Box::new(pattern));
        }
    }

    // Cursor: "updated_at:id" — books older than cursor position.
    if let Some(c) = cursor {
        if let Some((ts_str, cid)) = c.split_once(':') {
            if let Ok(ts) = ts_str.parse::<i64>() {
                conditions.push(
                    "(books.updated_at < ? OR (books.updated_at = ? AND books.id > ?))".to_string(),
                );
                param_values.push(Box::new(ts));
                param_values.push(Box::new(ts));
                param_values.push(Box::new(cid.to_string()));
            }
        }
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    // Count conditions = same as main conditions but without cursor.
    let count_where = {
        let mut cc: Vec<String> = Vec::new();
        if let Some(cid) = collection_id {
            cc.push("cb.collection_id = ?".to_string());
            let _ = cid;
        }
        if let Some(f) = filter {
            match f {
                "reading" | "finished" | "unread" => cc.push("books.status = ?".to_string()),
                "all" => {}
                _ => cc.push("books.genre = ?".to_string()),
            }
        }
        if search.is_some_and(|q| !q.is_empty()) {
            cc.push("(LOWER(books.title) LIKE ? OR LOWER(books.author) LIKE ?)".to_string());
        }
        if cc.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", cc.join(" AND "))
        }
    };
    let count_sql = format!("SELECT COUNT(*) FROM {from_clause}{count_where}");
    let mut count_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(cid) = collection_id {
        count_params.push(Box::new(cid.to_string()));
    }
    if let Some(f) = filter {
        match f {
            "reading" | "finished" | "unread" | "all" => {
                if f != "all" {
                    count_params.push(Box::new(f.to_string()));
                }
            }
            _ => {
                count_params.push(Box::new(f.to_string()));
            }
        }
    }
    if let Some(q) = search {
        if !q.is_empty() {
            let pattern = format!("%{}%", q.to_lowercase());
            count_params.push(Box::new(pattern.clone()));
            count_params.push(Box::new(pattern));
        }
    }
    let count_refs: Vec<&dyn rusqlite::types::ToSql> =
        count_params.iter().map(|p| p.as_ref()).collect();
    let total: usize = conn.query_row(&count_sql, count_refs.as_slice(), |r| r.get(0))?;

    // Main query with cursor + limit.
    let sql = format!(
        "SELECT books.id, books.title, books.author, books.description, books.cover_path, books.file_path, books.format, books.source_format, books.render_format, books.source_file_path, books.source_sha256, books.conversion_version, books.genre, books.pages, books.status, books.progress, books.current_cfi, books.created_at, books.updated_at, books.cover_data, books.preparation_state, books.preparation_error FROM {from_clause}{where_clause} ORDER BY books.updated_at DESC, books.id ASC LIMIT ?",
    );
    param_values.push(Box::new((limit + 1) as i64));

    let params_refs: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();

    let mut stmt = conn.prepare(&sql)?;
    let mut books: Vec<Book> = stmt
        .query_map(params_refs.as_slice(), |row| {
            let cover_blob: Option<Vec<u8>> = row.get(19)?;
            Ok(Book {
                id: row.get(0)?,
                title: row.get(1)?,
                author: row.get(2)?,
                description: row.get(3)?,
                cover_path: row.get(4)?,
                file_path: row.get(5)?,
                format: row.get(6)?,
                source_format: row.get(7)?,
                render_format: row.get(8)?,
                source_file_path: row.get(9)?,
                source_sha256: row.get(10)?,
                conversion_version: row.get::<_, Option<i32>>(11)?.unwrap_or(0),
                preparation_state: row.get(20)?,
                preparation_error: row.get(21)?,
                genre: row.get(12)?,
                pages: row.get(13)?,
                status: row.get(14)?,
                progress: row.get(15)?,
                current_cfi: row.get(16)?,
                created_at: row.get(17)?,
                updated_at: row.get(18)?,
                available: true,
                cover_data: cover_blob
                    .filter(|b| !b.is_empty())
                    .map(|b| cover_blob_to_data_uri(&b)),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let next_cursor = if books.len() > limit {
        books.truncate(limit);
        let last = &books[limit - 1];
        Some(format!("{}:{}", last.updated_at, last.id))
    } else {
        None
    };

    Ok(BookPage {
        books,
        next_cursor,
        total,
    })
}

/// Shared query helper for the single-book lookup. Same relative-path
/// guarantee as `query_books`.
pub(crate) fn query_book(db: &Db, id: &str) -> AppResult<Book> {
    let conn = db.reader();
    let book = conn.query_row(
        "SELECT id, title, author, description, cover_path, file_path, format, source_format, render_format, source_file_path, source_sha256, conversion_version, genre, pages, status, progress, current_cfi, created_at, updated_at, cover_data, preparation_state, preparation_error FROM books WHERE id = ?1",
        params![id],
        |row| {
            let cover_blob: Option<Vec<u8>> = row.get(19)?;
            Ok(Book {
                id: row.get(0)?,
                title: row.get(1)?,
                author: row.get(2)?,
                description: row.get(3)?,
                cover_path: row.get(4)?,
                file_path: row.get(5)?,
                format: row.get(6)?,
                source_format: row.get(7)?,
                render_format: row.get(8)?,
                source_file_path: row.get(9)?,
                source_sha256: row.get(10)?,
                conversion_version: row.get::<_, Option<i32>>(11)?.unwrap_or(0),
                preparation_state: row.get(20)?,
                preparation_error: row.get(21)?,
                genre: row.get(12)?,
                pages: row.get(13)?,
                status: row.get(14)?,
                progress: row.get(15)?,
                current_cfi: row.get(16)?,
                created_at: row.get(17)?,
                updated_at: row.get(18)?,
                available: true,
                cover_data: cover_blob.filter(|b| !b.is_empty()).map(|b| cover_blob_to_data_uri(&b)),
            })
        },
    )?;
    Ok(book)
}

/// Lightweight book query for MCP — computes `has_cover` from the BLOB
/// without actually loading/encoding cover bytes. Prevents hundreds of
/// MB of wasted DB reads + base64 allocations when MCP lists 1000 books.
pub(crate) fn query_books_lite(
    db: &Db,
    filter: Option<&str>,
    search: Option<&str>,
    limit: usize,
) -> AppResult<Vec<Book>> {
    let conn = db.reader();
    let mut conditions: Vec<String> = Vec::new();
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if let Some(f) = filter {
        match f {
            "reading" | "finished" | "unread" => {
                conditions.push("status = ?".to_string());
                param_values.push(Box::new(f.to_string()));
            }
            "all" => {}
            genre => {
                conditions.push("genre = ?".to_string());
                param_values.push(Box::new(genre.to_string()));
            }
        }
    }
    if let Some(q) = search {
        if !q.is_empty() {
            conditions.push("(LOWER(title) LIKE ? OR LOWER(author) LIKE ?)".to_string());
            let pattern = format!("%{}%", q.to_lowercase());
            param_values.push(Box::new(pattern.clone()));
            param_values.push(Box::new(pattern));
        }
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", conditions.join(" AND "))
    };

    let sql = format!(
        "SELECT id, title, author, description, cover_path, file_path, format, source_format, render_format, source_file_path, source_sha256, conversion_version, genre, pages, status, progress, current_cfi, created_at, updated_at, (cover_data IS NOT NULL AND LENGTH(cover_data) > 0) AS has_cover, preparation_state, preparation_error FROM books{where_clause} ORDER BY updated_at DESC LIMIT ?",
    );
    param_values.push(Box::new(limit as i64));
    let params_refs: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(|p| p.as_ref()).collect();

    let mut stmt = conn.prepare(&sql)?;
    let books = stmt
        .query_map(params_refs.as_slice(), |row| {
            let has_cover: bool = row.get(19)?;
            Ok(Book {
                id: row.get(0)?,
                title: row.get(1)?,
                author: row.get(2)?,
                description: row.get(3)?,
                cover_path: row.get(4)?,
                file_path: row.get(5)?,
                format: row.get(6)?,
                source_format: row.get(7)?,
                render_format: row.get(8)?,
                source_file_path: row.get(9)?,
                source_sha256: row.get(10)?,
                conversion_version: row.get::<_, Option<i32>>(11)?.unwrap_or(0),
                preparation_state: row.get(20)?,
                preparation_error: row.get(21)?,
                genre: row.get(12)?,
                pages: row.get(13)?,
                status: row.get(14)?,
                progress: row.get(15)?,
                current_cfi: row.get(16)?,
                created_at: row.get(17)?,
                updated_at: row.get(18)?,
                available: true,
                cover_data: if has_cover {
                    Some("has_cover".to_string())
                } else {
                    None
                },
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(books)
}

const DEFAULT_PAGE_SIZE: usize = 20;

#[tauri::command]
pub fn list_books(
    filter: Option<String>,
    search: Option<String>,
    collection_id: Option<String>,
    cursor: Option<String>,
    limit: Option<usize>,
    db: State<'_, Db>,
) -> AppResult<BookPage> {
    let page_size = limit.unwrap_or(DEFAULT_PAGE_SIZE);
    let mut page = query_books(
        &db,
        filter.as_deref(),
        search.as_deref(),
        collection_id.as_deref(),
        cursor.as_deref(),
        page_size,
    )?;
    for book in &mut page.books {
        resolve_book_paths(book, &db)?;
    }
    Ok(page)
}

#[derive(Debug, serde::Serialize)]
pub struct BookCounts {
    pub all: usize,
    pub reading: usize,
    pub finished: usize,
}

#[tauri::command]
pub fn get_book_counts(db: State<'_, Db>) -> AppResult<BookCounts> {
    let conn = db.reader();
    let all: usize = conn.query_row("SELECT COUNT(*) FROM books", [], |r| r.get(0))?;
    let reading: usize = conn.query_row(
        "SELECT COUNT(*) FROM books WHERE status = 'reading'",
        [],
        |r| r.get(0),
    )?;
    let finished: usize = conn.query_row(
        "SELECT COUNT(*) FROM books WHERE status = 'finished'",
        [],
        |r| r.get(0),
    )?;
    Ok(BookCounts {
        all,
        reading,
        finished,
    })
}

#[tauri::command]
pub fn get_book(id: String, db: State<'_, Db>) -> AppResult<Book> {
    let mut book = query_book(&db, &id)?;
    resolve_book_paths(&mut book, &db)?;
    Ok(book)
}

#[tauri::command]
pub fn get_text_book_document(
    book_id: String,
    db: State<'_, Db>,
    local_dir: State<'_, LocalDir>,
    app: AppHandle,
) -> AppResult<TextBookDocument> {
    crate::sync::validation::validate_entity_id(&book_id)?;
    let state: (String, Option<String>, Option<String>) = {
        let conn = db.reader();
        conn.query_row(
            "SELECT preparation_state, preparation_error, source_sha256
             FROM books WHERE id = ?1 AND render_format = 'text'",
            params![book_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?
    };
    match state.0.as_str() {
        "ready" => {
            let path = prepared_document_path(&local_dir.0, &book_id);
            // Reader-side cache access stays non-mutating. Recovery and
            // sidecar cleanup run only inside the preparation job while it
            // owns the database writer lock.
            match read_prepared_document(&path, state.2.as_deref()) {
                Some(document) => Ok(document),
                _ => {
                    if transition_text_preparation_state(&db, &book_id, "ready", "pending", None)? {
                        schedule_text_book_preparation(app, book_id);
                    }
                    Err(AppError::Other("TEXT_PREPARATION_PENDING".to_string()))
                }
            }
        }
        "pending" => {
            schedule_text_book_preparation(app, book_id);
            Err(AppError::Other("TEXT_PREPARATION_PENDING".to_string()))
        }
        "preparing" => Err(AppError::Other("TEXT_PREPARATION_PENDING".to_string())),
        "failed" => Err(AppError::Other(format!(
            "TEXT_PREPARATION_FAILED:{}",
            state.1.unwrap_or_else(|| "UNKNOWN".to_string())
        ))),
        _ => Err(AppError::Other("TEXT_PREPARATION_PENDING".to_string())),
    }
}

#[tauri::command]
pub fn retry_text_book_preparation(
    book_id: String,
    db: State<'_, Db>,
    app: AppHandle,
) -> AppResult<()> {
    crate::sync::validation::validate_entity_id(&book_id)?;
    if transition_text_preparation_state(&db, &book_id, "failed", "pending", None)? {
        emit_text_preparation_changed(&app, &book_id, "pending");
        schedule_text_book_preparation(app, book_id);
    }
    Ok(())
}

/// Check a book's local file state and trigger iCloud download only for an
/// actual evicted placeholder. A missing local file is not an iCloud retry.
#[tauri::command]
pub fn check_book_available(id: String, db: State<'_, Db>) -> AppResult<BookAvailability> {
    let conn = db.reader();
    let file_path: String = conn.query_row(
        "SELECT file_path FROM books WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )?;

    let abs_path = db.resolve_path(&file_path)?;
    let availability = icloud::file_availability(&abs_path);
    if availability == icloud::FileAvailability::ICloudPlaceholder {
        icloud::trigger_download_file(&abs_path);
    }
    Ok(BookAvailability {
        status: availability.as_str().to_string(),
        available: availability == icloud::FileAvailability::Available,
    })
}

pub(crate) fn do_delete_book(id: &str, db: &Db, sync: &SyncWriter) -> AppResult<()> {
    do_delete_book_with_note_policy(id, false, db, sync)
}

pub(crate) fn do_delete_book_with_note_policy(
    id: &str,
    preserve_book_notes: bool,
    db: &Db,
    sync: &SyncWriter,
) -> AppResult<()> {
    crate::sync::validation::validate_entity_id(id)?;
    let (file_path, source_file_path): (String, Option<String>) = {
        let conn = db.conn.lock().map_err(|e| AppError::Other(e.to_string()))?;
        conn.query_row(
            "SELECT file_path, source_file_path FROM books WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?
    };

    let now = chrono::Utc::now().timestamp_millis();
    let device = sync.self_device().to_string();
    sync.with_tx(db, now, |tx, events| {
        if preserve_book_notes {
            let detached_notes = {
                let mut statement = tx.prepare(
                    "SELECT id, anchor_kind, normalized_word, selected_text, content,
                            content_format, created_at
                     FROM notes WHERE book_id = ?1 AND scope = 'book'",
                )?;
                let notes = statement
                    .query_map(params![id], |row| {
                        Ok(NotePayload {
                            id: row.get(0)?,
                            book_id: None,
                            anchor_kind: row.get(1)?,
                            normalized_word: row.get(2)?,
                            scope: "detached".to_string(),
                            location: None,
                            selected_text: row.get(3)?,
                            content: row.get(4)?,
                            content_format: row.get(5)?,
                            created_at: row.get(6)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                notes
            };
            for note in detached_notes {
                tx.execute(
                    "UPDATE notes
                     SET book_id = NULL, scope = 'detached', location = NULL,
                         updated_at = ?2, updated_by_device = ?3
                     WHERE id = ?1",
                    params![note.id, now, device],
                )?;
                events.push(EventBody::NoteUpsert(note));
            }
        }
        tx.execute(
            "DELETE FROM chat_messages WHERE chat_id IN (SELECT id FROM chats WHERE book_id = ?1)",
            params![id],
        )?;
        tx.execute("DELETE FROM chats WHERE book_id = ?1", params![id])?;
        tx.execute(
            "DELETE FROM collection_books WHERE book_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM highlights WHERE book_id = ?1", params![id])?;
        tx.execute("DELETE FROM bookmarks WHERE book_id = ?1", params![id])?;
        tx.execute("DELETE FROM vocab_words WHERE book_id = ?1", params![id])?;
        tx.execute("DELETE FROM lookup_records WHERE book_id = ?1", params![id])?;
        tx.execute(
            "DELETE FROM word_mark_rules WHERE book_id = ?1",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM notes WHERE book_id = ?1 AND scope = 'book'",
            params![id],
        )?;
        tx.execute(
            "UPDATE notes SET book_id = NULL WHERE book_id = ?1 AND scope = 'global'",
            params![id],
        )?;
        tx.execute("DELETE FROM book_settings WHERE book_id = ?1", params![id])?;
        tx.execute("DELETE FROM books WHERE id = ?1", params![id])?;
        events.push(EventBody::BookDelete { id: id.to_string() });
        Ok(())
    })?;

    let abs_file = db.resolve_path(&file_path)?;
    let _ = fs::remove_file(&abs_file);
    if let Some(source_path) = source_file_path.filter(|path| path != &file_path) {
        let abs_source = db.resolve_path(&source_path)?;
        let _ = fs::remove_file(abs_source);
    }
    let cover_file = db.resolve_path(&format!("covers/{id}.img"))?;
    let _ = fs::remove_file(&cover_file);

    Ok(())
}

#[tauri::command]
pub fn delete_book(
    id: String,
    preserve_notes: Option<bool>,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
    local_dir: State<'_, LocalDir>,
) -> AppResult<()> {
    do_delete_book_with_note_policy(&id, preserve_notes.unwrap_or(false), &db, &sync)?;
    let prepared_path = prepared_document_path(&local_dir.0, &id);
    let _ = fs::remove_file(&prepared_path);
    if let Ok(backup_path) = prepared_document_backup_path(&prepared_path) {
        let _ = fs::remove_file(backup_path);
    }
    if let Ok(temporary_path) = prepared_document_temporary_path(&prepared_path) {
        let _ = fs::remove_file(temporary_path);
    }
    let _ = fs::remove_file(legacy_prepared_document_path(&local_dir.0, &id));
    Ok(())
}

#[tauri::command]
pub fn update_reading_progress(
    id: String,
    progress: i32,
    cfi: Option<String>,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<()> {
    let now = chrono::Utc::now().timestamp_millis();
    let device = sync.self_device().to_string();
    // Page-turn rate is dominated by this command; gate the event push on
    // the per-book throttle so a reading session doesn't balloon the log.
    // The SQL write always lands so the local UI stays current — only the
    // event publication is coalesced. Semantic transitions like
    // `mark_finished` deliberately do NOT consult the throttle.
    let emit = sync.should_emit_progress(&id);
    sync.with_tx(&db, now, |tx, events| {
        tx.execute(
            "UPDATE books SET progress = ?1, current_cfi = ?2, updated_at = ?3, updated_by_device = ?4 WHERE id = ?5",
            params![progress, cfi, now, device, id],
        )?;
        if emit {
            events.push(EventBody::BookProgressSet {
                book: id.clone(),
                progress,
                cfi: cfi.clone(),
            });
        }
        Ok(())
    })
}

#[tauri::command]
pub fn update_book_pages(id: String, pages: i32, db: State<'_, Db>) -> AppResult<()> {
    // Local-only — `pages` is derived from the book file on this device and
    // not part of the sync contract. Plain DB write, no SyncWriter.
    let conn = db.conn.lock().map_err(|e| AppError::Other(e.to_string()))?;
    conn.execute(
        "UPDATE books SET pages = ?1 WHERE id = ?2",
        params![pages, id],
    )?;
    Ok(())
}

#[tauri::command]
pub fn mark_finished(id: String, db: State<'_, Db>, sync: State<'_, SyncWriter>) -> AppResult<()> {
    let now = chrono::Utc::now().timestamp_millis();
    let device = sync.self_device().to_string();
    sync.with_tx(&db, now, |tx, events| {
        // Read the current cfi BEFORE the UPDATE so the synthesized
        // `book.progress.set` carries the resume position the local row
        // keeps. Local SQL doesn't touch `current_cfi` here, so emitting
        // `cfi: None` would silently null the column on every peer while
        // this device still has it — a snapshot-equivalence violation.
        let current_cfi: Option<String> = tx
            .query_row(
                "SELECT current_cfi FROM books WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        tx.execute(
            "UPDATE books SET status = 'finished', progress = 100, updated_at = ?1, updated_by_device = ?2 WHERE id = ?3",
            params![now, device, id],
        )?;
        // Mark-finished is two LWW columns moving in lockstep; the merge
        // engine has no `book.finished` event, so we publish the same pair
        // of events the user could have produced manually. The progress
        // event is published unconditionally — the throttle is for noisy
        // page-turn updates only, never for semantic transitions.
        events.push(EventBody::BookStatusSet {
            book: id.clone(),
            status: "finished".into(),
        });
        events.push(EventBody::BookProgressSet {
            book: id.clone(),
            progress: 100,
            cfi: current_cfi,
        });
        Ok(())
    })
}

pub(crate) fn do_update_book(
    id: &str,
    title: Option<&str>,
    author: Option<&str>,
    genre: Option<&str>,
    status: Option<&str>,
    db: &Db,
    sync: &SyncWriter,
) -> AppResult<Book> {
    let now = chrono::Utc::now().timestamp_millis();
    let device = sync.self_device().to_string();
    sync.with_tx(db, now, |tx, events| {
        if let Some(t) = title {
            tx.execute(
                "UPDATE books SET title = ?1, updated_at = ?2, updated_by_device = ?3 WHERE id = ?4",
                params![t, now, device, id],
            )?;
            events.push(EventBody::BookMetadataSet {
                book: id.to_string(),
                field: "title".into(),
                value: serde_json::Value::String(t.to_string()),
            });
        }
        if let Some(a) = author {
            tx.execute(
                "UPDATE books SET author = ?1, updated_at = ?2, updated_by_device = ?3 WHERE id = ?4",
                params![a, now, device, id],
            )?;
            events.push(EventBody::BookMetadataSet {
                book: id.to_string(),
                field: "author".into(),
                value: serde_json::Value::String(a.to_string()),
            });
        }
        if let Some(g) = genre {
            tx.execute(
                "UPDATE books SET genre = ?1, updated_at = ?2, updated_by_device = ?3 WHERE id = ?4",
                params![g, now, device, id],
            )?;
            events.push(EventBody::BookMetadataSet {
                book: id.to_string(),
                field: "genre".into(),
                value: serde_json::Value::String(g.to_string()),
            });
        }
        if let Some(s) = status {
            tx.execute(
                "UPDATE books SET status = ?1, updated_at = ?2, updated_by_device = ?3 WHERE id = ?4",
                params![s, now, device, id],
            )?;
            events.push(EventBody::BookStatusSet {
                book: id.to_string(),
                status: s.to_string(),
            });
        }
        Ok(())
    })?;
    query_book(db, id)
}

#[tauri::command]
pub fn update_book_status(
    id: String,
    status: String,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<()> {
    do_update_book(&id, None, None, None, Some(&status), &db, &sync)?;
    Ok(())
}

#[tauri::command]
pub fn update_book_metadata(
    id: String,
    title: String,
    author: String,
    db: State<'_, Db>,
    sync: State<'_, SyncWriter>,
) -> AppResult<()> {
    do_update_book(&id, Some(&title), Some(&author), None, None, &db, &sync)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use rusqlite::params;
    use tempfile::TempDir;

    /// Regression: slugify of a CJK title used to panic when the
    /// 60-byte truncation point landed mid-codepoint. The user-facing
    /// symptom was `import_book` hanging forever (Tauri's command
    /// runtime swallows the panic so the spinner never resolves).
    /// This particular title — "百年孤独 (root edition note)" — slugs
    /// to exactly 62 bytes so the cut at byte 60 falls inside the
    /// last `删` character.
    #[test]
    fn slugify_does_not_panic_on_cjk_title_at_byte_boundary() {
        let title = "百年孤独(根据马尔克斯指定版本翻译,未做任何增删)";
        let slug = slugify(title);
        // Must be valid UTF-8 (the .to_string() in slugify would have
        // panicked if the slice were invalid) and not empty.
        assert!(!slug.is_empty());
        assert!(slug.chars().count() > 0);
        // Must round-trip into book_filename without panicking.
        let _ = book_filename(title, "abcdef0123456789", "epub");
    }

    /// ASCII titles still get a meaningful slug after the truncation
    /// fix (regression safety on the common path).
    #[test]
    fn slugify_truncates_long_ascii_at_word_boundary() {
        let title = "the quick brown fox jumps over the lazy dog and then keeps on running";
        let slug = slugify(title);
        assert!(slug.len() <= 60);
        assert!(slug.starts_with("the-quick-brown-fox"));
        assert!(!slug.ends_with('-'));
    }

    #[test]
    fn detect_import_format_accepts_utf16_txt_bom() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("utf16.txt");
        fs::write(&path, [0xff, 0xfe, b'H', 0, b'i', 0]).unwrap();
        assert_eq!(detect_import_format(&path).unwrap(), ImportFormat::Txt);
        assert_eq!(decode_txt(&fs::read(&path).unwrap()).unwrap(), "Hi");
    }

    #[test]
    fn text_import_copies_source_and_defers_preparation() {
        let (dir, db) = setup();
        let source = dir.path().join("reader.txt");
        fs::write(&source, "Chapter 1\n\nFirst paragraph.").unwrap();
        let sync = SyncWriter::new("dev-A".to_string());

        let book = do_import_text(source.to_str().unwrap(), "txt", &db, &sync).unwrap();

        assert_eq!(book.format, "text");
        assert_eq!(book.render_format.as_deref(), Some("text"));
        assert_eq!(book.preparation_state, "pending");
        assert!(book.pages.is_none());
        assert!(db.resolve_path(&book.file_path).unwrap().is_file());
        assert!(dir
            .path()
            .join("books")
            .read_dir()
            .unwrap()
            .next()
            .is_none());
    }

    #[test]
    fn text_preparation_normalizes_markdown_and_html() {
        let dir = TempDir::new().unwrap();
        let markdown = dir.path().join("book.md");
        let html = dir.path().join("book.html");
        fs::write(&markdown, "# Chapter One\n\nHello **reader**.").unwrap();
        fs::write(&html, "<h1>Chapter Two</h1><p>Hello <em>reader</em>.</p>").unwrap();

        let markdown_document = prepare_text_document(&markdown, "markdown", None).unwrap();
        let html_document = prepare_text_document(&html, "html", None).unwrap();

        assert_eq!(markdown_document.version, TEXT_DOCUMENT_VERSION);
        let markdown_text = markdown_document
            .chunks
            .iter()
            .flat_map(|chunk| &chunk.blocks)
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let html_text = html_document
            .chunks
            .iter()
            .flat_map(|chunk| &chunk.blocks)
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(markdown_text.contains("Hello reader"));
        assert!(html_text.contains("Hello reader"));
    }

    #[test]
    fn text_preparation_hashes_the_source_bytes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("book.txt");
        fs::write(&path, "Chapter One\n\nHello reader.").unwrap();
        let actual_hash = source_sha256(&path).unwrap();

        let document = prepare_text_document(&path, "txt", Some(actual_hash.clone())).unwrap();
        assert_eq!(
            document.source_sha256.as_deref(),
            Some(actual_hash.as_str())
        );

        let error = prepare_text_document(&path, "txt", Some("0".repeat(64)))
            .err()
            .unwrap();
        assert!(error.to_string().contains("TEXT_SOURCE_HASH_MISMATCH"));
    }

    #[test]
    fn text_parser_builds_hierarchical_part_toc_without_generated_chunks() {
        let text = "PART ONE 1\nFirst.\nPART ONE 2\nSecond.\nPART TWO 1\nThird.\nPART TWO 22\nLast.\nEPILOGUE\nDone.";
        let (chunks, toc, legacy_locations) = text_document_parts(text, true);
        let entries = toc
            .iter()
            .map(|entry| (entry.title.as_str(), entry.depth))
            .collect::<Vec<_>>();

        assert_eq!(
            entries,
            [
                ("PART ONE", 0),
                ("1", 1),
                ("2", 1),
                ("PART TWO", 0),
                ("1", 1),
                ("22", 1),
                ("EPILOGUE", 0),
            ]
        );
        assert_eq!(
            chunks
                .iter()
                .flat_map(|chunk| &chunk.blocks)
                .filter(|block| block.kind == TextBookBlockKind::Heading)
                .count(),
            5
        );
        assert!(toc.iter().all(|entry| !entry.title.starts_with("Part ")));
        assert_eq!(text_toc_leaf_count(&toc), 5);
        // PART headings were ordinary V1 paragraphs, so old locations still
        // resolve to the exact same canonical source offsets.
        assert_eq!(legacy_locations[0][0], 0);
        assert_eq!(
            legacy_locations[0][2],
            "PART ONE 1\nFirst.\n".encode_utf16().count() as u64
        );
    }

    #[test]
    fn text_parser_recognizes_common_english_and_chinese_headings() {
        let text = "BOOK I\nCHAPTER ONE\nBody\nSECTION 2\nMore\nACT III\nPlay\nVOLUME TWO 3\nNext\n第一卷\n第一章 开始\n正文\n第十二章：结局\n第一章初见\nEPILOGUE\nEnd";
        let (_, toc, _) = text_document_parts(text, true);
        let entries = toc
            .iter()
            .map(|entry| (entry.title.as_str(), entry.depth))
            .collect::<Vec<_>>();

        assert_eq!(
            entries,
            [
                ("BOOK I", 0),
                ("CHAPTER ONE", 1),
                ("SECTION 2", 2),
                ("ACT III", 1),
                ("VOLUME TWO", 0),
                ("3", 1),
                ("第一卷", 0),
                ("第一章 开始", 1),
                ("第十二章：结局", 1),
                ("第一章初见", 1),
                ("EPILOGUE", 0),
            ]
        );
    }

    #[test]
    fn text_parser_builds_multilevel_book_and_play_trees() {
        let text = "VOLUME I\nBOOK ONE\nPART ONE\nCHAPTER 1\nSECTION 1\nText\nACT II\nSCENE 1\nPlay\nEPILOGUE\nEnd";
        let (_, toc, _) = text_document_parts(text, true);
        let entries = toc
            .iter()
            .map(|entry| (entry.title.as_str(), entry.depth))
            .collect::<Vec<_>>();

        assert_eq!(
            entries,
            [
                ("VOLUME I", 0),
                ("BOOK ONE", 1),
                ("PART ONE", 2),
                ("CHAPTER 1", 3),
                ("SECTION 1", 4),
                ("ACT II", 2),
                ("SCENE 1", 3),
                ("EPILOGUE", 0),
            ]
        );
        assert_eq!(text_toc_leaf_count(&toc), 3);

        let (_, play_toc, _) = text_document_parts("ACT I\nSCENE 1\nText\nSCENE 2\nMore", true);
        let play_entries = play_toc
            .iter()
            .map(|entry| (entry.title.as_str(), entry.depth))
            .collect::<Vec<_>>();
        assert_eq!(play_entries, [("ACT I", 0), ("SCENE 1", 1), ("SCENE 2", 1)]);
    }

    #[test]
    fn text_parser_recognizes_punctuated_english_headings() {
        let text = "Chapter 1.\nBody\n\nChapter One.\nMore\n\nPart Two.\nLast";
        let (_, toc, _) = text_document_parts(text, true);
        let entries = toc
            .iter()
            .map(|entry| (entry.title.as_str(), entry.depth))
            .collect::<Vec<_>>();

        assert_eq!(
            entries,
            [("Chapter 1.", 0), ("Chapter One.", 0), ("Part Two.", 0)]
        );
    }

    #[test]
    fn text_parser_recognizes_bare_number_chapters_without_a_parent() {
        let text = "1\nFirst\n\n2\nSecond\n\nI\nThird\n\nII\nFourth";
        let (_, toc, _) = text_document_parts(text, true);
        let entries = toc
            .iter()
            .map(|entry| (entry.title.as_str(), entry.depth))
            .collect::<Vec<_>>();

        assert_eq!(entries, [("1", 0), ("2", 0), ("I", 0), ("II", 0)]);
    }

    #[test]
    fn text_parser_does_not_treat_prose_as_a_heading() {
        let text = "The chapter was short.\nWe took part in the work.\nChapter books were nearby.\nChapter one was the longest\nPart one of the story\nPART CIVIL DUTY\n2024\ncontinues as prose\n第一次读到这一章时，我没有停下来\n\nmix";
        let (chunks, toc, _) = text_document_parts(text, true);
        assert_eq!(toc.len(), 1);
        assert_eq!(toc[0].title, "Reading");
        assert!(chunks
            .iter()
            .flat_map(|chunk| &chunk.blocks)
            .all(|block| block.kind == TextBookBlockKind::Paragraph));
    }

    #[test]
    fn text_parser_does_not_promote_inline_numbers_after_a_chapter() {
        let (_, toc, _) = text_document_parts("CHAPTER 1\nprose\n2024\ncontinues", true);
        let entries = toc
            .iter()
            .map(|entry| (entry.title.as_str(), entry.depth))
            .collect::<Vec<_>>();
        assert_eq!(entries, [("CHAPTER 1", 0)]);
    }

    #[test]
    fn text_parser_accepts_canonical_roman_and_compound_word_numbers() {
        assert!(canonical_roman_number("MCMXCIV"));
        assert!(!canonical_roman_number("CIVIL"));
        assert!(!canonical_roman_number("ILL"));

        let (_, toc, _) = text_document_parts("PART TWENTY ONE 3\nText\nCHAPTER XLII\nMore", true);
        let entries = toc
            .iter()
            .map(|entry| (entry.title.as_str(), entry.depth))
            .collect::<Vec<_>>();
        assert_eq!(
            entries,
            [("PART TWENTY ONE", 0), ("3", 1), ("CHAPTER XLII", 1)]
        );
    }

    #[test]
    fn text_parser_reflows_consistently_hard_wrapped_paragraphs() {
        let first = "alpha beta gamma delta epsilon zeta eta theta iota kappa";
        let second = "lambda mu nu xi omicron pi rho sigma tau upsilon omega";
        let mut groups = Vec::new();
        for index in 0..5 {
            groups.push(format!("{first}\n{second}  \nparagraph {index} ends."));
        }
        let text = groups.join("\n\n");
        let lines = normalized_text_lines(&text);
        let (chunks, _, legacy_locations) = text_document_parts(&text, true);
        let blocks = chunks
            .iter()
            .flat_map(|chunk| &chunk.blocks)
            .collect::<Vec<_>>();

        assert!(hard_wrap_width(&lines, &vec![None; lines.len()]).is_some());
        assert_eq!(blocks.len(), 5);
        assert_eq!(legacy_locations[0].len(), 15);
        assert_eq!(
            blocks[0].text,
            format!("{first} {second} paragraph 0 ends.")
        );
        assert_eq!(blocks[0].source_spans.len(), 5);
        assert_eq!(blocks[0].source_spans[2].source_start, utf16_len(first) + 1);
        assert_eq!(
            blocks[0].source_spans[4].source_start,
            utf16_len(first) + 1 + utf16_len(second) + 2 + 1
        );
        let (unreflowed, _, _) = text_document_parts(&text, false);
        assert_eq!(
            unreflowed
                .iter()
                .map(|chunk| chunk.blocks.len())
                .sum::<usize>(),
            15
        );
    }

    #[test]
    fn text_parser_reflows_prose_with_lowercase_continuation_lines() {
        let groups = (0..4)
            .map(|index| {
                format!(
                    "Alpha beta gamma delta epsilon zeta eta theta iota\n\
                     lambda mu nu xi omicron pi rho sigma tau upsilon\n\
                     continuation words remain part of the same paragraph\n\
                     final line {index} ends with ordinary punctuation."
                )
            })
            .collect::<Vec<_>>();
        let text = groups.join("\n\n");
        let lines = normalized_text_lines(&text);
        assert!(hard_wrap_width(&lines, &vec![None; lines.len()]).is_some());

        let (chunks, _, _) = text_document_parts(&text, true);
        assert_eq!(
            chunks.iter().map(|chunk| chunk.blocks.len()).sum::<usize>(),
            4
        );
    }

    #[test]
    fn text_parser_preserves_bullet_and_numbered_lists() {
        let bullet_list = (0..12)
            .map(|index| format!("- list item {index:02} keeps its deliberate line boundary"))
            .collect::<Vec<_>>()
            .join("\n");
        let numbered_list = (1..=12)
            .map(|index| format!("{index}. list item keeps its deliberate line boundary"))
            .collect::<Vec<_>>()
            .join("\n");
        let alpha_list = (b'A'..=b'L')
            .map(|marker| {
                format!(
                    "{}) list item keeps its deliberate line boundary",
                    char::from(marker)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let roman_list = [
            "I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X", "XI", "XII",
        ]
        .into_iter()
        .map(|marker| format!("{marker}. list item keeps its deliberate line boundary"))
        .collect::<Vec<_>>()
        .join("\n");
        let task_list = (0..12)
            .map(|index| format!("[ ] task {index:02} keeps its deliberate line boundary"))
            .collect::<Vec<_>>()
            .join("\n");

        for text in [
            bullet_list,
            numbered_list,
            alpha_list,
            roman_list,
            task_list,
        ] {
            let lines = normalized_text_lines(&text);
            assert_eq!(hard_wrap_width(&lines, &vec![None; lines.len()]), None);

            let (chunks, _, _) = text_document_parts(&text, true);
            assert_eq!(
                chunks.iter().map(|chunk| chunk.blocks.len()).sum::<usize>(),
                12
            );
        }
    }

    #[test]
    fn text_parser_preserves_repeated_capitalized_verse_lines() {
        let text = (0..12)
            .map(|index| format!("Silver light crosses the quiet water in verse {index:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = normalized_text_lines(&text);

        assert_eq!(hard_wrap_width(&lines, &vec![None; lines.len()]), None);
        let (chunks, _, _) = text_document_parts(&text, true);
        assert_eq!(
            chunks.iter().map(|chunk| chunk.blocks.len()).sum::<usize>(),
            12
        );
    }

    #[test]
    fn text_parser_preserves_repeated_lowercase_verse_lines() {
        let text = (0..12)
            .map(|index| format!("silver light crosses the quiet water in verse {index:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        let lines = normalized_text_lines(&text);

        assert!(hard_wrap_width(&lines, &vec![None; lines.len()]).is_some());
        let (chunks, _, _) = text_document_parts(&text, true);
        assert_eq!(
            chunks.iter().map(|chunk| chunk.blocks.len()).sum::<usize>(),
            12
        );
    }

    #[test]
    fn text_parser_preserves_an_embedded_lowercase_verse_stanza() {
        let first = "alpha beta gamma delta epsilon zeta eta theta iota kappa";
        let second = "lambda mu nu xi omicron pi rho sigma tau upsilon omega";
        let mut groups = (0..4)
            .map(|index| format!("{first}\n{second}\nparagraph {index} ends."))
            .collect::<Vec<_>>();
        let verse = [
            "silver light crosses the quiet water below",
            "wind leans softly through the winter reeds",
            "footsteps fade beyond the sleeping shore",
            "night keeps every word we could not say",
        ];
        groups.push(verse.join("\n"));
        let text = groups.join("\n\n");
        let lines = normalized_text_lines(&text);
        assert!(hard_wrap_width(&lines, &vec![None; lines.len()]).is_some());

        let (chunks, _, _) = text_document_parts(&text, true);
        let blocks = chunks
            .iter()
            .flat_map(|chunk| &chunk.blocks)
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>();
        assert_eq!(blocks.len(), 8);
        assert_eq!(&blocks[4..], verse);
    }

    #[test]
    fn text_parser_keeps_prose_after_an_indented_line_separate() {
        let first = "alpha beta gamma delta epsilon zeta eta theta iota kappa";
        let second = "lambda mu nu xi omicron pi rho sigma tau upsilon omega";
        let mut groups = (0..4)
            .map(|index| format!("{first}\n{second}\nparagraph {index} ends."))
            .collect::<Vec<_>>();
        groups.push(
            "    indented quotation keeps its deliberate boundary here\nfollowing prose remains separate from the indented quotation"
                .to_string(),
        );
        let text = groups.join("\n\n");
        let lines = normalized_text_lines(&text);
        assert!(hard_wrap_width(&lines, &vec![None; lines.len()]).is_some());

        let (chunks, _, _) = text_document_parts(&text, true);
        let blocks = chunks
            .iter()
            .flat_map(|chunk| &chunk.blocks)
            .map(|block| block.text.as_str())
            .collect::<Vec<_>>();
        assert!(blocks.contains(&"indented quotation keeps its deliberate boundary here"));
        assert!(blocks.contains(&"following prose remains separate from the indented quotation"));
    }

    #[test]
    fn text_reflow_uses_script_appropriate_separators() {
        let cjk_lines = normalized_text_lines("中文换行\n继续阅读");
        let mut cjk = block_from_line(&cjk_lines[0], TextBookBlockKind::Paragraph, None);
        append_reflowed_line(&mut cjk, &cjk_lines[1]);
        assert_eq!(cjk.text, "中文换行继续阅读");

        let hyphen_lines = normalized_text_lines("well-\nknown");
        let mut hyphen = block_from_line(&hyphen_lines[0], TextBookBlockKind::Paragraph, None);
        append_reflowed_line(&mut hyphen, &hyphen_lines[1]);
        assert_eq!(hyphen.text, "well-known");
    }

    #[test]
    fn text_parser_keeps_complete_lines_as_separate_paragraphs() {
        let text = (0..12)
            .map(|index| {
                format!(
                    "This is complete paragraph number {index}, with a natural sentence ending."
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let lines = normalized_text_lines(&text);
        assert_eq!(hard_wrap_width(&lines, &vec![None; lines.len()]), None);
        let (chunks, _, _) = text_document_parts(&text, true);
        assert_eq!(
            chunks.iter().map(|chunk| chunk.blocks.len()).sum::<usize>(),
            12
        );
    }

    #[test]
    fn text_preparation_does_not_reflow_markdown_soft_breaks() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("lines.md");
        let markdown = (0..12)
            .map(|index| format!("soft wrapped markdown line {index} without terminal punctuation"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&path, markdown).unwrap();

        let document = prepare_text_document(&path, "markdown", None).unwrap();
        assert_eq!(
            document
                .chunks
                .iter()
                .map(|chunk| chunk.blocks.len())
                .sum::<usize>(),
            12
        );
    }

    #[test]
    fn text_offsets_use_utf16_but_legacy_chunks_keep_utf8_threshold() {
        let emoji_lines = normalized_text_lines("A😀B\nNext");
        assert_eq!(emoji_lines[0].source_start, 0);
        assert_eq!(emoji_lines[0].source_end, 4);
        assert_eq!(emoji_lines[1].source_start, 5);

        let cjk_line = "文".repeat(8_000);
        let text = format!("{cjk_line}\n{cjk_line}");
        let locations = legacy_text_locations(&normalized_text_lines(&text));
        assert_eq!(locations.len(), 2);
        assert_eq!(locations[0], [0]);
        assert_eq!(locations[1], [8_001]);
    }

    fn sample_text_document(source_sha256: &str) -> TextBookDocument {
        TextBookDocument {
            version: TEXT_DOCUMENT_VERSION,
            source_sha256: Some(source_sha256.to_string()),
            coordinate_space: "normalized_utf16".to_string(),
            chunks: vec![TextBookChunk {
                blocks: vec![TextBookBlock {
                    kind: TextBookBlockKind::Paragraph,
                    text: "A paragraph".to_string(),
                    source_start: 0,
                    source_end: 11,
                    source_spans: vec![TextBookSourceSpan {
                        rendered_start: 0,
                        source_start: 0,
                        length: 11,
                    }],
                    depth: None,
                }],
            }],
            toc: vec![TextBookTocEntry {
                title: "Reading".to_string(),
                depth: 0,
                source_offset: 0,
            }],
            legacy_locations: vec![vec![0]],
        }
    }

    #[test]
    fn prepared_document_write_is_readable() {
        let dir = TempDir::new().unwrap();
        let document = sample_text_document("abc");
        let path = prepared_document_path(dir.path(), "book-id");
        write_prepared_document(&path, &document).unwrap();
        let mut replacement = document.clone();
        replacement.chunks[0].blocks[0].text = "Replacement".to_string();
        write_prepared_document(&path, &replacement).unwrap();

        let restored: TextBookDocument = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(restored.chunks[0].blocks[0].text, "Replacement");
        assert!(path.ends_with("book-id.v2.json"));

        let backup_path = prepared_document_backup_path(&path).unwrap();
        assert!(!backup_path.exists());
        fs::rename(&path, &backup_path).unwrap();
        let recovered = load_prepared_document(&path, Some("abc")).unwrap();
        assert_eq!(recovered.chunks[0].blocks[0].text, "Replacement");
        assert!(path.exists());
        assert!(!backup_path.exists());

        let temporary_path = prepared_document_temporary_path(&path).unwrap();
        fs::remove_file(&path).unwrap();
        fs::write(&temporary_path, serde_json::to_vec(&replacement).unwrap()).unwrap();
        let recovered = load_prepared_document(&path, Some("abc")).unwrap();
        assert_eq!(recovered.chunks[0].blocks[0].text, "Replacement");
        assert!(path.exists());
        assert!(!temporary_path.exists());
    }

    fn setup() -> (TempDir, Db) {
        let dir = TempDir::new().unwrap();
        let db = Db::init(dir.path()).unwrap();
        (dir, db)
    }

    fn insert_book(db: &Db, id: &str, format: &str) {
        let conn = db.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO books (id, title, author, file_path, format, status, progress, created_at, updated_at)
             VALUES (?1, 'Test', 'Author', 'books/test.epub', ?2, 'reading', 0, ?3, ?3)",
            params![id, format, now],
        ).unwrap();
    }

    fn insert_text_preparation_book(db: &Db, id: &str, state: &str, source_sha256: &str) {
        let conn = db.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO books
             (id, title, author, file_path, format, source_format, render_format,
              source_file_path, source_sha256, conversion_version, preparation_state,
              preparation_error, status, progress, created_at, updated_at)
             VALUES (?1, 'Text', 'Author', ?2, 'txt', 'txt', 'text', ?2, ?3, ?4, ?5,
                     'existing error', 'reading', 0, ?6, ?6)",
            params![
                id,
                format!("sources/{id}.txt"),
                source_sha256,
                TEXT_DOCUMENT_VERSION,
                state,
                now,
            ],
        )
        .unwrap();
    }

    fn text_preparation_source(id: &str, source_sha256: &str) -> TextPreparationSource {
        TextPreparationSource {
            file_path: Some(format!("sources/{id}.txt")),
            format: Some("txt".to_string()),
            sha256: Some(source_sha256.to_string()),
            conversion_version: TEXT_DOCUMENT_VERSION,
        }
    }

    #[test]
    fn repeated_pending_scan_does_not_reset_an_active_preparation() {
        let (_dir, db) = setup();
        insert_text_preparation_book(&db, "active", "preparing", "active-hash");
        insert_text_preparation_book(&db, "queued", "pending", "queued-hash");

        assert_eq!(pending_text_book_ids(&db, false).unwrap(), ["queued"]);
        let active: (String, Option<String>) = db
            .reader()
            .query_row(
                "SELECT preparation_state, preparation_error FROM books WHERE id = 'active'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            active,
            ("preparing".to_string(), Some("existing error".to_string()))
        );

        assert_eq!(
            pending_text_book_ids(&db, true).unwrap(),
            ["active", "queued"]
        );
        let active: (String, Option<String>) = db
            .reader()
            .query_row(
                "SELECT preparation_state, preparation_error FROM books WHERE id = 'active'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(active, ("pending".to_string(), None));
    }

    #[test]
    fn text_preparation_state_transitions_are_compare_and_set() {
        let (_dir, db) = setup();
        insert_text_preparation_book(&db, "retry", "failed", "hash");

        assert!(
            transition_text_preparation_state(&db, "retry", "failed", "pending", None,).unwrap()
        );
        assert!(
            !transition_text_preparation_state(&db, "retry", "failed", "pending", None,).unwrap()
        );
        assert!(
            !transition_text_preparation_state(&db, "retry", "ready", "pending", None,).unwrap()
        );

        db.conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE books SET preparation_state = 'ready' WHERE id = 'retry'",
                [],
            )
            .unwrap();
        assert!(
            transition_text_preparation_state(&db, "retry", "ready", "pending", None,).unwrap()
        );
        assert!(
            !transition_text_preparation_state(&db, "retry", "ready", "pending", None,).unwrap()
        );
    }

    #[test]
    fn preparation_publish_requires_current_source_and_surviving_book() {
        let (dir, db) = setup();
        let document = sample_text_document("hash");

        insert_text_preparation_book(&db, "current", "preparing", "hash");
        let current_path = prepared_document_path(dir.path(), "current");
        assert!(publish_current_text_preparation_job(
            &db,
            "current",
            &text_preparation_source("current", "hash"),
            &current_path,
            &document,
            1,
        )
        .unwrap());
        assert!(current_path.exists());
        let current_state: (String, i32) = db
            .reader()
            .query_row(
                "SELECT preparation_state, pages FROM books WHERE id = 'current'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(current_state, ("ready".to_string(), 1));

        insert_text_preparation_book(&db, "changed", "preparing", "new-hash");
        let changed_path = prepared_document_path(dir.path(), "changed");
        assert!(!publish_current_text_preparation_job(
            &db,
            "changed",
            &text_preparation_source("changed", "old-hash"),
            &changed_path,
            &document,
            1,
        )
        .unwrap());
        assert!(!changed_path.exists());

        insert_text_preparation_book(&db, "deleted", "preparing", "hash");
        db.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM books WHERE id = 'deleted'", [])
            .unwrap();
        let deleted_path = prepared_document_path(dir.path(), "deleted");
        assert!(!publish_current_text_preparation_job(
            &db,
            "deleted",
            &text_preparation_source("deleted", "hash"),
            &deleted_path,
            &document,
            1,
        )
        .unwrap());
        assert!(!deleted_path.exists());
    }

    #[test]
    fn test_format_defaults_to_epub() {
        let (_dir, db) = setup();
        let conn = db.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO books (id, title, author, file_path, status, progress, created_at, updated_at)
             VALUES ('b1', 'Test', 'Author', 'books/test.epub', 'reading', 0, ?1, ?1)",
            params![now],
        ).unwrap();

        let format: String = conn
            .query_row("SELECT format FROM books WHERE id = 'b1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(format, "epub");
    }

    #[test]
    fn test_format_pdf() {
        let (_dir, db) = setup();
        insert_book(&db, "b1", "pdf");

        let conn = db.conn.lock().unwrap();
        let format: String = conn
            .query_row("SELECT format FROM books WHERE id = 'b1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(format, "pdf");
    }

    #[test]
    fn test_import_pdf_inserts_with_correct_format() {
        let (dir, db) = setup();

        let src_path = dir.path().join("test.pdf");
        fs::write(&src_path, b"%PDF-1.4 fake content").unwrap();

        let conn = db.conn.lock().unwrap();
        let book_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp_millis();
        let rel_file_path = format!("books/{}.pdf", book_id);

        let dest = dir.path().join("books").join(format!("{}.pdf", book_id));
        fs::copy(&src_path, &dest).unwrap();

        conn.execute(
            "INSERT INTO books (id, title, author, file_path, format, status, progress, pages, created_at, updated_at)
             VALUES (?1, 'My PDF', 'PDF Author', ?2, 'pdf', 'unread', 0, 42, ?3, ?3)",
            params![book_id, rel_file_path, now],
        ).unwrap();

        let (title, format, pages): (String, String, i32) = conn
            .query_row(
                "SELECT title, format, pages FROM books WHERE id = ?1",
                params![book_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();

        assert_eq!(title, "My PDF");
        assert_eq!(format, "pdf");
        assert_eq!(pages, 42);
        assert!(dest.exists());
    }

    #[test]
    fn test_import_pdf_with_cover() {
        let (_dir, db) = setup();
        let book_id = "cover-test";
        let cover_bytes = b"\x89PNG fake png data";

        let conn = db.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp_millis();

        conn.execute(
            "INSERT INTO books (id, title, author, file_path, format, cover_data, status, progress, created_at, updated_at)
             VALUES (?1, 'PDF With Cover', 'Author', 'books/test.pdf', 'pdf', ?2, 'unread', 0, ?3, ?3)",
            params![book_id, cover_bytes.as_slice(), now],
        ).unwrap();

        let db_cover: Option<Vec<u8>> = conn
            .query_row(
                "SELECT cover_data FROM books WHERE id = ?1",
                params![book_id],
                |r| r.get(0),
            )
            .unwrap();

        assert_eq!(db_cover.as_deref(), Some(cover_bytes.as_slice()));
    }

    #[test]
    fn test_list_books_returns_format() {
        let (_dir, db) = setup();
        insert_book(&db, "b1", "epub");
        insert_book(&db, "b2", "pdf");

        let conn = db.conn.lock().unwrap();
        let mut stmt = conn.prepare(
        "SELECT id, title, author, description, cover_path, file_path, format, source_format, render_format, source_file_path, source_sha256, conversion_version, genre, pages, status, progress, current_cfi, created_at, updated_at, preparation_state, preparation_error FROM books ORDER BY id",
        ).unwrap();
        let books: Vec<Book> = stmt
            .query_map([], |row| {
                Ok(Book {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    author: row.get(2)?,
                    description: row.get(3)?,
                    cover_path: row.get(4)?,
                    file_path: row.get(5)?,
                    format: row.get(6)?,
                    source_format: row.get(7)?,
                    render_format: row.get(8)?,
                    source_file_path: row.get(9)?,
                    source_sha256: row.get(10)?,
                    conversion_version: row.get::<_, Option<i32>>(11)?.unwrap_or(0),
                    preparation_state: row.get(19)?,
                    preparation_error: row.get(20)?,
                    genre: row.get(12)?,
                    pages: row.get(13)?,
                    status: row.get(14)?,
                    progress: row.get(15)?,
                    current_cfi: row.get(16)?,
                    created_at: row.get(17)?,
                    updated_at: row.get(18)?,
                    available: true,
                    cover_data: None,
                })
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(books.len(), 2);
        assert_eq!(books[0].format, "epub");
        assert_eq!(books[1].format, "pdf");
    }

    #[test]
    fn test_import_pdf_none_author_defaults() {
        let (_dir, db) = setup();
        let conn = db.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp_millis();

        // Simulate import_pdf with author = None — exercise the same fallback
        // expression the command uses without literally writing
        // `None.unwrap_or_else(...)`, which clippy now flags.
        fn resolve(author: Option<String>) -> String {
            author.unwrap_or_else(|| "Unknown Author".to_string())
        }
        let resolved_author = resolve(None);

        conn.execute(
            "INSERT INTO books (id, title, author, file_path, format, status, progress, created_at, updated_at)
             VALUES ('b1', 'No Author PDF', ?1, 'books/test.pdf', 'pdf', 'unread', 0, ?2, ?2)",
            params![resolved_author, now],
        ).unwrap();

        let author_val: String = conn
            .query_row("SELECT author FROM books WHERE id = 'b1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(author_val, "Unknown Author");
    }

    #[test]
    fn test_import_pdf_with_all_metadata() {
        let (dir, db) = setup();

        let src = dir.path().join("academic-paper.pdf");
        fs::write(&src, b"%PDF-1.7 fake").unwrap();

        let book_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp_millis();
        let rel_path = format!("books/{}.pdf", book_id);
        let cover_bytes = b"\x89PNG fake cover";

        let dest = dir.path().join("books").join(format!("{}.pdf", book_id));
        fs::copy(&src, &dest).unwrap();

        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO books (id, title, author, description, file_path, format, pages, status, progress, created_at, updated_at, cover_data)
             VALUES (?1, 'Deep Learning', 'Ian Goodfellow, Yoshua Bengio', 'A comprehensive textbook', ?2, 'pdf', 800, 'unread', 0, ?3, ?3, ?4)",
            params![book_id, rel_path, now, cover_bytes.as_slice()],
        ).unwrap();

        let (title, author, desc, format, pages, has_cover): (String, String, Option<String>, String, Option<i32>, bool) = conn.query_row(
            "SELECT title, author, description, format, pages, (cover_data IS NOT NULL) FROM books WHERE id = ?1",
            params![book_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        ).unwrap();

        assert_eq!(title, "Deep Learning");
        assert_eq!(author, "Ian Goodfellow, Yoshua Bengio");
        assert_eq!(desc, Some("A comprehensive textbook".to_string()));
        assert_eq!(format, "pdf");
        assert_eq!(pages, Some(800));
        assert!(has_cover);
        assert!(dest.exists());
    }

    #[test]
    fn test_update_metadata_title_and_author() {
        let (_dir, db) = setup();
        insert_book(&db, "b1", "epub");

        let conn = db.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE books SET title = ?1, author = ?2, updated_at = ?3 WHERE id = ?4",
            params!["New Title", "New Author", now, "b1"],
        )
        .unwrap();

        let (title, author): (String, String) = conn
            .query_row("SELECT title, author FROM books WHERE id = 'b1'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(title, "New Title");
        assert_eq!(author, "New Author");
    }

    #[test]
    fn test_update_metadata_title_only() {
        let (_dir, db) = setup();
        insert_book(&db, "b1", "epub");

        let conn = db.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE books SET title = ?1, author = ?2, updated_at = ?3 WHERE id = ?4",
            params!["Changed Title", "Author", now, "b1"],
        )
        .unwrap();

        let (title, author): (String, String) = conn
            .query_row("SELECT title, author FROM books WHERE id = 'b1'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(title, "Changed Title");
        assert_eq!(author, "Author"); // unchanged (same value passed)
    }

    #[test]
    fn test_update_metadata_updates_timestamp() {
        let (_dir, db) = setup();
        insert_book(&db, "b1", "epub");

        let conn = db.conn.lock().unwrap();
        let before: i64 = conn
            .query_row("SELECT updated_at FROM books WHERE id = 'b1'", [], |r| {
                r.get(0)
            })
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));
        let now = chrono::Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE books SET title = ?1, author = ?2, updated_at = ?3 WHERE id = ?4",
            params!["New", "New", now, "b1"],
        )
        .unwrap();

        let after: i64 = conn
            .query_row("SELECT updated_at FROM books WHERE id = 'b1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn test_update_metadata_nonexistent_book() {
        let (_dir, db) = setup();

        let conn = db.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        let rows = conn
            .execute(
                "UPDATE books SET title = ?1, author = ?2, updated_at = ?3 WHERE id = ?4",
                params!["Title", "Author", now, "nonexistent"],
            )
            .unwrap();
        assert_eq!(rows, 0); // no rows affected
    }

    #[test]
    fn test_get_book_returns_format() {
        let (_dir, db) = setup();
        insert_book(&db, "b1", "pdf");

        let conn = db.conn.lock().unwrap();
        let book: Book = conn.query_row(
            "SELECT id, title, author, description, cover_path, file_path, format, source_format, render_format, source_file_path, source_sha256, conversion_version, genre, pages, status, progress, current_cfi, created_at, updated_at, preparation_state, preparation_error FROM books WHERE id = 'b1'",
            [],
            |row| Ok(Book {
                id: row.get(0)?, title: row.get(1)?, author: row.get(2)?,
                description: row.get(3)?, cover_path: row.get(4)?, file_path: row.get(5)?,
                format: row.get(6)?, source_format: row.get(7)?, render_format: row.get(8)?,
                source_file_path: row.get(9)?, source_sha256: row.get(10)?, conversion_version: row.get::<_, Option<i32>>(11)?.unwrap_or(0),
                preparation_state: row.get(19)?, preparation_error: row.get(20)?,
                genre: row.get(12)?, pages: row.get(13)?, status: row.get(14)?, progress: row.get(15)?, current_cfi: row.get(16)?,
                created_at: row.get(17)?, updated_at: row.get(18)?, available: true, cover_data: None,
            }),
        ).unwrap();

        assert_eq!(book.format, "pdf");
        assert_eq!(book.id, "b1");
    }

    // -----------------------------------------------------------------------
    // Pagination
    // -----------------------------------------------------------------------

    fn insert_book_with_ts(db: &Db, id: &str, status: &str, updated_at: i64) {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO books (id, title, author, file_path, format, status, progress, created_at, updated_at)
             VALUES (?1, ?2, 'Author', 'books/test.epub', 'epub', ?3, 0, ?4, ?4)",
            params![id, format!("Book {id}"), status, updated_at],
        ).unwrap();
    }

    #[test]
    fn pagination_returns_first_page() {
        let (_dir, db) = setup();
        for i in 0..5 {
            insert_book_with_ts(&db, &format!("b{i}"), "reading", 1000 + i);
        }
        let page = query_books(&db, None, None, None, None, 3).unwrap();
        assert_eq!(page.books.len(), 3);
        assert_eq!(page.total, 5);
        assert!(page.next_cursor.is_some());
    }

    #[test]
    fn pagination_cursor_returns_next_page() {
        let (_dir, db) = setup();
        for i in 0..5 {
            insert_book_with_ts(&db, &format!("b{i}"), "reading", 1000 + i);
        }
        let page1 = query_books(&db, None, None, None, None, 3).unwrap();
        let page2 = query_books(&db, None, None, None, page1.next_cursor.as_deref(), 3).unwrap();
        assert_eq!(page2.books.len(), 2);
        assert_eq!(page2.total, 5);
        assert!(page2.next_cursor.is_none());
    }

    #[test]
    fn pagination_no_more_pages() {
        let (_dir, db) = setup();
        for i in 0..3 {
            insert_book_with_ts(&db, &format!("b{i}"), "reading", 1000 + i);
        }
        let page = query_books(&db, None, None, None, None, 5).unwrap();
        assert_eq!(page.books.len(), 3);
        assert!(page.next_cursor.is_none());
    }

    #[test]
    fn pagination_filter_by_status() {
        let (_dir, db) = setup();
        insert_book_with_ts(&db, "b1", "reading", 1000);
        insert_book_with_ts(&db, "b2", "finished", 1001);
        insert_book_with_ts(&db, "b3", "reading", 1002);
        let page = query_books(&db, Some("reading"), None, None, None, 10).unwrap();
        assert_eq!(page.books.len(), 2);
        assert_eq!(page.total, 2);
    }

    #[test]
    fn pagination_search() {
        let (_dir, db) = setup();
        insert_book_with_ts(&db, "b1", "reading", 1000);
        insert_book_with_ts(&db, "b2", "reading", 1001);
        let page = query_books(&db, None, Some("Book b1"), None, None, 10).unwrap();
        assert_eq!(page.books.len(), 1);
        assert_eq!(page.books[0].id, "b1");
    }

    #[test]
    fn pagination_collection_filter() {
        let (_dir, db) = setup();
        insert_book_with_ts(&db, "b1", "reading", 1000);
        insert_book_with_ts(&db, "b2", "reading", 1001);
        insert_book_with_ts(&db, "b3", "reading", 1002);
        let conn = db.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO collections (id, name, sort_order, created_at, updated_at, updated_by_device)
             VALUES ('c1', 'Fiction', 0, ?1, ?1, 'dev')",
            params![now],
        ).unwrap();
        conn.execute(
            "INSERT INTO collection_books (collection_id, book_id, created_at, updated_at, updated_by_device)
             VALUES ('c1', 'b1', ?1, ?1, 'dev')",
            params![now],
        ).unwrap();
        conn.execute(
            "INSERT INTO collection_books (collection_id, book_id, created_at, updated_at, updated_by_device)
             VALUES ('c1', 'b3', ?1, ?1, 'dev')",
            params![now],
        ).unwrap();
        drop(conn);
        let page = query_books(&db, None, None, Some("c1"), None, 10).unwrap();
        assert_eq!(page.books.len(), 2);
        assert_eq!(page.total, 2);
    }

    #[test]
    fn pagination_collection_with_cursor() {
        let (_dir, db) = setup();
        for i in 0..5 {
            insert_book_with_ts(&db, &format!("b{i}"), "reading", 1000 + i);
        }
        let conn = db.conn.lock().unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        conn.execute(
            "INSERT INTO collections (id, name, sort_order, created_at, updated_at, updated_by_device)
             VALUES ('c1', 'All Five', 0, ?1, ?1, 'dev')",
            params![now],
        ).unwrap();
        for i in 0..5 {
            conn.execute(
                "INSERT INTO collection_books (collection_id, book_id, created_at, updated_at, updated_by_device)
                 VALUES ('c1', ?1, ?2, ?2, 'dev')",
                params![format!("b{i}"), now],
            ).unwrap();
        }
        drop(conn);
        let page1 = query_books(&db, None, None, Some("c1"), None, 3).unwrap();
        assert_eq!(page1.books.len(), 3);
        assert_eq!(page1.total, 5);
        assert!(page1.next_cursor.is_some());
        let page2 =
            query_books(&db, None, None, Some("c1"), page1.next_cursor.as_deref(), 3).unwrap();
        assert_eq!(page2.books.len(), 2);
        assert!(page2.next_cursor.is_none());
    }

    /// Build a minimal one-page PDF on disk. The page has a single
    /// filled rectangle so pdfium has something visible to rasterize —
    /// otherwise an empty page is technically valid but bitmap output
    /// could be all-white in a way that depends on background fill
    /// semantics we don't want to depend on.
    fn write_fixture_pdf(path: &std::path::Path) {
        use lopdf::content::{Content, Operation};
        use lopdf::{dictionary, Document, Object, Stream};

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let content = Content {
            operations: vec![
                Operation::new("q", vec![]),
                Operation::new("rg", vec![0.2.into(), 0.4.into(), 0.8.into()]),
                Operation::new("re", vec![100.into(), 100.into(), 200.into(), 300.into()]),
                Operation::new("f", vec![]),
                Operation::new("Q", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
        });
        let pages = dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Resources" => dictionary! {},
        };
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        doc.trailer.set("Root", catalog_id);
        doc.save(path).unwrap();
    }

    #[test]
    fn extract_pdf_renders_cover_for_valid_pdf() {
        let dir = TempDir::new().unwrap();
        let pdf = dir.path().join("fixture.pdf");
        write_fixture_pdf(&pdf);

        let out = extract_pdf(&pdf, "fallback");
        let cover = out.cover.expect("cover should be populated");
        // JPEG magic: FF D8 FF
        assert_eq!(&cover[..3], &[0xFF, 0xD8, 0xFF]);
        let decoded = image::load_from_memory(&cover).expect("output decodes as image");
        assert!(decoded.width() > 100 && decoded.height() > 100);
        assert_eq!(out.pages, 1);
    }

    #[test]
    fn extract_pdf_returns_fallback_for_corrupt_pdf() {
        let dir = TempDir::new().unwrap();
        let bogus = dir.path().join("bogus.pdf");
        std::fs::write(&bogus, b"this is not a PDF").unwrap();
        let out = extract_pdf(&bogus, "myfile");
        assert_eq!(out.title, "myfile");
        assert_eq!(out.author, "Unknown Author");
        assert_eq!(out.pages, 0);
        assert!(out.cover.is_none());
    }

    #[test]
    fn extract_pdf_returns_fallback_for_missing_file() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("nope.pdf");
        let out = extract_pdf(&missing, "myfile");
        assert_eq!(out.title, "myfile");
        assert!(out.cover.is_none());
    }

    #[test]
    fn do_import_pdf_populates_cover_data() {
        let (dir, db) = setup();
        let pdf = dir.path().join("fixture.pdf");
        write_fixture_pdf(&pdf);

        let sync = SyncWriter::new("dev-A".into());
        let book = do_import_pdf(pdf.to_str().unwrap(), &db, &sync).unwrap();
        assert_eq!(book.format, "pdf");
        assert!(book.cover_data.is_some(), "cover_data should be populated");

        let conn = db.conn.lock().unwrap();
        let stored: Option<Vec<u8>> = conn
            .query_row(
                "SELECT cover_data FROM books WHERE id = ?1",
                params![book.id],
                |r| r.get(0),
            )
            .unwrap();
        let bytes = stored.expect("cover_data BLOB present");
        assert_eq!(&bytes[..3], &[0xFF, 0xD8, 0xFF], "stored bytes are JPEG");
    }

    #[test]
    fn delete_book_removes_book_notes_and_markers_but_detaches_global_notes() {
        let (_dir, db) = setup();
        insert_book(&db, "b1", "epub");
        let now = chrono::Utc::now().timestamp_millis();
        let marker_id = crate::sync::events::word_mark_rule_id("b1", "term", "exact");
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO notes
                 (id, book_id, anchor_kind, normalized_word, scope, location, selected_text,
                  content, content_format, created_at, updated_at)
                 VALUES ('note-book', 'b1', 'word', 'term', 'book', NULL, 'term',
                         'book note', 'plain_text', ?1, ?1)",
                params![now],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO notes
                 (id, book_id, anchor_kind, normalized_word, scope, location, selected_text,
                  content, content_format, created_at, updated_at)
                 VALUES ('note-global', 'b1', 'word', 'term', 'global', NULL, 'term',
                         'global note', 'plain_text', ?1, ?1)",
                params![now],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO word_mark_rules
                 (id, book_id, normalized_word, display_word, match_mode, color, enabled,
                  created_at, updated_at)
                 VALUES (?1, 'b1', 'term', 'Term', 'exact', 'lookup', 1, ?2, ?2)",
                params![marker_id, now],
            )
            .unwrap();
        }

        let sync = SyncWriter::new("dev-A".into());
        do_delete_book("b1", &db, &sync).unwrap();

        let conn = db.conn.lock().unwrap();
        let notes: Vec<(String, Option<String>)> = conn
            .prepare("SELECT id, book_id FROM notes ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(notes, vec![("note-global".into(), None)]);
        let marker_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM word_mark_rules", [], |row| row.get(0))
            .unwrap();
        assert_eq!(marker_count, 0);
    }

    #[test]
    fn delete_book_can_preserve_book_notes_as_detached_material() {
        let (_dir, db) = setup();
        insert_book(&db, "b1", "epub");
        let now = chrono::Utc::now().timestamp_millis();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO notes
                 (id, book_id, anchor_kind, normalized_word, scope, location, selected_text,
                  content, content_format, created_at, updated_at)
                 VALUES ('note-book', 'b1', 'selection', NULL, 'book', 'epubcfi(/6/4!)',
                         'quoted passage', 'my note', 'plain_text', ?1, ?1)",
                params![now],
            )
            .unwrap();
        }

        let sync = SyncWriter::new("dev-A".into());
        do_delete_book_with_note_policy("b1", true, &db, &sync).unwrap();

        let conn = db.conn.lock().unwrap();
        let note: (Option<String>, String, Option<String>, String) = conn
            .query_row(
                "SELECT book_id, scope, location, selected_text FROM notes WHERE id = 'note-book'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            note,
            (None, "detached".into(), None, "quoted passage".into())
        );
    }
}
