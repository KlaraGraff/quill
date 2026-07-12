pub mod anthropic;
pub mod oauth;
pub mod openai_compat;
pub mod openai_responses;
pub mod router;
mod sse;

use std::sync::OnceLock;
use std::time::Duration;

/// Shared transport with bounded connection setup. Individual adapters also
/// enforce a first-byte and stream-idle timeout, rather than a total timeout
/// that would cut off legitimate long responses.
pub(crate) fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(30))
            .build()
            .expect("build shared AI HTTP client")
    })
}

pub(crate) const FIRST_BYTE_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(75);
const MAX_PROVIDER_ERROR_BYTES: usize = 64 * 1024;

fn retry_after_seconds(value: &str) -> Option<i64> {
    if let Ok(seconds) = value.trim().parse::<i64>() {
        return Some(seconds.clamp(1, 86_400));
    }
    chrono::DateTime::parse_from_rfc2822(value.trim())
        .ok()
        .map(|deadline| {
            (deadline.timestamp_millis() - chrono::Utc::now().timestamp_millis()) / 1000
        })
        .map(|seconds| seconds.clamp(1, 86_400))
}

/// Preserve rate-limit hints for the credential router without exposing
/// provider response bodies or credentials to the WebView.
pub(crate) async fn http_status_error(
    provider: &str,
    mut response: reqwest::Response,
) -> crate::error::AppError {
    let status = response.status();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(retry_after_seconds);
    let mut body = Vec::new();
    while body.len() < MAX_PROVIDER_ERROR_BYTES {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = MAX_PROVIDER_ERROR_BYTES - body.len();
                body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            }
            Ok(None) | Err(_) => break,
        }
    }
    let (error_type, error_code) = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .map(|value| {
            let error = value.get("error").unwrap_or(&value);
            (
                sanitized_error_field(error.get("type")),
                sanitized_error_field(error.get("code")),
            )
        })
        .unwrap_or_default();
    let hint = retry_after
        .map(|seconds| format!(" retry-after={seconds}"))
        .unwrap_or_default();
    let error_type = error_type
        .map(|value| format!(" type={value}"))
        .unwrap_or_default();
    let error_code = error_code
        .map(|value| format!(" code={value}"))
        .unwrap_or_default();
    crate::error::AppError::Ai(format!(
        "AI_PROVIDER_HTTP provider={provider} status={}{}{}{hint}",
        status.as_u16(),
        error_type,
        error_code,
    ))
}

fn sanitized_error_field(value: Option<&serde_json::Value>) -> Option<String> {
    let value = match value? {
        serde_json::Value::String(value) => value.as_str(),
        serde_json::Value::Number(value) => return Some(value.to_string()),
        _ => return None,
    };
    let sanitized: String = value
        .chars()
        .take(80)
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
        .collect();
    (!sanitized.is_empty()).then_some(sanitized)
}
