use super::*;

#[derive(Debug, Clone)]
pub(super) struct TextLine {
    pub(super) text: String,
    pub(super) source_start: u64,
    pub(super) source_end: u64,
    pub(super) separator_before: Option<u64>,
    pub(super) paragraph_break_before: bool,
    pub(super) leading_whitespace: usize,
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

pub(super) fn utf16_len(value: &str) -> u64 {
    value.encode_utf16().count() as u64
}

pub(super) fn normalized_text_lines(text: &str) -> Vec<TextLine> {
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

pub(super) fn legacy_text_locations(lines: &[TextLine]) -> Vec<Vec<u64>> {
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

pub(super) fn canonical_roman_number(value: &str) -> bool {
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

fn simple_number_word_kind(value: &str) -> Option<u8> {
    match value {
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

fn number_word_kind(value: &str) -> Option<u8> {
    let cleaned = trim_heading_separator(value).to_ascii_uppercase();
    simple_number_word_kind(&cleaned).or_else(|| {
        let (tens, units) = cleaned.split_once('-')?;
        (!units.contains('-')
            && simple_number_word_kind(tens) == Some(3)
            && simple_number_word_kind(units) == Some(2))
        .then_some(1)
    })
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

pub(super) fn hard_wrap_width(lines: &[TextLine], depths: &[Option<u8>]) -> Option<usize> {
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

pub(super) fn block_from_line(
    line: &TextLine,
    kind: TextBookBlockKind,
    depth: Option<u8>,
    starts_page: bool,
) -> TextBookBlock {
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
        starts_page,
    }
}

pub(super) fn append_reflowed_line(block: &mut TextBookBlock, line: &TextLine) {
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

pub(super) fn text_document_parts(
    text: &str,
    reflow_hard_wraps: bool,
) -> (Vec<TextBookChunk>, Vec<TextBookTocEntry>, Vec<Vec<u64>>) {
    let lines = normalized_text_lines(text);
    let legacy_locations = legacy_text_locations(&lines);
    let mut toc = Vec::new();
    let mut heading_stack = Vec::<HeadingContext>::new();
    let mut depths = Vec::with_capacity(lines.len());
    let mut page_starts = Vec::with_capacity(lines.len());

    for line in &lines {
        let heading = parse_heading(
            &line.text,
            heading_stack
                .iter()
                .any(|context| context.rank < HeadingRank::Chapter),
            line.paragraph_break_before,
        );
        let (depth, starts_page) = match heading {
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
                    (Some(child_depth), child_rank <= HeadingRank::Chapter)
                } else {
                    (Some(parent_depth), rank <= HeadingRank::Chapter)
                }
            }
            Some(ParsedHeading::Child { title, rank }) => {
                let (depth, _) = enter_heading_context(&mut heading_stack, rank, &title);
                toc.push(TextBookTocEntry {
                    title,
                    depth,
                    source_offset: line.source_start,
                });
                (Some(depth), rank <= HeadingRank::Chapter)
            }
            Some(ParsedHeading::TopLevel(title)) => {
                heading_stack.clear();
                toc.push(TextBookTocEntry {
                    title,
                    depth: 0,
                    source_offset: line.source_start,
                });
                (Some(0), true)
            }
            None => (None, false),
        };
        depths.push(depth);
        page_starts.push(starts_page);
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
    for (index, ((line, depth), starts_page)) in
        lines.iter().zip(depths).zip(page_starts).enumerate()
    {
        if let Some(depth) = depth {
            if let Some(paragraph) = paragraph.take() {
                blocks.push(paragraph);
            }
            blocks.push(block_from_line(
                line,
                TextBookBlockKind::Heading,
                Some(depth),
                starts_page,
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
            if let Some(paragraph) = paragraph.replace(block_from_line(
                line,
                TextBookBlockKind::Paragraph,
                None,
                false,
            )) {
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
