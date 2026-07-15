use futures::StreamExt;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::{AppHandle, Emitter};

use crate::commands::ai::{AiStreamChunk, ChatMessage};
use crate::error::{AppError, AppResult};

fn request_body(
    model: &str,
    temperature: f64,
    messages: &[ChatMessage],
    keep_alive: Option<&str>,
    max_tokens_override: Option<u32>,
) -> serde_json::Value {
    // Grounded chat internally separates cacheable and variable system text.
    // OpenAI-compatible APIs receive the original single combined message.
    let system = messages
        .iter()
        .filter(|message| matches!(message.role.as_str(), "system" | "system_cache_variable"))
        .map(|message| message.content.as_str())
        .collect::<String>();
    let mut api_messages = Vec::new();
    if !system.is_empty() {
        api_messages.push(serde_json::json!({ "role": "system", "content": system }));
    }
    api_messages.extend(
        messages
            .iter()
            .filter(|message| !matches!(message.role.as_str(), "system" | "system_cache_variable"))
            .map(|message| serde_json::json!({ "role": message.role, "content": message.content })),
    );
    let mut body = serde_json::json!({
        "model": model,
        "messages": api_messages,
        "temperature": temperature,
        "stream": true,
    });
    if let Some(keep_alive) = keep_alive {
        body["keep_alive"] = serde_json::json!(keep_alive);
    }
    if let Some(max_tokens) = max_tokens_override {
        body["max_tokens"] = serde_json::json!(max_tokens);
    }
    body
}

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
    let base = base_url.trim_end_matches('/');
    let url = if base.ends_with("/v1") {
        format!("{base}/chat/completions")
    } else {
        format!("{base}/v1/chat/completions")
    };

    let body = request_body(
        model,
        temperature,
        messages,
        keep_alive,
        max_tokens_override,
    );

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
                reasoning_delta: None,
                done: true,
                error: None,
            },
        );
        return Ok(true);
    }

    let parsed: serde_json::Value = serde_json::from_str(data)
        .map_err(|_| AppError::Ai("AI_STREAM_PROTOCOL_ERROR: invalid JSON event".to_string()))?;
    // A mid-stream error event carries the real reason (rate limit, quota,
    // content policy). Surface it so the router can cool the right credential
    // instead of treating the truncated stream as AI_STREAM_INCOMPLETE.
    if !parsed["error"].is_null() {
        return Err(crate::ai::stream_event_error(
            "OpenAI-compatible",
            &parsed["error"],
        ));
    }
    let choice_delta = &parsed["choices"][0]["delta"];
    let reasoning = choice_delta["reasoning_content"]
        .as_str()
        .or_else(|| choice_delta["reasoning"].as_str())
        .or_else(|| choice_delta["thinking"].as_str());
    if let Some(reasoning) = reasoning.filter(|value| !value.is_empty()) {
        emitted.store(true, Ordering::Relaxed);
        let _ = app.emit(
            event_name,
            AiStreamChunk {
                delta: String::new(),
                reasoning_delta: Some(reasoning.to_string()),
                done: false,
                error: None,
            },
        );
    }
    // Only a non-empty content delta counts as "emitted": a leading empty
    // chunk (some gateways send one before erroring) must not block failover
    // to another credential.
    if let Some(delta) = choice_delta["content"]
        .as_str()
        .filter(|value| !value.is_empty())
    {
        emitted.store(true, Ordering::Relaxed);
        let _ = app.emit(
            event_name,
            AiStreamChunk {
                delta: delta.to_string(),
                reasoning_delta: None,
                done: false,
                error: None,
            },
        );
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separated_system_content_is_sent_as_one_byte_identical_message() {
        let body = request_body(
            "model",
            0.2,
            &[
                ChatMessage {
                    role: "system".into(),
                    content: "stable".into(),
                },
                ChatMessage {
                    role: "system_cache_variable".into(),
                    content: " variable".into(),
                },
                ChatMessage {
                    role: "user".into(),
                    content: "Question".into(),
                },
            ],
            None,
            None,
        );
        assert_eq!(
            body["messages"][0],
            serde_json::json!({ "role": "system", "content": "stable variable" })
        );
        assert_eq!(
            body["messages"][1],
            serde_json::json!({ "role": "user", "content": "Question" })
        );
    }

    #[test]
    fn mid_stream_error_event_surfaces_provider_code() {
        // A gateway can send a rate-limit error mid-stream. It must become a
        // classified error, not be swallowed into AI_STREAM_INCOMPLETE.
        let error = crate::ai::stream_event_error(
            "OpenAI-compatible",
            &serde_json::json!({ "type": "rate_limit_error", "code": "rate_limited" }),
        );
        let message = error.to_string();
        assert!(message.contains("type=rate_limit_error"));
        assert!(message.contains("code=rate_limited"));
    }
}
