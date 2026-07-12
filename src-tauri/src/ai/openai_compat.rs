use futures::StreamExt;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::{AppHandle, Emitter};

use crate::commands::ai::{AiStreamChunk, ChatMessage};
use crate::error::{AppError, AppResult};

#[allow(clippy::too_many_arguments)]
pub async fn stream_chat(
    app: &AppHandle,
    base_url: &str,
    api_key: &str,
    model: &str,
    temperature: f64,
    messages: &[ChatMessage],
    keep_alive: Option<&str>,
    event_name: &str,
    max_tokens_override: Option<u32>,
    emitted: Arc<AtomicBool>,
) -> AppResult<()> {
    let client = crate::ai::http_client();
    let url = format!("{}/v1/chat/completions", base_url.trim_end_matches('/'));

    let mut body = serde_json::json!({
        "model": model,
        "messages": messages.iter().map(|m| serde_json::json!({
            "role": m.role,
            "content": m.content,
        })).collect::<Vec<_>>(),
        "temperature": temperature,
        "stream": true,
    });
    if let Some(ka) = keep_alive {
        body["keep_alive"] = serde_json::json!(ka);
    }
    if let Some(mt) = max_tokens_override {
        body["max_tokens"] = serde_json::json!(mt);
    }

    let mut request = client.post(&url).json(&body);
    if !api_key.is_empty() {
        request = request.bearer_auth(api_key);
    }

    let response = tokio::time::timeout(crate::ai::FIRST_BYTE_TIMEOUT, request.send())
        .await
        .map_err(|_| AppError::Ai("AI_FIRST_BYTE_TIMEOUT".to_string()))?
        .map_err(|e| AppError::Ai(e.to_string()))?;

    if !response.status().is_success() {
        return Err(crate::ai::http_status_error("OpenAI-compatible", response).await);
    }

    let mut stream = response.bytes_stream();
    let mut decoder = crate::ai::sse::SseDecoder::new();

    while let Some(chunk) = tokio::time::timeout(crate::ai::STREAM_IDLE_TIMEOUT, stream.next())
        .await
        .map_err(|_| AppError::Ai("AI_STREAM_IDLE_TIMEOUT".to_string()))?
    {
        let chunk = chunk.map_err(|e| AppError::Ai(e.to_string()))?;
        for data in decoder.push(&chunk)? {
            if process_data(app, event_name, &data, &emitted)? {
                return Ok(());
            }
        }
    }

    for data in decoder.finish()? {
        if process_data(app, event_name, &data, &emitted)? {
            return Ok(());
        }
    }

    Err(AppError::Ai("AI_STREAM_INCOMPLETE".to_string()))
}

fn process_data(
    app: &AppHandle,
    event_name: &str,
    data: &str,
    emitted: &AtomicBool,
) -> AppResult<bool> {
    if data == "[DONE]" {
        let _ = app.emit(
            event_name,
            AiStreamChunk {
                delta: String::new(),
                done: true,
                error: None,
            },
        );
        return Ok(true);
    }

    let parsed: serde_json::Value = serde_json::from_str(data)
        .map_err(|_| AppError::Ai("AI_STREAM_PROTOCOL_ERROR: invalid JSON event".to_string()))?;
    if let Some(delta) = parsed["choices"][0]["delta"]["content"].as_str() {
        emitted.store(true, Ordering::Relaxed);
        let _ = app.emit(
            event_name,
            AiStreamChunk {
                delta: delta.to_string(),
                done: false,
                error: None,
            },
        );
    }
    Ok(false)
}
