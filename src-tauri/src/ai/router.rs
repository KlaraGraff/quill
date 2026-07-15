//! Provider-neutral AI request routing and API credential failover.
//!
//! Secret values never leave the Rust backend. Profile metadata stores only a
//! local secret reference, masked suffix, priority, and health state.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::{Duration, Instant};

use chrono::Utc;
use futures::StreamExt;
use rusqlite::{params, OptionalExtension};
use tauri::{AppHandle, Emitter, Listener};
use tokio::sync::watch;

use crate::commands::ai::ChatMessage;
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::secrets::Secrets;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AiProfileView {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub auth_mode: String,
    pub base_url: Option<String>,
    pub model: String,
    pub temperature: f64,
    pub keep_alive: Option<String>,
    pub enabled: bool,
    pub priority: i64,
    pub state: String,
    pub cooldown_until: Option<i64>,
    pub last_error_kind: Option<String>,
    pub last_used_at: Option<i64>,
    pub last_latency_ms: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AiCredentialView {
    pub id: String,
    pub profile_id: String,
    pub label: String,
    pub masked_suffix: String,
    pub enabled: bool,
    pub priority: i64,
    pub state: String,
    pub cooldown_until: Option<i64>,
    pub last_error_kind: Option<String>,
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AiConnectionTestResult {
    pub success: bool,
    pub profile_id: String,
    pub provider: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_response_ms: Option<u64>,
    pub total_ms: u64,
    pub tested_at: i64,
    pub attempt_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    pub attempts: Vec<AiConnectionTestAttempt>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AiConnectionTestAttempt {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_detail: Option<String>,
    pub latency_ms: u64,
    pub request_sent: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AiCompletion {
    pub text: String,
    pub profile_id: String,
    pub provider: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_token_ms: Option<u64>,
    pub total_ms: u64,
}

#[derive(Debug, Clone)]
struct AiProfile {
    view: AiProfileView,
}

#[derive(Debug, Clone)]
struct AiCredential {
    view: AiCredentialView,
    secret_ref: String,
}

type NormalizedProfileConfig = (
    String,
    String,
    String,
    Option<String>,
    String,
    f64,
    Option<String>,
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AiErrorKind {
    Cancelled,
    CredentialInvalid,
    Auth,
    Permission,
    RateLimit,
    Quota,
    Network,
    Provider5xx,
    Protocol,
    Request,
    NotConfigured,
}

impl AiErrorKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::CredentialInvalid => "credential_invalid",
            Self::Auth => "auth",
            Self::Permission => "permission",
            Self::RateLimit => "rate_limit",
            Self::Quota => "quota",
            Self::Network => "network",
            Self::Provider5xx => "provider_5xx",
            Self::Protocol => "protocol",
            Self::Request => "request",
            Self::NotConfigured => "not_configured",
        }
    }

    fn retryable(self) -> bool {
        matches!(
            self,
            Self::CredentialInvalid
                | Self::Auth
                | Self::Permission
                | Self::RateLimit
                | Self::Quota
                | Self::Network
                | Self::Provider5xx
                | Self::Protocol
        )
    }
}

fn classify_error(error: &AppError) -> AiErrorKind {
    let message = error.to_string().to_ascii_lowercase();
    if message.contains("ai_request_cancelled") {
        AiErrorKind::Cancelled
    } else if [
        "invalid_api_key",
        "invalid_api_key_error",
        "authentication_error",
        "invalid_x_api_key",
        "api_key_revoked",
        "key_revoked",
    ]
    .iter()
    .any(|code| message.contains(&format!("code={code}")))
    {
        AiErrorKind::CredentialInvalid
    } else if message.contains("status=401") || message.contains("unauthorized") {
        AiErrorKind::Auth
    } else if [
        "content_policy_violation",
        "content_filter",
        "moderation_blocked",
        "safety_violation",
    ]
    .iter()
    .any(|code| {
        message.contains(&format!("code={code}")) || message.contains(&format!("type={code}"))
    }) {
        // A policy rejection belongs to this request, not to the credential.
        // Trying every key would repeat the same rejected request.
        AiErrorKind::Request
    } else if message.contains("status=403") || message.contains("forbidden") {
        AiErrorKind::Permission
    } else if message.contains("status=402")
        || message.contains("quota")
        || message.contains("insufficient")
    {
        AiErrorKind::Quota
    } else if message.contains("status=429") || message.contains("rate limit") {
        AiErrorKind::RateLimit
    } else if [" 500", " 502", " 503", " 504"]
        .iter()
        .any(|code| message.contains(&format!("status={}", code.trim())))
    {
        AiErrorKind::Provider5xx
    } else if message.contains("ai_stream_incomplete") || message.contains("protocol") {
        AiErrorKind::Protocol
    } else if message.contains("status=400")
        || message.contains("status=404")
        || message.contains("status=413")
        || message.contains("status=422")
        || message.contains("ai_model_list_invalid")
        || message.contains("ai_model_list_empty")
        || message.contains("ai_model_list_too_large")
    {
        AiErrorKind::Request
    } else if message.contains("ai_not_configured")
        || message.contains("ai_no_usable_keys")
        || message.contains("ai_keys_disabled")
        || message.contains("ai_all_keys_invalid")
    {
        AiErrorKind::NotConfigured
    } else {
        AiErrorKind::Network
    }
}

fn is_cancelled(error: &AppError) -> bool {
    error.to_string().contains("AI_REQUEST_CANCELLED")
}

fn sanitized_error_detail(error: &AppError, secret: Option<&str>) -> String {
    let mut detail = error
        .to_string()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(secret) = secret.filter(|value| !value.is_empty()) {
        detail = detail.replace(secret, "[redacted]");
    }
    detail.chars().take(300).collect()
}

fn now() -> i64 {
    Utc::now().timestamp_millis()
}

fn cancellation_registry() -> &'static Mutex<HashMap<String, watch::Sender<bool>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, watch::Sender<bool>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register_request(request_id: &str) -> watch::Receiver<bool> {
    let (sender, receiver) = watch::channel(false);
    if let Ok(mut registry) = cancellation_registry().lock() {
        registry.insert(request_id.to_string(), sender);
    }
    receiver
}

pub fn finish_request(request_id: &str) {
    if let Ok(mut registry) = cancellation_registry().lock() {
        registry.remove(request_id);
    }
}

pub fn cancel_request(request_id: &str) -> bool {
    cancellation_registry()
        .lock()
        .ok()
        .and_then(|registry| registry.get(request_id).cloned())
        .is_some_and(|sender| sender.send(true).is_ok())
}

pub fn request_is_cancelled(request_id: &str) -> bool {
    cancellation_registry()
        .lock()
        .ok()
        .and_then(|registry| registry.get(request_id).cloned())
        .is_some_and(|sender| *sender.borrow())
}

fn suffix(value: &str) -> String {
    value
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}

fn compensation_failure(
    operation: &str,
    primary: &dyn std::fmt::Display,
    compensation: &dyn std::fmt::Display,
) -> AppError {
    AppError::Other(format!(
        "{operation}: primary=[{primary}]; compensation=[{compensation}]"
    ))
}

pub fn migrate_legacy_config(db: &Db, secrets: &Secrets) -> AppResult<()> {
    let mut conn = db.conn.lock().map_err(|e| AppError::Other(e.to_string()))?;
    let profile_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM ai_profiles", [], |row| row.get(0))?;
    if profile_count > 0 {
        return Ok(());
    }

    let get = |key: &str| -> Option<String> {
        conn.query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .ok()
    };
    let provider = get("ai_provider").unwrap_or_else(|| "openai".to_string());
    let profile_id = uuid::Uuid::new_v4().to_string();
    let created_at = now();
    let profile_label = get("ai_provider_label")
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| provider.clone());
    let auth_mode = get("ai_auth_mode").unwrap_or_else(|| "api_key".to_string());
    let base_url = get("ai_base_url");
    let model = get("ai_model").unwrap_or_else(|| "gpt-4o-mini".to_string());
    let temperature = get("ai_temperature")
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.3);
    let keep_alive = get("ai_keep_alive");
    // Startup migration is metadata-only. Reading an old Keychain item here
    // would show a system password dialog before the user has any context.
    let has_legacy_ai_config = [
        "ai_provider",
        "ai_provider_label",
        "ai_auth_mode",
        "ai_base_url",
        "ai_model",
    ]
    .iter()
    .any(|key| get(key).is_some());
    let legacy_key_exists = secrets.has_stored_secret_metadata("ai_api_key")
        || get("ai_api_key_configured").is_some_and(|value| value == "true")
        // Builds before profile metadata existed stored the one API key only
        // in Keychain. Existing AI settings are a safe, metadata-only signal
        // that the legacy account should be offered for import on first use.
        || (auth_mode == "api_key" && provider != "ollama" && has_legacy_ai_config);
    let credential = legacy_key_exists.then(|| {
        let id = uuid::Uuid::new_v4().to_string();
        (id, "ai_api_key".to_string())
    });

    let result = (|| -> AppResult<()> {
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO ai_profiles (id, label, provider, auth_mode, base_url, model, temperature, keep_alive, enabled, priority, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, 0, ?9, ?9)",
            params![profile_id, profile_label, provider, auth_mode, base_url, model, temperature, keep_alive, created_at],
        )?;
        if let Some((credential_id, secret_ref)) = credential.as_ref() {
            tx.execute(
                "INSERT INTO ai_credentials (id, profile_id, label, secret_ref, masked_suffix, enabled, priority, state, created_at, updated_at) VALUES (?1, ?2, 'Primary key', ?3, ?4, 1, 0, 'active', ?5, ?5)",
                params![credential_id, profile_id, secret_ref, "", created_at],
            )?;
        }
        tx.commit()?;
        Ok(())
    })();
    drop(conn);
    result?;
    if credential.is_some() {
        // This is a durable metadata hint only. The old Keychain namespace is
        // not probed until the user confirms the later import explanation.
        secrets.register_legacy_candidate("ai_api_key")?;
    }
    Ok(())
}

const PROFILE_COLUMNS: &str =
    "id, label, provider, auth_mode, base_url, model, temperature, keep_alive, enabled, priority, state, cooldown_until, last_error_kind, last_used_at, last_latency_ms";

fn row_to_profile(row: &rusqlite::Row<'_>) -> rusqlite::Result<AiProfile> {
    Ok(AiProfile {
        view: AiProfileView {
            id: row.get(0)?,
            label: row.get(1)?,
            provider: row.get(2)?,
            auth_mode: row.get(3)?,
            base_url: row.get(4)?,
            model: row.get(5)?,
            temperature: row.get(6)?,
            keep_alive: row.get(7)?,
            enabled: row.get::<_, i64>(8)? != 0,
            priority: row.get(9)?,
            state: row.get(10)?,
            cooldown_until: row.get(11)?,
            last_error_kind: row.get(12)?,
            last_used_at: row.get(13)?,
            last_latency_ms: row.get(14)?,
        },
    })
}

fn profile_by_id(db: &Db, id: &str) -> AppResult<AiProfile> {
    let conn = db.reader();
    conn.query_row(
        &format!("SELECT {PROFILE_COLUMNS} FROM ai_profiles WHERE id = ?1"),
        params![id],
        row_to_profile,
    )
    .optional()?
    .ok_or_else(|| AppError::Other("AI_PROFILE_NOT_FOUND".to_string()))
}

fn profiles(db: &Db, enabled_only: bool) -> AppResult<Vec<AiProfile>> {
    let conn = db.reader();
    let where_clause = if enabled_only {
        " WHERE enabled = 1"
    } else {
        ""
    };
    let mut statement = conn.prepare(&format!(
        "SELECT {PROFILE_COLUMNS} FROM ai_profiles{where_clause} ORDER BY priority ASC, created_at ASC"
    ))?;
    let profiles = statement
        .query_map([], row_to_profile)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::from)?;
    Ok(profiles)
}

fn active_profile(db: &Db) -> AppResult<AiProfile> {
    profiles(db, true)?
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Other("AI_NOT_CONFIGURED".to_string()))
}

fn credentials_for(db: &Db, profile_id: &str) -> AppResult<Vec<AiCredential>> {
    let conn = db.reader();
    let timestamp = now();
    let mut statement = conn.prepare(
        "SELECT id, profile_id, label, secret_ref, masked_suffix, enabled, priority, state, cooldown_until, last_error_kind, last_used_at FROM ai_credentials WHERE profile_id = ?1 AND enabled = 1 AND state != 'invalid' AND (cooldown_until IS NULL OR cooldown_until <= ?2) ORDER BY priority ASC, created_at ASC"
    )?;
    let credentials = statement
        .query_map(params![profile_id, timestamp], |row| {
            Ok(AiCredential {
                secret_ref: row.get(3)?,
                view: AiCredentialView {
                    id: row.get(0)?,
                    profile_id: row.get(1)?,
                    label: row.get(2)?,
                    masked_suffix: row.get(4)?,
                    enabled: row.get::<_, i64>(5)? != 0,
                    priority: row.get(6)?,
                    state: row.get(7)?,
                    cooldown_until: row.get(8)?,
                    last_error_kind: row.get(9)?,
                    last_used_at: row.get(10)?,
                },
            })
        })
        .map_err(AppError::from)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::from)?;
    Ok(credentials)
}

fn all_credentials_for(db: &Db, profile_id: &str) -> AppResult<Vec<AiCredential>> {
    let conn = db.reader();
    let mut statement = conn.prepare(
        "SELECT id, profile_id, label, secret_ref, masked_suffix, enabled, priority, state, cooldown_until, last_error_kind, last_used_at FROM ai_credentials WHERE profile_id = ?1 ORDER BY priority ASC, created_at ASC"
    )?;
    let credentials = statement
        .query_map(params![profile_id], |row| {
            Ok(AiCredential {
                secret_ref: row.get(3)?,
                view: AiCredentialView {
                    id: row.get(0)?,
                    profile_id: row.get(1)?,
                    label: row.get(2)?,
                    masked_suffix: row.get(4)?,
                    enabled: row.get::<_, i64>(5)? != 0,
                    priority: row.get(6)?,
                    state: row.get(7)?,
                    cooldown_until: row.get(8)?,
                    last_error_kind: row.get(9)?,
                    last_used_at: row.get(10)?,
                },
            })
        })
        .map_err(AppError::from)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::from)?;
    Ok(credentials)
}

fn credential_by_id(db: &Db, id: &str) -> AppResult<AiCredential> {
    let conn = db.reader();
    conn.query_row(
        "SELECT id, profile_id, label, secret_ref, masked_suffix, enabled, priority, state, cooldown_until, last_error_kind, last_used_at FROM ai_credentials WHERE id = ?1",
        params![id],
        |row| {
            Ok(AiCredential {
                secret_ref: row.get(3)?,
                view: AiCredentialView {
                    id: row.get(0)?,
                    profile_id: row.get(1)?,
                    label: row.get(2)?,
                    masked_suffix: row.get(4)?,
                    enabled: row.get::<_, i64>(5)? != 0,
                    priority: row.get(6)?,
                    state: row.get(7)?,
                    cooldown_until: row.get(8)?,
                    last_error_kind: row.get(9)?,
                    last_used_at: row.get(10)?,
                },
            })
        },
    )
    .optional()?
    .ok_or_else(|| AppError::Other("AI_CREDENTIAL_NOT_FOUND".to_string()))
}

fn retry_after_ms(error: &AppError) -> Option<i64> {
    let marker = "retry-after=";
    let message = error.to_string().to_ascii_lowercase();
    let value = message.split(marker).nth(1)?.split_whitespace().next()?;
    value
        .parse::<i64>()
        .ok()
        .map(|seconds| seconds.clamp(1, 86_400) * 1000)
}

fn update_credential_health(
    db: &Db,
    credential: &AiCredential,
    error: Option<AiErrorKind>,
    retry_after: Option<i64>,
) {
    if error == Some(AiErrorKind::Cancelled) {
        return;
    }
    let Ok(conn) = db.conn.lock() else {
        return;
    };
    let timestamp = now();
    let (state, cooldown) = match error {
        None => ("active", None),
        Some(AiErrorKind::CredentialInvalid) => ("invalid", None),
        Some(AiErrorKind::Auth | AiErrorKind::Permission) => {
            ("cooldown", Some(timestamp + 5 * 60 * 1000))
        }
        Some(AiErrorKind::Quota) => ("quota", Some(timestamp + 60 * 60 * 1000)),
        Some(AiErrorKind::RateLimit) => (
            "cooldown",
            Some(timestamp + retry_after.unwrap_or(60 * 1000)),
        ),
        Some(AiErrorKind::Network | AiErrorKind::Provider5xx | AiErrorKind::Protocol) => {
            ("cooldown", Some(timestamp + 30 * 1000))
        }
        Some(AiErrorKind::Request | AiErrorKind::NotConfigured) => ("active", None),
        Some(AiErrorKind::Cancelled) => unreachable!("cancelled requests do not update health"),
    };
    let _ = conn.execute(
        "UPDATE ai_credentials SET state = ?1, cooldown_until = ?2, last_error_kind = ?3, last_used_at = ?4, updated_at = ?4 WHERE id = ?5",
        params![state, cooldown, error.map(AiErrorKind::as_str), timestamp, credential.view.id],
    );
}

fn update_profile_health(
    db: &Db,
    profile: &AiProfile,
    error: Option<AiErrorKind>,
    retry_after: Option<i64>,
    latency_ms: Option<u64>,
) {
    let timestamp = now();
    let Some((state, cooldown)) = profile_health_state(error, retry_after, timestamp) else {
        return;
    };
    let Ok(conn) = db.conn.lock() else {
        return;
    };
    let latency = latency_ms.map(|value| value.min(i64::MAX as u64) as i64);
    let _ = conn.execute(
        "UPDATE ai_profiles SET state = ?1, cooldown_until = ?2, last_error_kind = ?3, last_used_at = ?4, last_latency_ms = COALESCE(?5, last_latency_ms), updated_at = ?4 WHERE id = ?6",
        params![state, cooldown, error.map(AiErrorKind::as_str), timestamp, latency, profile.view.id],
    );
}

fn profile_health_state(
    error: Option<AiErrorKind>,
    retry_after: Option<i64>,
    timestamp: i64,
) -> Option<(&'static str, Option<i64>)> {
    let state = match error {
        None => ("active", None),
        Some(AiErrorKind::CredentialInvalid) => ("invalid", None),
        Some(AiErrorKind::Auth | AiErrorKind::Permission) => {
            ("cooldown", Some(timestamp + 5 * 60 * 1000))
        }
        Some(AiErrorKind::Quota) => ("quota", Some(timestamp + 60 * 60 * 1000)),
        Some(AiErrorKind::RateLimit) => (
            "cooldown",
            Some(timestamp + retry_after.unwrap_or(60 * 1000)),
        ),
        Some(AiErrorKind::Network | AiErrorKind::Provider5xx | AiErrorKind::Protocol) => {
            ("cooldown", Some(timestamp + 30 * 1000))
        }
        Some(AiErrorKind::Request) => ("active", None),
        Some(AiErrorKind::NotConfigured) => ("unavailable", None),
        Some(AiErrorKind::Cancelled) => return None,
    };
    Some(state)
}

async fn wait_cancelled(cancel: &mut watch::Receiver<bool>) {
    if cancel.changed().await.is_err() {
        std::future::pending::<()>().await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn stream_once(
    app: &AppHandle,
    profile: &AiProfile,
    api_key: &str,
    oauth_account_id: Option<&str>,
    messages: &[ChatMessage],
    event_name: &str,
    max_tokens: Option<u32>,
    emitted: Arc<AtomicBool>,
    cancel: &mut watch::Receiver<bool>,
) -> AppResult<()> {
    if *cancel.borrow() {
        return Err(AppError::Other("AI_REQUEST_CANCELLED".to_string()));
    }
    let base_url = resolve_base_url(&profile.view)?;
    let stream: Pin<Box<dyn Future<Output = AppResult<()>> + Send + '_>> =
        match profile.view.provider.as_str() {
            "anthropic" => Box::pin(crate::ai::anthropic::stream_chat(
                app,
                base_url,
                api_key,
                &profile.view.model,
                profile.view.temperature,
                messages,
                false,
                event_name,
                max_tokens,
                emitted,
            )),
            _ if profile.view.auth_mode == "oauth" && profile.view.provider == "openai" => {
                Box::pin(crate::ai::openai_responses::stream_chat(
                    app,
                    "https://chatgpt.com/backend-api/codex",
                    api_key,
                    &profile.view.model,
                    messages,
                    oauth_account_id,
                    event_name,
                    emitted,
                ))
            }
            _ => Box::pin(crate::ai::openai_compat::stream_chat(
                app,
                base_url,
                api_key,
                &profile.view.model,
                profile.view.temperature,
                messages,
                (profile.view.provider == "ollama")
                    .then_some(profile.view.keep_alive.as_deref())
                    .flatten(),
                event_name,
                max_tokens,
                emitted,
            )),
        };
    tokio::select! {
        result = stream => result,
        _ = wait_cancelled(cancel) => {
            Err(AppError::Other("AI_REQUEST_CANCELLED".to_string()))
        }
    }
}

fn resolve_base_url(profile: &AiProfileView) -> AppResult<&str> {
    if let Some(configured) = profile
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(configured);
    }
    match profile.provider.as_str() {
        "openai" => Ok("https://api.openai.com"),
        "anthropic" => Ok("https://api.anthropic.com"),
        "ollama" => Ok("http://localhost:11434"),
        "custom" => Err(AppError::Other("AI_CUSTOM_BASE_URL_REQUIRED".to_string())),
        _ => Err(AppError::Other("AI_PROVIDER_UNSUPPORTED".to_string())),
    }
}

fn models_endpoint(profile: &AiProfileView) -> AppResult<String> {
    let base = resolve_base_url(profile)?.trim_end_matches('/');
    match profile.provider.as_str() {
        "ollama" => Ok(if base.ends_with("/api") {
            format!("{base}/tags")
        } else {
            format!("{base}/api/tags")
        }),
        "openai" | "anthropic" | "custom" => Ok(if base.ends_with("/v1") {
            format!("{base}/models")
        } else {
            format!("{base}/v1/models")
        }),
        _ => Err(AppError::Other("AI_PROVIDER_UNSUPPORTED".to_string())),
    }
}

async fn read_json_limited(response: reqwest::Response) -> AppResult<serde_json::Value> {
    const MAX_BYTES: usize = 1024 * 1024;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BYTES as u64)
    {
        return Err(AppError::Other("AI_MODEL_LIST_TOO_LARGE".to_string()));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| AppError::Ai(error.to_string()))?;
        if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
            return Err(AppError::Other("AI_MODEL_LIST_TOO_LARGE".to_string()));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| AppError::Other("AI_MODEL_LIST_INVALID".to_string()))
}

fn parse_model_ids(provider: &str, value: &serde_json::Value) -> AppResult<Vec<String>> {
    let values = if provider == "ollama" {
        value.get("models").and_then(serde_json::Value::as_array)
    } else {
        value.get("data").and_then(serde_json::Value::as_array)
    }
    .ok_or_else(|| AppError::Other("AI_MODEL_LIST_INVALID".to_string()))?;

    let mut models = BTreeSet::new();
    for item in values.iter().take(2_000) {
        let id = if provider == "ollama" {
            item.get("model")
                .or_else(|| item.get("name"))
                .and_then(serde_json::Value::as_str)
        } else {
            item.get("id").and_then(serde_json::Value::as_str)
        };
        if let Some(id) = id
            .map(str::trim)
            .filter(|id| !id.is_empty() && id.len() <= 256)
        {
            models.insert(id.to_string());
        }
    }
    if models.is_empty() {
        return Err(AppError::Other("AI_MODEL_LIST_EMPTY".to_string()));
    }
    Ok(models.into_iter().collect())
}

async fn list_models_once(
    profile: &AiProfile,
    endpoint: &str,
    api_key: Option<&str>,
) -> AppResult<Vec<String>> {
    let mut request = crate::ai::http_client().get(endpoint);
    if let Some(key) = api_key {
        request = if profile.view.provider == "anthropic" {
            request
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01")
        } else {
            request.bearer_auth(key)
        };
    }
    let response = tokio::time::timeout(crate::ai::FIRST_BYTE_TIMEOUT, request.send())
        .await
        .map_err(|_| AppError::Ai("AI_FIRST_BYTE_TIMEOUT".to_string()))?
        .map_err(|error| AppError::Ai(error.to_string()))?;
    if !response.status().is_success() {
        return Err(crate::ai::http_status_error("model-list", response).await);
    }
    let value = read_json_limited(response).await?;
    parse_model_ids(&profile.view.provider, &value)
}

pub async fn list_models(
    db: &Db,
    secrets: &Secrets,
    profile_id: &str,
    provider: String,
    auth_mode: String,
    base_url: Option<String>,
) -> AppResult<Vec<String>> {
    let mut profile = profile_by_id(db, profile_id)?;
    let (_, provider, auth_mode, base_url, _, _, _) = normalize_profile_config(
        profile.view.label.clone(),
        provider,
        auth_mode,
        base_url,
        profile.view.model.clone(),
        profile.view.temperature,
        profile.view.keep_alive.clone(),
    )?;
    profile.view.provider = provider;
    profile.view.auth_mode = auth_mode;
    profile.view.base_url = base_url;

    if profile.view.auth_mode == "oauth" {
        return Err(AppError::Other("AI_MODEL_LIST_UNSUPPORTED".to_string()));
    }

    // Model discovery is an explicit settings action. Use enabled credentials
    // even when inference health put them in cooldown, but do not mutate that
    // health here: a provider may deny or omit `/models` while inference still
    // works, and this request may be probing an unsaved URL/provider draft.
    let endpoint = models_endpoint(&profile.view)?;
    if profile.view.provider == "ollama" {
        return list_models_once(&profile, &endpoint, None).await;
    }

    let candidates: Vec<_> = all_credentials_for(db, profile_id)?
        .into_iter()
        .filter(|credential| credential.view.enabled)
        .collect();
    if candidates.is_empty() {
        return Err(AppError::Other("AI_NO_USABLE_KEYS".to_string()));
    }

    let mut last_error = None;
    for credential in candidates {
        let Some(key) = secrets
            .get(&credential.secret_ref)?
            .filter(|value| !value.trim().is_empty())
        else {
            last_error = Some(AppError::Other("AI_CREDENTIAL_UNAVAILABLE".to_string()));
            continue;
        };

        match list_models_once(&profile, &endpoint, Some(&key)).await {
            Ok(models) => return Ok(models),
            Err(error) => {
                let kind = classify_error(&error);
                if !kind.retryable() {
                    return Err(error);
                }
                log::warn!(
                    "ai router: profile={} credential={} model discovery failed kind={}, trying next candidate",
                    profile.view.id,
                    credential.view.id,
                    kind.as_str()
                );
                last_error = Some(error);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| AppError::Other("AI_NO_USABLE_KEYS".to_string())))
}

pub async fn stream_with_failover(
    app: &AppHandle,
    db: &Db,
    secrets: &Secrets,
    messages: &[ChatMessage],
    event_name: &str,
    max_tokens: Option<u32>,
    request_id: Option<&str>,
) -> AppResult<()> {
    let mut cancel = request_id
        .and_then(|id| {
            cancellation_registry()
                .lock()
                .ok()
                .and_then(|registry| registry.get(id).map(watch::Sender::subscribe))
        })
        .unwrap_or_else(|| {
            request_id
                .map(register_request)
                .unwrap_or_else(|| watch::channel(false).1)
        });
    let result = stream_with_failover_inner(
        app,
        db,
        secrets,
        messages,
        event_name,
        max_tokens,
        &mut cancel,
    )
    .await;
    if let Some(id) = request_id {
        finish_request(id);
    }
    result.map(|_| ())
}

/// Run the same routed stream without exposing its token event name to the
/// frontend. Existing provider adapters emit through `AppHandle`, so a private
/// per-request listener collects those deltas until the adapters can be moved
/// to a provider-neutral sink.
pub async fn complete_with_failover(
    app: &AppHandle,
    db: &Db,
    secrets: &Secrets,
    messages: &[ChatMessage],
    max_tokens: Option<u32>,
    request_id: Option<&str>,
    forward_event_name: Option<&str>,
) -> AppResult<AiCompletion> {
    let event_name = format!("ai-internal-completion-{}", uuid::Uuid::new_v4());
    let output = Arc::new(Mutex::new(String::new()));
    let first_token_ms = Arc::new(Mutex::new(None));
    let started = Instant::now();
    let listener_output = Arc::clone(&output);
    let listener_first_token = Arc::clone(&first_token_ms);
    let forward_event_name = forward_event_name.map(str::to_string);
    let forward_app = app.clone();
    let listener_id = app.listen(event_name.clone(), move |event| {
        let Ok(chunk) = serde_json::from_str::<crate::commands::ai::AiStreamChunk>(event.payload())
        else {
            return;
        };
        if let Some(event_name) = forward_event_name.as_deref() {
            let _ = forward_app.emit(event_name, &chunk);
        }
        if chunk.done || chunk.delta.is_empty() {
            return;
        }
        if let Ok(mut first) = listener_first_token.lock() {
            first.get_or_insert_with(|| started.elapsed().as_millis() as u64);
        }
        if let Ok(mut text) = listener_output.lock() {
            text.push_str(&chunk.delta);
        }
    });

    let mut cancel = request_id
        .and_then(|id| {
            cancellation_registry()
                .lock()
                .ok()
                .and_then(|registry| registry.get(id).map(watch::Sender::subscribe))
        })
        .unwrap_or_else(|| {
            request_id
                .map(register_request)
                .unwrap_or_else(|| watch::channel(false).1)
        });
    let routed = stream_with_failover_inner(
        app,
        db,
        secrets,
        messages,
        &event_name,
        max_tokens,
        &mut cancel,
    )
    .await;
    app.unlisten(listener_id);
    if let Some(id) = request_id {
        finish_request(id);
    }

    let profile = routed?;
    let total_ms = started.elapsed().as_millis() as u64;
    let text = output
        .lock()
        .map_err(|error| AppError::Other(error.to_string()))?
        .clone();
    let first_token_ms = *first_token_ms
        .lock()
        .map_err(|error| AppError::Other(error.to_string()))?;
    Ok(AiCompletion {
        text,
        profile_id: profile.id,
        provider: profile.provider,
        model: profile.model,
        first_token_ms,
        total_ms,
    })
}

#[allow(clippy::too_many_arguments)]
async fn stream_with_profile_inner(
    app: &AppHandle,
    db: &Db,
    secrets: &Secrets,
    profile_id: &str,
    messages: &[ChatMessage],
    event_name: &str,
    max_tokens: Option<u32>,
    cancel: &mut watch::Receiver<bool>,
) -> AppResult<AiProfileView> {
    let profile = profile_by_id(db, profile_id)?;
    if !profile.view.enabled {
        return Err(AppError::Other("AI_PROFILE_DISABLED".to_string()));
    }
    if profile.view.auth_mode == "oauth" && profile.view.provider == "openai" {
        let (token, account_id) = crate::ai::oauth::get_valid_token(secrets).await?;
        stream_once(
            app,
            &profile,
            &token,
            account_id.as_deref(),
            messages,
            event_name,
            max_tokens,
            Arc::new(AtomicBool::new(false)),
            cancel,
        )
        .await?;
        return Ok(profile.view);
    }
    if profile.view.provider == "ollama" {
        stream_once(
            app,
            &profile,
            "",
            None,
            messages,
            event_name,
            max_tokens,
            Arc::new(AtomicBool::new(false)),
            cancel,
        )
        .await?;
        return Ok(profile.view);
    }
    let mut last_error = None;
    for credential in credentials_for(db, profile_id)? {
        let Some(key) = secrets
            .get(&credential.secret_ref)?
            .filter(|value| !value.trim().is_empty())
        else {
            continue;
        };
        match stream_once(
            app,
            &profile,
            &key,
            None,
            messages,
            event_name,
            max_tokens,
            Arc::new(AtomicBool::new(false)),
            cancel,
        )
        .await
        {
            Ok(()) => return Ok(profile.view),
            Err(error) if is_cancelled(&error) => return Err(error),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| AppError::Other("AI_NO_USABLE_KEYS".to_string())))
}

pub async fn complete_with_profile(
    app: &AppHandle,
    db: &Db,
    secrets: &Secrets,
    profile_id: &str,
    messages: &[ChatMessage],
    max_tokens: Option<u32>,
    request_id: Option<&str>,
) -> AppResult<AiCompletion> {
    let event_name = format!("ai-internal-profile-completion-{}", uuid::Uuid::new_v4());
    let output = Arc::new(Mutex::new(String::new()));
    let listener_output = Arc::clone(&output);
    let listener_id = app.listen(event_name.clone(), move |event| {
        let Ok(chunk) = serde_json::from_str::<crate::commands::ai::AiStreamChunk>(event.payload())
        else {
            return;
        };
        if !chunk.done && !chunk.delta.is_empty() {
            if let Ok(mut text) = listener_output.lock() {
                text.push_str(&chunk.delta);
            }
        }
    });
    let started = Instant::now();
    let mut cancel = request_id
        .and_then(|id| {
            cancellation_registry()
                .lock()
                .ok()
                .and_then(|registry| registry.get(id).map(watch::Sender::subscribe))
        })
        .unwrap_or_else(|| {
            request_id
                .map(register_request)
                .unwrap_or_else(|| watch::channel(false).1)
        });
    let routed = stream_with_profile_inner(
        app,
        db,
        secrets,
        profile_id,
        messages,
        &event_name,
        max_tokens,
        &mut cancel,
    )
    .await;
    app.unlisten(listener_id);
    if let Some(id) = request_id {
        finish_request(id);
    }
    let profile = routed?;
    let text = output
        .lock()
        .map_err(|error| AppError::Other(error.to_string()))?
        .clone();
    Ok(AiCompletion {
        text,
        profile_id: profile.id,
        provider: profile.provider,
        model: profile.model,
        first_token_ms: None,
        total_ms: started.elapsed().as_millis() as u64,
    })
}

fn connection_test_token_limit(profile: &AiProfile) -> Option<u32> {
    // OpenAI-compatible reasoning endpoints frequently reject `max_tokens`
    // (some require `max_completion_tokens`, while others accept neither).
    // The production request leaves the field unset, so the health probe does
    // the same. Anthropic requires a limit and accepts this small value.
    (profile.view.provider == "anthropic").then_some(64)
}

async fn stream_with_failover_inner(
    app: &AppHandle,
    db: &Db,
    secrets: &Secrets,
    messages: &[ChatMessage],
    event_name: &str,
    max_tokens: Option<u32>,
    cancel: &mut watch::Receiver<bool>,
) -> AppResult<AiProfileView> {
    let enabled_profiles = profiles(db, true)?;
    if enabled_profiles.is_empty() {
        return Err(AppError::Other("AI_NOT_CONFIGURED".to_string()));
    }
    let timestamp = now();
    let profiles: Vec<_> = enabled_profiles
        .into_iter()
        .filter(|profile| {
            profile
                .view
                .cooldown_until
                .is_none_or(|deadline| deadline <= timestamp)
        })
        .collect();
    if profiles.is_empty() {
        return Err(AppError::Other("AI_KEYS_COOLING_DOWN".to_string()));
    }

    let mut last_error = None;
    let mut configured_credentials = Vec::new();

    for profile in profiles {
        if profile.view.auth_mode == "oauth" && profile.view.provider == "openai" {
            let emitted = Arc::new(AtomicBool::new(false));
            let (token, account_id) = match crate::ai::oauth::get_valid_token(secrets).await {
                Ok(token) => token,
                Err(error) => {
                    let kind = classify_error(&error);
                    update_profile_health(db, &profile, Some(kind), retry_after_ms(&error), None);
                    if is_cancelled(&error) || !kind.retryable() {
                        return Err(error);
                    }
                    log::warn!(
                        "ai router: profile={} oauth unavailable, trying next profile",
                        profile.view.id
                    );
                    last_error = Some(error);
                    continue;
                }
            };
            let started = Instant::now();
            let result = stream_once(
                app,
                &profile,
                &token,
                account_id.as_deref(),
                messages,
                event_name,
                max_tokens,
                Arc::clone(&emitted),
                cancel,
            )
            .await;
            let latency = started.elapsed().as_millis() as u64;
            match result {
                Ok(()) => {
                    update_profile_health(db, &profile, None, None, Some(latency));
                    return Ok(profile.view.clone());
                }
                Err(error) => {
                    if is_cancelled(&error) {
                        return Err(error);
                    }
                    let kind = classify_error(&error);
                    update_profile_health(
                        db,
                        &profile,
                        Some(kind),
                        retry_after_ms(&error),
                        Some(latency),
                    );
                    if emitted.load(Ordering::Relaxed) || !kind.retryable() {
                        return Err(error);
                    }
                    log::warn!(
                        "ai router: profile={} failed kind={}, trying next profile",
                        profile.view.id,
                        kind.as_str()
                    );
                    last_error = Some(error);
                    continue;
                }
            }
        }

        if profile.view.provider == "ollama" {
            let emitted = Arc::new(AtomicBool::new(false));
            let started = Instant::now();
            let result = stream_once(
                app,
                &profile,
                "",
                None,
                messages,
                event_name,
                max_tokens,
                Arc::clone(&emitted),
                cancel,
            )
            .await;
            let latency = started.elapsed().as_millis() as u64;
            match result {
                Ok(()) => {
                    update_profile_health(db, &profile, None, None, Some(latency));
                    return Ok(profile.view.clone());
                }
                Err(error) => {
                    if is_cancelled(&error) {
                        return Err(error);
                    }
                    let kind = classify_error(&error);
                    update_profile_health(
                        db,
                        &profile,
                        Some(kind),
                        retry_after_ms(&error),
                        Some(latency),
                    );
                    if emitted.load(Ordering::Relaxed) || !kind.retryable() {
                        return Err(error);
                    }
                    log::warn!(
                        "ai router: profile={} failed kind={}, trying next profile",
                        profile.view.id,
                        kind.as_str()
                    );
                    last_error = Some(error);
                    continue;
                }
            }
        }

        let all = all_credentials_for(db, &profile.view.id)?;
        configured_credentials.extend(all.clone());
        let candidates = credentials_for(db, &profile.view.id)?;
        let profile_started = Instant::now();
        let mut profile_failure = None;
        for credential in candidates {
            let Some(key) = secrets
                .get(&credential.secret_ref)?
                .filter(|value| !value.trim().is_empty())
            else {
                update_credential_health(
                    db,
                    &credential,
                    Some(AiErrorKind::CredentialInvalid),
                    None,
                );
                profile_failure = Some((AiErrorKind::CredentialInvalid, None));
                last_error = Some(AppError::Other("AI_CREDENTIAL_UNAVAILABLE".to_string()));
                continue;
            };
            let emitted = Arc::new(AtomicBool::new(false));
            match stream_once(
                app,
                &profile,
                &key,
                None,
                messages,
                event_name,
                max_tokens,
                Arc::clone(&emitted),
                cancel,
            )
            .await
            {
                Ok(()) => {
                    update_credential_health(db, &credential, None, None);
                    update_profile_health(
                        db,
                        &profile,
                        None,
                        None,
                        Some(profile_started.elapsed().as_millis() as u64),
                    );
                    return Ok(profile.view.clone());
                }
                Err(error) => {
                    if is_cancelled(&error) {
                        return Err(error);
                    }
                    let kind = classify_error(&error);
                    let retry_after = retry_after_ms(&error);
                    update_credential_health(db, &credential, Some(kind), retry_after);
                    profile_failure = Some((kind, retry_after));
                    if emitted.load(Ordering::Relaxed) || !kind.retryable() {
                        update_profile_health(
                            db,
                            &profile,
                            Some(kind),
                            retry_after,
                            Some(profile_started.elapsed().as_millis() as u64),
                        );
                        return Err(error);
                    }
                    log::warn!(
                        "ai router: profile={} credential={} failed kind={}, trying next candidate",
                        profile.view.id,
                        credential.view.id,
                        kind.as_str()
                    );
                    last_error = Some(error);
                }
            }
        }
        if let Some((kind, retry_after)) = profile_failure {
            update_profile_health(
                db,
                &profile,
                Some(kind),
                retry_after,
                Some(profile_started.elapsed().as_millis() as u64),
            );
        }
    }

    if let Some(error) = last_error {
        return Err(error);
    }

    let code = if configured_credentials.is_empty() {
        "AI_NOT_CONFIGURED"
    } else if configured_credentials.iter().all(|item| !item.view.enabled) {
        "AI_KEYS_DISABLED"
    } else if configured_credentials
        .iter()
        .filter(|item| item.view.enabled)
        .all(|item| item.view.state == "invalid")
    {
        "AI_ALL_KEYS_INVALID"
    } else if configured_credentials.iter().any(|item| {
        item.view.enabled
            && item
                .view
                .cooldown_until
                .is_some_and(|deadline| deadline > now())
    }) {
        "AI_KEYS_COOLING_DOWN"
    } else {
        "AI_NO_USABLE_KEYS"
    };
    Err(AppError::Other(code.to_string()))
}

pub fn list_credentials(db: &Db, profile_id: Option<&str>) -> AppResult<Vec<AiCredentialView>> {
    let profile_id = match profile_id {
        Some(id) => profile_by_id(db, id)?.view.id,
        None => active_profile(db)?.view.id,
    };
    all_credentials_for(db, &profile_id)
        .map(|items| items.into_iter().map(|item| item.view).collect())
}

pub fn active_profile_view(db: &Db) -> AppResult<AiProfileView> {
    Ok(active_profile(db)?.view)
}

/// Return the active API-key profile as an OpenAI-compatible embedding source.
/// Anthropic and OAuth/CLI routes do not provide a compatible embeddings API.
pub(crate) fn embedding_source(
    db: &Db,
    secrets: &Secrets,
) -> AppResult<Option<crate::ai::grounding::vector::EmbeddingSource>> {
    let explicit = {
        let conn = db.reader();
        let get = |key: &str| -> Option<String> {
            conn.query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .ok()
        };
        (get("ai_embedding_configured") == Some("true".to_string())).then(|| {
            (
                get("ai_embedding_endpoint"),
                get("ai_embedding_model"),
                get("ai_embedding_dimensions").and_then(|value| value.parse::<usize>().ok()),
            )
        })
    };
    if let Some((Some(endpoint), Some(model), Some(dimensions))) = explicit {
        return Ok(Some(crate::ai::grounding::vector::EmbeddingSource {
            profile_id: "explicit".to_string(),
            endpoint,
            model,
            api_key: secrets.get(crate::ai::grounding::vector::EMBEDDING_SECRET_REF)?,
            dimensions,
        }));
    }
    let profile = match active_profile(db) {
        Ok(profile) => profile,
        Err(AppError::Other(code)) if code == "AI_NOT_CONFIGURED" => return Ok(None),
        Err(error) => return Err(error),
    };
    if profile.view.auth_mode != "api_key"
        || !matches!(profile.view.provider.as_str(), "openai" | "custom")
    {
        return Ok(None);
    }
    let Some(credential) = credentials_for(db, &profile.view.id)?.into_iter().next() else {
        return Ok(None);
    };
    let Some(api_key) = secrets.get(&credential.secret_ref)? else {
        return Ok(None);
    };
    if api_key.trim().is_empty() {
        return Ok(None);
    }
    let base_url = resolve_base_url(&profile.view)?.trim_end_matches('/');
    let endpoint = if base_url.ends_with("/v1") {
        format!("{base_url}/embeddings")
    } else {
        format!("{base_url}/v1/embeddings")
    };
    Ok(Some(crate::ai::grounding::vector::EmbeddingSource {
        profile_id: profile.view.id.clone(),
        endpoint,
        model: crate::ai::grounding::vector::DEFAULT_EMBEDDING_MODEL.to_string(),
        api_key: Some(api_key),
        dimensions: crate::ai::grounding::vector::DEFAULT_EMBEDDING_DIMENSIONS,
    }))
}

pub fn migrate_embedding_source(db: &Db, secrets: &Secrets) -> AppResult<()> {
    let should_migrate = {
        let conn = db.reader();
        let vector_enabled = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'ai_vector_retrieval'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .is_some_and(|value| value == "true");
        let explicit = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'ai_embedding_configured'",
                [],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .is_some_and(|value| value == "true");
        vector_enabled && !explicit
    };
    if !should_migrate {
        return Ok(());
    }
    let Some(source) = embedding_source(db, secrets)? else {
        return Ok(());
    };
    if let Some(credential) = credentials_for(db, &source.profile_id)?.into_iter().next() {
        let _ = secrets.copy_local(
            &credential.secret_ref,
            crate::ai::grounding::vector::EMBEDDING_SECRET_REF,
        )?;
    }
    let conn = db
        .conn
        .lock()
        .map_err(|error| AppError::Other(error.to_string()))?;
    let dimensions = source.dimensions.to_string();
    for (key, value) in [
        ("ai_embedding_endpoint", source.endpoint.as_str()),
        ("ai_embedding_model", source.model.as_str()),
        ("ai_embedding_dimensions", dimensions.as_str()),
        ("ai_embedding_configured", "true"),
    ] {
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
    }
    Ok(())
}

pub fn list_profiles(db: &Db) -> AppResult<Vec<AiProfileView>> {
    profiles(db, false).map(|items| items.into_iter().map(|item| item.view).collect())
}

fn normalize_profile_config(
    label: String,
    provider: String,
    auth_mode: String,
    base_url: Option<String>,
    model: String,
    temperature: f64,
    keep_alive: Option<String>,
) -> AppResult<NormalizedProfileConfig> {
    let label = label.trim().to_string();
    let provider = provider.trim().to_ascii_lowercase();
    let auth_mode = auth_mode.trim().to_ascii_lowercase();
    let model = model.trim().to_string();
    let base_url = base_url
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let keep_alive = keep_alive
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    if label.is_empty() || label.chars().count() > 100 {
        return Err(AppError::Other("AI_PROFILE_LABEL_INVALID".to_string()));
    }
    if !matches!(
        provider.as_str(),
        "openai" | "anthropic" | "ollama" | "custom"
    ) {
        return Err(AppError::Other("AI_PROVIDER_UNSUPPORTED".to_string()));
    }
    if !matches!(auth_mode.as_str(), "api_key" | "oauth")
        || (auth_mode == "oauth" && provider != "openai")
    {
        return Err(AppError::Other("AI_AUTH_MODE_INVALID".to_string()));
    }
    if model.is_empty() || model.chars().count() > 200 {
        return Err(AppError::Other("AI_MODEL_INVALID".to_string()));
    }
    if !temperature.is_finite() || !(0.0..=2.0).contains(&temperature) {
        return Err(AppError::Other("AI_TEMPERATURE_INVALID".to_string()));
    }
    if provider == "custom" && base_url.is_none() {
        return Err(AppError::Other("AI_CUSTOM_BASE_URL_REQUIRED".to_string()));
    }
    if let Some(url) = base_url.as_deref() {
        let parsed = reqwest::Url::parse(url)
            .map_err(|_| AppError::Other("AI_BASE_URL_INVALID".to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(AppError::Other("AI_BASE_URL_INVALID".to_string()));
        }
    }

    Ok((
        label,
        provider,
        auth_mode,
        base_url,
        model,
        temperature,
        keep_alive,
    ))
}

#[allow(clippy::too_many_arguments)]
pub fn create_profile(
    db: &Db,
    label: String,
    provider: String,
    auth_mode: String,
    base_url: Option<String>,
    model: String,
    temperature: f64,
    keep_alive: Option<String>,
    enabled: bool,
) -> AppResult<AiProfileView> {
    let (label, provider, auth_mode, base_url, model, temperature, keep_alive) =
        normalize_profile_config(
            label,
            provider,
            auth_mode,
            base_url,
            model,
            temperature,
            keep_alive,
        )?;
    let id = uuid::Uuid::new_v4().to_string();
    let timestamp = now();
    let mut conn = db
        .conn
        .lock()
        .map_err(|error| AppError::Other(error.to_string()))?;
    let tx = conn.transaction()?;
    let priority: i64 = tx.query_row(
        "SELECT COALESCE(MAX(priority) + 1, 0) FROM ai_profiles",
        [],
        |row| row.get(0),
    )?;
    tx.execute(
        "INSERT INTO ai_profiles (id, label, provider, auth_mode, base_url, model, temperature, keep_alive, enabled, priority, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
        params![id, label, provider, auth_mode, base_url, model, temperature, keep_alive, enabled as i64, priority, timestamp],
    )?;
    tx.commit()?;
    drop(conn);
    Ok(profile_by_id(db, &id)?.view)
}

pub fn duplicate_profile(db: &Db, id: &str, label: Option<String>) -> AppResult<AiProfileView> {
    let source = profile_by_id(db, id)?.view;
    create_profile(
        db,
        label.unwrap_or_else(|| format!("{} copy", source.label)),
        source.provider,
        source.auth_mode,
        source.base_url,
        source.model,
        source.temperature,
        source.keep_alive,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn save_profile(
    db: &Db,
    id: String,
    label: String,
    provider: String,
    auth_mode: String,
    base_url: Option<String>,
    model: String,
    temperature: f64,
    keep_alive: Option<String>,
) -> AppResult<AiProfileView> {
    let existing = profile_by_id(db, &id)?.view;
    let (label, provider, auth_mode, base_url, model, temperature, keep_alive) =
        normalize_profile_config(
            label,
            provider,
            auth_mode,
            base_url,
            model,
            temperature,
            keep_alive,
        )?;
    let credential_health_stale = existing.provider != provider
        || existing.auth_mode != auth_mode
        || existing.base_url != base_url;
    let profile_health_stale = credential_health_stale
        || existing.model != model
        || existing.temperature != temperature
        || existing.keep_alive != keep_alive;
    let timestamp = now();
    let mut conn = db.conn.lock().map_err(|e| AppError::Other(e.to_string()))?;
    let tx = conn.transaction()?;
    let changed = tx.execute(
        "UPDATE ai_profiles SET label = ?1, provider = ?2, auth_mode = ?3, base_url = ?4, model = ?5, temperature = ?6, keep_alive = ?7, state = CASE WHEN ?8 = 1 THEN 'active' ELSE state END, cooldown_until = CASE WHEN ?8 = 1 THEN NULL ELSE cooldown_until END, last_error_kind = CASE WHEN ?8 = 1 THEN NULL ELSE last_error_kind END, last_used_at = CASE WHEN ?8 = 1 THEN NULL ELSE last_used_at END, last_latency_ms = CASE WHEN ?8 = 1 THEN NULL ELSE last_latency_ms END, updated_at = ?9 WHERE id = ?10",
        params![label, provider, auth_mode, base_url, model, temperature, keep_alive, profile_health_stale as i64, timestamp, id],
    )?;
    if changed != 1 {
        return Err(AppError::Other("AI_PROFILE_NOT_FOUND".to_string()));
    }
    if credential_health_stale {
        tx.execute(
            "UPDATE ai_credentials SET state = 'active', cooldown_until = NULL, last_error_kind = NULL, last_used_at = NULL, updated_at = ?1 WHERE profile_id = ?2",
            params![timestamp, id],
        )?;
    }
    tx.commit()?;
    drop(conn);
    Ok(profile_by_id(db, &id)?.view)
}

pub fn set_profile_enabled(db: &Db, id: &str, enabled: bool) -> AppResult<()> {
    let changed = db
        .conn
        .lock()
        .map_err(|error| AppError::Other(error.to_string()))?
        .execute(
            "UPDATE ai_profiles SET enabled = ?1, updated_at = ?2 WHERE id = ?3",
            params![enabled as i64, now(), id],
        )?;
    if changed != 1 {
        return Err(AppError::Other("AI_PROFILE_NOT_FOUND".to_string()));
    }
    Ok(())
}

pub fn reorder_profiles(db: &Db, ids: &[String]) -> AppResult<()> {
    let unique: HashSet<&str> = ids.iter().map(String::as_str).collect();
    if unique.len() != ids.len() {
        return Err(AppError::Other("AI_PROFILE_ORDER_INVALID".to_string()));
    }
    let existing = list_profiles(db)?;
    let existing_ids: HashSet<&str> = existing.iter().map(|profile| profile.id.as_str()).collect();
    if unique != existing_ids {
        return Err(AppError::Other("AI_PROFILE_ORDER_INCOMPLETE".to_string()));
    }

    let timestamp = now();
    let mut conn = db
        .conn
        .lock()
        .map_err(|error| AppError::Other(error.to_string()))?;
    let tx = conn.transaction()?;
    for (priority, id) in ids.iter().enumerate() {
        let changed = tx.execute(
            "UPDATE ai_profiles SET priority = ?1, updated_at = ?2 WHERE id = ?3",
            params![priority as i64, timestamp, id],
        )?;
        if changed != 1 {
            return Err(AppError::Other("AI_PROFILE_NOT_FOUND".to_string()));
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn delete_profile(db: &Db, secrets: &Secrets, id: &str) -> AppResult<()> {
    profile_by_id(db, id)?;
    let credentials = all_credentials_for(db, id)?;
    let snapshots = credentials
        .iter()
        .map(|credential| {
            secrets
                .snapshot_state(&credential.secret_ref)
                .map(|snapshot| (credential.secret_ref.clone(), snapshot))
        })
        .collect::<AppResult<Vec<_>>>()?;
    let mut removed = Vec::with_capacity(snapshots.len());
    for (secret_ref, snapshot) in &snapshots {
        if let Err(error) = secrets.delete(secret_ref) {
            let mut rollback_errors = Vec::new();
            for (_, removed_snapshot) in &removed {
                if let Err(restore_error) = secrets.restore_state(removed_snapshot) {
                    log::error!(
                        "ai router: failed to restore local credential after delete rollback: {restore_error}"
                    );
                    rollback_errors.push(restore_error.to_string());
                }
            }
            if !rollback_errors.is_empty() {
                return Err(compensation_failure(
                    "AI_PROFILE_SECRET_DELETE_ROLLBACK_FAILED",
                    &error,
                    &rollback_errors.join(" | "),
                ));
            }
            return Err(error);
        }
        removed.push((secret_ref.clone(), snapshot.clone()));
    }

    let delete_result = (|| -> AppResult<()> {
        let mut conn = db
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM ai_credentials WHERE profile_id = ?1",
            params![id],
        )?;
        let changed = tx.execute("DELETE FROM ai_profiles WHERE id = ?1", params![id])?;
        if changed != 1 {
            return Err(AppError::Other("AI_PROFILE_NOT_FOUND".to_string()));
        }
        tx.commit()?;
        Ok(())
    })();
    if let Err(error) = delete_result {
        let mut rollback_errors = Vec::new();
        for (_, snapshot) in &removed {
            if let Err(restore_error) = secrets.restore_state(snapshot) {
                log::error!(
                    "ai router: failed to restore local credential after metadata rollback: {restore_error}"
                );
                rollback_errors.push(restore_error.to_string());
            }
        }
        if !rollback_errors.is_empty() {
            return Err(compensation_failure(
                "AI_PROFILE_METADATA_DELETE_ROLLBACK_FAILED",
                &error,
                &rollback_errors.join(" | "),
            ));
        }
        return Err(error);
    }
    Ok(())
}

pub fn add_credential(
    db: &Db,
    secrets: &Secrets,
    profile_id: String,
    label: String,
    value: String,
) -> AppResult<AiCredentialView> {
    profile_by_id(db, &profile_id)?;
    let value = value.trim();
    if value.is_empty() {
        return Err(AppError::Other("AI_API_KEY_EMPTY".to_string()));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let secret_ref = format!("ai_api_key/{id}");
    let timestamp = now();
    secrets.set(&secret_ref, value)?;
    let insert_result = (|| -> AppResult<i64> {
        let mut conn = db
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let tx = conn.transaction()?;
        let priority: i64 = tx.query_row(
            "SELECT COALESCE(MAX(priority) + 1, 0) FROM ai_credentials WHERE profile_id = ?1",
            params![profile_id],
            |row| row.get(0),
        )?;
        tx.execute(
            "INSERT INTO ai_credentials (id, profile_id, label, secret_ref, masked_suffix, enabled, priority, state, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, 'active', ?7, ?7)",
            params![id, profile_id, if label.trim().is_empty() { "API key" } else { label.trim() }, secret_ref, suffix(value), priority, timestamp],
        )?;
        tx.commit()?;
        Ok(priority)
    })();
    let priority = match insert_result {
        Ok(priority) => priority,
        Err(error) => {
            if let Err(cleanup_error) = secrets.delete(&secret_ref) {
                return Err(compensation_failure(
                    "AI_CREDENTIAL_ADD_ROLLBACK_FAILED",
                    &error,
                    &cleanup_error,
                ));
            }
            return Err(error);
        }
    };
    Ok(AiCredentialView {
        id,
        profile_id,
        label: if label.trim().is_empty() {
            "API key".to_string()
        } else {
            label
        },
        masked_suffix: suffix(value),
        enabled: true,
        priority,
        state: "active".to_string(),
        cooldown_until: None,
        last_error_kind: None,
        last_used_at: None,
    })
}

pub fn replace_credential(db: &Db, secrets: &Secrets, id: &str, value: &str) -> AppResult<()> {
    let value = value.trim();
    if value.is_empty() {
        return Err(AppError::Other("AI_API_KEY_EMPTY".to_string()));
    }
    let secret_ref = credential_by_id(db, id)?.secret_ref;
    // Preserve the complete local state for rollback. This includes a pending
    // legacy-import marker when the user replaces a credential without first
    // granting access to its old per-item Keychain record.
    let previous = secrets.snapshot_state(&secret_ref)?;
    secrets.set(&secret_ref, value)?;
    let conn = db.conn.lock().map_err(|e| AppError::Other(e.to_string()))?;
    if let Err(error) = conn.execute("UPDATE ai_credentials SET masked_suffix = ?1, state = 'active', cooldown_until = NULL, last_error_kind = NULL, updated_at = ?2 WHERE id = ?3", params![suffix(value), now(), id]) {
        if let Err(restore_error) = secrets.restore_state(&previous) {
            return Err(compensation_failure(
                "AI_CREDENTIAL_REPLACE_ROLLBACK_FAILED",
                &error,
                &restore_error,
            ));
        }
        return Err(error.into());
    }
    Ok(())
}

pub fn set_credential_enabled(db: &Db, id: &str, enabled: bool) -> AppResult<()> {
    let changed = db
        .conn
        .lock()
        .map_err(|e| AppError::Other(e.to_string()))?
        .execute(
            "UPDATE ai_credentials SET enabled = ?1, updated_at = ?2 WHERE id = ?3",
            params![enabled as i64, now(), id],
        )?;
    if changed != 1 {
        return Err(AppError::Other("AI_CREDENTIAL_NOT_FOUND".to_string()));
    }
    Ok(())
}

pub fn reorder_credentials(db: &Db, ids: &[String]) -> AppResult<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let unique: HashSet<&str> = ids.iter().map(String::as_str).collect();
    if unique.len() != ids.len() {
        return Err(AppError::Other("AI_CREDENTIAL_ORDER_INVALID".to_string()));
    }
    let first = credential_by_id(db, &ids[0])?;
    let existing = all_credentials_for(db, &first.view.profile_id)?;
    let existing_ids: HashSet<&str> = existing
        .iter()
        .map(|credential| credential.view.id.as_str())
        .collect();
    if unique != existing_ids {
        return Err(AppError::Other(
            "AI_CREDENTIAL_ORDER_INCOMPLETE".to_string(),
        ));
    }

    let timestamp = now();
    let mut conn = db.conn.lock().map_err(|e| AppError::Other(e.to_string()))?;
    let tx = conn.transaction()?;
    for (priority, id) in ids.iter().enumerate() {
        let changed = tx.execute(
            "UPDATE ai_credentials SET priority = ?1, updated_at = ?2 WHERE id = ?3",
            params![priority as i64, timestamp, id],
        )?;
        if changed != 1 {
            return Err(AppError::Other("AI_CREDENTIAL_NOT_FOUND".to_string()));
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn delete_credential(db: &Db, secrets: &Secrets, id: &str) -> AppResult<()> {
    let secret_ref = credential_by_id(db, id)?.secret_ref;
    let snapshot = secrets.snapshot_state(&secret_ref)?;
    secrets.delete(&secret_ref)?;
    let delete_result = db
        .conn
        .lock()
        .map_err(|e| AppError::Other(e.to_string()))?
        .execute("DELETE FROM ai_credentials WHERE id = ?1", params![id]);
    let changed = match delete_result {
        Ok(changed) => changed,
        Err(error) => {
            if let Err(restore_error) = secrets.restore_state(&snapshot) {
                return Err(compensation_failure(
                    "AI_CREDENTIAL_DELETE_ROLLBACK_FAILED",
                    &error,
                    &restore_error,
                ));
            }
            return Err(error.into());
        }
    };
    if changed != 1 {
        let not_found = AppError::Other("AI_CREDENTIAL_NOT_FOUND".to_string());
        if let Err(restore_error) = secrets.restore_state(&snapshot) {
            return Err(compensation_failure(
                "AI_CREDENTIAL_DELETE_ROLLBACK_FAILED",
                &not_found,
                &restore_error,
            ));
        }
        return Err(not_found);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn timed_stream_once(
    app: &AppHandle,
    profile: &AiProfile,
    api_key: &str,
    oauth_account_id: Option<&str>,
    messages: &[ChatMessage],
    event_name: &str,
    max_tokens: Option<u32>,
) -> (AppResult<()>, Option<u64>, u64) {
    let started = Instant::now();
    let emitted = Arc::new(AtomicBool::new(false));
    let (_cancel_guard, mut cancel) = watch::channel(false);
    let mut stream = Box::pin(stream_once(
        app,
        profile,
        api_key,
        oauth_account_id,
        messages,
        event_name,
        max_tokens,
        Arc::clone(&emitted),
        &mut cancel,
    ));
    let mut ticker = tokio::time::interval(Duration::from_millis(2));
    let mut first_response_ms = None;
    let result = loop {
        tokio::select! {
            result = &mut stream => break result,
            _ = ticker.tick(), if first_response_ms.is_none() => {
                if emitted.load(Ordering::Relaxed) {
                    first_response_ms = Some(started.elapsed().as_millis() as u64);
                }
            }
        }
    };
    let total_ms = started.elapsed().as_millis() as u64;
    if first_response_ms.is_none() && emitted.load(Ordering::Relaxed) {
        first_response_ms = Some(total_ms);
    }
    (result, first_response_ms, total_ms)
}

fn connection_test_result(
    profile: &AiProfile,
    success: bool,
    credential_id: Option<String>,
    first_response_ms: Option<u64>,
    total_ms: u64,
    error_kind: Option<&str>,
    attempts: Vec<AiConnectionTestAttempt>,
) -> AiConnectionTestResult {
    let attempt_count = attempts.len();
    AiConnectionTestResult {
        success,
        profile_id: profile.view.id.clone(),
        provider: profile.view.provider.clone(),
        model: profile.view.model.clone(),
        credential_id,
        first_response_ms,
        total_ms,
        tested_at: now(),
        attempt_count,
        error_kind: error_kind.map(str::to_string),
        attempts,
    }
}

fn connection_test_attempt(
    credential: Option<&AiCredential>,
    error: Option<&AppError>,
    error_kind: Option<AiErrorKind>,
    latency_ms: u64,
    request_sent: bool,
    secret: Option<&str>,
) -> AiConnectionTestAttempt {
    AiConnectionTestAttempt {
        credential_id: credential.map(|value| value.view.id.clone()),
        credential_label: credential.map(|value| value.view.label.clone()),
        error_kind: error_kind.map(AiErrorKind::as_str).map(str::to_string),
        error_detail: error.map(|value| sanitized_error_detail(value, secret)),
        latency_ms,
        request_sent,
    }
}

#[allow(clippy::too_many_arguments)]
fn profile_for_connection_test(
    db: &Db,
    profile_id: &str,
    provider: String,
    auth_mode: String,
    base_url: Option<String>,
    model: String,
    temperature: f64,
    keep_alive: Option<String>,
) -> AppResult<(AiProfile, bool)> {
    let mut profile = profile_by_id(db, profile_id)?;
    let (_, provider, auth_mode, base_url, model, temperature, keep_alive) =
        normalize_profile_config(
            profile.view.label.clone(),
            provider,
            auth_mode,
            base_url,
            model,
            temperature,
            keep_alive,
        )?;
    let uses_saved_config = profile.view.provider == provider
        && profile.view.auth_mode == auth_mode
        && profile.view.base_url == base_url
        && profile.view.model == model
        && profile.view.temperature == temperature
        && profile.view.keep_alive == keep_alive;
    profile.view.provider = provider;
    profile.view.auth_mode = auth_mode;
    profile.view.base_url = base_url;
    profile.view.model = model;
    profile.view.temperature = temperature;
    profile.view.keep_alive = keep_alive;
    Ok((profile, uses_saved_config))
}

#[allow(clippy::too_many_arguments)]
pub async fn test_profile(
    app: &AppHandle,
    db: &Db,
    secrets: &Secrets,
    profile_id: &str,
    provider: String,
    auth_mode: String,
    base_url: Option<String>,
    model: String,
    temperature: f64,
    keep_alive: Option<String>,
) -> AppResult<AiConnectionTestResult> {
    let (profile, record_health) = profile_for_connection_test(
        db,
        profile_id,
        provider,
        auth_mode,
        base_url,
        model,
        temperature,
        keep_alive,
    )?;
    let messages = [ChatMessage {
        role: "user".to_string(),
        content: "Reply with OK.".to_string(),
    }];
    let overall_started = Instant::now();

    if profile.view.auth_mode == "oauth" && profile.view.provider == "openai" {
        let (token, account_id) = match crate::ai::oauth::get_valid_token(secrets).await {
            Ok(token) => token,
            Err(error) => {
                let kind = classify_error(&error);
                let total_ms = overall_started.elapsed().as_millis() as u64;
                if record_health {
                    update_profile_health(db, &profile, Some(kind), retry_after_ms(&error), None);
                }
                return Ok(connection_test_result(
                    &profile,
                    false,
                    None,
                    None,
                    total_ms,
                    Some(kind.as_str()),
                    vec![connection_test_attempt(
                        None,
                        Some(&error),
                        Some(kind),
                        total_ms,
                        false,
                        None,
                    )],
                ));
            }
        };
        let event_name = format!("ai-profile-test-{}", uuid::Uuid::new_v4());
        let (result, first_response_ms, _) = timed_stream_once(
            app,
            &profile,
            &token,
            account_id.as_deref(),
            &messages,
            &event_name,
            connection_test_token_limit(&profile),
        )
        .await;
        let total_ms = overall_started.elapsed().as_millis() as u64;
        let kind = result.as_ref().err().map(classify_error);
        let attempt = connection_test_attempt(
            None,
            result.as_ref().err(),
            kind,
            total_ms,
            true,
            Some(&token),
        );
        if record_health {
            update_profile_health(
                db,
                &profile,
                kind,
                result.as_ref().err().and_then(retry_after_ms),
                Some(total_ms),
            );
        }
        return Ok(connection_test_result(
            &profile,
            result.is_ok(),
            None,
            first_response_ms,
            total_ms,
            kind.map(AiErrorKind::as_str),
            vec![attempt],
        ));
    }

    if profile.view.provider == "ollama" {
        let event_name = format!("ai-profile-test-{}", uuid::Uuid::new_v4());
        let (result, first_response_ms, _) = timed_stream_once(
            app,
            &profile,
            "",
            None,
            &messages,
            &event_name,
            connection_test_token_limit(&profile),
        )
        .await;
        let total_ms = overall_started.elapsed().as_millis() as u64;
        let kind = result.as_ref().err().map(classify_error);
        let attempt =
            connection_test_attempt(None, result.as_ref().err(), kind, total_ms, true, None);
        if record_health {
            update_profile_health(
                db,
                &profile,
                kind,
                result.as_ref().err().and_then(retry_after_ms),
                Some(total_ms),
            );
        }
        return Ok(connection_test_result(
            &profile,
            result.is_ok(),
            None,
            first_response_ms,
            total_ms,
            kind.map(AiErrorKind::as_str),
            vec![attempt],
        ));
    }

    let candidates: Vec<_> = all_credentials_for(db, profile_id)?
        .into_iter()
        .filter(|credential| credential.view.enabled)
        .collect();
    if candidates.is_empty() {
        if record_health {
            update_profile_health(db, &profile, Some(AiErrorKind::NotConfigured), None, None);
        }
        return Ok(connection_test_result(
            &profile,
            false,
            None,
            None,
            overall_started.elapsed().as_millis() as u64,
            Some("not_configured"),
            Vec::new(),
        ));
    }

    let mut attempts = Vec::new();
    let mut last_credential_id = None;
    let mut last_first_response_ms = None;
    let mut last_error_kind = Some(AiErrorKind::CredentialInvalid);
    let mut last_retry_after = None;
    for credential in candidates {
        let attempt_started = Instant::now();
        last_credential_id = Some(credential.view.id.clone());
        last_first_response_ms = None;
        let key = match secrets.get(&credential.secret_ref) {
            Ok(Some(key)) if !key.trim().is_empty() => key,
            result => {
                let error = match result {
                    Err(error) => error,
                    _ => AppError::Other("AI_CREDENTIAL_UNAVAILABLE".to_string()),
                };
                if record_health {
                    update_credential_health(
                        db,
                        &credential,
                        Some(AiErrorKind::CredentialInvalid),
                        None,
                    );
                }
                attempts.push(connection_test_attempt(
                    Some(&credential),
                    Some(&error),
                    Some(AiErrorKind::CredentialInvalid),
                    attempt_started.elapsed().as_millis() as u64,
                    false,
                    None,
                ));
                last_error_kind = Some(AiErrorKind::CredentialInvalid);
                last_retry_after = None;
                continue;
            }
        };
        let event_name = format!("ai-profile-test-{}", uuid::Uuid::new_v4());
        let (result, first_response_ms, attempt_ms) = timed_stream_once(
            app,
            &profile,
            &key,
            None,
            &messages,
            &event_name,
            connection_test_token_limit(&profile),
        )
        .await;
        last_first_response_ms = first_response_ms;
        match result {
            Ok(()) => {
                let total_ms = overall_started.elapsed().as_millis() as u64;
                attempts.push(connection_test_attempt(
                    Some(&credential),
                    None,
                    None,
                    attempt_ms,
                    true,
                    Some(&key),
                ));
                if record_health {
                    update_credential_health(db, &credential, None, None);
                    update_profile_health(db, &profile, None, None, Some(total_ms));
                }
                return Ok(connection_test_result(
                    &profile,
                    true,
                    Some(credential.view.id),
                    first_response_ms,
                    total_ms,
                    None,
                    attempts,
                ));
            }
            Err(error) => {
                let kind = classify_error(&error);
                let retry_after = retry_after_ms(&error);
                if record_health {
                    update_credential_health(db, &credential, Some(kind), retry_after);
                }
                attempts.push(connection_test_attempt(
                    Some(&credential),
                    Some(&error),
                    Some(kind),
                    attempt_ms,
                    true,
                    Some(&key),
                ));
                last_error_kind = Some(kind);
                last_retry_after = retry_after;
                if !kind.retryable() {
                    break;
                }
            }
        }
    }
    let total_ms = overall_started.elapsed().as_millis() as u64;
    if record_health {
        if let Some(kind) = last_error_kind {
            update_profile_health(db, &profile, Some(kind), last_retry_after, Some(total_ms));
        }
    }
    Ok(connection_test_result(
        &profile,
        false,
        last_credential_id,
        last_first_response_ms,
        total_ms,
        last_error_kind.map(AiErrorKind::as_str),
        attempts,
    ))
}

pub fn has_configured_service(db: &Db) -> bool {
    let Ok(profiles) = profiles(db, true) else {
        return false;
    };
    profiles.into_iter().any(|profile| {
        if profile.view.provider == "ollama" {
            return true;
        }
        if profile.view.auth_mode == "oauth" && profile.view.provider == "openai" {
            return true;
        }
        all_credentials_for(db, &profile.view.id)
            .unwrap_or_default()
            .into_iter()
            .find(|credential| credential.view.enabled)
            .is_some()
    })
}

/// Validate that a routed stream has a locally readable credential before the
/// command detaches into a background task.
pub fn ensure_stream_credentials_accessible(db: &Db, secrets: &Secrets) -> AppResult<()> {
    let timestamp = now();
    for profile in profiles(db, true)?.into_iter().filter(|profile| {
        profile
            .view
            .cooldown_until
            .is_none_or(|deadline| deadline <= timestamp)
    }) {
        if profile.view.provider == "ollama" {
            return Ok(());
        }
        if profile.view.auth_mode == "oauth" && profile.view.provider == "openai" {
            // Probe only the first required token here. The detached OAuth path
            // reads the remaining values from the same local database.
            if secrets
                .get("oauth_access_token")?
                .is_some_and(|value| !value.trim().is_empty())
            {
                return Ok(());
            }
            continue;
        }
        for credential in credentials_for(db, &profile.view.id)? {
            if secrets
                .get(&credential.secret_ref)?
                .is_some_and(|value| !value.trim().is_empty())
            {
                return Ok(());
            }
        }
    }
    Ok(())
}

pub async fn test_credential(
    app: &AppHandle,
    db: &Db,
    secrets: &Secrets,
    credential_id: &str,
) -> AppResult<()> {
    let credential = credential_by_id(db, credential_id)?;
    let profile = profile_by_id(db, &credential.view.profile_id)?;
    let key = secrets
        .get(&credential.secret_ref)?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::Other("AI_CREDENTIAL_UNAVAILABLE".to_string()))?;
    let messages = [ChatMessage {
        role: "user".to_string(),
        content: "Reply with OK.".to_string(),
    }];
    let event_name = format!("ai-credential-test-{}", uuid::Uuid::new_v4());
    let (result, _, total_ms) = timed_stream_once(
        app,
        &profile,
        &key,
        None,
        &messages,
        &event_name,
        connection_test_token_limit(&profile),
    )
    .await;
    update_credential_health(
        db,
        &credential,
        result.as_ref().err().map(classify_error),
        result.as_ref().err().and_then(retry_after_ms),
    );
    update_profile_health(
        db,
        &profile,
        result.as_ref().err().map(classify_error),
        result.as_ref().err().and_then(retry_after_ms),
        Some(total_ms),
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn dropped_cancel_sender_does_not_cancel_request() {
        let receiver = watch::channel(false).1;
        let mut receiver = receiver;

        assert!(
            tokio::time::timeout(Duration::from_millis(20), wait_cancelled(&mut receiver),)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn live_cancel_sender_wakes_request() {
        let (sender, mut receiver) = watch::channel(false);
        sender.send(true).unwrap();

        tokio::time::timeout(Duration::from_millis(20), wait_cancelled(&mut receiver))
            .await
            .expect("cancellation should wake the request");
    }

    #[test]
    fn cancelled_errors_are_classified_separately_from_network_failures() {
        let error = AppError::Other("AI_REQUEST_CANCELLED".to_string());

        assert_eq!(classify_error(&error), AiErrorKind::Cancelled);
        assert!(!classify_error(&error).retryable());
    }

    #[test]
    fn profile_health_distinguishes_invalid_and_unconfigured_credentials() {
        assert_eq!(
            profile_health_state(Some(AiErrorKind::CredentialInvalid), None, 1_000),
            Some(("invalid", None))
        );
        assert_eq!(
            profile_health_state(Some(AiErrorKind::NotConfigured), None, 1_000),
            Some(("unavailable", None))
        );
        assert_eq!(
            profile_health_state(Some(AiErrorKind::Cancelled), None, 1_000),
            None
        );
    }

    #[test]
    fn connection_attempt_serialization_is_diagnostic_and_redacted() {
        let error = AppError::Other(format!(
            "provider rejected secret-token {}",
            "x".repeat(400)
        ));
        let attempt = connection_test_attempt(
            None,
            Some(&error),
            Some(AiErrorKind::Auth),
            42,
            true,
            Some("secret-token"),
        );
        let value = serde_json::to_value(&attempt).unwrap();

        assert_eq!(value["error_kind"], "auth");
        assert_eq!(value["latency_ms"], 42);
        assert_eq!(value["request_sent"], true);
        let detail = value["error_detail"].as_str().unwrap();
        assert!(!detail.contains("secret-token"));
        assert!(detail.chars().count() <= 300);
    }

    async fn model_list_server(
        responses: Vec<(&'static str, &'static str)>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut requests = Vec::with_capacity(responses.len());
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 2048];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let read = stream.read(&mut buffer).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                requests.push(String::from_utf8_lossy(&request).into_owned());
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        (format!("http://{address}"), handle)
    }

    fn model_list_test_profile(db: &Db, base_url: String) -> AiProfileView {
        create_profile(
            db,
            "Models".to_string(),
            "custom".to_string(),
            "api_key".to_string(),
            Some(base_url),
            "placeholder".to_string(),
            0.2,
            None,
            true,
        )
        .unwrap()
    }

    fn profile(provider: &str, base_url: Option<&str>) -> AiProfileView {
        AiProfileView {
            id: "profile".to_string(),
            label: "Profile".to_string(),
            provider: provider.to_string(),
            auth_mode: "api_key".to_string(),
            base_url: base_url.map(str::to_string),
            model: "model".to_string(),
            temperature: 0.2,
            keep_alive: None,
            enabled: true,
            priority: 0,
            state: "active".to_string(),
            cooldown_until: None,
            last_error_kind: None,
            last_used_at: None,
            last_latency_ms: None,
        }
    }

    #[test]
    fn model_endpoints_normalize_provider_base_urls() {
        assert_eq!(
            models_endpoint(&profile("openai", None)).unwrap(),
            "https://api.openai.com/v1/models"
        );
        assert_eq!(
            models_endpoint(&profile("custom", Some("https://gateway.example/v1/"))).unwrap(),
            "https://gateway.example/v1/models"
        );
        assert_eq!(
            models_endpoint(&profile("anthropic", Some("https://api.anthropic.com/"))).unwrap(),
            "https://api.anthropic.com/v1/models"
        );
        assert_eq!(
            models_endpoint(&profile("ollama", Some("http://localhost:11434/api/"))).unwrap(),
            "http://localhost:11434/api/tags"
        );
    }

    #[test]
    fn openai_model_ids_are_trimmed_sorted_and_deduplicated() {
        let value = serde_json::json!({
            "data": [
                {"id": " z-model "},
                {"id": "a-model"},
                {"id": "a-model"},
                {"id": ""},
                {"unexpected": true}
            ]
        });
        assert_eq!(
            parse_model_ids("openai", &value).unwrap(),
            vec!["a-model".to_string(), "z-model".to_string()]
        );
    }

    #[test]
    fn ollama_model_ids_accept_model_and_legacy_name_fields() {
        let value = serde_json::json!({
            "models": [
                {"model": "qwen3:latest"},
                {"name": "llama3.2:latest"}
            ]
        });
        assert_eq!(
            parse_model_ids("ollama", &value).unwrap(),
            vec!["llama3.2:latest".to_string(), "qwen3:latest".to_string()]
        );
    }

    #[test]
    fn malformed_or_empty_model_lists_are_rejected() {
        assert!(parse_model_ids("openai", &serde_json::json!({"models": []})).is_err());
        assert!(parse_model_ids("openai", &serde_json::json!({"data": []})).is_err());
    }

    #[test]
    fn deleting_credential_removes_metadata_and_local_secret() {
        let directory = tempfile::TempDir::new().unwrap();
        let db = Db::init(directory.path()).unwrap();
        let secrets = Secrets::init_in_memory().unwrap();
        let profile = create_profile(
            &db,
            "Delete test".to_string(),
            "custom".to_string(),
            "api_key".to_string(),
            Some("https://api.example/v1".to_string()),
            "model".to_string(),
            0.2,
            None,
            true,
        )
        .unwrap();
        let credential = add_credential(
            &db,
            &secrets,
            profile.id,
            "Primary".to_string(),
            "secret".to_string(),
        )
        .unwrap();
        let secret_ref = credential_by_id(&db, &credential.id).unwrap().secret_ref;

        delete_credential(&db, &secrets, &credential.id).unwrap();

        assert!(credential_by_id(&db, &credential.id).is_err());
        assert_eq!(secrets.get(&secret_ref).unwrap(), None);
    }

    #[test]
    fn deleting_profile_removes_all_local_credentials() {
        let directory = tempfile::TempDir::new().unwrap();
        let db = Db::init(directory.path()).unwrap();
        let secrets = Secrets::init_in_memory().unwrap();
        let profile = create_profile(
            &db,
            "Delete profile".to_string(),
            "custom".to_string(),
            "api_key".to_string(),
            Some("https://api.example/v1".to_string()),
            "model".to_string(),
            0.2,
            None,
            true,
        )
        .unwrap();
        let first = add_credential(
            &db,
            &secrets,
            profile.id.clone(),
            "First".to_string(),
            "first-secret".to_string(),
        )
        .unwrap();
        let second = add_credential(
            &db,
            &secrets,
            profile.id.clone(),
            "Second".to_string(),
            "second-secret".to_string(),
        )
        .unwrap();
        let refs = [first.id, second.id].map(|id| credential_by_id(&db, &id).unwrap().secret_ref);

        delete_profile(&db, &secrets, &profile.id).unwrap();

        assert!(profile_by_id(&db, &profile.id).is_err());
        for secret_ref in refs {
            assert_eq!(secrets.get(&secret_ref).unwrap(), None);
        }
    }

    #[test]
    fn add_credential_reports_primary_and_cleanup_failures() {
        let directory = tempfile::TempDir::new().unwrap();
        let db = Db::init(directory.path()).unwrap();
        let secrets = Secrets::init_in_memory().unwrap();
        let profile = create_profile(
            &db,
            "Rollback add".to_string(),
            "custom".to_string(),
            "api_key".to_string(),
            Some("https://api.example/v1".to_string()),
            "model".to_string(),
            0.2,
            None,
            true,
        )
        .unwrap();
        db.conn
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER fail_credential_insert
                 BEFORE INSERT ON ai_credentials
                 BEGIN SELECT RAISE(ABORT, 'forced metadata insert failure'); END;",
            )
            .unwrap();
        secrets.fail_next_delete_for_test();

        let error = add_credential(
            &db,
            &secrets,
            profile.id,
            "Primary".to_string(),
            "secret".to_string(),
        )
        .unwrap_err()
        .to_string();

        assert!(error.starts_with("AI_CREDENTIAL_ADD_ROLLBACK_FAILED:"));
        assert!(error.contains("forced metadata insert failure"));
        assert!(error.contains("TEST_SECRET_DELETE_FAILED"));
    }

    #[test]
    fn replace_credential_reports_primary_and_restore_failures() {
        let directory = tempfile::TempDir::new().unwrap();
        let db = Db::init(directory.path()).unwrap();
        let secrets = Secrets::init_in_memory().unwrap();
        let profile = create_profile(
            &db,
            "Rollback replace".to_string(),
            "custom".to_string(),
            "api_key".to_string(),
            Some("https://api.example/v1".to_string()),
            "model".to_string(),
            0.2,
            None,
            true,
        )
        .unwrap();
        let credential = add_credential(
            &db,
            &secrets,
            profile.id,
            "Primary".to_string(),
            "old-secret".to_string(),
        )
        .unwrap();
        db.conn
            .lock()
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER fail_credential_update
                 BEFORE UPDATE OF masked_suffix ON ai_credentials
                 BEGIN SELECT RAISE(ABORT, 'forced metadata update failure'); END;",
            )
            .unwrap();
        secrets.fail_next_restore_for_test();

        let error = replace_credential(&db, &secrets, &credential.id, "new-secret")
            .unwrap_err()
            .to_string();

        assert!(error.starts_with("AI_CREDENTIAL_REPLACE_ROLLBACK_FAILED:"));
        assert!(error.contains("forced metadata update failure"));
        assert!(error.contains("TEST_SECRET_RESTORE_FAILED"));
    }

    #[test]
    fn stream_preflight_reads_local_credentials_without_vault_state() {
        let directory = tempfile::TempDir::new().unwrap();
        let db = Db::init(directory.path()).unwrap();
        let secrets = Secrets::init_in_memory().unwrap();
        let profile = create_profile(
            &db,
            "Local credential".to_string(),
            "custom".to_string(),
            "api_key".to_string(),
            Some("https://api.example/v1".to_string()),
            "model".to_string(),
            0.2,
            None,
            true,
        )
        .unwrap();
        add_credential(
            &db,
            &secrets,
            profile.id,
            "Primary".to_string(),
            "secret".to_string(),
        )
        .unwrap();
        secrets.lock_for_test().unwrap();

        assert!(ensure_stream_credentials_accessible(&db, &secrets).is_ok());
    }

    #[test]
    fn stream_preflight_skips_missing_credentials_before_using_fallback() {
        let directory = tempfile::TempDir::new().unwrap();
        let db = Db::init(directory.path()).unwrap();
        let secrets = Secrets::init_in_memory().unwrap();
        let profile = create_profile(
            &db,
            "Fallback".to_string(),
            "custom".to_string(),
            "api_key".to_string(),
            Some("https://api.example/v1".to_string()),
            "model".to_string(),
            0.2,
            None,
            true,
        )
        .unwrap();
        let missing = add_credential(
            &db,
            &secrets,
            profile.id.clone(),
            "Missing".to_string(),
            "removed".to_string(),
        )
        .unwrap();
        let fallback = add_credential(
            &db,
            &secrets,
            profile.id,
            "Fallback".to_string(),
            "available".to_string(),
        )
        .unwrap();
        let missing_ref = credential_by_id(&db, &missing.id).unwrap().secret_ref;
        let fallback_ref = credential_by_id(&db, &fallback.id).unwrap().secret_ref;
        secrets.delete(&missing_ref).unwrap();

        assert!(ensure_stream_credentials_accessible(&db, &secrets).is_ok());
        assert_eq!(
            secrets.get(&fallback_ref).unwrap().as_deref(),
            Some("available")
        );
    }

    #[tokio::test]
    async fn model_list_tries_the_next_enabled_credential_after_auth_failure() {
        let directory = tempfile::TempDir::new().unwrap();
        let db = Db::init(directory.path()).unwrap();
        let secrets = Secrets::init_in_memory().unwrap();
        let (base_url, server) = model_list_server(vec![
            (
                "401 Unauthorized",
                r#"{"error":{"code":"invalid_api_key"}}"#,
            ),
            ("200 OK", r#"{"data":[{"id":"backup-model"}]}"#),
        ])
        .await;
        let profile = model_list_test_profile(&db, base_url);
        let first = add_credential(
            &db,
            &secrets,
            profile.id.clone(),
            "First".to_string(),
            "bad-key".to_string(),
        )
        .unwrap();
        let second = add_credential(
            &db,
            &secrets,
            profile.id.clone(),
            "Second".to_string(),
            "backup-key".to_string(),
        )
        .unwrap();

        let models = list_models(
            &db,
            &secrets,
            &profile.id,
            profile.provider.clone(),
            profile.auth_mode.clone(),
            profile.base_url.clone(),
        )
        .await
        .unwrap();
        let requests = server.await.unwrap();

        assert_eq!(models, vec!["backup-model".to_string()]);
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("Bearer bad-key"));
        assert!(requests[1].contains("Bearer backup-key"));
        let credentials = list_credentials(&db, Some(&profile.id)).unwrap();
        assert_eq!(credentials[0].id, first.id);
        assert_eq!(credentials[0].state, "active");
        assert!(credentials[0].last_used_at.is_none());
        assert_eq!(credentials[1].id, second.id);
        assert_eq!(credentials[1].state, "active");
        assert!(credentials[1].last_used_at.is_none());
    }

    #[tokio::test]
    async fn model_list_does_not_rotate_keys_for_request_errors() {
        let directory = tempfile::TempDir::new().unwrap();
        let db = Db::init(directory.path()).unwrap();
        let secrets = Secrets::init_in_memory().unwrap();
        let (base_url, server) = model_list_server(vec![(
            "400 Bad Request",
            r#"{"error":{"code":"invalid_endpoint"}}"#,
        )])
        .await;
        let profile = model_list_test_profile(&db, base_url);
        add_credential(
            &db,
            &secrets,
            profile.id.clone(),
            "First".to_string(),
            "first-key".to_string(),
        )
        .unwrap();
        add_credential(
            &db,
            &secrets,
            profile.id.clone(),
            "Second".to_string(),
            "second-key".to_string(),
        )
        .unwrap();

        let error = list_models(
            &db,
            &secrets,
            &profile.id,
            profile.provider.clone(),
            profile.auth_mode.clone(),
            profile.base_url.clone(),
        )
        .await
        .unwrap_err();
        let requests = server.await.unwrap();

        assert!(error.to_string().contains("status=400"));
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("Bearer first-key"));
        let credentials = list_credentials(&db, Some(&profile.id)).unwrap();
        assert!(credentials[0].last_error_kind.is_none());
        assert!(credentials[0].last_used_at.is_none());
        assert!(credentials[1].last_used_at.is_none());
    }
}
