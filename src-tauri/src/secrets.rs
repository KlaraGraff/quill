use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use keyring::Entry;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use zeroize::Zeroizing;

use crate::db::Db;
use crate::error::{AppError, AppResult};

// Historical v1.4 vault identity. It is read only during the explicit one-time
// migration and is never touched by routine credential reads or writes.
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

#[derive(Default)]
struct MigrationSession {
    master_key: Option<Zeroizing<Vec<u8>>>,
    master_key_error: Option<String>,
    authorized: bool,
    pending_request: Option<String>,
    #[cfg(test)]
    fail_next_delete: bool,
    #[cfg(test)]
    fail_next_restore: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatus {
    encrypted_secret_count: i64,
    legacy_keychain_candidate_count: i64,
    pending_migration_count: i64,
}

#[derive(Clone)]
struct EncryptedSecretSnapshot {
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
    local_value: Option<String>,
    local_created_at: Option<i64>,
    legacy_candidate_created_at: Option<i64>,
    tombstone_created_at: Option<i64>,
}

/// Local credential storage for API keys and OAuth tokens.
///
/// Routine reads and writes use only the local `secrets` table. The old v1.4
/// encrypted vault and its Keychain master key remain available exclusively to
/// the user-triggered migration command.
#[derive(Clone)]
pub struct Secrets {
    pub conn: Arc<Mutex<Connection>>,
    migration_session: Arc<Mutex<MigrationSession>>,
    operation_lock: Arc<Mutex<()>>,
    use_keychain: bool,
    #[cfg(test)]
    legacy_keychain_values: Arc<Mutex<HashMap<String, String>>>,
}

impl Secrets {
    pub fn init(local_dir: &PathBuf) -> AppResult<Self> {
        fs::create_dir_all(local_dir)?;
        let db_path = local_dir.join("secrets.db");
        Self::prepare_private_file(&db_path)?;
        let conn = Connection::open(&db_path)?;
        Self::initialize_schema(&conn)?;
        Self::harden_sqlite_files(&db_path)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            migration_session: Arc::new(Mutex::new(MigrationSession::default())),
            operation_lock: Arc::new(Mutex::new(())),
            use_keychain: true,
            #[cfg(test)]
            legacy_keychain_values: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn initialize_schema(conn: &Connection) -> AppResult<()> {
        conn.execute_batch(
            "PRAGMA journal_mode=DELETE;
             PRAGMA secure_delete=ON;
             CREATE TABLE IF NOT EXISTS secrets (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL,
                 created_at INTEGER NOT NULL DEFAULT 0
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
        let journal_mode =
            conn.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))?;
        if !matches!(journal_mode.as_str(), "delete" | "memory") {
            return Err(AppError::Other(format!(
                "CREDENTIAL_DB_JOURNAL_MODE_UNSAFE:{journal_mode}"
            )));
        }
        let has_created_at = conn
            .prepare("PRAGMA table_info(secrets)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .any(|name| name == "created_at");
        if !has_created_at {
            conn.execute_batch(
                "ALTER TABLE secrets ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
        Ok(())
    }

    #[cfg(unix)]
    fn prepare_private_file(path: &Path) -> AppResult<()> {
        use std::fs::OpenOptions;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    #[cfg(not(unix))]
    fn prepare_private_file(path: &Path) -> AppResult<()> {
        use std::fs::OpenOptions;

        OpenOptions::new().create(true).append(true).open(path)?;
        Ok(())
    }

    fn harden_sqlite_files(db_path: &Path) -> AppResult<()> {
        Self::prepare_private_file(db_path)?;
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut path = db_path.as_os_str().to_os_string();
            path.push(suffix);
            let path = PathBuf::from(path);
            if path.exists() {
                Self::harden_existing_file(&path)?;
            }
        }
        Ok(())
    }

    #[cfg(unix)]
    fn harden_existing_file(path: &Path) -> AppResult<()> {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    #[cfg(not(unix))]
    fn harden_existing_file(_path: &Path) -> AppResult<()> {
        Ok(())
    }

    /// Metadata only. This never opens or probes the operating-system store.
    pub fn status(&self) -> AppResult<VaultStatus> {
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let encrypted_secret_count =
            conn.query_row("SELECT COUNT(*) FROM encrypted_secrets", [], |row| {
                row.get(0)
            })?;
        let legacy_keychain_candidate_count = conn.query_row(
            "SELECT COUNT(*) FROM legacy_secret_candidates c
             WHERE NOT EXISTS (SELECT 1 FROM secrets s WHERE s.key = c.key)
               AND NOT EXISTS (
                   SELECT 1 FROM secret_migration_tombstones t WHERE t.key = c.key
               )",
            [],
            |row| row.get(0),
        )?;
        let pending_migration_count = conn.query_row(
            "SELECT COUNT(*) FROM (
                 SELECT key FROM encrypted_secrets
                 UNION
                 SELECT c.key FROM legacy_secret_candidates c
                 WHERE NOT EXISTS (SELECT 1 FROM secrets s WHERE s.key = c.key)
                   AND NOT EXISTS (
                       SELECT 1 FROM secret_migration_tombstones t WHERE t.key = c.key
                   )
             )",
            [],
            |row| row.get(0),
        )?;
        Ok(VaultStatus {
            encrypted_secret_count,
            legacy_keychain_candidate_count,
            pending_migration_count,
        })
    }

    /// Called only after the user accepts the migration explanation.
    pub fn authorize(&self, reason: &str, request_id: Option<&str>) -> AppResult<()> {
        if reason != "migrate" {
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
                .migration_session
                .lock()
                .map_err(|error| AppError::Other(error.to_string()))?;
            Self::consume_pending_request(&mut session, request_id)?;
            session.authorized = true;
            session.master_key = None;
            session.master_key_error = None;
        }

        if self.encrypted_secret_count()? == 0 {
            return Ok(());
        }
        if !self.use_keychain {
            let mut session = self
                .migration_session
                .lock()
                .map_err(|error| AppError::Other(error.to_string()))?;
            session.master_key = Some(Zeroizing::new(vec![7_u8; 32]));
            return Ok(());
        }

        let master_key = (|| -> AppResult<Zeroizing<Vec<u8>>> {
            let entry = Entry::new(VAULT_KEYCHAIN_SERVICE, VAULT_MASTER_ACCOUNT)
                .map_err(Self::keychain_error)?;
            let encoded = entry.get_password().map_err(Self::keychain_error)?;
            Self::decode_master_key(&encoded)
        })();
        let master_key = match master_key {
            Ok(master_key) => master_key,
            Err(AppError::Other(code))
                if matches!(
                    code.as_str(),
                    "VAULT_MASTER_KEY_MISSING" | "VAULT_MASTER_KEY_INVALID"
                ) =>
            {
                // A missing or malformed v1.4 master key must not prevent the
                // same explicit migration action from recovering independent
                // per-item credentials saved by still older releases.
                let mut session = self
                    .migration_session
                    .lock()
                    .map_err(|error| AppError::Other(error.to_string()))?;
                session.master_key_error = Some(code);
                return Ok(());
            }
            Err(error) => {
                self.clear_migration_session()?;
                return Err(error);
            }
        };
        let mut session = self
            .migration_session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        session.master_key = Some(master_key);
        session.master_key_error = None;
        Ok(())
    }

    pub fn deny(&self, reason: &str, request_id: Option<&str>) -> AppResult<()> {
        if reason != "migrate" {
            return Err(AppError::Other(
                "VAULT_CONFIRMATION_REASON_INVALID".to_string(),
            ));
        }
        let _operation = self
            .operation_lock
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let mut session = self
            .migration_session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        Self::consume_pending_request(&mut session, request_id)?;
        session.authorized = false;
        session.master_key = None;
        session.master_key_error = None;
        Ok(())
    }

    pub fn get(&self, key: &str) -> AppResult<Option<String>> {
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

    pub fn set(&self, key: &str, value: &str) -> AppResult<()> {
        let _operation = self
            .operation_lock
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let tx = conn.unchecked_transaction()?;
        Self::store_local_in_transaction(&tx, key, value)?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_many(&self, values: &[(&str, Option<&str>)]) -> AppResult<()> {
        let _operation = self
            .operation_lock
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let tx = conn.unchecked_transaction()?;
        for (key, value) in values {
            match value {
                Some(value) => Self::store_local_in_transaction(&tx, key, value)?,
                None => Self::delete_in_transaction(&tx, key)?,
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Imports all readable old-vault values into local storage in one
    /// transaction. A local value always wins over its older encrypted copy.
    /// No call site other than the explicit AI-settings migration action may
    /// invoke this method.
    pub fn migrate_to_local(&self) -> AppResult<i64> {
        let _operation = self
            .operation_lock
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        if self.status()?.pending_migration_count == 0 {
            self.clear_migration_session()?;
            return Ok(0);
        }
        let (master_key, mut migration_error) = {
            let mut session = self
                .migration_session
                .lock()
                .map_err(|error| AppError::Other(error.to_string()))?;
            if !session.authorized {
                let request_id = session
                    .pending_request
                    .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
                    .clone();
                return Err(AppError::Other(format!(
                    "VAULT_CONFIRM_REQUIRED:migrate:{request_id}"
                )));
            }
            (
                session
                    .master_key
                    .as_ref()
                    .map(|key| Zeroizing::new(key.as_slice().to_vec())),
                session.master_key_error.clone(),
            )
        };

        let (encrypted, local_keys, candidates) = {
            let conn = self
                .conn
                .lock()
                .map_err(|error| AppError::Other(error.to_string()))?;
            let encrypted = conn
                .prepare(
                    "SELECT key, nonce, ciphertext, algorithm, key_version
                     FROM encrypted_secrets ORDER BY key",
                )?
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let local_keys = conn
                .prepare("SELECT key FROM secrets")?
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<HashSet<_>, _>>()?;
            let candidates = conn
                .prepare(
                    "SELECT c.key FROM legacy_secret_candidates c
                     WHERE NOT EXISTS (
                         SELECT 1 FROM secret_migration_tombstones t WHERE t.key = c.key
                     )
                     ORDER BY c.key",
                )?
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            (encrypted, local_keys, candidates)
        };

        let mut imported: HashMap<String, Zeroizing<String>> = HashMap::new();
        let mut encrypted_keys_to_delete = Vec::new();
        for (name, nonce, ciphertext, algorithm, key_version) in encrypted {
            if local_keys.contains(&name) {
                // The local value is already the active value. This preserves
                // the established migration rule while removing its obsolete
                // encrypted duplicate in the same transaction below.
                encrypted_keys_to_delete.push(name);
                continue;
            }
            let Some(key) = master_key.as_ref() else {
                migration_error.get_or_insert_with(|| "VAULT_MASTER_KEY_MISSING".to_string());
                continue;
            };
            if algorithm != VAULT_ALGORITHM || key_version != VAULT_KEY_VERSION {
                migration_error.get_or_insert_with(|| "VAULT_DATA_CORRUPT".to_string());
                continue;
            }
            match Self::decrypt_value(&name, &nonce, &ciphertext, key.as_slice()) {
                Ok(value) => {
                    encrypted_keys_to_delete.push(name.clone());
                    imported.insert(name, Zeroizing::new(value));
                }
                Err(error) => {
                    migration_error.get_or_insert_with(|| error.to_string());
                }
            }
        }

        let mut missing_candidates = Vec::new();
        for candidate in &candidates {
            if local_keys.contains(candidate) || imported.contains_key(candidate) {
                continue;
            }
            let mut found = None;
            if self.use_keychain {
                for service in LEGACY_KEYCHAIN_SERVICES {
                    let entry = Entry::new(service, candidate).map_err(Self::keychain_error)?;
                    match entry.get_password() {
                        Ok(value) => {
                            found = Some(Zeroizing::new(value));
                            break;
                        }
                        Err(keyring::Error::NoEntry) => {}
                        Err(error) => return Err(Self::keychain_error(error)),
                    }
                }
            }
            #[cfg(test)]
            if !self.use_keychain {
                found = self
                    .legacy_keychain_values
                    .lock()
                    .map_err(|error| AppError::Other(error.to_string()))?
                    .get(candidate)
                    .cloned()
                    .map(Zeroizing::new);
            }
            match found {
                Some(value) => {
                    imported.insert(candidate.clone(), value);
                }
                None => missing_candidates.push(candidate.clone()),
            }
        }

        let imported_count = imported.len() as i64;
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let tx = conn.unchecked_transaction()?;
        for (key, value) in &imported {
            Self::store_local_in_transaction(&tx, key, value.as_str())?;
        }
        for key in encrypted_keys_to_delete {
            tx.execute("DELETE FROM encrypted_secrets WHERE key = ?1", params![key])?;
        }
        for candidate in &candidates {
            tx.execute(
                "DELETE FROM legacy_secret_candidates WHERE key = ?1",
                params![candidate],
            )?;
        }
        for candidate in missing_candidates {
            tx.execute(
                "INSERT INTO secret_migration_tombstones (key, created_at)
                 VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET created_at = excluded.created_at",
                params![candidate, chrono::Utc::now().timestamp_millis()],
            )?;
        }
        tx.commit()?;
        drop(conn);
        self.clear_migration_session()?;
        let remaining = self.status()?.pending_migration_count;
        log::info!(
            "secrets: migrated {imported_count} credential(s) to local storage; {remaining} pending"
        );
        if remaining > 0 {
            if let Some(error) = migration_error {
                return Err(AppError::Other(format!(
                    "VAULT_PARTIAL_MIGRATION:imported={imported_count}:pending={remaining}:{error}"
                )));
            }
        }
        Ok(imported_count)
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
        let local = conn
            .query_row(
                "SELECT value, created_at FROM secrets WHERE key = ?1",
                params![key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
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
            local_value: local.as_ref().map(|value| value.0.clone()),
            local_created_at: local.map(|value| value.1),
            legacy_candidate_created_at,
            tombstone_created_at,
        })
    }

    pub fn restore_state(&self, snapshot: &SecretStateSnapshot) -> AppResult<()> {
        let _operation = self
            .operation_lock
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        #[cfg(test)]
        {
            let mut session = self
                .migration_session
                .lock()
                .map_err(|error| AppError::Other(error.to_string()))?;
            if std::mem::take(&mut session.fail_next_restore) {
                return Err(AppError::Other("TEST_SECRET_RESTORE_FAILED".to_string()));
            }
        }
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
        if let Some(value) = snapshot.local_value.as_ref() {
            tx.execute(
                "INSERT INTO secrets (key, value, created_at) VALUES (?1, ?2, ?3)",
                params![snapshot.key, value, snapshot.local_created_at.unwrap_or(0)],
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
        Ok(())
    }

    pub fn delete_prefix(&self, prefix: &str) -> AppResult<()> {
        let _operation = self
            .operation_lock
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let mut keys = conn
            .prepare(
                "SELECT key FROM encrypted_secrets WHERE key LIKE ?1
                 UNION SELECT key FROM secrets WHERE key LIKE ?1
                 UNION SELECT key FROM legacy_secret_candidates WHERE key LIKE ?1",
            )?
            .query_map(params![format!("{prefix}%")], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        keys.extend(
            SENSITIVE_KEYS
                .iter()
                .filter(|key| key.starts_with(prefix))
                .map(|key| (*key).to_string()),
        );
        keys.sort();
        keys.dedup();
        let tx = conn.unchecked_transaction()?;
        for key in keys {
            Self::delete_in_transaction(&tx, &key)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete(&self, key: &str) -> AppResult<()> {
        let _operation = self
            .operation_lock
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        #[cfg(test)]
        {
            let mut session = self
                .migration_session
                .lock()
                .map_err(|error| AppError::Other(error.to_string()))?;
            if std::mem::take(&mut session.fail_next_delete) {
                return Err(AppError::Other("TEST_SECRET_DELETE_FAILED".to_string()));
            }
        }
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let tx = conn.unchecked_transaction()?;
        Self::delete_in_transaction(&tx, key)?;
        tx.commit()?;
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

    fn store_local_in_transaction(
        tx: &rusqlite::Transaction<'_>,
        key: &str,
        value: &str,
    ) -> AppResult<()> {
        tx.execute(
            "INSERT INTO secrets (key, value, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value,
                 created_at = excluded.created_at",
            params![key, value, chrono::Utc::now().timestamp_millis()],
        )?;
        tx.execute("DELETE FROM encrypted_secrets WHERE key = ?1", params![key])?;
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

    /// Move older plaintext settings into the local-only credential database.
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
        if values.is_empty() {
            return Ok(());
        }
        {
            let conn = self
                .conn
                .lock()
                .map_err(|error| AppError::Other(error.to_string()))?;
            let tx = conn.unchecked_transaction()?;
            for (key, value) in &values {
                Self::store_local_in_transaction(&tx, key, value)?;
            }
            tx.commit()?;
        }
        let db_conn = db
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        let tx = db_conn.unchecked_transaction()?;
        for (key, _) in values {
            tx.execute("DELETE FROM settings WHERE key = ?1", params![key])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Rebuild migration hints from profile metadata without probing Keychain.
    /// This covers users who jump directly from a pre-vault version: their
    /// profile rows already exist, so the single-profile migration has no work
    /// to do, while the referenced value may still live only in Keychain.
    pub fn register_legacy_candidates(&self, db: &Db) -> AppResult<()> {
        let (mut candidates, has_oauth_profile) = {
            let conn = db.reader();
            let candidates = conn
                .prepare("SELECT secret_ref FROM ai_credentials")?
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            let has_oauth_profile = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM ai_profiles WHERE auth_mode = 'oauth')",
                [],
                |row| row.get::<_, i64>(0),
            )? != 0;
            (candidates, has_oauth_profile)
        };
        if has_oauth_profile {
            candidates.extend(
                SENSITIVE_KEYS
                    .iter()
                    .filter(|key| key.starts_with("oauth_"))
                    .map(|key| (*key).to_string()),
            );
        }
        candidates.sort();
        candidates.dedup();
        for candidate in candidates {
            self.register_legacy_candidate(&candidate)?;
        }
        Ok(())
    }

    /// Persist a metadata-only hint without reading the old Keychain item.
    pub fn register_legacy_candidate(&self, key: &str) -> AppResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        conn.execute(
            "INSERT INTO legacy_secret_candidates (key, created_at)
             SELECT ?1, ?2
             WHERE NOT EXISTS (SELECT 1 FROM secrets WHERE key = ?1)
               AND NOT EXISTS (SELECT 1 FROM encrypted_secrets WHERE key = ?1)
               AND NOT EXISTS (
                   SELECT 1 FROM secret_migration_tombstones WHERE key = ?1
               )
             ON CONFLICT(key) DO NOTHING",
            params![key, chrono::Utc::now().timestamp_millis()],
        )?;
        Ok(())
    }

    pub fn has_stored_secret_metadata(&self, key: &str) -> bool {
        let Ok(conn) = self.conn.lock() else {
            return false;
        };
        conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM encrypted_secrets WHERE key = ?1
                 UNION ALL SELECT 1 FROM secrets WHERE key = ?1
                 UNION ALL SELECT 1 FROM legacy_secret_candidates WHERE key = ?1
             )",
            params![key],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
            != 0
    }

    pub fn is_sensitive_key(key: &str) -> bool {
        SENSITIVE_KEYS.contains(&key) || key.starts_with("ai_api_key/") || key.starts_with("oauth_")
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

    fn decode_master_key(encoded: &str) -> AppResult<Zeroizing<Vec<u8>>> {
        let decoded = Zeroizing::new(
            STANDARD_NO_PAD
                .decode(encoded)
                .map_err(|_| AppError::Other("VAULT_MASTER_KEY_INVALID".to_string()))?,
        );
        if decoded.len() != 32 {
            return Err(AppError::Other("VAULT_MASTER_KEY_INVALID".to_string()));
        }
        Ok(decoded)
    }

    fn decrypt_value(
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

    fn consume_pending_request(
        session: &mut MigrationSession,
        request_id: Option<&str>,
    ) -> AppResult<()> {
        let request_id = request_id
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| AppError::Other("VAULT_MIGRATION_REQUEST_INVALID".to_string()))?;
        if session.pending_request.as_deref() != Some(request_id) {
            return Err(AppError::Other(
                "VAULT_MIGRATION_REQUEST_EXPIRED".to_string(),
            ));
        }
        session.pending_request = None;
        Ok(())
    }

    fn clear_migration_session(&self) -> AppResult<()> {
        let mut session = self
            .migration_session
            .lock()
            .map_err(|error| AppError::Other(error.to_string()))?;
        session.master_key = None;
        session.master_key_error = None;
        session.authorized = false;
        session.pending_request = None;
        Ok(())
    }

    fn keychain_error(error: keyring::Error) -> AppError {
        if matches!(&error, keyring::Error::NoEntry) {
            return AppError::Other("VAULT_MASTER_KEY_MISSING".to_string());
        }
        if Self::keychain_os_status(&error).is_some_and(Self::is_denied_os_status) {
            return AppError::Other("VAULT_ACCESS_DENIED".to_string());
        }
        let description = error.to_string().to_ascii_lowercase();
        let denied = description.contains("cancel")
            || description.contains("denied")
            || description.contains("auth")
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
            migration_session: Arc::new(Mutex::new(MigrationSession::default())),
            operation_lock: Arc::new(Mutex::new(())),
            use_keychain: false,
            legacy_keychain_values: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn lock_for_test(&self) -> AppResult<()> {
        self.clear_migration_session()
    }

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

    fn store_encrypted_for_test(&self, key: &str, value: &str, nonce: [u8; 12]) {
        let master_key = [7_u8; 32];
        let cipher = Aes256Gcm::new_from_slice(&master_key).unwrap();
        let aad = Self::aad_for(key);
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: value.as_bytes(),
                    aad: &aad,
                },
            )
            .unwrap();
        self.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO encrypted_secrets
                     (key, nonce, ciphertext, algorithm, key_version, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    key,
                    nonce.as_slice(),
                    ciphertext,
                    VAULT_ALGORITHM,
                    VAULT_KEY_VERSION,
                    1_i64
                ],
            )
            .unwrap();
    }

    fn authorize_pending_migration_for_test(&self) {
        let error = self.migrate_to_local().unwrap_err().to_string();
        let request_id = error.rsplit(':').next().unwrap();
        self.authorize("migrate", Some(request_id)).unwrap();
    }

    fn authorize_pending_migration_with_master_error_for_test(&self, error: &str) {
        let request = self.migrate_to_local().unwrap_err().to_string();
        let request_id = request.rsplit(':').next().unwrap();
        let mut session = self.migration_session.lock().unwrap();
        Self::consume_pending_request(&mut session, Some(request_id)).unwrap();
        session.authorized = true;
        session.master_key = None;
        session.master_key_error = Some(error.to_string());
    }

    pub fn seed_legacy_keychain_value_for_test(&self, key: &str, value: &str) {
        self.legacy_keychain_values
            .lock()
            .unwrap()
            .insert(key.to_string(), value.to_string());
    }

    pub fn fail_next_delete_for_test(&self) {
        self.migration_session.lock().unwrap().fail_next_delete = true;
    }

    pub fn fail_next_restore_for_test(&self) {
        self.migration_session.lock().unwrap().fail_next_restore = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_round_trip_never_requires_migration_authorization() {
        let secrets = Secrets::init_in_memory().unwrap();
        let secure_delete = secrets
            .conn
            .lock()
            .unwrap()
            .query_row("PRAGMA secure_delete", [], |row| row.get::<_, i64>(0))
            .unwrap();
        assert_eq!(secure_delete, 1);
        secrets.set("ai_api_key/test", "secret-value").unwrap();
        secrets.lock_for_test().unwrap();
        assert_eq!(
            secrets.get("ai_api_key/test").unwrap().as_deref(),
            Some("secret-value")
        );
        assert!(!secrets.has_encrypted_secret("ai_api_key/test").unwrap());
    }

    #[test]
    fn encrypted_vault_is_migrated_only_after_explicit_authorization() {
        let secrets = Secrets::init_in_memory().unwrap();
        secrets.store_encrypted_for_test("ai_api_key/legacy", "legacy-key", [1_u8; 12]);

        assert_eq!(secrets.get("ai_api_key/legacy").unwrap(), None);
        let error = secrets.migrate_to_local().unwrap_err().to_string();
        assert!(error.starts_with("VAULT_CONFIRM_REQUIRED:migrate:"));
        let request_id = error.rsplit(':').next().unwrap();
        secrets.authorize("migrate", Some(request_id)).unwrap();

        assert_eq!(secrets.migrate_to_local().unwrap(), 1);
        assert_eq!(
            secrets.get("ai_api_key/legacy").unwrap().as_deref(),
            Some("legacy-key")
        );
        assert!(!secrets.has_encrypted_secret("ai_api_key/legacy").unwrap());
        assert_eq!(secrets.status().unwrap().pending_migration_count, 0);
    }

    #[test]
    fn newer_local_value_wins_and_old_ciphertext_is_removed() {
        let secrets = Secrets::init_in_memory().unwrap();
        secrets.store_encrypted_for_test("ai_api_key/shared", "old-key", [2_u8; 12]);
        {
            let conn = secrets.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO secrets (key, value, created_at) VALUES (?1, ?2, ?3)",
                params!["ai_api_key/shared", "new-key", 2_i64],
            )
            .unwrap();
        }
        secrets.authorize_pending_migration_for_test();

        assert_eq!(secrets.migrate_to_local().unwrap(), 0);
        assert_eq!(
            secrets.get("ai_api_key/shared").unwrap().as_deref(),
            Some("new-key")
        );
        assert!(!secrets.has_encrypted_secret("ai_api_key/shared").unwrap());
    }

    #[test]
    fn corrupt_vault_row_is_retained_while_readable_rows_migrate() {
        let secrets = Secrets::init_in_memory().unwrap();
        secrets.store_encrypted_for_test("ai_api_key/good", "good", [3_u8; 12]);
        secrets.store_encrypted_for_test("ai_api_key/bad", "bad", [4_u8; 12]);
        secrets
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE encrypted_secrets SET ciphertext = zeroblob(16)
                 WHERE key = 'ai_api_key/bad'",
                [],
            )
            .unwrap();
        secrets.authorize_pending_migration_for_test();

        let error = secrets.migrate_to_local().unwrap_err().to_string();
        assert!(error.starts_with("VAULT_PARTIAL_MIGRATION:imported=1:pending=1:"));
        assert!(error.ends_with("VAULT_DATA_CORRUPT"));
        assert_eq!(
            secrets.get("ai_api_key/good").unwrap().as_deref(),
            Some("good")
        );
        assert!(!secrets.has_encrypted_secret("ai_api_key/good").unwrap());
        assert!(secrets.has_encrypted_secret("ai_api_key/bad").unwrap());
    }

    #[test]
    fn missing_master_key_does_not_block_per_item_legacy_import() {
        let secrets = Secrets::init_in_memory().unwrap();
        secrets.store_encrypted_for_test("ai_api_key/vault", "vault-key", [9_u8; 12]);
        secrets
            .register_legacy_candidate("ai_api_key/per-item")
            .unwrap();
        secrets.seed_legacy_keychain_value_for_test("ai_api_key/per-item", "legacy-key");
        secrets.authorize_pending_migration_with_master_error_for_test("VAULT_MASTER_KEY_MISSING");

        let error = secrets.migrate_to_local().unwrap_err().to_string();
        assert!(error.starts_with("VAULT_PARTIAL_MIGRATION:imported=1:pending=1:"));
        assert!(error.ends_with("VAULT_MASTER_KEY_MISSING"));
        assert_eq!(
            secrets.get("ai_api_key/per-item").unwrap().as_deref(),
            Some("legacy-key")
        );
        assert!(secrets.has_encrypted_secret("ai_api_key/vault").unwrap());
        assert_eq!(secrets.status().unwrap().pending_migration_count, 1);
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
    fn deleting_a_credential_prevents_legacy_resurrection() {
        let secrets = Secrets::init_in_memory().unwrap();
        secrets
            .register_legacy_candidate("ai_api_key/legacy")
            .unwrap();
        secrets.delete("ai_api_key/legacy").unwrap();

        assert!(!secrets.has_stored_secret_metadata("ai_api_key/legacy"));
        assert_eq!(secrets.status().unwrap().pending_migration_count, 0);
    }

    #[test]
    fn migration_authorization_rejects_stale_request_ids() {
        let secrets = Secrets::init_in_memory().unwrap();
        secrets.store_encrypted_for_test("ai_api_key/legacy", "key", [5_u8; 12]);
        let error = secrets.migrate_to_local().unwrap_err().to_string();
        let request_id = error.rsplit(':').next().unwrap();

        let stale = secrets.authorize("migrate", Some("00000000-0000-0000-0000-000000000000"));
        assert_eq!(
            stale.unwrap_err().to_string(),
            "VAULT_MIGRATION_REQUEST_EXPIRED"
        );
        secrets.deny("migrate", Some(request_id)).unwrap();
        assert_eq!(secrets.get("ai_api_key/legacy").unwrap(), None);
    }

    #[test]
    fn jump_upgrade_registers_missing_keychain_refs_from_existing_profiles() {
        let directory = tempfile::TempDir::new().unwrap();
        let db = Db::init(directory.path()).unwrap();
        {
            let conn = db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO ai_profiles
                     (id, label, provider, auth_mode, base_url, model, temperature,
                      keep_alive, enabled, priority, created_at, updated_at)
                 VALUES ('api', 'API', 'custom', 'api_key', 'https://example.test/v1',
                         'model', 0.3, NULL, 1, 0, 1, 1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO ai_profiles
                     (id, label, provider, auth_mode, base_url, model, temperature,
                      keep_alive, enabled, priority, created_at, updated_at)
                 VALUES ('oauth', 'OAuth', 'openai', 'oauth', NULL,
                         'model', 0.3, NULL, 1, 1, 1, 1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO ai_credentials
                     (id, profile_id, label, secret_ref, masked_suffix, enabled,
                      priority, state, created_at, updated_at)
                 VALUES ('credential', 'api', 'Primary', 'ai_api_key/jump', '1234',
                         1, 0, 'active', 1, 1)",
                [],
            )
            .unwrap();
        }
        let secrets = Secrets::init_in_memory().unwrap();

        secrets.register_legacy_candidates(&db).unwrap();

        let candidates = secrets
            .conn
            .lock()
            .unwrap()
            .prepare("SELECT key FROM legacy_secret_candidates ORDER BY key")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            candidates,
            vec![
                "ai_api_key/jump".to_string(),
                "oauth_access_token".to_string(),
                "oauth_account_id".to_string(),
                "oauth_expires_at".to_string(),
                "oauth_refresh_token".to_string(),
            ]
        );
        assert_eq!(secrets.status().unwrap().pending_migration_count, 5);
        assert_eq!(secrets.get("ai_api_key/jump").unwrap(), None);
    }

    #[test]
    fn denied_os_statuses_are_classified_structurally() {
        assert!(Secrets::is_denied_os_status(-25293));
        assert!(Secrets::is_denied_os_status(-128));
        assert!(Secrets::is_denied_os_status(-25308));
        assert!(!Secrets::is_denied_os_status(-25291));
    }

    #[cfg(unix)]
    #[test]
    fn file_store_is_created_with_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("quill-secrets-{}", uuid::Uuid::new_v4()));
        let secrets = Secrets::init(&dir).unwrap();
        let journal_mode = secrets
            .conn
            .lock()
            .unwrap()
            .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .unwrap();
        assert_eq!(journal_mode, "delete");
        drop(secrets);
        let mode = fs::metadata(dir.join("secrets.db"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        assert!(!dir.join("secrets.db-wal").exists());
        assert!(!dir.join("secrets.db-shm").exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
