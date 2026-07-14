use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use keyring::credential::CredentialPersistence;
use keyring::Entry;
use rand::{rngs::OsRng, RngCore};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

use crate::db::Db;
use crate::error::{AppError, AppResult};

const VAULT_KEYCHAIN_SERVICE: &str = "com.ryoyamada.quill";
const VAULT_MASTER_ACCOUNT: &str = "vault-master-key-v1";
const LEGACY_KEYCHAIN_SERVICES: &[&str] = &[
    "com.klaragraff.quill",
    "com.klagragraff.quill",
    "com.wycstudios.quill",
];
const VAULT_ALGORITHM: i64 = 1;
const VAULT_KEY_VERSION: i64 = 1;
const AAD_PREFIX: &[u8] = b"quill-secret-v1\0";

const SENSITIVE_KEYS: &[&str] = &[
    "ai_api_key",
    "oauth_access_token",
    "oauth_refresh_token",
    "oauth_expires_at",
    "oauth_account_id",
];

enum MasterKeyState {
    Locked,
    Ready(Zeroizing<Vec<u8>>),
    Denied,
    Unavailable,
}

struct VaultSession {
    master_key: MasterKeyState,
    pending_master_request: Option<String>,
    legacy_candidates: HashSet<String>,
    authorized_legacy_keys: HashSet<String>,
    denied_legacy_keys: HashSet<String>,
    missing_legacy_entries: HashSet<String>,
    pending_imports: HashMap<String, String>,
}

impl Default for VaultSession {
    fn default() -> Self {
        Self {
            master_key: MasterKeyState::Locked,
            pending_master_request: None,
            legacy_candidates: HashSet::new(),
            authorized_legacy_keys: HashSet::new(),
            denied_legacy_keys: HashSet::new(),
            missing_legacy_entries: HashSet::new(),
            pending_imports: HashMap::new(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatus {
    state: &'static str,
    encrypted_secret_count: i64,
    pending_local_migration_count: i64,
}

#[derive(Clone)]
pub struct EncryptedSecretSnapshot {
    key: String,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
    algorithm: i64,
    key_version: i64,
    updated_at: i64,
}

#[derive(Clone)]
pub struct SecretStateSnapshot {
    key: String,
    encrypted: Option<EncryptedSecretSnapshot>,
    legacy_value: Option<String>,
    legacy_candidate_created_at: Option<i64>,
    tombstone_created_at: Option<i64>,
}

/// Sensitive values are encrypted in a local-only SQLite vault. The operating
/// system credential store contains exactly one random master key, never one
/// item per API key.
#[derive(Clone)]
pub struct Secrets {
    pub conn: Arc<Mutex<Connection>>,
    session: Arc<Mutex<VaultSession>>,
    operation_lock: Arc<Mutex<()>>,
    use_keychain: bool,
}

impl Secrets {
    pub fn init(local_dir: &PathBuf) -> AppResult<Self> {
        fs::create_dir_all(local_dir)?;
        let db_path = local_dir.join("secrets.db");
        let conn = Connection::open(&db_path)?;
        Self::initialize_schema(&conn)?;

        if !matches!(
            keyring::default::default_credential_builder().persistence(),
            CredentialPersistence::UntilDelete
        ) {
            return Err(AppError::Other(
                "SYSTEM_CREDENTIAL_STORE_NOT_PERSISTENT".to_string(),
            ));
        }

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            session: Arc::new(Mutex::new(VaultSession::default())),
            operation_lock: Arc::new(Mutex::new(())),
            use_keychain: true,
        })
    }

    fn initialize_schema(conn: &Connection) -> AppResult<()> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA secure_delete=ON;
             CREATE TABLE IF NOT EXISTS secrets (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS encrypted_secrets (
                 key TEXT PRIMARY KEY,
                 nonce BLOB NOT NULL CHECK(length(nonce) = 12),
                 ciphertext BLOB NOT NULL CHECK(length(ciphertext) >= 16),
                 algorithm INTEGER NOT NULL DEFAULT 1,
                 key_version INTEGER NOT NULL DEFAULT 1,
                 updated_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS secret_migration_tombstones (
                 key TEXT PRIMARY KEY,
                 created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS legacy_secret_candidates (
                 key TEXT PRIMARY KEY,
                 created_at INTEGER NOT NULL
             );",
        )?;
        Ok(())
    }

    /// This command is intentionally metadata-only and never accesses Keychain.
    pub fn status(&self) -> AppResult<VaultStatus> {
        let (encrypted_secret_count, pending_local_migration_count) = {
            let conn = self
                .conn
                .lock()
                .map_err(|error| AppError::Other(error.to_string()))?;
            let encrypted =
                conn.query_row("SELECT COUNT(*) FROM encrypted_secrets", [], |row| {
                    row.get(0)
                })?;
            let pending = conn.query_row("SELECT COUNT(*) FROM secrets", [], |row| row.get(0))?;
            (encrypted, pending)
        };
        let session = self
            .session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let state = match session.master_key {
            MasterKeyState::Locked => "locked",
            MasterKeyState::Ready(_) => "ready",
            MasterKeyState::Denied => "denied",
            MasterKeyState::Unavailable => "unavailable",
        };
        Ok(VaultStatus {
            state,
            encrypted_secret_count,
            pending_local_migration_count,
        })
    }

    /// Called only after the user accepts Quill's explanatory confirmation.
    /// No other method may turn a locked vault into a Keychain access request.
    pub fn authorize(&self, reason: &str, request_id: Option<&str>) -> AppResult<()> {
        if !matches!(reason, "create" | "unlock" | "import") {
            return Err(AppError::Other(
                "VAULT_CONFIRMATION_REASON_INVALID".to_string(),
            ));
        }
        let _operation = self
            .operation_lock
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;

        {
            let mut session = self
                .session
                .lock()
                .map_err(|error| AppError::Other(error.to_string()))?;
            let master_ready = matches!(session.master_key, MasterKeyState::Ready(_));
            if reason == "import" {
                // Import approval and vault unlock are separate capabilities.
                // Resolve and validate the requested legacy item even when the
                // master key is already cached.
                Self::authorize_pending_import(&mut session, request_id)?;
                if master_ready {
                    return Ok(());
                }
            } else {
                Self::authorize_pending_master(&mut session, request_id)?;
                if master_ready {
                    return Ok(());
                }
            }
            session.master_key = MasterKeyState::Locked;
        }

        if !self.use_keychain {
            let mut session = self
                .session
                .lock()
                .map_err(|error| AppError::Other(error.to_string()))?;
            session.master_key = MasterKeyState::Ready(Zeroizing::new(vec![7_u8; 32]));
            session.pending_master_request = None;
            return Ok(());
        }

        let entry = Entry::new(VAULT_KEYCHAIN_SERVICE, VAULT_MASTER_ACCOUNT)
            .map_err(|error| self.record_keychain_error(error))?;
        let master_key = match entry.get_password() {
            Ok(encoded) => self.decode_master_key(&encoded)?,
            Err(keyring::Error::NoEntry) => {
                if self.encrypted_secret_count()? > 0 {
                    self.set_master_state(MasterKeyState::Unavailable)?;
                    return Err(AppError::Other("VAULT_MASTER_KEY_MISSING".to_string()));
                }
                let mut key = Zeroizing::new(vec![0_u8; 32]);
                OsRng.fill_bytes(key.as_mut_slice());
                let encoded = Zeroizing::new(STANDARD_NO_PAD.encode(key.as_slice()));
                if let Err(error) = entry.set_password(encoded.as_str()) {
                    return Err(self.record_keychain_error(error));
                }
                key
            }
            Err(error) => return Err(self.record_keychain_error(error)),
        };

        let mut session = self
            .session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        session.master_key = MasterKeyState::Ready(master_key);
        session.pending_master_request = None;
        Ok(())
    }

    /// Starts a deliberate create/unlock attempt without touching Keychain.
    /// This is used by explicit settings actions after a prior denial.
    pub fn prepare_write(&self) -> AppResult<()> {
        let reason = if self.encrypted_secret_count()? > 0 {
            "unlock"
        } else {
            "create"
        };
        let mut session = self
            .session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        if matches!(session.master_key, MasterKeyState::Ready(_)) {
            return Ok(());
        }
        session.master_key = MasterKeyState::Locked;
        let request_id = Self::pending_master_request(&mut session);
        Err(AppError::Other(format!(
            "VAULT_CONFIRM_REQUIRED:{reason}:{request_id}"
        )))
    }

    /// Records an in-app cancellation without touching the operating-system
    /// credential store. The session remains denied until an explicit retry.
    pub fn deny(&self, reason: &str, request_id: Option<&str>) -> AppResult<()> {
        let mut session = self
            .session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        match reason {
            "create" | "unlock" => {
                Self::deny_pending_master(&mut session, request_id)?;
                session.master_key = MasterKeyState::Denied;
            }
            "import" => {
                let request_id = request_id
                    .filter(|value| !value.trim().is_empty())
                    .ok_or_else(|| AppError::Other("VAULT_IMPORT_REQUEST_INVALID".to_string()))?;
                let key = session
                    .pending_imports
                    .remove(request_id)
                    .ok_or_else(|| AppError::Other("VAULT_IMPORT_REQUEST_EXPIRED".to_string()))?;
                session.authorized_legacy_keys.remove(&key);
                session.denied_legacy_keys.insert(key);
            }
            _ => {
                return Err(AppError::Other(
                    "VAULT_CONFIRMATION_REASON_INVALID".to_string(),
                ))
            }
        }
        Ok(())
    }

    pub fn get(&self, key: &str) -> AppResult<Option<String>> {
        if let Some((nonce, ciphertext)) = self.encrypted_value(key)? {
            let master_key = self.master_key_or_confirmation("unlock")?;
            return self
                .decrypt_value(key, &nonce, &ciphertext, master_key.as_slice())
                .map(Some);
        }

        if self.has_migration_tombstone(key)? {
            return Ok(None);
        }
        if !self.is_legacy_candidate(key)? {
            return Ok(None);
        }

        if self.legacy_import_is_denied(key)? {
            return Err(AppError::Other("VAULT_ACCESS_DENIED".to_string()));
        }

        // Serialize the check/authorize/import sequence. Without this second
        // boundary, concurrent AI requests could both pass the authorization
        // check and show two macOS prompts after the first one was denied.
        let _operation = self
            .operation_lock
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        if let Some((nonce, ciphertext)) = self.encrypted_value(key)? {
            let master_key = self.master_key_or_confirmation("unlock")?;
            return self
                .decrypt_value(key, &nonce, &ciphertext, master_key.as_slice())
                .map(Some);
        }
        if self.has_migration_tombstone(key)? {
            return Ok(None);
        }
        if self.legacy_import_is_denied(key)? {
            return Err(AppError::Other("VAULT_ACCESS_DENIED".to_string()));
        }
        if !self.legacy_import_is_authorized(key)? {
            let mut session = self
                .session
                .lock()
                .map_err(|error| AppError::Other(error.to_string()))?;
            let request_id = session
                .pending_imports
                .iter()
                .find_map(|(request_id, pending_key)| {
                    (pending_key == key).then(|| request_id.clone())
                })
                .unwrap_or_else(|| {
                    let request_id = uuid::Uuid::new_v4().to_string();
                    session
                        .pending_imports
                        .insert(request_id.clone(), key.to_string());
                    request_id
                });
            return Err(AppError::Other(format!(
                "VAULT_CONFIRM_REQUIRED:import:{request_id}"
            )));
        }

        let master_key = match self.master_key_or_confirmation("import") {
            Ok(master_key) => master_key,
            Err(AppError::Other(message)) if message.starts_with("VAULT_CONFIRM_REQUIRED:") => {
                return Err(AppError::Other(message));
            }
            Err(error) => return Err(error),
        };

        if let Some(value) = self.local_legacy_value(key)? {
            self.store_encrypted(key, &value, master_key.as_slice())?;
            self.finish_legacy_import(key)?;
            return Ok(Some(value));
        }
        if !self.use_keychain {
            return Ok(None);
        }

        for service in LEGACY_KEYCHAIN_SERVICES {
            if self.legacy_entry_is_missing(service, key)? {
                continue;
            }
            let entry = match Entry::new(service, key) {
                Ok(entry) => entry,
                Err(error) => {
                    self.revoke_legacy_authorization(key)?;
                    return Err(Self::keychain_error(error));
                }
            };
            match entry.get_password() {
                Ok(value) => {
                    self.store_encrypted(key, &value, master_key.as_slice())?;
                    // Do not delete the legacy item here. SecItemDelete may
                    // present a second authorization prompt immediately after
                    // a successful read. The encrypted record takes precedence,
                    // and removing the migration candidate prevents future
                    // legacy lookups by this app.
                    self.finish_legacy_import(key)?;
                    log::info!("secrets: imported a legacy credential into the encrypted vault");
                    return Ok(Some(value));
                }
                Err(keyring::Error::NoEntry) => {
                    self.mark_legacy_entry_missing(service, key)?;
                }
                Err(error) => {
                    // The master key is already unlocked here. A denial applies
                    // only to this legacy item, so require a fresh in-app import
                    // confirmation next time without locking the whole vault.
                    self.deny_legacy_import(key)?;
                    return Err(Self::keychain_error(error));
                }
            }
        }
        self.record_migration_tombstone(key)?;
        self.finish_legacy_import(key)?;
        Ok(None)
    }

    pub fn set(&self, key: &str, value: &str) -> AppResult<()> {
        let reason = if self.encrypted_secret_count()? > 0 {
            "unlock"
        } else {
            "create"
        };
        let _operation = self
            .operation_lock
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let master_key = self.master_key_or_confirmation(reason)?;
        self.store_encrypted(key, value, master_key.as_slice())
    }

    pub fn set_many(&self, values: &[(&str, Option<&str>)]) -> AppResult<()> {
        let reason = if self.encrypted_secret_count()? > 0 {
            "unlock"
        } else {
            "create"
        };
        let _operation = self
            .operation_lock
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let master_key = self.master_key_or_confirmation(reason)?;
        let encrypted = values
            .iter()
            .map(|(key, value)| {
                value
                    .map(|value| self.encrypt_value(key, value, master_key.as_slice()))
                    .transpose()
                    .map(|payload| ((*key).to_string(), payload))
            })
            .collect::<AppResult<Vec<_>>>()?;
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let tx = conn.unchecked_transaction()?;
        for (key, payload) in encrypted {
            match payload {
                Some((nonce, ciphertext)) => {
                    Self::store_encrypted_in_transaction(&tx, &key, &nonce, &ciphertext)?;
                }
                None => Self::delete_in_transaction(&tx, &key)?,
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn snapshot_state(&self, key: &str) -> AppResult<SecretStateSnapshot> {
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let encrypted = conn
            .query_row(
                "SELECT key, nonce, ciphertext, algorithm, key_version, updated_at
                 FROM encrypted_secrets WHERE key = ?1",
                params![key],
                |row| {
                    Ok(EncryptedSecretSnapshot {
                        key: row.get(0)?,
                        nonce: row.get(1)?,
                        ciphertext: row.get(2)?,
                        algorithm: row.get(3)?,
                        key_version: row.get(4)?,
                        updated_at: row.get(5)?,
                    })
                },
            )
            .optional()?;
        let legacy_value = conn
            .query_row(
                "SELECT value FROM secrets WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        let legacy_candidate_created_at = conn
            .query_row(
                "SELECT created_at FROM legacy_secret_candidates WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        let tombstone_created_at = conn
            .query_row(
                "SELECT created_at FROM secret_migration_tombstones WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(SecretStateSnapshot {
            key: key.to_string(),
            encrypted,
            legacy_value,
            legacy_candidate_created_at,
            tombstone_created_at,
        })
    }

    #[cfg(test)]
    pub fn has_encrypted_secret(&self, key: &str) -> AppResult<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM encrypted_secrets WHERE key = ?1)",
            params![key],
            |row| row.get::<_, i64>(0),
        )? != 0)
    }

    pub fn restore_state(&self, snapshot: &SecretStateSnapshot) -> AppResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM encrypted_secrets WHERE key = ?1",
            params![snapshot.key],
        )?;
        tx.execute("DELETE FROM secrets WHERE key = ?1", params![snapshot.key])?;
        tx.execute(
            "DELETE FROM legacy_secret_candidates WHERE key = ?1",
            params![snapshot.key],
        )?;
        tx.execute(
            "DELETE FROM secret_migration_tombstones WHERE key = ?1",
            params![snapshot.key],
        )?;
        if let Some(encrypted) = snapshot.encrypted.as_ref() {
            tx.execute(
                "INSERT INTO encrypted_secrets
                     (key, nonce, ciphertext, algorithm, key_version, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    encrypted.key,
                    encrypted.nonce,
                    encrypted.ciphertext,
                    encrypted.algorithm,
                    encrypted.key_version,
                    encrypted.updated_at
                ],
            )?;
        }
        if let Some(value) = snapshot.legacy_value.as_ref() {
            tx.execute(
                "INSERT INTO secrets (key, value) VALUES (?1, ?2)",
                params![snapshot.key, value],
            )?;
        }
        if let Some(created_at) = snapshot.legacy_candidate_created_at {
            tx.execute(
                "INSERT INTO legacy_secret_candidates (key, created_at) VALUES (?1, ?2)",
                params![snapshot.key, created_at],
            )?;
        }
        if let Some(created_at) = snapshot.tombstone_created_at {
            tx.execute(
                "INSERT INTO secret_migration_tombstones (key, created_at) VALUES (?1, ?2)",
                params![snapshot.key, created_at],
            )?;
        }
        tx.commit()?;
        drop(conn);
        self.restore_legacy_candidate_in_session(snapshot)?;
        Ok(())
    }

    pub fn delete_prefix(&self, prefix: &str) -> AppResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let mut keys = {
            let mut statement = conn.prepare(
                "SELECT key FROM encrypted_secrets WHERE key LIKE ?1
                 UNION SELECT key FROM secrets WHERE key LIKE ?1",
            )?;
            let values = statement
                .query_map(params![format!("{prefix}%")], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            values
        };
        keys.extend(
            SENSITIVE_KEYS
                .iter()
                .filter(|key| key.starts_with(prefix))
                .map(|key| (*key).to_string()),
        );
        keys.sort();
        keys.dedup();
        let tx = conn.unchecked_transaction()?;
        for key in &keys {
            Self::delete_in_transaction(&tx, key)?;
        }
        tx.commit()?;
        drop(conn);
        for key in keys {
            self.forget_legacy_candidate_in_session(&key)?;
        }
        Ok(())
    }

    pub fn delete(&self, key: &str) -> AppResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let tx = conn.unchecked_transaction()?;
        Self::delete_in_transaction(&tx, key)?;
        tx.commit()?;
        drop(conn);
        self.forget_legacy_candidate_in_session(key)?;
        Ok(())
    }

    fn delete_in_transaction(tx: &rusqlite::Transaction<'_>, key: &str) -> AppResult<()> {
        tx.execute("DELETE FROM encrypted_secrets WHERE key = ?1", params![key])?;
        tx.execute("DELETE FROM secrets WHERE key = ?1", params![key])?;
        tx.execute(
            "DELETE FROM legacy_secret_candidates WHERE key = ?1",
            params![key],
        )?;
        tx.execute(
            "INSERT INTO secret_migration_tombstones (key, created_at) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET created_at = excluded.created_at",
            params![key, chrono::Utc::now().timestamp_millis()],
        )?;
        Ok(())
    }

    fn forget_legacy_candidate_in_session(&self, key: &str) -> AppResult<()> {
        let mut session = self
            .session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        session.legacy_candidates.remove(key);
        session.authorized_legacy_keys.remove(key);
        session.denied_legacy_keys.remove(key);
        session
            .pending_imports
            .retain(|_, pending_key| pending_key != key);
        Ok(())
    }

    fn restore_legacy_candidate_in_session(&self, snapshot: &SecretStateSnapshot) -> AppResult<()> {
        let mut session = self
            .session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        if snapshot.legacy_candidate_created_at.is_some() || snapshot.legacy_value.is_some() {
            session.legacy_candidates.insert(snapshot.key.clone());
        } else {
            session.legacy_candidates.remove(&snapshot.key);
        }
        session.authorized_legacy_keys.remove(&snapshot.key);
        session.denied_legacy_keys.remove(&snapshot.key);
        session
            .pending_imports
            .retain(|_, pending_key| pending_key != &snapshot.key);
        Ok(())
    }

    /// Move already-plaintext legacy settings out of the main database without
    /// touching Keychain. Encryption is deferred until the user confirms import.
    pub fn migrate_from_settings(&self, db: &Db) -> AppResult<()> {
        let values: Vec<(String, String)> = {
            let db_conn = db
                .conn
                .lock()
                .map_err(|error| AppError::Other(error.to_string()))?;
            SENSITIVE_KEYS
                .iter()
                .filter_map(|key| {
                    db_conn
                        .query_row(
                            "SELECT value FROM settings WHERE key = ?1",
                            params![key],
                            |row| row.get::<_, String>(0),
                        )
                        .ok()
                        .map(|value| ((*key).to_string(), value))
                })
                .collect()
        };
        for (key, value) in values {
            {
                let conn = self
                    .conn
                    .lock()
                    .map_err(|error| AppError::Other(error.to_string()))?;
                conn.execute(
                    "INSERT INTO secrets (key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                    params![key, value],
                )?;
            }
            let db_conn = db
                .conn
                .lock()
                .map_err(|error| AppError::Other(error.to_string()))?;
            db_conn.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
        }
        Ok(())
    }

    /// Register only metadata-backed legacy items. This never reads Keychain
    /// and prevents unrelated status checks from prompting for nonexistent
    /// OAuth or API-key entries.
    pub fn register_legacy_candidates(&self, db: &Db) -> AppResult<()> {
        let mut candidates = {
            let conn = self
                .conn
                .lock()
                .map_err(|error| AppError::Other(error.to_string()))?;
            let values = conn
                .prepare(
                    "SELECT key FROM secrets
                     UNION SELECT key FROM legacy_secret_candidates",
                )?
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<HashSet<_>, _>>()?;
            values
        };
        let db_conn = db
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        candidates.extend(
            db_conn
                .prepare("SELECT secret_ref FROM ai_credentials")?
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?,
        );
        let has_oauth_profile = db_conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM ai_profiles WHERE auth_mode = 'oauth')",
            [],
            |row| row.get::<_, i64>(0),
        )? != 0;
        drop(db_conn);
        if has_oauth_profile {
            candidates.extend(
                SENSITIVE_KEYS
                    .iter()
                    .filter(|key| key.starts_with("oauth_"))
                    .map(|key| (*key).to_string()),
            );
        }
        let mut session = self
            .session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        session.legacy_candidates = candidates;
        Ok(())
    }

    /// Persist a metadata-only hint that an older release may have stored this
    /// logical secret in Keychain. This method never opens or probes Keychain.
    pub fn register_legacy_candidate(&self, key: &str) -> AppResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        conn.execute(
            "INSERT INTO legacy_secret_candidates (key, created_at) VALUES (?1, ?2)
             ON CONFLICT(key) DO NOTHING",
            params![key, chrono::Utc::now().timestamp_millis()],
        )?;
        drop(conn);
        let mut session = self
            .session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        session.legacy_candidates.insert(key.to_string());
        Ok(())
    }

    #[cfg(test)]
    fn persisted_legacy_candidates(&self) -> AppResult<Vec<String>> {
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let mut statement =
            conn.prepare("SELECT key FROM legacy_secret_candidates ORDER BY key")?;
        let candidates = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(candidates)
    }

    pub fn has_stored_secret_metadata(&self, key: &str) -> bool {
        let Ok(conn) = self.conn.lock() else {
            return false;
        };
        let stored = conn
            .query_row(
                "SELECT EXISTS(
                 SELECT 1 FROM encrypted_secrets WHERE key = ?1
                 UNION ALL SELECT 1 FROM secrets WHERE key = ?1
                 UNION ALL SELECT 1 FROM legacy_secret_candidates WHERE key = ?1
             )",
                params![key],
                |row| row.get::<_, i64>(0),
            )
            .unwrap_or(0)
            != 0;
        drop(conn);
        stored
            || self
                .session
                .lock()
                .is_ok_and(|session| session.legacy_candidates.contains(key))
    }

    pub fn is_sensitive_key(key: &str) -> bool {
        SENSITIVE_KEYS.contains(&key) || key.starts_with("ai_api_key/") || key.starts_with("oauth_")
    }

    fn encrypted_value(&self, key: &str) -> AppResult<Option<(Vec<u8>, Vec<u8>)>> {
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let value = conn
            .query_row(
                "SELECT nonce, ciphertext FROM encrypted_secrets
                 WHERE key = ?1 AND algorithm = ?2 AND key_version = ?3",
                params![key, VAULT_ALGORITHM, VAULT_KEY_VERSION],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(value)
    }

    fn local_legacy_value(&self, key: &str) -> AppResult<Option<String>> {
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        Ok(conn
            .query_row(
                "SELECT value FROM secrets WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?)
    }

    fn store_encrypted(&self, key: &str, value: &str, master_key: &[u8]) -> AppResult<()> {
        let (nonce_bytes, ciphertext) = self.encrypt_value(key, value, master_key)?;
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let tx = conn.unchecked_transaction()?;
        Self::store_encrypted_in_transaction(&tx, key, &nonce_bytes, &ciphertext)?;
        tx.commit()?;
        Ok(())
    }

    fn encrypt_value(
        &self,
        key: &str,
        value: &str,
        master_key: &[u8],
    ) -> AppResult<([u8; 12], Vec<u8>)> {
        let cipher = Aes256Gcm::new_from_slice(master_key)
            .map_err(|_| AppError::Other("VAULT_MASTER_KEY_INVALID".to_string()))?;
        let mut nonce_bytes = [0_u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let aad = Self::aad_for(key);
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce_bytes),
                Payload {
                    msg: value.as_bytes(),
                    aad: &aad,
                },
            )
            .map_err(|_| AppError::Other("VAULT_ENCRYPTION_FAILED".to_string()))?;
        Ok((nonce_bytes, ciphertext))
    }

    fn store_encrypted_in_transaction(
        tx: &rusqlite::Transaction<'_>,
        key: &str,
        nonce_bytes: &[u8; 12],
        ciphertext: &[u8],
    ) -> AppResult<()> {
        tx.execute(
            "INSERT INTO encrypted_secrets
                 (key, nonce, ciphertext, algorithm, key_version, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(key) DO UPDATE SET
                 nonce = excluded.nonce,
                 ciphertext = excluded.ciphertext,
                 algorithm = excluded.algorithm,
                 key_version = excluded.key_version,
                 updated_at = excluded.updated_at",
            params![
                key,
                nonce_bytes.as_slice(),
                ciphertext,
                VAULT_ALGORITHM,
                VAULT_KEY_VERSION,
                chrono::Utc::now().timestamp_millis()
            ],
        )?;
        tx.execute("DELETE FROM secrets WHERE key = ?1", params![key])?;
        tx.execute(
            "DELETE FROM legacy_secret_candidates WHERE key = ?1",
            params![key],
        )?;
        tx.execute(
            "DELETE FROM secret_migration_tombstones WHERE key = ?1",
            params![key],
        )?;
        Ok(())
    }

    fn decrypt_value(
        &self,
        key: &str,
        nonce: &[u8],
        ciphertext: &[u8],
        master_key: &[u8],
    ) -> AppResult<String> {
        if nonce.len() != 12 {
            return Err(AppError::Other("VAULT_DATA_CORRUPT".to_string()));
        }
        let cipher = Aes256Gcm::new_from_slice(master_key)
            .map_err(|_| AppError::Other("VAULT_MASTER_KEY_INVALID".to_string()))?;
        let aad = Self::aad_for(key);
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    Nonce::from_slice(nonce),
                    Payload {
                        msg: ciphertext,
                        aad: &aad,
                    },
                )
                .map_err(|_| AppError::Other("VAULT_DATA_CORRUPT".to_string()))?,
        );
        String::from_utf8(plaintext.to_vec())
            .map_err(|_| AppError::Other("VAULT_DATA_CORRUPT".to_string()))
    }

    fn aad_for(key: &str) -> Vec<u8> {
        let mut aad = Vec::with_capacity(AAD_PREFIX.len() + key.len());
        aad.extend_from_slice(AAD_PREFIX);
        aad.extend_from_slice(key.as_bytes());
        aad
    }

    fn decode_master_key(&self, encoded: &str) -> AppResult<Zeroizing<Vec<u8>>> {
        let decoded = match STANDARD_NO_PAD.decode(encoded) {
            Ok(decoded) => Zeroizing::new(decoded),
            Err(_) => {
                self.set_master_state(MasterKeyState::Unavailable)?;
                return Err(AppError::Other("VAULT_MASTER_KEY_INVALID".to_string()));
            }
        };
        if decoded.len() != 32 {
            self.set_master_state(MasterKeyState::Unavailable)?;
            return Err(AppError::Other("VAULT_MASTER_KEY_INVALID".to_string()));
        }
        Ok(decoded)
    }

    fn master_key_or_confirmation(&self, reason: &str) -> AppResult<Zeroizing<Vec<u8>>> {
        let mut session = self
            .session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        match &session.master_key {
            MasterKeyState::Ready(key) => Ok(Zeroizing::new(key.as_slice().to_vec())),
            MasterKeyState::Locked => {
                let request_id = Self::pending_master_request(&mut session);
                Err(AppError::Other(format!(
                    "VAULT_CONFIRM_REQUIRED:{reason}:{request_id}"
                )))
            }
            MasterKeyState::Denied => Err(AppError::Other("VAULT_ACCESS_DENIED".to_string())),
            MasterKeyState::Unavailable => {
                Err(AppError::Other("VAULT_ACCESS_UNAVAILABLE".to_string()))
            }
        }
    }

    fn pending_master_request(session: &mut VaultSession) -> String {
        session
            .pending_master_request
            .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone()
    }

    fn authorize_pending_master(
        session: &mut VaultSession,
        request_id: Option<&str>,
    ) -> AppResult<()> {
        let request_id = request_id
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| AppError::Other("VAULT_MASTER_REQUEST_INVALID".to_string()))?;
        if session.pending_master_request.as_deref() != Some(request_id) {
            return Err(AppError::Other("VAULT_MASTER_REQUEST_EXPIRED".to_string()));
        }
        session.pending_master_request = None;
        Ok(())
    }

    fn deny_pending_master(session: &mut VaultSession, request_id: Option<&str>) -> AppResult<()> {
        Self::authorize_pending_master(session, request_id)
    }

    fn legacy_import_is_authorized(&self, key: &str) -> AppResult<bool> {
        let session = self
            .session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        Ok(session.authorized_legacy_keys.contains(key))
    }

    fn legacy_import_is_denied(&self, key: &str) -> AppResult<bool> {
        let session = self
            .session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        Ok(session.denied_legacy_keys.contains(key))
    }

    fn legacy_entry_cache_key(service: &str, key: &str) -> String {
        format!("{service}\0{key}")
    }

    fn legacy_entry_is_missing(&self, service: &str, key: &str) -> AppResult<bool> {
        let session = self
            .session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        Ok(session
            .missing_legacy_entries
            .contains(&Self::legacy_entry_cache_key(service, key)))
    }

    fn mark_legacy_entry_missing(&self, service: &str, key: &str) -> AppResult<()> {
        let mut session = self
            .session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        session
            .missing_legacy_entries
            .insert(Self::legacy_entry_cache_key(service, key));
        Ok(())
    }

    fn revoke_legacy_authorization(&self, key: &str) -> AppResult<()> {
        let mut session = self
            .session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        session.authorized_legacy_keys.remove(key);
        Ok(())
    }

    fn deny_legacy_import(&self, key: &str) -> AppResult<()> {
        let mut session = self
            .session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        session.authorized_legacy_keys.remove(key);
        session.denied_legacy_keys.insert(key.to_string());
        session
            .pending_imports
            .retain(|_, pending_key| pending_key != key);
        Ok(())
    }

    fn finish_legacy_import(&self, key: &str) -> AppResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        conn.execute(
            "DELETE FROM legacy_secret_candidates WHERE key = ?1",
            params![key],
        )?;
        drop(conn);
        let mut session = self
            .session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        session.legacy_candidates.remove(key);
        session.authorized_legacy_keys.remove(key);
        session.denied_legacy_keys.remove(key);
        session
            .pending_imports
            .retain(|_, pending_key| pending_key != key);
        Ok(())
    }

    fn is_legacy_candidate(&self, key: &str) -> AppResult<bool> {
        let session = self
            .session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        Ok(session.legacy_candidates.contains(key))
    }

    fn authorize_pending_import(
        session: &mut VaultSession,
        request_id: Option<&str>,
    ) -> AppResult<()> {
        let request_id = request_id
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| AppError::Other("VAULT_IMPORT_REQUEST_INVALID".to_string()))?;
        let key = session
            .pending_imports
            .remove(request_id)
            .ok_or_else(|| AppError::Other("VAULT_IMPORT_REQUEST_EXPIRED".to_string()))?;
        session.denied_legacy_keys.remove(&key);
        session.authorized_legacy_keys.insert(key);
        Ok(())
    }

    fn encrypted_secret_count(&self) -> AppResult<i64> {
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        Ok(
            conn.query_row("SELECT COUNT(*) FROM encrypted_secrets", [], |row| {
                row.get(0)
            })?,
        )
    }

    fn has_migration_tombstone(&self, key: &str) -> AppResult<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM secret_migration_tombstones WHERE key = ?1)",
            params![key],
            |row| row.get::<_, i64>(0),
        )? != 0)
    }

    fn record_migration_tombstone(&self, key: &str) -> AppResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        conn.execute(
            "INSERT INTO secret_migration_tombstones (key, created_at) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET created_at = excluded.created_at",
            params![key, chrono::Utc::now().timestamp_millis()],
        )?;
        Ok(())
    }

    fn set_master_state(&self, state: MasterKeyState) -> AppResult<()> {
        let mut session = self
            .session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        session.master_key = state;
        Ok(())
    }

    fn record_keychain_error(&self, error: keyring::Error) -> AppError {
        let app_error = Self::keychain_error(error);
        let state = if matches!(&app_error, AppError::Other(message) if message == "VAULT_ACCESS_DENIED")
        {
            MasterKeyState::Denied
        } else {
            MasterKeyState::Unavailable
        };
        let _ = self.set_master_state(state);
        app_error
    }

    fn keychain_error(error: keyring::Error) -> AppError {
        if Self::keychain_os_status(&error).is_some_and(Self::is_denied_os_status) {
            return AppError::Other("VAULT_ACCESS_DENIED".to_string());
        }
        let description = error.to_string().to_ascii_lowercase();
        let denied = description.contains("cancel")
            || description.contains("denied")
            || description.contains("auth")
            // macOS commonly returns errSecAuthFailed for a rejected prompt,
            // a cancelled prompt, or an incorrect login Keychain password.
            || description.contains("-25293")
            || description.contains("user canceled")
            || description.contains("user cancelled")
            || description.contains("interaction is not allowed");
        if denied {
            AppError::Other("VAULT_ACCESS_DENIED".to_string())
        } else {
            AppError::Other("VAULT_ACCESS_UNAVAILABLE".to_string())
        }
    }

    #[cfg(target_os = "macos")]
    fn keychain_os_status(error: &keyring::Error) -> Option<i32> {
        let platform_error = match error {
            keyring::Error::PlatformFailure(error) | keyring::Error::NoStorageAccess(error) => {
                error.as_ref()
            }
            _ => return None,
        };
        platform_error
            .downcast_ref::<security_framework::base::Error>()
            .map(|error| error.code())
    }

    #[cfg(not(target_os = "macos"))]
    fn keychain_os_status(_error: &keyring::Error) -> Option<i32> {
        None
    }

    fn is_denied_os_status(status: i32) -> bool {
        matches!(status, -25293 | -128 | -25308)
    }
}

#[cfg(test)]
impl Secrets {
    pub fn init_in_memory() -> AppResult<Self> {
        let conn = Connection::open_in_memory()?;
        Self::initialize_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            session: Arc::new(Mutex::new(VaultSession {
                master_key: MasterKeyState::Ready(Zeroizing::new(vec![7_u8; 32])),
                pending_master_request: None,
                legacy_candidates: HashSet::new(),
                authorized_legacy_keys: HashSet::new(),
                denied_legacy_keys: HashSet::new(),
                missing_legacy_entries: HashSet::new(),
                pending_imports: HashMap::new(),
            })),
            operation_lock: Arc::new(Mutex::new(())),
            use_keychain: false,
        })
    }

    pub fn lock_for_test(&self) -> AppResult<()> {
        let mut session = self
            .session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        session.master_key = MasterKeyState::Locked;
        session.pending_master_request = None;
        Ok(())
    }

    // Test-only state transitions keep production helpers private to the
    // production impl while allowing denial behavior to be verified directly.
    fn deny_master_for_test(&self) -> AppResult<()> {
        self.set_master_state(MasterKeyState::Denied)
    }

    fn master_confirmation_request(&self, reason: &str) -> AppResult<String> {
        Ok(self
            .master_key_or_confirmation(reason)
            .unwrap_err()
            .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypted_round_trip_uses_fresh_nonces() {
        let secrets = Secrets::init_in_memory().unwrap();
        secrets.set("ai_api_key/test", "secret-value").unwrap();
        let first = secrets.encrypted_value("ai_api_key/test").unwrap().unwrap();
        assert_eq!(
            secrets.get("ai_api_key/test").unwrap().as_deref(),
            Some("secret-value")
        );

        secrets.set("ai_api_key/test", "secret-value").unwrap();
        let second = secrets.encrypted_value("ai_api_key/test").unwrap().unwrap();
        assert_ne!(first.0, second.0);
        assert_ne!(first.1, b"secret-value");
    }

    #[test]
    fn aad_prevents_ciphertext_from_moving_between_keys() {
        let secrets = Secrets::init_in_memory().unwrap();
        secrets.set("ai_api_key/one", "secret-value").unwrap();
        let (nonce, ciphertext) = secrets.encrypted_value("ai_api_key/one").unwrap().unwrap();
        let key = secrets.master_key_or_confirmation("unlock").unwrap();
        assert!(secrets
            .decrypt_value("ai_api_key/two", &nonce, &ciphertext, key.as_slice())
            .is_err());
    }

    #[test]
    fn deletion_tombstone_prevents_legacy_resurrection() {
        let secrets = Secrets::init_in_memory().unwrap();
        secrets.set("oauth_access_token", "token").unwrap();
        secrets.delete("oauth_access_token").unwrap();
        assert_eq!(secrets.get("oauth_access_token").unwrap(), None);
    }

    #[test]
    fn denied_master_key_stops_the_session_from_prompting_again() {
        let secrets = Secrets::init_in_memory().unwrap();
        secrets.set("ai_api_key/test", "secret-value").unwrap();
        secrets.deny_master_for_test().unwrap();

        let error = secrets.get("ai_api_key/test").unwrap_err().to_string();
        assert_eq!(error, "VAULT_ACCESS_DENIED");
    }

    #[test]
    fn master_authorization_requires_the_current_backend_request_id() {
        let secrets = Secrets::init_in_memory().unwrap();
        secrets.lock_for_test().unwrap();
        let request = secrets.master_confirmation_request("create").unwrap();
        let request_id = request.rsplit(':').next().unwrap();

        let stale = secrets.authorize("create", Some("00000000-0000-0000-0000-000000000000"));
        assert_eq!(
            stale.unwrap_err().to_string(),
            "VAULT_MASTER_REQUEST_EXPIRED"
        );

        secrets.authorize("create", Some(request_id)).unwrap();
        assert!(secrets.master_key_or_confirmation("unlock").is_ok());
    }

    #[test]
    fn denied_master_request_invalidates_all_concurrent_confirmations() {
        let secrets = Secrets::init_in_memory().unwrap();
        secrets.lock_for_test().unwrap();
        let first = secrets.master_confirmation_request("unlock").unwrap();
        let second = secrets.master_confirmation_request("unlock").unwrap();
        assert_eq!(first, second);
        let request_id = first.rsplit(':').next().unwrap();

        secrets.deny("unlock", Some(request_id)).unwrap();
        assert_eq!(
            secrets
                .authorize("unlock", Some(request_id))
                .unwrap_err()
                .to_string(),
            "VAULT_MASTER_REQUEST_EXPIRED"
        );
        assert_eq!(
            secrets
                .master_key_or_confirmation("unlock")
                .unwrap_err()
                .to_string(),
            "VAULT_ACCESS_DENIED"
        );
    }

    #[test]
    fn batch_update_removes_optional_values_atomically() {
        let secrets = Secrets::init_in_memory().unwrap();
        secrets
            .set_many(&[
                ("oauth_access_token", Some("access")),
                ("oauth_account_id", Some("account")),
            ])
            .unwrap();
        secrets
            .set_many(&[
                ("oauth_access_token", Some("new-access")),
                ("oauth_account_id", None),
            ])
            .unwrap();
        assert_eq!(
            secrets.get("oauth_access_token").unwrap().as_deref(),
            Some("new-access")
        );
        assert_eq!(secrets.get("oauth_account_id").unwrap(), None);
    }

    #[test]
    fn legacy_candidate_is_persisted_without_reading_a_secret() {
        let secrets = Secrets::init_in_memory().unwrap();
        secrets.register_legacy_candidate("ai_api_key").unwrap();
        assert_eq!(
            secrets.persisted_legacy_candidates().unwrap(),
            vec!["ai_api_key".to_string()]
        );
        assert_eq!(secrets.get("unrelated_key").unwrap(), None);
    }

    #[test]
    fn deleting_a_credential_also_removes_its_legacy_candidate() {
        let secrets = Secrets::init_in_memory().unwrap();
        secrets
            .register_legacy_candidate("ai_api_key/legacy")
            .unwrap();

        secrets.delete("ai_api_key/legacy").unwrap();

        assert!(secrets.persisted_legacy_candidates().unwrap().is_empty());
        assert!(!secrets.has_stored_secret_metadata("ai_api_key/legacy"));
    }

    #[test]
    fn denied_os_statuses_are_classified_structurally() {
        assert!(Secrets::is_denied_os_status(-25293));
        assert!(Secrets::is_denied_os_status(-128));
        assert!(Secrets::is_denied_os_status(-25308));
        assert!(!Secrets::is_denied_os_status(-25291));
    }

    #[test]
    fn missing_legacy_entry_cache_is_scoped_to_service_and_key() {
        let secrets = Secrets::init_in_memory().unwrap();
        secrets
            .mark_legacy_entry_missing("legacy.service", "ai_api_key/one")
            .unwrap();

        assert!(secrets
            .legacy_entry_is_missing("legacy.service", "ai_api_key/one")
            .unwrap());
        assert!(!secrets
            .legacy_entry_is_missing("legacy.service", "ai_api_key/two")
            .unwrap());
        assert!(!secrets
            .legacy_entry_is_missing("other.service", "ai_api_key/one")
            .unwrap());
    }
}
