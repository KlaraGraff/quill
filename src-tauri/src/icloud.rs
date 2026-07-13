//! iCloud Drive helpers — file-presence checks for a user-selected folder.
//!
//! - **Eviction handling** (`is_file_downloaded`,
//!   `icloud_placeholder_path`, `has_icloud_placeholder`,
//!   `trigger_download_file`) for book and cover binaries that live in
//!   iCloud Documents and may be evicted.

use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
use objc2_foundation::{NSFileManager, NSString};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileAvailability {
    Available,
    ICloudPlaceholder,
    Missing,
}

impl FileAvailability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::ICloudPlaceholder => "icloud_placeholder",
            Self::Missing => "missing",
        }
    }
}

/// Classify a book path without treating every missing path as an iCloud
/// download. A missing local file needs a different recovery path from an
/// evicted iCloud item.
pub fn file_availability(path: &Path) -> FileAvailability {
    if path.exists() {
        FileAvailability::Available
    } else if has_icloud_placeholder(path) {
        FileAvailability::ICloudPlaceholder
    } else {
        FileAvailability::Missing
    }
}

/// Check whether a file is locally available (not an iCloud placeholder).
///
/// iCloud evicts files by replacing `foo.epub` with `.foo.epub.icloud`.
/// Returns `true` if the real file exists on disk.
pub fn is_file_downloaded(path: &Path) -> bool {
    file_availability(path) == FileAvailability::Available
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

    #[test]
    fn file_availability_distinguishes_missing_from_placeholder() {
        let dir = TempDir::new().unwrap();
        let available = dir.path().join("available.epub");
        fs::write(&available, "epub data").unwrap();
        assert_eq!(file_availability(&available), FileAvailability::Available);

        let missing = dir.path().join("missing.epub");
        assert_eq!(file_availability(&missing), FileAvailability::Missing);

        fs::write(dir.path().join(".evicted.epub.icloud"), "placeholder").unwrap();
        let evicted = dir.path().join("evicted.epub");
        assert_eq!(
            file_availability(&evicted),
            FileAvailability::ICloudPlaceholder
        );
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
