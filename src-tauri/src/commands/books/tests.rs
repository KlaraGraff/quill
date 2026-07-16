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
    let (chunks, toc, _) = text_document_parts(text, true);
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
    let page_start_titles = chunks
        .iter()
        .flat_map(|chunk| &chunk.blocks)
        .filter(|block| block.starts_page)
        .map(|block| block.text.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        page_start_titles,
        [
            "VOLUME I",
            "BOOK ONE",
            "PART ONE",
            "CHAPTER 1",
            "ACT II",
            "SCENE 1",
            "EPILOGUE",
        ]
    );
    assert!(!page_start_titles.contains(&"SECTION 1"));

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
fn text_parser_accepts_valid_hyphenated_word_numbers_only() {
    let text = "BOOK ONE\nCHAPTER TWENTY-ONE\nBody\nCHAPTER ONE HUNDRED TWENTY-THREE\nMore\nCHAPTER TWENTY-TEN\nProse\nCHAPTER TWENTY--ONE\nEnd";
    let (_, toc, _) = text_document_parts(text, true);
    let entries = toc
        .iter()
        .map(|entry| (entry.title.as_str(), entry.depth))
        .collect::<Vec<_>>();

    assert_eq!(
        entries,
        [
            ("BOOK ONE", 0),
            ("CHAPTER TWENTY-ONE", 1),
            ("CHAPTER ONE HUNDRED TWENTY-THREE", 1),
        ]
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
    let mut cjk = block_from_line(&cjk_lines[0], TextBookBlockKind::Paragraph, None, false);
    append_reflowed_line(&mut cjk, &cjk_lines[1]);
    assert_eq!(cjk.text, "中文换行继续阅读");

    let hyphen_lines = normalized_text_lines("well-\nknown");
    let mut hyphen = block_from_line(&hyphen_lines[0], TextBookBlockKind::Paragraph, None, false);
    append_reflowed_line(&mut hyphen, &hyphen_lines[1]);
    assert_eq!(hyphen.text, "well-known");
}

#[test]
fn text_parser_keeps_complete_lines_as_separate_paragraphs() {
    let text = (0..12)
        .map(|index| {
            format!("This is complete paragraph number {index}, with a natural sentence ending.")
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
                starts_page: false,
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
    assert!(path.ends_with("book-id.v3.json"));

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

#[test]
fn prepared_document_recovery_fails_when_sidecar_cannot_be_published() {
    let dir = TempDir::new().unwrap();
    let document = sample_text_document("abc");
    let path = prepared_document_path(dir.path(), "book-id");
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("occupied"), b"keep target non-empty").unwrap();

    let temporary_path = prepared_document_temporary_path(&path).unwrap();
    fs::write(&temporary_path, serde_json::to_vec(&document).unwrap()).unwrap();

    assert!(load_prepared_document(&path, Some("abc")).is_none());
    assert!(path.is_dir());
    assert!(temporary_path.is_file());
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

    assert!(transition_text_preparation_state(&db, "retry", "failed", "pending", None,).unwrap());
    assert!(!transition_text_preparation_state(&db, "retry", "failed", "pending", None,).unwrap());
    assert!(!transition_text_preparation_state(&db, "retry", "ready", "pending", None,).unwrap());

    db.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE books SET preparation_state = 'ready' WHERE id = 'retry'",
            [],
        )
        .unwrap();
    assert!(transition_text_preparation_state(&db, "retry", "ready", "pending", None,).unwrap());
    assert!(!transition_text_preparation_state(&db, "retry", "ready", "pending", None,).unwrap());
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
    )
    .unwrap();

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
    )
    .unwrap();
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
    )
    .unwrap();
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
    let page2 = query_books(&db, None, None, Some("c1"), page1.next_cursor.as_deref(), 3).unwrap();
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
fn pdf_with_reported_long_cjk_name_finishes_import_and_indexing() {
    let (dir, db) = setup();
    let title = "被讨厌的勇气：“自我启发之父”阿德勒的哲学课 = 嫌われる勇気：自己啓発の源流「アドラー」の教え ([曰] 岸见一郎，[日] 古贺史健 著；渠海霞 译) (z-library.sk, 1lib.sk, z-lib.sk)(1)";
    let pdf = dir.path().join(format!("{title}.pdf"));
    write_fixture_pdf(&pdf);

    let sync = SyncWriter::new("dev-A".into());
    let book = do_import_from_path(pdf.to_str().unwrap(), &db, &sync).unwrap();

    assert_eq!(book.title, title);
    assert_eq!(book.pages, Some(1));
    assert!(db.resolve_path(&book.file_path).unwrap().is_file());
    assert_eq!(
        crate::ai::grounding::index::ensure_index(&db, &book.id).unwrap(),
        crate::ai::grounding::index::IndexStatus::Ready,
    );
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

#[test]
fn delete_book_command_persists_book_and_chat_tombstones() {
    let (_dir, db) = setup();
    insert_book(&db, "b1", "epub");
    let now = chrono::Utc::now().timestamp_millis();
    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO highlights
             (id, book_id, cfi_range, color, created_at, updated_at)
             VALUES ('h1', 'b1', 'epubcfi(/6/2!)', 'yellow', ?1, ?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO bookmarks (id, book_id, cfi, created_at, updated_at)
             VALUES ('bm1', 'b1', 'epubcfi(/6/2!)', ?1, ?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO vocab_words
             (id, book_id, word, definition, created_at, updated_at)
             VALUES ('v1', 'b1', 'term', 'definition', ?1, ?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO lookup_records
             (id, book_id, lookup_text, normalized_text, definition,
              created_at, last_looked_up_at)
             VALUES ('lookup1', 'b1', 'Term', 'term', 'definition', ?1, ?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO collections
             (id, name, sort_order, created_at, updated_at)
             VALUES ('c1', 'Collection', 0, ?1, ?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO collection_books
             (collection_id, book_id, created_at, updated_at)
             VALUES ('c1', 'b1', ?1, ?1)",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO book_settings (book_id, key, value)
             VALUES ('b1', 'font_size', '18')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO word_mark_rules
             (id, book_id, normalized_word, display_word, match_mode, color,
              enabled, created_at, updated_at, updated_by_device)
             VALUES ('rule1', 'b1', 'term', 'Term', 'exact', 'lookup', 1,
                     ?1, ?1, 'dev-A')",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO word_mark_exceptions
             (id, rule_id, book_id, normalized_word, location, excluded,
              created_at, updated_at, updated_by_device)
             VALUES ('exception1', 'rule1', 'b1', 'term', 'location', 1,
                     ?1, ?1, 'dev-A')",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chats
             (id, book_id, title, pinned, created_at, updated_at, updated_by_device)
             VALUES ('ch1', 'b1', 'Chat', 0, ?1, ?1, 'dev-A')",
            params![now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chat_messages
             (id, chat_id, role, content, created_at, updated_at)
             VALUES ('m1', 'ch1', 'user', 'hello', ?1, ?1)",
            params![now],
        )
        .unwrap();
    }

    let sync = SyncWriter::new("dev-A".into());
    do_delete_book("b1", &db, &sync).unwrap();

    let conn = db.conn.lock().unwrap();
    let tombstones: Vec<(String, String, i64)> = conn
        .prepare("SELECT entity, id, ts FROM _tombstones ORDER BY entity, id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(tombstones.len(), 2);
    assert_eq!(&tombstones[0].0, "book");
    assert_eq!(&tombstones[0].1, "b1");
    assert_eq!(&tombstones[1].0, "chat");
    assert_eq!(&tombstones[1].1, "ch1");
    assert_eq!(
        tombstones[0].2, tombstones[1].2,
        "the cascaded chat tombstone must use the book-delete timestamp"
    );

    for table in [
        "books",
        "highlights",
        "bookmarks",
        "vocab_words",
        "lookup_records",
        "collection_books",
        "book_settings",
        "word_mark_rules",
        "word_mark_exceptions",
        "chats",
        "chat_messages",
    ] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "book deletion must clear {table}");
    }
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM collections", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1,
        "deleting a book must not delete its collection"
    );
}
