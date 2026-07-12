use futures::StreamExt;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::{AppHandle, Emitter};

use crate::commands::ai::{AiStreamChunk, ChatMessage};
use crate::error::{AppError, AppResult};

/// Stream chat using OpenAI's Responses API (`/responses`).
/// When using OAuth tokens, requests go to `chatgpt.com/backend-api/codex`
/// with the `chatgpt-account-id` header (same as Codex CLI).
///
/// Request format matches `ResponsesApiRequest` from the Codex CLI source:
///   model, instructions, input, tools, tool_choice, parallel_tool_calls,
///   store, stream — no temperature, no max_output_tokens.
#[allow(clippy::too_many_arguments)]
pub async fn stream_chat(
    app: &AppHandle,
    base_url: &str,
    api_key: &str,
    model: &str,
    messages: &[ChatMessage],
    account_id: Option<&str>,
    event_name: &str,
    emitted: Arc<AtomicBool>,
) -> AppResult<()> {
    let client = crate::ai::http_client();
    let url = format!("{}/responses", base_url.trim_end_matches('/'));

    // Responses API uses top-level "instructions" for system messages,
    // and "input" for user/assistant messages only.
    let instructions: String = messages
        .iter()
        .filter(|m| m.role == "system")
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");

    let input: Vec<serde_json::Value> = messages
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
        "instructions": instructions,
        "input": input,
        "stream": true,
        "store": false,
    });

    let mut request = client.post(&url).bearer_auth(api_key).json(&body);
    if let Some(acct) = account_id {
        request = request.header("chatgpt-account-id", acct);
    }

    let response = tokio::time::timeout(crate::ai::FIRST_BYTE_TIMEOUT, request.send())
        .await
        .map_err(|_| AppError::Ai("AI_FIRST_BYTE_TIMEOUT".to_string()))?
        .map_err(|e| AppError::Ai(e.to_string()))?;

    if !response.status().is_success() {
        return Err(crate::ai::http_status_error("OpenAI", response).await);
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
        "response.output_text.delta" => {
            if let Some(delta) = parsed["delta"].as_str() {
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
        }
        "response.completed" => {
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
