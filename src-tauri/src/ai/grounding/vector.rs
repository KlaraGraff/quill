use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use super::retrieve::{lexical_ranks, retrieve_ranked, RetrievedChunk, SpoilerCutoff};
use super::RETRIEVAL_TOP_K;
use crate::ai::router;
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::secrets::Secrets;

pub const DEFAULT_EMBEDDING_MODEL: &str = "text-embedding-3-small";
pub const DEFAULT_EMBEDDING_DIMENSIONS: usize = 1_536;
pub const EMBEDDING_SECRET_REF: &str = "ai_embedding_api_key";
const RRF_K: f64 = 60.0;
const EMBEDDING_BATCH_SIZE: usize = 32;

#[derive(Clone)]
pub(crate) struct EmbeddingSource {
    pub(crate) profile_id: String,
    pub(crate) endpoint: String,
    pub(crate) model: String,
    pub(crate) api_key: Option<String>,
    pub(crate) dimensions: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VectorAvailability {
    pub available: bool,
    pub reason: Option<String>,
    pub dimensions: Option<usize>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingProbeResult {
    pub ok: bool,
    pub dimensions: usize,
    pub latency_ms: u64,
    pub error: Option<String>,
}

fn configured_dimensions(conn: &Connection) -> usize {
    conn.query_row(
        "SELECT value FROM settings WHERE key = 'ai_embedding_dimensions'",
        [],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .and_then(|value| value.parse::<usize>().ok())
    .filter(|value| (1..=65_536).contains(value))
    .unwrap_or(DEFAULT_EMBEDDING_DIMENSIONS)
}

pub fn ensure_configured_vector_table(conn: &Connection) -> AppResult<()> {
    ensure_vector_table(conn, configured_dimensions(conn))
}

pub fn ensure_vector_table(conn: &Connection, dimensions: usize) -> AppResult<()> {
    if !(1..=65_536).contains(&dimensions) {
        return Err(AppError::Other(
            "AI_EMBEDDING_DIMENSIONS_UNSUPPORTED".to_string(),
        ));
    }
    let existing: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'book_chunk_vectors'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let expected = format!("float[{dimensions}]");
    let rebuild = existing.is_none()
        || existing
            .as_deref()
            .is_some_and(|sql| !sql.contains(&expected));
    if existing
        .as_deref()
        .is_some_and(|sql| !sql.contains(&expected))
    {
        conn.execute_batch("DROP TABLE book_chunk_vectors;")?;
    }
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS book_chunk_vectors USING vec0(
            chunk_id TEXT PRIMARY KEY,
            book_id TEXT,
            embedding float[{dimensions}]
        );"
    ))?;
    if rebuild {
        let model = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'ai_embedding_model'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap_or_else(|_| DEFAULT_EMBEDDING_MODEL.to_string());
        let mut statement = conn.prepare(
            "SELECT chunk_id, book_id, embedding FROM book_chunk_embeddings
             WHERE dimensions = ?1 AND model = ?2",
        )?;
        let rows = statement
            .query_map(params![dimensions as i64, model], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        for (chunk_id, book_id, blob) in rows {
            if blob.len() != dimensions * 4 {
                continue;
            }
            let vector = blob
                .chunks_exact(4)
                .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("four bytes")))
                .collect::<Vec<_>>();
            conn.execute(
                "INSERT INTO book_chunk_vectors (chunk_id, book_id, embedding) VALUES (?1, ?2, ?3)",
                params![chunk_id, book_id, embedding_json(&vector)?],
            )?;
        }
    }
    Ok(())
}

fn embedding_json(embedding: &[f32]) -> AppResult<String> {
    serde_json::to_string(embedding).map_err(|error| AppError::Other(error.to_string()))
}

fn embedding_blob(embedding: &[f32]) -> Vec<u8> {
    embedding
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn validate_embedding(embedding: &[f32], dimensions: usize) -> AppResult<()> {
    if embedding.len() != dimensions || embedding.iter().any(|value| !value.is_finite()) {
        return Err(AppError::Other(
            "AI_EMBEDDING_DIMENSIONS_UNSUPPORTED".to_string(),
        ));
    }
    Ok(())
}

async fn embeddings(source: &EmbeddingSource, input: Vec<String>) -> AppResult<Vec<Vec<f32>>> {
    embeddings_internal(source, input, true).await
}

async fn embeddings_internal(
    source: &EmbeddingSource,
    input: Vec<String>,
    enforce_dimensions: bool,
) -> AppResult<Vec<Vec<f32>>> {
    let mut request = crate::ai::http_client()
        .post(&source.endpoint)
        .json(&serde_json::json!({ "model": source.model, "input": input }));
    if let Some(key) = source
        .api_key
        .as_deref()
        .filter(|key| !key.trim().is_empty())
    {
        request = request.bearer_auth(key);
    }
    let response = tokio::time::timeout(crate::ai::FIRST_BYTE_TIMEOUT, request.send())
        .await
        .map_err(|_| AppError::Ai("AI_EMBEDDING_FIRST_BYTE_TIMEOUT".to_string()))?
        .map_err(|error| AppError::Ai(error.to_string()))?;
    if !response.status().is_success() {
        return Err(crate::ai::http_status_error("Embedding", response).await);
    }
    #[derive(serde::Deserialize)]
    struct EmbeddingItem {
        index: usize,
        embedding: Vec<f32>,
    }
    #[derive(serde::Deserialize)]
    struct EmbeddingResponse {
        data: Vec<EmbeddingItem>,
    }
    let mut data = response
        .json::<EmbeddingResponse>()
        .await
        .map_err(|_| AppError::Ai("AI_EMBEDDING_RESPONSE_INVALID".to_string()))?
        .data;
    data.sort_by_key(|item| item.index);
    if data.len() != input.len()
        || data
            .iter()
            .enumerate()
            .any(|(index, item)| item.index != index)
    {
        return Err(AppError::Ai("AI_EMBEDDING_RESPONSE_INVALID".to_string()));
    }
    let embeddings = data
        .into_iter()
        .map(|item| item.embedding)
        .collect::<Vec<_>>();
    if enforce_dimensions {
        for embedding in &embeddings {
            validate_embedding(embedding, source.dimensions)?;
        }
    }
    Ok(embeddings)
}

fn record_capability(db: &Db, source: &EmbeddingSource, available: bool, reason: Option<&str>) {
    let Ok(conn) = db.conn.lock() else {
        return;
    };
    let _ = conn.execute(
        "INSERT INTO ai_embedding_capabilities (profile_id, endpoint, model, available, reason, checked_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(profile_id) DO UPDATE SET endpoint = excluded.endpoint, model = excluded.model,
             available = excluded.available, reason = excluded.reason, checked_at = excluded.checked_at",
        params![
            source.profile_id,
            source.endpoint,
            source.model,
            available as i64,
            reason,
            chrono::Utc::now().timestamp_millis(),
        ],
    );
}

pub fn availability(db: &Db, secrets: &Secrets) -> AppResult<VectorAvailability> {
    let Some(source) = router::embedding_source(db, secrets)? else {
        return Ok(VectorAvailability {
            available: false,
            reason: Some("requires_compatible_provider".to_string()),
            dimensions: None,
            model: None,
        });
    };
    let cached = {
        let conn = db.reader();
        conn.query_row(
            "SELECT available, reason FROM ai_embedding_capabilities
             WHERE profile_id = ?1 AND endpoint = ?2 AND model = ?3",
            params![source.profile_id, source.endpoint, source.model],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?
    };
    if let Some((_available, reason)) = cached.filter(|(available, _)| *available == 0) {
        return Ok(VectorAvailability {
            available: false,
            reason: reason.or_else(|| Some("embedding_endpoint_unavailable".to_string())),
            dimensions: Some(source.dimensions),
            model: Some(source.model),
        });
    }
    Ok(VectorAvailability {
        available: true,
        reason: None,
        dimensions: Some(source.dimensions),
        model: Some(source.model),
    })
}

pub async fn enable(db: &Db, secrets: &Secrets) -> AppResult<()> {
    let source = router::embedding_source(db, secrets)?
        .ok_or_else(|| AppError::Other("AI_EMBEDDING_SOURCE_UNAVAILABLE".to_string()))?;
    let probe = embeddings(
        &source,
        vec!["Quill embedding capability probe".to_string()],
    )
    .await;
    match probe {
        Ok(_) => {
            record_capability(db, &source, true, None);
            let conn = db
                .conn
                .lock()
                .map_err(|error| AppError::Other(error.to_string()))?;
            conn.execute(
                "INSERT INTO settings (key, value) VALUES ('ai_vector_retrieval', 'true')
                 ON CONFLICT(key) DO UPDATE SET value = 'true'",
                [],
            )?;
            Ok(())
        }
        Err(error) => {
            record_capability(db, &source, false, Some("embedding_endpoint_unavailable"));
            Err(error)
        }
    }
}

pub fn source(db: &Db, secrets: &Secrets) -> AppResult<Option<EmbeddingSource>> {
    router::embedding_source(db, secrets)
}

pub fn has_complete_embeddings(
    db: &Db,
    book_id: &str,
    source: &EmbeddingSource,
) -> AppResult<bool> {
    let conn = db.reader();
    let counts: (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COUNT(e.chunk_id)
         FROM book_chunks c
         LEFT JOIN book_chunk_embeddings e ON e.chunk_id = c.id AND e.model = ?2 AND e.dimensions = ?3
         WHERE c.book_id = ?1",
        params![book_id, source.model, source.dimensions as i64],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(counts.0 > 0 && counts.0 == counts.1)
}

pub async fn ensure_embeddings(db: &Db, book_id: &str, source: &EmbeddingSource) -> AppResult<()> {
    let (source_sha256, chunks) = {
        let conn = db.reader();
        let source_sha256: String = conn.query_row(
            "SELECT source_sha256 FROM book_index_state WHERE book_id = ?1 AND status = 'ready'",
            params![book_id],
            |row| row.get(0),
        )?;
        let mut statement = conn.prepare(
            "SELECT c.id, c.text
             FROM book_chunks c
             LEFT JOIN book_chunk_embeddings e
               ON e.chunk_id = c.id AND e.model = ?2 AND e.source_sha256 = ?3 AND e.dimensions = ?4
             WHERE c.book_id = ?1 AND e.chunk_id IS NULL
             ORDER BY c.chunk_index",
        )?;
        let chunks = statement
            .query_map(
                params![
                    book_id,
                    source.model,
                    source_sha256,
                    source.dimensions as i64
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        (source_sha256, chunks)
    };
    for batch in chunks.chunks(EMBEDDING_BATCH_SIZE) {
        let input = batch.iter().map(|(_, text)| text.clone()).collect();
        let vectors = embeddings(source, input).await?;
        let mut conn = db
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        ensure_vector_table(&conn, source.dimensions)?;
        let transaction = conn.transaction()?;
        for ((chunk_id, _), vector) in batch.iter().zip(vectors.iter()) {
            let encoded = embedding_json(vector)?;
            transaction.execute(
                "INSERT INTO book_chunk_embeddings
                 (chunk_id, book_id, embedding, dimensions, model, source_sha256, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(chunk_id) DO UPDATE SET embedding = excluded.embedding,
                     dimensions = excluded.dimensions, model = excluded.model,
                     source_sha256 = excluded.source_sha256, created_at = excluded.created_at",
                params![
                    chunk_id,
                    book_id,
                    embedding_blob(vector),
                    source.dimensions as i64,
                    source.model,
                    source_sha256,
                    chrono::Utc::now().timestamp_millis(),
                ],
            )?;
            transaction.execute(
                "DELETE FROM book_chunk_vectors WHERE chunk_id = ?1",
                params![chunk_id],
            )?;
            transaction.execute(
                "INSERT INTO book_chunk_vectors (chunk_id, book_id, embedding) VALUES (?1, ?2, ?3)",
                params![chunk_id, book_id, encoded],
            )?;
        }
        transaction.commit()?;
    }
    Ok(())
}

pub async fn query_embedding(source: &EmbeddingSource, query: String) -> AppResult<Vec<f32>> {
    embeddings(source, vec![query])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Ai("AI_EMBEDDING_RESPONSE_INVALID".to_string()))
}

pub async fn probe_and_save(
    db: &Db,
    secrets: &Secrets,
    endpoint: String,
    model: String,
    api_key: Option<String>,
) -> AppResult<EmbeddingProbeResult> {
    let endpoint = endpoint.trim().to_string();
    let model = model.trim().to_string();
    let parsed = reqwest::Url::parse(&endpoint)
        .map_err(|_| AppError::Other("AI_EMBEDDING_CONFIG_INVALID".to_string()))?;
    if model.is_empty() || !matches!(parsed.scheme(), "http" | "https") {
        return Err(AppError::Other("AI_EMBEDDING_CONFIG_INVALID".to_string()));
    }
    let started = std::time::Instant::now();
    let effective_key = match api_key.as_ref() {
        Some(value) => (!value.trim().is_empty()).then(|| value.clone()),
        None => secrets.get(EMBEDDING_SECRET_REF)?,
    };
    let source = EmbeddingSource {
        profile_id: "explicit".to_string(),
        endpoint: endpoint.clone(),
        model: model.clone(),
        api_key: effective_key,
        dimensions: DEFAULT_EMBEDDING_DIMENSIONS,
    };
    let response =
        embeddings_internal(&source, vec!["Quill embedding probe".to_string()], false).await;
    let latency_ms = started.elapsed().as_millis() as u64;
    let vector = match response {
        Ok(mut values) => values.pop().unwrap_or_default(),
        Err(error) => {
            return Ok(EmbeddingProbeResult {
                ok: false,
                dimensions: 0,
                latency_ms,
                error: Some(error.to_string()),
            });
        }
    };
    let dimensions = vector.len();
    if dimensions == 0 || dimensions > 65_536 || vector.iter().any(|value| !value.is_finite()) {
        return Ok(EmbeddingProbeResult {
            ok: false,
            dimensions: 0,
            latency_ms,
            error: Some("AI_EMBEDDING_RESPONSE_INVALID".to_string()),
        });
    }
    if let Some(value) = api_key.as_deref() {
        secrets.set_many(&[(
            EMBEDDING_SECRET_REF,
            (!value.trim().is_empty()).then_some(value),
        )])?;
    }
    let dimensions_text = dimensions.to_string();
    let conn = db
        .conn
        .lock()
        .map_err(|error| AppError::Other(error.to_string()))?;
    let tx = conn.unchecked_transaction()?;
    for (key, value) in [
        ("ai_embedding_endpoint", endpoint.as_str()),
        ("ai_embedding_model", model.as_str()),
        ("ai_embedding_dimensions", dimensions_text.as_str()),
        ("ai_embedding_configured", "true"),
    ] {
        tx.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
    }
    ensure_vector_table(&tx, dimensions)?;
    tx.execute(
        "INSERT INTO ai_embedding_capabilities (profile_id, endpoint, model, available, reason, checked_at)
         VALUES ('explicit', ?1, ?2, 1, NULL, ?3)
         ON CONFLICT(profile_id) DO UPDATE SET endpoint=excluded.endpoint, model=excluded.model,
           available=1, reason=NULL, checked_at=excluded.checked_at",
        params![endpoint, model, chrono::Utc::now().timestamp_millis()],
    )?;
    tx.commit()?;
    Ok(EmbeddingProbeResult {
        ok: true,
        dimensions,
        latency_ms,
        error: None,
    })
}

pub fn rrf_merge(lexical: &[String], semantic: &[String]) -> Vec<(String, f64)> {
    let mut scores = HashMap::<String, f64>::new();
    for (index, chunk_id) in lexical.iter().enumerate() {
        *scores.entry(chunk_id.clone()).or_default() += 1.0 / (RRF_K + index as f64 + 1.0);
    }
    for (index, chunk_id) in semantic.iter().enumerate() {
        *scores.entry(chunk_id.clone()).or_default() += 1.0 / (RRF_K + index as f64 + 1.0);
    }
    let mut merged = scores.into_iter().collect::<Vec<_>>();
    merged.sort_by(|(left_id, left_score), (right_id, right_score)| {
        right_score
            .total_cmp(left_score)
            .then_with(|| left_id.cmp(right_id))
    });
    // The downstream retrieval pipeline expects lower scores to rank first.
    merged
        .into_iter()
        .map(|(chunk_id, score)| (chunk_id, -score))
        .collect()
}

fn vector_ranks(
    conn: &Connection,
    book_id: &str,
    embedding: &[f32],
    cutoff: Option<SpoilerCutoff>,
) -> AppResult<Vec<String>> {
    let query = embedding_json(embedding)?;
    let rows = conn
        .prepare(
            "SELECT chunk_id FROM book_chunk_vectors
             WHERE embedding MATCH ?1 AND k = ?2 AND book_id = ?3
             ORDER BY distance",
        )?
        .query_map(
            params![query, (RETRIEVAL_TOP_K * 4) as i64, book_id],
            |row| row.get(0),
        )?
        .collect::<Result<Vec<String>, _>>()?;
    if cutoff.is_none() {
        return Ok(rows.into_iter().take(RETRIEVAL_TOP_K).collect());
    }
    let cutoff = cutoff.expect("cutoff checked");
    let mut allowed = Vec::with_capacity(RETRIEVAL_TOP_K);
    let mut statement = conn.prepare(
        "SELECT section_index, char_start FROM book_chunks WHERE id = ?1 AND book_id = ?2",
    )?;
    for chunk_id in rows {
        let position = statement
            .query_row(params![chunk_id, book_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?))
            })
            .optional()?;
        if position.is_some_and(|(section, start)| cutoff.allows(section, start)) {
            allowed.push(chunk_id);
            if allowed.len() == RETRIEVAL_TOP_K {
                break;
            }
        }
    }
    Ok(allowed)
}

pub fn hybrid_retrieve(
    conn: &Connection,
    book_id: &str,
    query_text: &str,
    query_vector: &[f32],
    budget_tokens: usize,
    cutoff: Option<SpoilerCutoff>,
) -> AppResult<Vec<RetrievedChunk>> {
    let lexical = lexical_ranks(conn, book_id, query_text, cutoff)?;
    let semantic = vector_ranks(conn, book_id, query_vector, cutoff)?;
    if semantic.is_empty() {
        return retrieve_ranked(conn, book_id, &lexical, budget_tokens, cutoff);
    }
    let lexical_ids = lexical
        .iter()
        .map(|(chunk_id, _)| chunk_id.clone())
        .collect::<Vec<_>>();
    let ranked = rrf_merge(&lexical_ids, &semantic);
    retrieve_ranked(conn, book_id, &ranked, budget_tokens, cutoff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrf_promotes_a_chunk_returned_by_both_retrievers() {
        let ranked = rrf_merge(
            &["lexical".to_string(), "both".to_string()],
            &["semantic".to_string(), "both".to_string()],
        );
        assert_eq!(ranked[0].0, "both");
        assert_eq!(ranked[1].0, "lexical");
        assert_eq!(ranked[2].0, "semantic");
    }

    #[test]
    fn missing_semantic_results_keep_lexical_rank_order() {
        let ranked = rrf_merge(&["first".to_string(), "second".to_string()], &[]);
        assert_eq!(
            ranked
                .into_iter()
                .map(|(chunk_id, _)| chunk_id)
                .collect::<Vec<_>>(),
            vec!["first", "second"]
        );
    }

    #[test]
    fn sqlite_vec_returns_nearest_chunks_for_a_book() {
        let directory = tempfile::TempDir::new().unwrap();
        let db = Db::init(directory.path()).unwrap();
        let conn = db.conn.lock().unwrap();
        let mut near = vec![0.0; DEFAULT_EMBEDDING_DIMENSIONS];
        near[0] = 1.0;
        let mut far = vec![0.0; DEFAULT_EMBEDDING_DIMENSIONS];
        far[1] = 1.0;
        conn.execute(
            "INSERT INTO book_chunk_vectors (chunk_id, book_id, embedding) VALUES (?1, 'book', ?2)",
            params!["near", embedding_json(&near).unwrap()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO book_chunk_vectors (chunk_id, book_id, embedding) VALUES (?1, 'book', ?2)",
            params!["far", embedding_json(&far).unwrap()],
        )
        .unwrap();

        assert_eq!(vector_ranks(&conn, "book", &near, None).unwrap()[0], "near");
    }

    #[test]
    fn vector_ranks_filter_chunks_after_the_spoiler_cutoff() {
        let directory = tempfile::TempDir::new().unwrap();
        let db = Db::init(directory.path()).unwrap();
        let conn = db.conn.lock().unwrap();
        let query = vec![1.0; DEFAULT_EMBEDDING_DIMENSIONS];
        for (id, section) in [("allowed", 0_i64), ("blocked", 2_i64)] {
            conn.execute(
                "INSERT INTO book_chunks
                 (id, book_id, chunk_index, section_index, text, snippet, token_estimate, created_at)
                 VALUES (?1, 'book', ?2, ?2, ?1, ?1, 1, 1)",
                params![id, section],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO book_chunk_vectors (chunk_id, book_id, embedding) VALUES (?1, 'book', ?2)",
                params![id, embedding_json(&query).unwrap()],
            )
            .unwrap();
        }
        assert_eq!(
            vector_ranks(&conn, "book", &query, Some(SpoilerCutoff::Section(0))).unwrap(),
            vec!["allowed"]
        );
    }

    #[test]
    fn dimension_change_rebuilds_vector_table_from_matching_cache() {
        let directory = tempfile::TempDir::new().unwrap();
        let db = Db::init(directory.path()).unwrap();
        let conn = db.conn.lock().unwrap();
        let vector = vec![0.25_f32, 0.5, 0.75];
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('ai_embedding_model', 'model')
             ON CONFLICT(key) DO UPDATE SET value = 'model'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO book_chunk_embeddings
             (chunk_id, book_id, embedding, dimensions, model, source_sha256, created_at)
             VALUES ('cached', 'book', ?1, 3, 'model', 'hash', 1)",
            params![embedding_blob(&vector)],
        )
        .unwrap();
        ensure_vector_table(&conn, 3).unwrap();
        assert_eq!(
            vector_ranks(&conn, "book", &vector, None).unwrap(),
            vec!["cached"]
        );
    }
}
