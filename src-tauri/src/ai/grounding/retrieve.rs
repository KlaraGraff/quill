use std::collections::{BTreeMap, HashMap};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::chunk::estimate_tokens;
use super::segment::{segment_for_fts, SegmentMode};
use super::RETRIEVAL_TOP_K;
use crate::error::AppResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpoilerCutoff {
    Character(i64),
    Section(i64),
}

impl SpoilerCutoff {
    pub fn allows(self, section_index: i64, char_start: Option<i64>) -> bool {
        match self {
            Self::Character(offset) => char_start.is_some_and(|start| start <= offset),
            Self::Section(section) => section_index <= section,
        }
    }

    pub fn allows_section_summary(self, section_index: i64, char_end: Option<i64>) -> bool {
        match self {
            Self::Character(offset) => char_end.is_some_and(|end| end <= offset),
            Self::Section(section) => section_index <= section,
        }
    }

    fn sql_parts(self) -> (i64, i64) {
        match self {
            Self::Character(offset) => (1, offset),
            Self::Section(section) => (2, section),
        }
    }
}

fn cutoff_sql_parts(cutoff: Option<SpoilerCutoff>) -> (i64, i64) {
    cutoff.map(SpoilerCutoff::sql_parts).unwrap_or((0, 0))
}

#[derive(Debug, Clone, PartialEq)]
pub struct RetrievedChunk {
    pub chunk_id: String,
    pub chunk_index: i64,
    pub section_index: i64,
    pub section_href: Option<String>,
    pub section_title: Option<String>,
    pub char_start: Option<i64>,
    pub char_end: Option<i64>,
    pub snippet: String,
    pub text: String,
    pub token_estimate: usize,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CitedSource {
    pub marker: String,
    pub chunk_id: String,
    pub section_index: i64,
    pub section_href: Option<String>,
    pub section_title: Option<String>,
    pub snippet: String,
    pub char_start: Option<i64>,
    pub char_end: Option<i64>,
}

impl RetrievedChunk {
    pub fn cited_source(&self, marker: String) -> CitedSource {
        CitedSource {
            marker,
            chunk_id: self.chunk_id.clone(),
            section_index: self.section_index,
            section_href: self.section_href.clone(),
            section_title: self.section_title.clone(),
            snippet: self.snippet.clone(),
            char_start: self.char_start,
            char_end: self.char_end,
        }
    }
}

fn fts_query(query_text: &str) -> String {
    segment_for_fts(query_text, SegmentMode::Query)
        .split_whitespace()
        .filter(|token| token.len() >= 2)
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn row_to_chunk(row: &rusqlite::Row<'_>, score: f64) -> rusqlite::Result<RetrievedChunk> {
    Ok(RetrievedChunk {
        chunk_id: row.get(0)?,
        chunk_index: row.get(1)?,
        section_index: row.get(2)?,
        section_href: row.get(3)?,
        section_title: row.get(4)?,
        char_start: row.get(5)?,
        char_end: row.get(6)?,
        text: row.get(7)?,
        snippet: row.get(8)?,
        token_estimate: row.get::<_, i64>(9)? as usize,
        score,
    })
}

fn truncate_to_budget(value: &str, budget: usize) -> String {
    if estimate_tokens(value) <= budget {
        return value.to_string();
    }
    let mut end = 0;
    for (index, character) in value.char_indices() {
        let next = index + character.len_utf8();
        if estimate_tokens(&value[..next]) > budget {
            break;
        }
        end = next;
    }
    value[..end].trim_end().to_string()
}

/// Query FTS5, add immediate reading-order neighbors, then merge and budget
/// excerpts. Lower SQLite BM25 scores are better.
pub(crate) fn lexical_ranks(
    conn: &Connection,
    book_id: &str,
    query_text: &str,
    cutoff: Option<SpoilerCutoff>,
) -> AppResult<Vec<(String, f64)>> {
    let query = fts_query(query_text);
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let (cutoff_kind, cutoff_value) = cutoff_sql_parts(cutoff);
    let hits = conn
        .prepare(
            "SELECT book_chunks_fts.chunk_id, bm25(book_chunks_fts) AS score
             FROM book_chunks_fts
             JOIN book_chunks ON book_chunks.id = book_chunks_fts.chunk_id
               AND book_chunks.book_id = book_chunks_fts.book_id
             WHERE book_chunks_fts MATCH ?1 AND book_chunks_fts.book_id = ?2
               AND (?3 = 0
                 OR (?3 = 1 AND book_chunks.char_start <= ?4)
                 OR (?3 = 2 AND book_chunks.section_index <= ?4))
             ORDER BY score LIMIT ?5",
        )?
        .query_map(
            params![
                query,
                book_id,
                cutoff_kind,
                cutoff_value,
                RETRIEVAL_TOP_K as i64
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    if hits.is_empty() {
        return Ok(Vec::new());
    }
    Ok(hits)
}

pub(crate) fn retrieve_ranked(
    conn: &Connection,
    book_id: &str,
    hits: &[(String, f64)],
    budget_tokens: usize,
    cutoff: Option<SpoilerCutoff>,
) -> AppResult<Vec<RetrievedChunk>> {
    if hits.is_empty() {
        return Ok(Vec::new());
    }

    let mut chunks_by_id = HashMap::new();
    {
        let mut statement = conn.prepare(
            "SELECT id, chunk_index, section_index, section_href, section_title, char_start, char_end, text, snippet, token_estimate
             FROM book_chunks WHERE id = ?1 AND book_id = ?2",
        )?;
        for (id, score) in hits {
            let chunk = statement
                .query_row(params![id, book_id], |row| row_to_chunk(row, *score))
                .optional()?;
            let Some(chunk) = chunk else {
                continue;
            };
            if cutoff.is_some_and(|value| !value.allows(chunk.section_index, chunk.char_start)) {
                continue;
            }
            chunks_by_id.insert(id.clone(), chunk);
        }
    }

    let mut expanded_scores: BTreeMap<i64, f64> = BTreeMap::new();
    for (id, score) in hits {
        if let Some(hit) = chunks_by_id.get(id) {
            for index in (hit.chunk_index - 1)..=(hit.chunk_index + 1) {
                if index >= 0 {
                    expanded_scores
                        .entry(index)
                        .and_modify(|old| *old = old.min(*score))
                        .or_insert(*score);
                }
            }
        }
    }
    // Retain score lookup by id to avoid relying on the uniqueness of BM25 values.
    let hit_scores = hits.iter().cloned().collect::<HashMap<_, _>>();
    let mut candidates: BTreeMap<i64, RetrievedChunk> = BTreeMap::new();
    let mut statement = conn.prepare(
        "SELECT id, chunk_index, section_index, section_href, section_title, char_start, char_end, text, snippet, token_estimate
         FROM book_chunks WHERE book_id = ?1 AND chunk_index = ?2",
    )?;
    for (index, fallback_score) in expanded_scores {
        let maybe_chunk = statement
            .query_row(params![book_id, index], |row| {
                row_to_chunk(row, fallback_score)
            })
            .optional()?;
        let Some(mut chunk) = maybe_chunk else {
            continue;
        };
        if cutoff.is_some_and(|value| !value.allows(chunk.section_index, chunk.char_start)) {
            continue;
        }
        if let Some(score) = hit_scores.get(&chunk.chunk_id) {
            chunk.score = *score;
        }
        candidates.insert(index, chunk);
    }
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    let mut merged = Vec::new();
    let mut current: Option<RetrievedChunk> = None;
    let mut current_last_index: Option<i64> = None;
    for chunk in candidates.into_values() {
        match current.as_mut() {
            Some(existing) if current_last_index == Some(chunk.chunk_index - 1) => {
                existing.text.push('\n');
                existing.text.push_str(&chunk.text);
                existing.token_estimate += chunk.token_estimate;
                existing.score = existing.score.min(chunk.score);
                existing.char_start = match (existing.char_start, chunk.char_start) {
                    (Some(left), Some(right)) => Some(left.min(right)),
                    (left, right) => left.or(right),
                };
                existing.char_end = match (existing.char_end, chunk.char_end) {
                    (Some(left), Some(right)) => Some(left.max(right)),
                    (left, right) => left.or(right),
                };
                current_last_index = Some(chunk.chunk_index);
            }
            Some(_) => {
                merged.push(current.take().expect("current exists"));
                current_last_index = Some(chunk.chunk_index);
                current = Some(chunk);
            }
            None => {
                current_last_index = Some(chunk.chunk_index);
                current = Some(chunk);
            }
        }
    }
    if let Some(chunk) = current {
        merged.push(chunk);
    }

    let best_chunk_id = merged
        .iter()
        .min_by(|left, right| left.score.total_cmp(&right.score))
        .map(|chunk| chunk.chunk_id.clone())
        .unwrap_or_default();
    let mut total = merged
        .iter()
        .map(|chunk| chunk.token_estimate)
        .sum::<usize>();
    while merged.len() > 1 && total > budget_tokens {
        let worst = merged
            .iter()
            .enumerate()
            .filter(|(_, chunk)| chunk.chunk_id != best_chunk_id)
            .max_by(|(_, left), (_, right)| left.score.total_cmp(&right.score))
            .map(|(index, _)| index)
            .unwrap_or_else(|| merged.len() - 1);
        total -= merged[worst].token_estimate;
        merged.remove(worst);
    }
    if merged.len() == 1 && merged[0].token_estimate > budget_tokens {
        merged[0].text = truncate_to_budget(&merged[0].text, budget_tokens);
        merged[0].token_estimate = estimate_tokens(&merged[0].text);
    }
    merged.sort_by_key(|chunk| chunk.chunk_index);
    Ok(merged)
}

pub fn retrieve(
    conn: &Connection,
    book_id: &str,
    query_text: &str,
    budget_tokens: usize,
    cutoff: Option<SpoilerCutoff>,
) -> AppResult<Vec<RetrievedChunk>> {
    let hits = lexical_ranks(conn, book_id, query_text, cutoff)?;
    retrieve_ranked(conn, book_id, &hits, budget_tokens, cutoff)
}

pub fn total_book_tokens(conn: &Connection, book_id: &str) -> AppResult<usize> {
    let total: i64 = conn.query_row(
        "SELECT COALESCE(SUM(token_estimate), 0) FROM book_chunks WHERE book_id = ?1",
        params![book_id],
        |row| row.get(0),
    )?;
    Ok(total.max(0) as usize)
}

pub fn retrieve_all(
    conn: &Connection,
    book_id: &str,
    cutoff: Option<SpoilerCutoff>,
) -> AppResult<Vec<RetrievedChunk>> {
    let mut statement = conn.prepare(
        "SELECT id, chunk_index, section_index, section_href, section_title, char_start, char_end,
                text, snippet, token_estimate
         FROM book_chunks WHERE book_id = ?1 ORDER BY chunk_index",
    )?;
    let chunks = statement
        .query_map(params![book_id], |row| row_to_chunk(row, 0.0))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(crate::error::AppError::from)?;
    Ok(chunks
        .into_iter()
        .filter(|chunk| {
            cutoff.is_none_or(|value| value.allows(chunk.section_index, chunk.char_start))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::grounding::segment::{segment_for_fts, SegmentMode};

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE book_chunks (id TEXT PRIMARY KEY, book_id TEXT, chunk_index INTEGER, section_index INTEGER, section_href TEXT, section_title TEXT, char_start INTEGER, char_end INTEGER, text TEXT, snippet TEXT, token_estimate INTEGER); CREATE VIRTUAL TABLE book_chunks_fts USING fts5(seg_text, chunk_id UNINDEXED, book_id UNINDEXED);").unwrap();
        for (index, text) in [
            "Alpha setup.",
            "The rare signal appears here.",
            "Neighbor context.",
            "Unrelated material.",
            "宝玉 appears in a Chinese name.",
        ]
        .iter()
        .enumerate()
        {
            let id = format!("c{index}");
            conn.execute("INSERT INTO book_chunks VALUES (?1, 'book', ?2, 0, NULL, 'Chapter', ?2, ?2, ?3, ?3, 20)", params![id, index as i64, text]).unwrap();
            conn.execute(
                "INSERT INTO book_chunks_fts VALUES (?1, ?2, 'book')",
                params![segment_for_fts(text, SegmentMode::Index), id],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn retrieves_hit_with_neighbors_and_merges_by_reading_order() {
        let result = retrieve(&setup(), "book", "rare signal", 500, None).unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].text.contains("Alpha setup."));
        assert!(result[0].text.contains("Neighbor context."));
    }

    #[test]
    fn supports_two_character_chinese_queries() {
        let result = retrieve(&setup(), "book", "宝玉", 500, None).unwrap();
        assert!(result.iter().any(|chunk| chunk.text.contains("宝玉")));
    }

    #[test]
    fn empty_and_non_matching_queries_are_empty() {
        let conn = setup();
        assert!(retrieve(&conn, "book", "", 100, None).unwrap().is_empty());
        assert!(retrieve(&conn, "book", "not-present", 100, None)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn returns_all_chunks_in_reading_order() {
        let result = retrieve_all(&setup(), "book", None).unwrap();
        assert_eq!(result.len(), 5);
        assert_eq!(result[0].chunk_id, "c0");
        assert_eq!(total_book_tokens(&setup(), "book").unwrap(), 100);
    }

    #[test]
    fn lexical_hits_and_neighbors_respect_character_cutoff() {
        let conn = setup();
        let result = retrieve(
            &conn,
            "book",
            "rare signal",
            500,
            Some(SpoilerCutoff::Character(1)),
        )
        .unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].text.contains("rare signal"));
        assert!(!result[0].text.contains("Neighbor context"));

        let blocked = retrieve(
            &conn,
            "book",
            "Neighbor context",
            500,
            Some(SpoilerCutoff::Character(1)),
        )
        .unwrap();
        assert!(blocked.is_empty());
    }

    #[test]
    fn full_text_retrieval_respects_character_cutoff() {
        let result = retrieve_all(&setup(), "book", Some(SpoilerCutoff::Character(2))).unwrap();
        assert_eq!(
            result
                .iter()
                .map(|chunk| chunk.chunk_id.as_str())
                .collect::<Vec<_>>(),
            vec!["c0", "c1", "c2"]
        );
    }

    #[test]
    fn section_cutoff_filters_hits_neighbors_and_full_text() {
        let conn = setup();
        conn.execute("UPDATE book_chunks SET section_index = chunk_index", [])
            .unwrap();
        let result = retrieve(
            &conn,
            "book",
            "rare signal",
            500,
            Some(SpoilerCutoff::Section(1)),
        )
        .unwrap();
        assert_eq!(result.len(), 1);
        assert!(result[0].text.contains("rare signal"));
        assert!(!result[0].text.contains("Neighbor context"));

        let all = retrieve_all(&conn, "book", Some(SpoilerCutoff::Section(1))).unwrap();
        assert_eq!(
            all.iter()
                .map(|chunk| chunk.chunk_id.as_str())
                .collect::<Vec<_>>(),
            vec!["c0", "c1"]
        );
    }
}
