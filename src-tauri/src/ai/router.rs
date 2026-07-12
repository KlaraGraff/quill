//! Provider-neutral AI request routing and API credential failover.
//!
//! Secrets never leave this module: the database stores only a Keychain
//! reference, masked suffix, priority, and health metadata for each key.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};

use chrono::Utc;
use rusqlite::params;
use tauri::AppHandle;
use tokio::sync::watch;

use crate::commands::ai::ChatMessage;
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::secrets::Secrets;

#[derive(Debug, Clone, serde::Serialize)]
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
}

#[derive(Debug, Clone, serde::Serialize)]
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

#[derive(Debug, Clone)]
struct AiProfile {
    view: AiProfileView,
}

#[derive(Debug, Clone)]
struct AiCredential {
    view: AiCredentialView,
    secret_ref: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AiErrorKind {
    CredentialInvalid,
    Auth,
    Permission,
    RateLimit,
    Quota,
    Network,
    Provider5xx,
    Protocol,
    Request,
}

impl AiErrorKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::CredentialInvalid => "credential_invalid",
            Self::Auth => "auth",
            Self::Permission => "permission",
            Self::RateLimit => "rate_limit",
            Self::Quota => "quota",
            Self::Network => "network",
            Self::Provider5xx => "provider_5xx",
            Self::Protocol => "protocol",
            Self::Request => "request",
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
    if [
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
    {
        AiErrorKind::Request
    } else {
        AiErrorKind::Network
    }
}

fn is_cancelled(error: &AppError) -> bool {
    error.to_string().contains("AI_REQUEST_CANCELLED")
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
    let legacy_key = secrets
        .get("ai_api_key")
        .filter(|value| !value.trim().is_empty());
    let credential = legacy_key.as_ref().map(|key| {
        let id = uuid::Uuid::new_v4().to_string();
        (id.clone(), format!("ai_api_key/{id}"), key)
    });

    if let Some((_, secret_ref, key)) = credential.as_ref() {
        // Keychain write happens before the SQL transaction. If the
        // transaction fails below, the cleanup keeps the new secret private
        // and leaves the legacy credential untouched for a later retry.
        secrets.set(secret_ref, key)?;
    }

    let result = (|| -> AppResult<()> {
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO ai_profiles (id, label, provider, auth_mode, base_url, model, temperature, keep_alive, enabled, priority, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, 0, ?9, ?9)",
            params![profile_id, profile_label, provider, auth_mode, base_url, model, temperature, keep_alive, created_at],
        )?;
        if let Some((credential_id, secret_ref, key)) = credential.as_ref() {
            tx.execute(
                "INSERT INTO ai_credentials (id, profile_id, label, secret_ref, masked_suffix, enabled, priority, state, created_at, updated_at) VALUES (?1, ?2, 'Primary key', ?3, ?4, 1, 0, 'active', ?5, ?5)",
                params![credential_id, profile_id, secret_ref, suffix(key), created_at],
            )?;
        }
        tx.commit()?;
        Ok(())
    })();
    drop(conn);

    if let Err(error) = result {
        if let Some((_, secret_ref, _)) = credential.as_ref() {
            let _ = secrets.delete(secret_ref);
        }
        return Err(error);
    }

    if credential.is_some() {
        if let Err(error) = secrets.delete("ai_api_key") {
            log::warn!("ai router: migrated legacy API key but could not remove its old Keychain item: {error}");
        }
    }
    Ok(())
}

fn active_profile(db: &Db) -> AppResult<AiProfile> {
    let conn = db.reader();
    conn.query_row(
        "SELECT id, label, provider, auth_mode, base_url, model, temperature, keep_alive, enabled, priority FROM ai_profiles WHERE enabled = 1 ORDER BY priority ASC, created_at ASC LIMIT 1",
        [],
        |row| Ok(AiProfile {
            view: AiProfileView {
                id: row.get(0)?, label: row.get(1)?, provider: row.get(2)?, auth_mode: row.get(3)?,
                base_url: row.get(4)?, model: row.get(5)?, temperature: row.get(6)?, keep_alive: row.get(7)?,
                enabled: row.get::<_, i64>(8)? != 0, priority: row.get(9)?,
            },
        }),
    ).map_err(Into::into)
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
        Some(AiErrorKind::Request) => ("active", None),
    };
    let _ = conn.execute(
        "UPDATE ai_credentials SET state = ?1, cooldown_until = ?2, last_error_kind = ?3, last_used_at = ?4, updated_at = ?4 WHERE id = ?5",
        params![state, cooldown, error.map(AiErrorKind::as_str), timestamp, credential.view.id],
    );
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
        changed = cancel.changed() => {
            let _ = changed;
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
    result
}

async fn stream_with_failover_inner(
    app: &AppHandle,
    db: &Db,
    secrets: &Secrets,
    messages: &[ChatMessage],
    event_name: &str,
    max_tokens: Option<u32>,
    cancel: &mut watch::Receiver<bool>,
) -> AppResult<()> {
    let profile =
        active_profile(db).map_err(|_| AppError::Other("AI_NOT_CONFIGURED".to_string()))?;
    if profile.view.auth_mode == "oauth" && profile.view.provider == "openai" {
        let (token, account_id) = crate::ai::oauth::get_valid_token(secrets).await?;
        return stream_once(
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
        .await;
    }
    if profile.view.provider == "ollama" {
        return stream_once(
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
        .await;
    }

    let candidates = credentials_for(db, &profile.view.id)?;
    if candidates.is_empty() {
        let all = all_credentials_for(db, &profile.view.id)?;
        let code = if all.is_empty() {
            "AI_NOT_CONFIGURED"
        } else if all.iter().all(|item| !item.view.enabled) {
            "AI_KEYS_DISABLED"
        } else if all.iter().all(|item| item.view.state == "invalid") {
            "AI_ALL_KEYS_INVALID"
        } else if all.iter().any(|item| {
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
        return Err(AppError::Other(code.to_string()));
    }
    let mut last_error = None;
    for credential in candidates {
        let Some(key) = secrets
            .get(&credential.secret_ref)
            .filter(|value| !value.trim().is_empty())
        else {
            update_credential_health(db, &credential, Some(AiErrorKind::CredentialInvalid), None);
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
                return Ok(());
            }
            Err(error) => {
                if is_cancelled(&error) {
                    return Err(error);
                }
                let kind = classify_error(&error);
                update_credential_health(db, &credential, Some(kind), retry_after_ms(&error));
                if emitted.load(Ordering::Relaxed) || !kind.retryable() {
                    return Err(error);
                }
                log::warn!(
                    "ai router: credential={} failed kind={}, trying next configured credential",
                    credential.view.id,
                    kind.as_str()
                );
                last_error = Some(error);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| AppError::Other("AI_NOT_CONFIGURED".to_string())))
}

pub fn list_credentials(db: &Db, profile_id: Option<&str>) -> AppResult<Vec<AiCredentialView>> {
    let profile_id = match profile_id {
        Some(id) => id.to_string(),
        None => active_profile(db)?.view.id,
    };
    all_credentials_for(db, &profile_id)
        .map(|items| items.into_iter().map(|item| item.view).collect())
}

pub fn active_profile_view(db: &Db) -> AppResult<AiProfileView> {
    Ok(active_profile(db)?.view)
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
    let timestamp = now();
    let conn = db.conn.lock().map_err(|e| AppError::Other(e.to_string()))?;
    conn.execute(
        "UPDATE ai_profiles SET label = ?1, provider = ?2, auth_mode = ?3, base_url = ?4, model = ?5, temperature = ?6, keep_alive = ?7, updated_at = ?8 WHERE id = ?9",
        params![label, provider, auth_mode, base_url, model, temperature, keep_alive, timestamp, id],
    )?;
    drop(conn);
    active_profile_view(db)
}

pub fn add_credential(
    db: &Db,
    secrets: &Secrets,
    profile_id: String,
    label: String,
    value: String,
) -> AppResult<AiCredentialView> {
    let value = value.trim();
    if value.is_empty() {
        return Err(AppError::Other("AI_API_KEY_EMPTY".to_string()));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let secret_ref = format!("ai_api_key/{id}");
    let timestamp = now();
    let priority: i64 = db
        .conn
        .lock()
        .map_err(|e| AppError::Other(e.to_string()))?
        .query_row(
            "SELECT COALESCE(MAX(priority) + 1, 0) FROM ai_credentials WHERE profile_id = ?1",
            params![profile_id],
            |row| row.get(0),
        )?;
    secrets.set(&secret_ref, value)?;
    let conn = db.conn.lock().map_err(|e| AppError::Other(e.to_string()))?;
    if let Err(error) = conn.execute(
        "INSERT INTO ai_credentials (id, profile_id, label, secret_ref, masked_suffix, enabled, priority, state, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, 'active', ?7, ?7)",
        params![id, profile_id, if label.trim().is_empty() { "API key" } else { label.trim() }, secret_ref, suffix(value), priority, timestamp],
    ) {
        let _ = secrets.delete(&secret_ref);
        return Err(error.into());
    }
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
    let secret_ref: String = db.reader().query_row(
        "SELECT secret_ref FROM ai_credentials WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )?;
    let old_value = secrets.get(&secret_ref);
    secrets.set(&secret_ref, value)?;
    let conn = db.conn.lock().map_err(|e| AppError::Other(e.to_string()))?;
    if let Err(error) = conn.execute("UPDATE ai_credentials SET masked_suffix = ?1, state = 'active', cooldown_until = NULL, last_error_kind = NULL, updated_at = ?2 WHERE id = ?3", params![suffix(value), now(), id]) {
        match old_value {
            Some(previous) => { let _ = secrets.set(&secret_ref, &previous); }
            None => { let _ = secrets.delete(&secret_ref); }
        }
        return Err(error.into());
    }
    Ok(())
}

pub fn set_credential_enabled(db: &Db, id: &str, enabled: bool) -> AppResult<()> {
    db.conn
        .lock()
        .map_err(|e| AppError::Other(e.to_string()))?
        .execute(
            "UPDATE ai_credentials SET enabled = ?1, updated_at = ?2 WHERE id = ?3",
            params![enabled as i64, now(), id],
        )?;
    Ok(())
}

pub fn reorder_credentials(db: &Db, ids: &[String]) -> AppResult<()> {
    let conn = db.conn.lock().map_err(|e| AppError::Other(e.to_string()))?;
    for (priority, id) in ids.iter().enumerate() {
        conn.execute(
            "UPDATE ai_credentials SET priority = ?1, updated_at = ?2 WHERE id = ?3",
            params![priority as i64, now(), id],
        )?;
    }
    Ok(())
}

pub fn delete_credential(db: &Db, secrets: &Secrets, id: &str) -> AppResult<()> {
    let secret_ref: String = db.reader().query_row(
        "SELECT secret_ref FROM ai_credentials WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )?;
    let old_value = secrets.get(&secret_ref);
    secrets.delete(&secret_ref)?;
    let delete_result = db
        .conn
        .lock()
        .map_err(|e| AppError::Other(e.to_string()))?
        .execute("DELETE FROM ai_credentials WHERE id = ?1", params![id]);
    if let Err(error) = delete_result {
        if let Some(value) = old_value {
            let _ = secrets.set(&secret_ref, &value);
        }
        return Err(error.into());
    }
    Ok(())
}

pub async fn test_credential(
    app: &AppHandle,
    db: &Db,
    secrets: &Secrets,
    credential_id: &str,
) -> AppResult<()> {
    let profile = active_profile(db)?;
    // A manual connection test is also the recovery path for a credential
    // previously marked invalid after an authentication failure. Runtime
    // routing intentionally skips invalid and cooling-down credentials, but
    // applying that filter here made an invalid key impossible to test after
    // the user fixed the provider-side issue.
    let candidates = all_credentials_for(db, &profile.view.id)?;
    let credential = candidates
        .into_iter()
        .find(|candidate| candidate.view.id == credential_id)
        .ok_or_else(|| AppError::Other("AI_CREDENTIAL_NOT_FOUND".to_string()))?;
    let key = secrets
        .get(&credential.secret_ref)
        .ok_or_else(|| AppError::Other("AI_CREDENTIAL_UNAVAILABLE".to_string()))?;
    let emitted = Arc::new(AtomicBool::new(false));
    let event_name = format!("ai-credential-test-{credential_id}");
    let messages = [ChatMessage {
        role: "user".to_string(),
        content: "Reply with OK.".to_string(),
    }];
    let result = stream_once(
        app,
        &profile,
        &key,
        None,
        &messages,
        &event_name,
        Some(8),
        emitted,
        &mut watch::channel(false).1,
    )
    .await;
    update_credential_health(
        db,
        &credential,
        result.as_ref().err().map(classify_error),
        result.as_ref().err().and_then(retry_after_ms),
    );
    result
}
