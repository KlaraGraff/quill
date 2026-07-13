use rusqlite::params;
use std::collections::HashMap;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::ai::router::{self, AiCredentialView, AiProfileView};
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::secrets::Secrets;

#[tauri::command]
pub fn get_all_settings(
    db: State<'_, Db>,
    secrets: State<'_, Secrets>,
) -> AppResult<HashMap<String, String>> {
    // Release the reader lock before asking the AI router for credentials.
    // `list_credentials` may read the same connection again when no profile
    // id is supplied; keeping this guard alive would deadlock the command.
    let mut settings = {
        let conn = db.reader();
        let mut stmt = conn.prepare("SELECT key, value FROM settings")?;
        let result = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .filter_map(|row| match row {
                Ok((key, value)) if !Secrets::is_sensitive_key(&key) => Some(Ok((key, value))),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        result
    };

    // Never expose secret values to the webview. The UI only needs to know
    // whether a key exists so it can preserve it when unrelated settings save.
    let configured = router::has_configured_service(&db, &secrets) || {
        secrets
            .get("ai_api_key")
            .is_some_and(|value| !value.trim().is_empty())
    };
    settings.insert("ai_api_key_configured".to_string(), configured.to_string());

    Ok(settings)
}

#[tauri::command]
pub fn ai_api_key_configured(db: State<'_, Db>, secrets: State<'_, Secrets>) -> bool {
    router::has_configured_service(&db, &secrets) || {
        secrets
            .get("ai_api_key")
            .is_some_and(|value| !value.trim().is_empty())
    }
}

#[tauri::command]
pub fn get_setting(key: String, db: State<'_, Db>) -> AppResult<Option<String>> {
    if Secrets::is_sensitive_key(&key) {
        return Err(AppError::Other("SECRET_READ_FORBIDDEN".to_string()));
    }

    let conn = db.reader();
    let result = conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |row| row.get(0),
    );
    match result {
        Ok(value) => Ok(Some(value)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[tauri::command]
pub fn set_setting(key: String, value: String, db: State<'_, Db>) -> AppResult<()> {
    if Secrets::is_sensitive_key(&key) {
        return Err(AppError::Other(
            "SECRET_WRITE_REQUIRES_DEDICATED_COMMAND".to_string(),
        ));
    }

    let conn = db.conn.lock().map_err(|e| AppError::Other(e.to_string()))?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2",
        params![key, value],
    )?;
    Ok(())
}

#[tauri::command]
pub fn set_settings_bulk(settings: HashMap<String, String>, db: State<'_, Db>) -> AppResult<()> {
    let conn = db.conn.lock().map_err(|e| AppError::Other(e.to_string()))?;
    for (key, value) in settings {
        if Secrets::is_sensitive_key(&key) {
            return Err(AppError::Other(
                "SECRET_WRITE_REQUIRES_DEDICATED_COMMAND".to_string(),
            ));
        }
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, value],
        )?;
    }
    Ok(())
}

#[tauri::command]
pub fn set_ai_api_key(
    value: String,
    db: State<'_, Db>,
    secrets: State<'_, Secrets>,
) -> AppResult<()> {
    let profile = router::active_profile_view(&db)?;
    let existing = router::list_credentials(&db, Some(&profile.id))?;
    if let Some(credential) = existing.first() {
        router::replace_credential(&db, &secrets, &credential.id, &value)
    } else {
        router::add_credential(&db, &secrets, profile.id, "Primary key".to_string(), value)
            .map(|_| ())
    }
}

#[tauri::command]
pub fn ai_active_profile(db: State<'_, Db>) -> AppResult<AiProfileView> {
    router::active_profile_view(&db)
}

#[tauri::command]
pub fn ai_list_profiles(db: State<'_, Db>) -> AppResult<Vec<AiProfileView>> {
    router::list_profiles(&db)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn ai_create_profile(
    label: String,
    provider: String,
    auth_mode: String,
    base_url: Option<String>,
    model: String,
    temperature: f64,
    keep_alive: Option<String>,
    enabled: Option<bool>,
    db: State<'_, Db>,
) -> AppResult<AiProfileView> {
    router::create_profile(
        &db,
        label,
        provider,
        auth_mode,
        base_url,
        model,
        temperature,
        keep_alive,
        enabled.unwrap_or(true),
    )
}

#[tauri::command]
pub fn ai_duplicate_profile(
    id: String,
    label: Option<String>,
    db: State<'_, Db>,
) -> AppResult<AiProfileView> {
    router::duplicate_profile(&db, &id, label)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn ai_save_profile(
    id: String,
    label: String,
    provider: String,
    auth_mode: String,
    base_url: Option<String>,
    model: String,
    temperature: f64,
    keep_alive: Option<String>,
    db: State<'_, Db>,
) -> AppResult<AiProfileView> {
    router::save_profile(
        &db,
        id,
        label,
        provider,
        auth_mode,
        base_url,
        model,
        temperature,
        keep_alive,
    )
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn ai_update_profile(
    id: String,
    label: String,
    provider: String,
    auth_mode: String,
    base_url: Option<String>,
    model: String,
    temperature: f64,
    keep_alive: Option<String>,
    db: State<'_, Db>,
) -> AppResult<AiProfileView> {
    router::save_profile(
        &db,
        id,
        label,
        provider,
        auth_mode,
        base_url,
        model,
        temperature,
        keep_alive,
    )
}

#[tauri::command]
pub fn ai_set_profile_enabled(id: String, enabled: bool, db: State<'_, Db>) -> AppResult<()> {
    router::set_profile_enabled(&db, &id, enabled)
}

#[tauri::command]
pub fn ai_reorder_profiles(ids: Vec<String>, db: State<'_, Db>) -> AppResult<()> {
    router::reorder_profiles(&db, &ids)
}

#[tauri::command]
pub fn ai_delete_profile(
    id: String,
    db: State<'_, Db>,
    secrets: State<'_, Secrets>,
) -> AppResult<()> {
    router::delete_profile(&db, &secrets, &id)
}

#[tauri::command]
pub async fn ai_list_models(
    profile_id: String,
    provider: Option<String>,
    auth_mode: Option<String>,
    base_url: Option<String>,
    db: State<'_, Db>,
    secrets: State<'_, Secrets>,
) -> AppResult<Vec<String>> {
    router::list_models(&db, &secrets, &profile_id, provider, auth_mode, base_url).await
}

#[tauri::command]
pub async fn ai_test_profile(
    id: String,
    app: AppHandle,
    db: State<'_, Db>,
    secrets: State<'_, Secrets>,
) -> AppResult<router::AiConnectionTestResult> {
    router::test_profile(&app, &db, &secrets, &id).await
}

#[tauri::command]
pub fn ai_list_credentials(
    profile_id: Option<String>,
    db: State<'_, Db>,
) -> AppResult<Vec<AiCredentialView>> {
    router::list_credentials(&db, profile_id.as_deref())
}

#[tauri::command]
pub fn ai_add_credential(
    profile_id: String,
    label: String,
    value: String,
    db: State<'_, Db>,
    secrets: State<'_, Secrets>,
) -> AppResult<AiCredentialView> {
    router::add_credential(&db, &secrets, profile_id, label, value)
}

#[tauri::command]
pub fn ai_replace_credential(
    id: String,
    value: String,
    db: State<'_, Db>,
    secrets: State<'_, Secrets>,
) -> AppResult<()> {
    router::replace_credential(&db, &secrets, &id, &value)
}

#[tauri::command]
pub fn ai_set_credential_enabled(id: String, enabled: bool, db: State<'_, Db>) -> AppResult<()> {
    router::set_credential_enabled(&db, &id, enabled)
}

#[tauri::command]
pub fn ai_reorder_credentials(ids: Vec<String>, db: State<'_, Db>) -> AppResult<()> {
    router::reorder_credentials(&db, &ids)
}

#[tauri::command]
pub fn ai_delete_credential(
    id: String,
    db: State<'_, Db>,
    secrets: State<'_, Secrets>,
) -> AppResult<()> {
    router::delete_credential(&db, &secrets, &id)
}

#[tauri::command]
pub async fn ai_test_credential(
    id: String,
    app: AppHandle,
    db: State<'_, Db>,
    secrets: State<'_, Secrets>,
) -> AppResult<()> {
    router::test_credential(&app, &db, &secrets, &id).await
}

#[tauri::command]
pub fn get_book_settings(book_id: String, db: State<'_, Db>) -> AppResult<HashMap<String, String>> {
    let conn = db.reader();
    let mut stmt = conn.prepare("SELECT key, value FROM book_settings WHERE book_id = ?1")?;
    let settings = stmt
        .query_map(params![book_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<HashMap<_, _>, _>>()?;
    Ok(settings)
}

#[tauri::command]
pub fn set_book_settings_bulk(
    book_id: String,
    settings: HashMap<String, String>,
    db: State<'_, Db>,
) -> AppResult<()> {
    let conn = db.conn.lock().map_err(|e| AppError::Other(e.to_string()))?;
    for (key, value) in settings {
        conn.execute(
            "INSERT INTO book_settings (book_id, key, value) VALUES (?1, ?2, ?3) ON CONFLICT(book_id, key) DO UPDATE SET value = ?3",
            params![book_id, key, value],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::db::Db;
    use rusqlite::params;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn setup() -> (TempDir, Db) {
        let dir = TempDir::new().unwrap();
        let db = Db::init(dir.path()).unwrap();
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO books (id, title, author, file_path, status, progress, created_at, updated_at)
             VALUES ('book1', 'Test Book', 'Author', 'books/test.epub', 'reading', 0, '2024-01-01', '2024-01-01')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO books (id, title, author, file_path, status, progress, created_at, updated_at)
             VALUES ('book2', 'Second Book', 'Author 2', 'books/test2.epub', 'reading', 0, '2024-01-01', '2024-01-01')",
            [],
        ).unwrap();
        drop(conn);
        (dir, db)
    }

    fn get_book_settings(db: &Db, book_id: &str) -> HashMap<String, String> {
        let conn = db.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT key, value FROM book_settings WHERE book_id = ?1")
            .unwrap();
        stmt.query_map(params![book_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<Result<HashMap<_, _>, _>>()
        .unwrap()
    }

    fn set_book_settings_bulk(db: &Db, book_id: &str, settings: HashMap<String, String>) {
        let conn = db.conn.lock().unwrap();
        for (key, value) in settings {
            conn.execute(
                "INSERT INTO book_settings (book_id, key, value) VALUES (?1, ?2, ?3) ON CONFLICT(book_id, key) DO UPDATE SET value = ?3",
                params![book_id, key, value],
            ).unwrap();
        }
    }

    #[test]
    fn test_book_settings_roundtrip() {
        let (_dir, db) = setup();
        let mut settings = HashMap::new();
        settings.insert("font_family".to_string(), "inter".to_string());
        settings.insert("font_size".to_string(), "32".to_string());

        set_book_settings_bulk(&db, "book1", settings);

        let result = get_book_settings(&db, "book1");
        assert_eq!(result.get("font_family").unwrap(), "inter");
        assert_eq!(result.get("font_size").unwrap(), "32");
    }

    #[test]
    fn test_book_settings_isolation() {
        let (_dir, db) = setup();

        let mut s1 = HashMap::new();
        s1.insert("font_family".to_string(), "inter".to_string());
        set_book_settings_bulk(&db, "book1", s1);

        let mut s2 = HashMap::new();
        s2.insert("font_family".to_string(), "georgia".to_string());
        set_book_settings_bulk(&db, "book2", s2);

        assert_eq!(
            get_book_settings(&db, "book1").get("font_family").unwrap(),
            "inter"
        );
        assert_eq!(
            get_book_settings(&db, "book2").get("font_family").unwrap(),
            "georgia"
        );
    }

    #[test]
    fn test_book_settings_cleaned_on_book_delete() {
        let (_dir, db) = setup();

        let mut settings = HashMap::new();
        settings.insert("font_size".to_string(), "28".to_string());
        set_book_settings_bulk(&db, "book1", settings);

        assert_eq!(get_book_settings(&db, "book1").len(), 1);

        let conn = db.conn.lock().unwrap();
        conn.execute("DELETE FROM book_settings WHERE book_id = 'book1'", [])
            .unwrap();
        conn.execute("DELETE FROM books WHERE id = 'book1'", [])
            .unwrap();
        drop(conn);

        assert!(get_book_settings(&db, "book1").is_empty());
    }

    #[test]
    fn test_book_settings_upsert() {
        let (_dir, db) = setup();

        let mut s1 = HashMap::new();
        s1.insert("font_size".to_string(), "24".to_string());
        set_book_settings_bulk(&db, "book1", s1);

        let mut s2 = HashMap::new();
        s2.insert("font_size".to_string(), "30".to_string());
        set_book_settings_bulk(&db, "book1", s2);

        assert_eq!(
            get_book_settings(&db, "book1").get("font_size").unwrap(),
            "30"
        );
    }
}

/// Emit an open-settings event to the main window from any window.
#[tauri::command]
pub fn open_settings_on_main(section: String, app: AppHandle) -> AppResult<()> {
    app.emit_to("main", "open-settings", &section)
        .map_err(|e| AppError::Other(e.to_string()))?;
    Ok(())
}

/// Show the main window and switch its library surface from a reader window.
#[tauri::command]
pub fn open_library_on_main(filter: String, app: AppHandle) -> AppResult<()> {
    const ALLOWED_FILTERS: &[&str] = &["all", "reading", "finished", "vocab", "chats", "notes"];
    if !ALLOWED_FILTERS.contains(&filter.as_str()) && !filter.starts_with("collection:") {
        return Err(AppError::Other("LIBRARY_FILTER_INVALID".to_string()));
    }
    app.emit_to("main", "open-library-filter", &filter)
        .map_err(|error| AppError::Other(error.to_string()))?;
    if let Some(window) = app.get_webview_window("main") {
        window
            .show()
            .map_err(|error| AppError::Other(error.to_string()))?;
        window
            .set_focus()
            .map_err(|error| AppError::Other(error.to_string()))?;
    }
    Ok(())
}
