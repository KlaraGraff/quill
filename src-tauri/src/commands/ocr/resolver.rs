use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::error::{AppError, AppResult};

use super::assets::{
    absolute_asset_path, get_local_state, list_book_assets, verified_state_matches_file, BookAsset,
};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResolvedAsset {
    pub asset: Option<BookAsset>,
    pub absolute_path: PathBuf,
    pub content_sha256: Option<String>,
    pub selection_reason: String,
}

fn resolver_error(code: &str) -> AppError {
    AppError::Other(code.to_string())
}

pub(crate) fn resolve_active_asset(
    conn: &Connection,
    data_dir: &Path,
    book_id: &str,
) -> AppResult<ResolvedAsset> {
    crate::sync::validation::validate_entity_id(book_id)?;

    let source = conn
        .query_row(
            "SELECT file_path, source_sha256 FROM books WHERE id = ?1",
            params![book_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?
        .ok_or_else(|| resolver_error("BOOK_NOT_FOUND"))?;
    for asset in list_book_assets(conn, book_id)? {
        if Some(asset.source_sha256.as_str()) != source.1.as_deref() {
            continue;
        }
        let Some(state) = get_local_state(conn, &asset.id)? else {
            continue;
        };
        let path = absolute_asset_path(data_dir, &asset)?;
        if verified_state_matches_file(&state, &asset, &path) {
            return Ok(ResolvedAsset {
                content_sha256: Some(asset.content_sha256.clone()),
                asset: Some(asset),
                absolute_path: path,
                selection_reason: "latest_verified_ocr".to_string(),
            });
        }
    }

    crate::sync::validation::validate_book_file_path(&source.0)?;
    Ok(ResolvedAsset {
        asset: None,
        absolute_path: data_dir.join(source.0),
        content_sha256: source.1,
        selection_reason: "source".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::commands::ocr::assets::{
        expected_relative_path, insert_asset, set_local_state, NewBookAsset,
    };

    fn open_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::Db::run_migrations_on(&conn).unwrap();
        conn.execute(
            "INSERT INTO books (
                 id, title, author, file_path, format, source_format,
                 source_file_path, source_sha256, status, progress,
                 created_at, updated_at
             ) VALUES (
                 'book-1', 'Scanned', 'Author', 'books/source.pdf', 'pdf',
                 'pdf', 'books/source.pdf', 'source-hash', 'unread', 0, 1, 1
             )",
            [],
        )
        .unwrap();
        conn
    }

    fn add_asset(conn: &Connection, id: &str, updated_at: i64) -> BookAsset {
        let path = expected_relative_path("book-1", id);
        insert_asset(
            conn,
            NewBookAsset {
                id,
                book_id: "book-1",
                relative_path: &path,
                content_sha256: id,
                byte_size: 4,
                source_sha256: "source-hash",
                pipeline_version: Some("17.8.1"),
                language_profile: "chi_sim+eng",
                quality_profile: "fast",
                page_count: 1,
                supersedes_asset_id: None,
                created_at: updated_at,
                updated_at,
                updated_by_device: "dev-a",
            },
        )
        .unwrap()
    }

    #[test]
    fn latest_verified_asset_wins() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("books")).unwrap();
        fs::write(dir.path().join("books/source.pdf"), b"source").unwrap();
        let conn = open_db();
        let old = add_asset(&conn, "asset-old", 2);
        let new = add_asset(&conn, "asset-new", 3);
        fs::write(dir.path().join(&old.relative_path), b"data").unwrap();
        fs::write(dir.path().join(&new.relative_path), b"data").unwrap();
        set_local_state(&conn, &old.id, "available_verified", Some(4), None, 4).unwrap();
        set_local_state(&conn, &new.id, "available_verified", Some(4), None, 4).unwrap();

        let resolved = resolve_active_asset(&conn, dir.path(), "book-1").unwrap();
        assert_eq!(resolved.asset.unwrap().id, "asset-new");
        assert_eq!(resolved.selection_reason, "latest_verified_ocr");
    }

    #[test]
    fn missing_or_unverified_asset_falls_back_to_book_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("books")).unwrap();
        let conn = open_db();
        let asset = add_asset(&conn, "asset-new", 3);
        set_local_state(&conn, &asset.id, "available_verified", Some(4), None, 4).unwrap();

        let resolved = resolve_active_asset(&conn, dir.path(), "book-1").unwrap();
        assert!(resolved.asset.is_none());
        assert_eq!(resolved.selection_reason, "source");
        assert_eq!(resolved.absolute_path, dir.path().join("books/source.pdf"));
        assert_eq!(resolved.content_sha256.as_deref(), Some("source-hash"));
    }

    #[test]
    fn asset_from_replaced_source_is_not_activated() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("books")).unwrap();
        let conn = open_db();
        let asset = add_asset(&conn, "asset-old-source", 3);
        fs::write(dir.path().join(&asset.relative_path), b"data").unwrap();
        set_local_state(&conn, &asset.id, "available_verified", Some(4), None, 4).unwrap();
        conn.execute(
            "UPDATE books SET source_sha256 = 'replacement-hash' WHERE id = 'book-1'",
            [],
        )
        .unwrap();

        let resolved = resolve_active_asset(&conn, dir.path(), "book-1").unwrap();
        assert!(resolved.asset.is_none());
        assert_eq!(resolved.selection_reason, "source");
        assert_eq!(
            resolved.content_sha256.as_deref(),
            Some("replacement-hash")
        );
    }
}
