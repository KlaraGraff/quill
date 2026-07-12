//! iCloud Drive helpers — file-presence checks for a user-selected folder.
//!
//! - **Eviction handling** (`is_file_downloaded`,
//!   `icloud_placeholder_path`, `has_icloud_placeholder`,
//!   `trigger_download_file`) for book and cover binaries that live in
//!   iCloud Documents and may be evicted.

use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
use objc2_foundation::{NSFileManager, NSString};

/// Check whether a file is locally available (not an iCloud placeholder).
///
/// iCloud evicts files by replacing `foo.epub` with `.foo.epub.icloud`.
/// Returns `true` if the real file exists on disk.
pub fn is_file_downloaded(path: &Path) -> bool {
    if path.exists() {
        return true;
    }
    // If the real file doesn't exist, it might be an iCloud placeholder — either way, not available.
    false
}

/// Returns the iCloud placeholder path for a given file.
/// e.g. `/dir/foo.epub` → `/dir/.foo.epub.icloud`
#[allow(dead_code)]
pub fn icloud_placeholder_path(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    let name = path.file_name()?.to_str()?;
    Some(parent.join(format!(".{}.icloud", name)))
}

/// Check if a file has an iCloud placeholder (evicted by iCloud).
#[allow(dead_code)]
pub fn has_icloud_placeholder(path: &Path) -> bool {
    icloud_placeholder_path(path).is_some_and(|p| p.exists())
}

/// Trigger iCloud to download a specific file.
#[cfg(target_os = "macos")]
pub fn trigger_download_file(path: &Path) {
    use objc2_foundation::NSURL;
    let fm = NSFileManager::defaultManager();
    let path_str = NSString::from_str(&path.to_string_lossy());
    let url = NSURL::fileURLWithPath(&path_str);
    let _ = fm.startDownloadingUbiquitousItemAtURL_error(&url);
}

#[cfg(not(target_os = "macos"))]
pub fn trigger_download_file(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // --- is_file_downloaded ---

    #[test]
    fn test_is_file_downloaded_real_file_exists() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("book.epub");
        fs::write(&file, "epub data").unwrap();
        assert!(is_file_downloaded(&file));
    }

    #[test]
    fn test_is_file_downloaded_missing_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("book.epub");
        assert!(!is_file_downloaded(&file));
    }

    #[test]
    fn test_is_file_downloaded_placeholder_only() {
        let dir = TempDir::new().unwrap();
        // Real file doesn't exist, but placeholder does
        let placeholder = dir.path().join(".book.epub.icloud");
        fs::write(&placeholder, "placeholder").unwrap();
        let file = dir.path().join("book.epub");
        assert!(!is_file_downloaded(&file));
    }

    // --- icloud_placeholder_path ---

    #[test]
    fn test_icloud_placeholder_path() {
        let path = Path::new("/data/books/my-book_abc12345.epub");
        let placeholder = icloud_placeholder_path(path).unwrap();
        assert_eq!(
            placeholder,
            PathBuf::from("/data/books/.my-book_abc12345.epub.icloud")
        );
    }

    // --- has_icloud_placeholder ---

    #[test]
    fn test_has_icloud_placeholder_true() {
        let dir = TempDir::new().unwrap();
        let placeholder = dir.path().join(".book.epub.icloud");
        fs::write(&placeholder, "placeholder").unwrap();
        let file = dir.path().join("book.epub");
        assert!(has_icloud_placeholder(&file));
    }

    #[test]
    fn test_has_icloud_placeholder_false() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("book.epub");
        assert!(!has_icloud_placeholder(&file));
    }
}
