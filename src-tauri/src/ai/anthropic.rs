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
    use_bearer_auth: bool,
    event_name: &str,
    max_tokens_override: Option<u32>,
    emitted: Arc<AtomicBool>,
) -> AppResult<()> {
    let client = crate::ai::http_client();

    // Anthropic uses a separate system parameter
    let system_msg = messages
        .iter()
        .filter(|m| m.role == "system")
        .map(|m| m.content.clone())
        .collect::<Vec<_>>()
        .join("\n\n");

    let api_messages: Vec<serde_json::Value> = messages
        .iter()
        .filter(|m| m.role != "system")
        .map(|m| {
            serde_json::json!({
                "role": m.role,
                "content": m.content,
            })
        })
        .collect();

    let body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens_override.unwrap_or(4096),
        "system": system_msg,
        "messages": api_messages,
        "temperature": temperature,
        "stream": true,
    });

    let mut request = client
        .post(format!("{}/v1/messages", base_url.trim_end_matches('/')))
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json");

    if use_bearer_auth {
        request = request.bearer_auth(api_key);
    } else {
        request = request.header("x-api-key", api_key);
    }

    let response = tokio::time::timeout(crate::ai::FIRST_BYTE_TIMEOUT, request.json(&body).send())
        .await
        .map_err(|_| AppError::Ai("AI_FIRST_BYTE_TIMEOUT".to_string()))?
        .map_err(|e| AppError::Ai(e.to_string()))?;

    if !response.status().is_success() {
        return Err(crate::ai::http_status_error("Anthropic", response).await);
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
    let parsed: serde_json::Value = serde_json::from_str(data)
        .map_err(|_| AppError::Ai("AI_STREAM_PROTOCOL_ERROR: invalid JSON event".to_string()))?;
    match parsed["type"].as_str().unwrap_or("") {
        "content_block_delta" => {
            if let Some(text) = parsed["delta"]["text"].as_str() {
                emitted.store(true, Ordering::Relaxed);
                let _ = app.emit(
                    event_name,
                    AiStreamChunk {
                        delta: text.to_string(),
                        done: false,
                        error: None,
                    },
                );
            }
        }
        "message_stop" => {
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
        _ => {}
    }
    Ok(false)
}
