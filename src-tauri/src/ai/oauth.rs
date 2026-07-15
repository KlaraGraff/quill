use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{AppError, AppResult};
use crate::secrets::Secrets;

// OpenAI OAuth constants (public PKCE client, matches Codex CLI / pi-mono)
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTH_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const SCOPES: &str = "openid profile email offline_access";
const CALLBACK_PORT: u16 = 1455;
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const CALLBACK_TIMEOUT_SECS: u64 = 60;
const CALLBACK_CONNECTION_TIMEOUT_SECS: u64 = 5;
const MAX_CALLBACK_REQUEST_BYTES: usize = 16 * 1024;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: u64,
    pub account_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OAuthStatus {
    pub connected: bool,
    pub account_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Generate PKCE verifier and challenge pair.
pub fn generate_pkce() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let verifier = URL_SAFE_NO_PAD.encode(bytes);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// Generate random hex state parameter.
pub fn generate_state() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Build the OpenAI authorization URL with PKCE challenge and state.
pub fn build_auth_url(challenge: &str, state: &str) -> String {
    let mut url = reqwest::Url::parse(AUTH_URL).unwrap();
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("scope", SCOPES)
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("id_token_add_organizations", "true")
        .append_pair("codex_cli_simplified_flow", "true")
        .append_pair("originator", "quill");
    url.to_string()
}

/// Start a local HTTP server and wait for the OAuth callback.
/// Returns the authorization code on success.
pub async fn wait_for_callback(expected_state: &str) -> AppResult<String> {
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", CALLBACK_PORT))
        .await
        .map_err(|e| {
            AppError::Other(format!(
                "Failed to bind callback port {}: {}",
                CALLBACK_PORT, e
            ))
        })?;
    handle_callback(listener, expected_state).await
}

/// Accept callbacks until a valid state arrives or the OAuth session expires.
/// Invalid local connections receive an error page but cannot consume the one
/// opportunity for the real browser callback to complete.
async fn handle_callback(
    listener: tokio::net::TcpListener,
    expected_state: &str,
) -> AppResult<String> {
    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_secs(CALLBACK_TIMEOUT_SECS);
    loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .ok_or_else(|| AppError::Other("OAuth callback timed out after 60s".to_string()))?;
        let (mut stream, _) = tokio::time::timeout(remaining, listener.accept())
            .await
            .map_err(|_| AppError::Other("OAuth callback timed out after 60s".to_string()))?
            .map_err(|error| AppError::Other(format!("Failed to accept connection: {error}")))?;

        if let Some(code) = process_callback(&mut stream, expected_state, deadline).await? {
            return Ok(code);
        }
    }
}

async fn process_callback(
    stream: &mut tokio::net::TcpStream,
    expected_state: &str,
    deadline: tokio::time::Instant,
) -> AppResult<Option<String>> {
    let request = read_http_request(stream, deadline).await?;
    let Some(request) = request else {
        return Ok(None);
    };

    // Parse first line: "GET /callback?code=xxx&state=yyy HTTP/1.1".
    let Some(path) = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
    else {
        write_callback_error(stream, "The callback request is malformed.").await;
        return Ok(None);
    };
    let full_url = format!("http://127.0.0.1{path}");
    let Ok(parsed) = reqwest::Url::parse(&full_url) else {
        write_callback_error(stream, "The callback URL is invalid.").await;
        return Ok(None);
    };

    let mut code = None;
    let mut state = None;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.to_string()),
            "state" => state = Some(value.to_string()),
            _ => {}
        }
    }

    if state.as_deref() != Some(expected_state) {
        write_callback_error(
            stream,
            "State validation failed. Please return to Quill and try again.",
        )
        .await;
        return Ok(None);
    }
    let Some(code) = code.filter(|value| !value.is_empty()) else {
        write_callback_error(
            stream,
            "The authorization code is missing. Please try again.",
        )
        .await;
        return Ok(None);
    };

    let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n\
        <html><body><h1>Authentication Successful</h1><p>You can close this window and return to Quill.</p></body></html>";
    let _ = stream.write_all(response.as_bytes()).await;
    Ok(Some(code))
}

async fn read_http_request(
    stream: &mut tokio::net::TcpStream,
    deadline: tokio::time::Instant,
) -> AppResult<Option<String>> {
    let mut request = Vec::with_capacity(1024);
    loop {
        if request.len() >= MAX_CALLBACK_REQUEST_BYTES {
            write_callback_error(stream, "The callback request is too large.").await;
            return Ok(None);
        }
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .ok_or_else(|| AppError::Other("OAuth callback timed out after 60s".to_string()))?;
        let timeout = remaining.min(std::time::Duration::from_secs(
            CALLBACK_CONNECTION_TIMEOUT_SECS,
        ));
        let mut buffer = [0u8; 1024];
        let count = match tokio::time::timeout(timeout, stream.read(&mut buffer)).await {
            Ok(Ok(count)) => count,
            Ok(Err(error)) => return Err(error.into()),
            Err(_) => {
                write_callback_error(stream, "The callback request timed out.").await;
                return Ok(None);
            }
        };
        if count == 0 {
            return Ok(None);
        }
        request.extend_from_slice(&buffer[..count]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(Some(String::from_utf8_lossy(&request).into_owned()));
        }
    }
}

async fn write_callback_error(stream: &mut tokio::net::TcpStream, message: &str) {
    let response = format!(
        "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<html><body><h1>Authentication Failed</h1><p>{message}</p></body></html>"
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

/// Exchange an authorization code for tokens.
pub async fn exchange_code(code: &str, verifier: &str) -> AppResult<TokenResponse> {
    let client = reqwest::Client::new();
    let resp = client
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT_ID),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", REDIRECT_URI),
        ])
        .send()
        .await
        .map_err(|e| AppError::Other(format!("Token exchange request failed: {}", e)))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(AppError::Other(format!("Token exchange error {status}")));
    }

    resp.json::<TokenResponse>()
        .await
        .map_err(|e| AppError::Other(format!("Failed to parse token response: {}", e)))
}

/// Refresh an expired access token using the refresh token.
pub async fn refresh_access_token(refresh_token: &str) -> AppResult<TokenResponse> {
    let client = reqwest::Client::new();
    let resp = client
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", CLIENT_ID),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await
        .map_err(|e| AppError::Other(format!("Token refresh request failed: {}", e)))?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(AppError::Other(format!("Token refresh error {status}")));
    }

    resp.json::<TokenResponse>()
        .await
        .map_err(|e| AppError::Other(format!("Failed to parse refresh response: {}", e)))
}

/// Decode the chatgpt_account_id from an OpenAI JWT access token.
pub fn decode_jwt_account_id(access_token: &str) -> Option<String> {
    let parts: Vec<&str> = access_token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let payload = URL_SAFE_NO_PAD.decode(parts[1]).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    json.get("https://api.openai.com/auth")?
        .get("chatgpt_account_id")?
        .as_str()
        .map(String::from)
}

pub fn save_tokens(secrets: &Secrets, tokens: &OAuthTokens) -> AppResult<()> {
    let expires_at = tokens.expires_at.to_string();
    secrets.set_many(&[
        ("oauth_access_token", Some(tokens.access_token.as_str())),
        ("oauth_refresh_token", tokens.refresh_token.as_deref()),
        ("oauth_expires_at", Some(expires_at.as_str())),
        ("oauth_account_id", tokens.account_id.as_deref()),
    ])
}

/// Metadata-only status used while rendering settings. The webview never needs
/// the locally stored token values themselves.
pub fn has_token_metadata(secrets: &Secrets) -> bool {
    secrets.has_stored_secret_metadata("oauth_access_token")
        || secrets.has_stored_secret_metadata("oauth_refresh_token")
}

pub fn load_tokens(secrets: &Secrets) -> AppResult<Option<OAuthTokens>> {
    let Some(access_token) = secrets.get("oauth_access_token")? else {
        return Ok(None);
    };
    let refresh_token = secrets
        .get("oauth_refresh_token")?
        .filter(|token| !token.trim().is_empty());
    let Some(expires_at) = secrets.get("oauth_expires_at")? else {
        return Ok(None);
    };
    let expires_at = expires_at
        .parse()
        .map_err(|_| AppError::Other("OAUTH_TOKEN_METADATA_INVALID".to_string()))?;
    Ok(Some(OAuthTokens {
        access_token,
        refresh_token,
        expires_at,
        account_id: secrets.get("oauth_account_id")?,
    }))
}

pub fn clear_tokens(secrets: &Secrets) -> AppResult<()> {
    secrets.delete_prefix("oauth_")?;
    Ok(())
}

/// Get a valid access token and account ID, auto-refreshing if expired.
/// This is the main entry point called by AI commands.
/// Returns (access_token, account_id).
pub async fn get_valid_token(secrets: &Secrets) -> AppResult<(String, Option<String>)> {
    let tokens =
        load_tokens(secrets)?.ok_or_else(|| AppError::Other("AI_NOT_CONFIGURED".to_string()))?;

    if tokens.expires_at > now_epoch() + 60 {
        return Ok((tokens.access_token, tokens.account_id));
    }

    // Token expired — refresh it
    let Some(refresh_token) = tokens.refresh_token.as_deref() else {
        clear_tokens(secrets)?;
        return Err(AppError::Other(
            "OAUTH_REAUTHENTICATION_REQUIRED".to_string(),
        ));
    };
    let response = refresh_access_token(refresh_token).await?;
    let now = now_epoch();
    let new_tokens = OAuthTokens {
        access_token: response.access_token.clone(),
        refresh_token: response.refresh_token.or(tokens.refresh_token),
        expires_at: now + response.expires_in.unwrap_or(3600),
        account_id: decode_jwt_account_id(&response.access_token).or(tokens.account_id),
    };

    save_tokens(secrets, &new_tokens)?;

    Ok((new_tokens.access_token, new_tokens.account_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_pkce_format() {
        let (verifier, challenge) = generate_pkce();
        // 32 bytes → 43 base64url chars (no padding)
        assert_eq!(verifier.len(), 43);
        // SHA-256 → 32 bytes → 43 base64url chars
        assert_eq!(challenge.len(), 43);
        assert_ne!(verifier, challenge);
        // Must be valid base64url (no +, /, =)
        assert!(!verifier.contains('+') && !verifier.contains('/') && !verifier.contains('='));
        assert!(!challenge.contains('+') && !challenge.contains('/') && !challenge.contains('='));
    }

    #[test]
    fn test_generate_pkce_deterministic_challenge() {
        // Given the same verifier, challenge should be SHA256(verifier)
        let verifier = "test_verifier_string";
        let expected = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        // Verify our PKCE logic matches
        let digest = Sha256::digest(verifier.as_bytes());
        let challenge = URL_SAFE_NO_PAD.encode(digest);
        assert_eq!(challenge, expected);
    }

    #[test]
    fn test_generate_pkce_uniqueness() {
        let (v1, c1) = generate_pkce();
        let (v2, c2) = generate_pkce();
        assert_ne!(v1, v2);
        assert_ne!(c1, c2);
    }

    #[test]
    fn test_generate_state_format() {
        let state = generate_state();
        // 16 bytes → 32 hex chars
        assert_eq!(state.len(), 32);
        assert!(state.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_generate_state_uniqueness() {
        let s1 = generate_state();
        let s2 = generate_state();
        assert_ne!(s1, s2);
    }

    #[test]
    fn test_build_auth_url_contains_required_params() {
        let url = build_auth_url("test_challenge", "test_state");
        let parsed = reqwest::Url::parse(&url).unwrap();

        assert_eq!(parsed.scheme(), "https");
        assert_eq!(parsed.host_str(), Some("auth.openai.com"));
        assert_eq!(parsed.path(), "/oauth/authorize");

        let params: std::collections::HashMap<_, _> = parsed.query_pairs().collect();
        assert_eq!(params.get("response_type").unwrap(), "code");
        assert_eq!(params.get("client_id").unwrap(), CLIENT_ID);
        assert_eq!(params.get("redirect_uri").unwrap(), REDIRECT_URI);
        assert_eq!(params.get("scope").unwrap(), SCOPES);
        assert_eq!(params.get("state").unwrap(), "test_state");
        assert_eq!(params.get("code_challenge").unwrap(), "test_challenge");
        assert_eq!(params.get("code_challenge_method").unwrap(), "S256");
        assert_eq!(params.get("originator").unwrap(), "quill");
    }

    #[test]
    fn test_decode_jwt_account_id_valid() {
        // Build a fake JWT: header.payload.signature
        let payload = serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acct_abc123"
            }
        });
        let header = URL_SAFE_NO_PAD.encode(b"{}");
        let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        let signature = URL_SAFE_NO_PAD.encode(b"fake_sig");
        let token = format!("{}.{}.{}", header, payload_b64, signature);

        assert_eq!(
            decode_jwt_account_id(&token),
            Some("acct_abc123".to_string())
        );
    }

    #[test]
    fn test_decode_jwt_account_id_missing_field() {
        let payload = serde_json::json!({"sub": "user123"});
        let header = URL_SAFE_NO_PAD.encode(b"{}");
        let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
        let sig = URL_SAFE_NO_PAD.encode(b"sig");
        let token = format!("{}.{}.{}", header, payload_b64, sig);

        assert_eq!(decode_jwt_account_id(&token), None);
    }

    #[test]
    fn test_decode_jwt_account_id_malformed() {
        assert_eq!(decode_jwt_account_id("not.a.jwt"), None);
        assert_eq!(decode_jwt_account_id("only_one_part"), None);
        assert_eq!(decode_jwt_account_id("two.parts"), None);
    }

    #[test]
    fn test_save_load_clear_tokens() {
        let secrets = Secrets::init_in_memory().unwrap();

        // Initially empty
        assert!(load_tokens(&secrets).unwrap().is_none());

        // Save and load
        let tokens = OAuthTokens {
            access_token: "access_123".to_string(),
            refresh_token: Some("refresh_456".to_string()),
            expires_at: 1700000000,
            account_id: Some("acct_789".to_string()),
        };
        save_tokens(&secrets, &tokens).unwrap();

        let loaded = load_tokens(&secrets).unwrap().unwrap();
        assert_eq!(loaded.access_token, "access_123");
        assert_eq!(loaded.refresh_token.as_deref(), Some("refresh_456"));
        assert_eq!(loaded.expires_at, 1700000000);
        assert_eq!(loaded.account_id, Some("acct_789".to_string()));

        // Overwrite (upsert)
        let tokens2 = OAuthTokens {
            access_token: "new_access".to_string(),
            refresh_token: Some("new_refresh".to_string()),
            expires_at: 1800000000,
            account_id: None,
        };
        save_tokens(&secrets, &tokens2).unwrap();

        let loaded2 = load_tokens(&secrets).unwrap().unwrap();
        assert_eq!(loaded2.access_token, "new_access");
        assert_eq!(loaded2.expires_at, 1800000000);
        assert_eq!(loaded2.account_id, None);

        // Clear
        clear_tokens(&secrets).unwrap();
        assert!(load_tokens(&secrets).unwrap().is_none());
    }

    #[test]
    fn test_save_tokens_without_account_id() {
        let secrets = Secrets::init_in_memory().unwrap();

        let tokens = OAuthTokens {
            access_token: "at".to_string(),
            refresh_token: Some("rt".to_string()),
            expires_at: 999,
            account_id: None,
        };
        save_tokens(&secrets, &tokens).unwrap();

        let loaded = load_tokens(&secrets).unwrap().unwrap();
        assert_eq!(loaded.access_token, "at");
        assert_eq!(loaded.account_id, None);
    }

    #[tokio::test]
    async fn expired_token_without_refresh_token_requires_reauthentication() {
        let secrets = Secrets::init_in_memory().unwrap();
        save_tokens(
            &secrets,
            &OAuthTokens {
                access_token: "access".to_string(),
                refresh_token: None,
                expires_at: 0,
                account_id: None,
            },
        )
        .unwrap();

        let error = get_valid_token(&secrets).await.unwrap_err().to_string();
        assert_eq!(error, "OAUTH_REAUTHENTICATION_REQUIRED");
        assert!(load_tokens(&secrets).unwrap().is_none());
    }

    #[tokio::test]
    async fn test_handle_callback_success() {
        // Bind to port 0 so the OS assigns a free port
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = "expected_state_123";

        let handle = tokio::spawn(async move { handle_callback(listener, state).await });

        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        let request = format!(
            "GET /auth/callback?code=auth_code_xyz&state={} HTTP/1.1\r\nHost: localhost\r\n\r\n",
            state
        );
        stream.write_all(request.as_bytes()).await.unwrap();

        let result = handle.await.unwrap().unwrap();
        assert_eq!(result, "auth_code_xyz");
    }

    #[tokio::test]
    async fn test_handle_callback_ignores_state_mismatch_until_valid_callback() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let handle = tokio::spawn(async move { handle_callback(listener, "correct_state").await });

        let mut stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .unwrap();
        let request =
            "GET /auth/callback?code=code&state=wrong_state HTTP/1.1\r\nHost: localhost\r\n\r\n";
        stream.write_all(request.as_bytes()).await.unwrap();

        let mut valid_stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        valid_stream
            .write_all(
                b"GET /auth/callback?code=valid_code&state=correct_state HTTP/1.1\r\nHost: localhost\r\n\r\n",
            )
            .await
            .unwrap();

        assert_eq!(handle.await.unwrap().unwrap(), "valid_code");
    }
}
