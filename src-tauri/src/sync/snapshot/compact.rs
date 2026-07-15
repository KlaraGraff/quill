use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use rusqlite::Connection;
use ulid::Ulid;

use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::sync::{log::EventLog, merge};

use super::apply::dump_state;
use super::rows::{CompactReport, Snapshot};
use super::{
    COMPACT_AGE_THRESHOLD_MS, COMPACT_LOG_BYTE_THRESHOLD, COMPACT_LOG_EVENT_THRESHOLD,
    MAX_SNAPSHOT_BYTES, SNAPSHOT_SCHEMA_VERSION,
};

impl Snapshot {
    /// Atomic write — temp file, fsync, rename, parent-dir fsync.
    /// Crash-safe: when this returns, the snapshot's contents AND its
    /// new directory entry are both on disk. Without the parent-dir
    /// fsync, a power loss between `rename` and the next implicit
    /// directory flush can resurrect the previous snapshot at the
    /// path — which would silently corrupt compaction (the caller
    /// would proceed to truncate the source log against a snapshot
    /// that's no longer on disk).
    pub fn write_atomic(&self, path: &Path) -> AppResult<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("snapshot.json.tmp");
        let bytes = serde_json::to_vec(self)
            .map_err(|e| AppError::Other(format!("snapshot serialize: {e}")))?;
        let mut f = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)?;
        fsync_parent_dir(path)?;
        Ok(())
    }

    pub fn read_from(path: &Path) -> AppResult<Self> {
        let bytes = read_snapshot_bytes(path)?;
        serde_json::from_slice(&bytes).map_err(|e| AppError::Other(format!("snapshot parse: {e}")))
    }

    /// Like `read_from` but skips iCloud-evicted files and applies a
    /// timeout. Returns `None` when the file is evicted or the read
    /// exceeds `timeout`.
    ///
    /// `on_stall` / `on_success` callbacks let the caller track stalled
    /// paths and skip them on subsequent ticks. `on_thread_done` runs
    /// inside the spawned thread when `fs::read` completes — used for
    /// in-flight tracking.
    pub fn read_from_with_timeout(
        path: &Path,
        timeout: std::time::Duration,
        on_stall: impl FnOnce(&Path),
        on_success: impl FnOnce(&Path),
        on_thread_done: impl FnOnce() + Send + 'static,
    ) -> AppResult<Option<Self>> {
        use crate::icloud;
        if !path.exists() {
            if icloud::has_icloud_placeholder(path) {
                log::info!(
                    "sync: peer snapshot {} is iCloud-evicted — triggering download, skipping",
                    path.display(),
                );
                icloud::trigger_download_file(path);
            }
            return Ok(None);
        }
        let path_buf = path.to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = read_snapshot_bytes(&path_buf);
            on_thread_done();
            let _ = tx.send(result);
        });
        match rx.recv_timeout(timeout) {
            Ok(Ok(bytes)) => {
                on_success(path);
                let snap: Self = serde_json::from_slice(&bytes)
                    .map_err(|e| AppError::Other(format!("snapshot parse: {e}")))?;
                Ok(Some(snap))
            }
            Ok(Err(AppError::Io(e))) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                log::warn!(
                    "sync: timed out reading peer snapshot {} after {}s — backing off",
                    path.display(),
                    timeout.as_secs(),
                );
                on_stall(path);
                Ok(None)
            }
        }
    }
}

fn read_snapshot_bytes(path: &Path) -> AppResult<Vec<u8>> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > MAX_SNAPSHOT_BYTES {
        return Err(AppError::Other(format!(
            "snapshot exceeds {MAX_SNAPSHOT_BYTES} byte limit: {}",
            path.display()
        )));
    }
    let file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_SNAPSHOT_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SNAPSHOT_BYTES {
        return Err(AppError::Other(format!(
            "snapshot exceeds {MAX_SNAPSHOT_BYTES} byte limit: {}",
            path.display()
        )));
    }
    Ok(bytes)
}

/// Open the parent directory of `path` and `fsync` it. POSIX requires
/// this for a preceding `rename` to actually survive a power cut: the
/// data write + `fsync` makes the temp file durable, the rename
/// updates the in-memory directory, but the directory entry only
/// hits the disk when the directory itself is fsynced. Without it,
/// `compact_own_log` could leave the empty log entry durable while
/// the snapshot's new directory entry is still in cache, dropping
/// every event the log held.
///
/// Best-effort no-op on Windows — we don't ship sync there in v1, and
/// `File::open(parent)` on a directory has different semantics that
/// would need a separate `CreateFileW` path.
#[cfg(unix)]
fn fsync_parent_dir(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        let dir = fs::File::open(parent)?;
        dir.sync_all()?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn fsync_parent_dir(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Compaction — fold prior snapshot + log into a fresh snapshot, truncate log.
// ---------------------------------------------------------------------------

/// True when the device's own log meets any compaction threshold:
/// size > 2 MB, > 5000 events, or last snapshot is older than 30 days
/// (and the log has at least one event to fold). Cheap — only `stat`s
/// the log + reads the snapshot header, never the full event stream.
///
/// `false` when the log doesn't exist yet (fresh enable, no events
/// emitted) or every threshold is below the limit.
pub fn should_compact(shared_dir: &Path, device: &str) -> bool {
    let log_path = shared_dir.join("logs").join(format!("{device}.jsonl"));
    let snap_path = shared_dir
        .join("logs")
        .join(format!("{device}.snapshot.json"));

    let Ok(meta) = fs::metadata(&log_path) else {
        return false;
    };
    if meta.len() > COMPACT_LOG_BYTE_THRESHOLD {
        return true;
    }

    // Cheap line count via byte scan — avoids deserializing every event
    // just to decide whether compaction is needed.
    let log_lines =
        count_lines_bounded(&log_path, COMPACT_LOG_EVENT_THRESHOLD + 1).unwrap_or_default();
    if log_lines > COMPACT_LOG_EVENT_THRESHOLD {
        return true;
    }
    if log_lines == 0 {
        return false;
    }

    // The age trigger only applies when a snapshot exists. The
    // first-snapshot case is owned by `sync_enable` /
    // `migration::run_migration`; if neither has run yet, compaction
    // staying out of the way is the right move (compacting an empty
    // pre-bootstrap state would just publish a snapshot of nothing).
    if let Ok(snap) = Snapshot::read_from(&snap_path) {
        let now = chrono::Utc::now().timestamp_millis();
        if now - snap.generated_at > COMPACT_AGE_THRESHOLD_MS {
            return true;
        }
    }

    false
}

fn count_lines_bounded(path: &Path, stop_after: usize) -> std::io::Result<usize> {
    let mut file = fs::File::open(path)?;
    let mut buffer = [0_u8; 16 * 1024];
    let mut lines = 0;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(lines);
        }
        lines += buffer[..read].iter().filter(|byte| **byte == b'\n').count();
        if lines >= stop_after {
            return Ok(lines);
        }
    }
}

/// Compact the device's own log: fold the existing snapshot + every
/// event currently in the log into a fresh snapshot, then truncate
/// the log to events past the new watermark (typically empty).
///
/// Concurrency: the entire read-fold-write-truncate sequence runs
/// inside `EventLog::with_locked_log`, so concurrent `append`s from
/// `SyncWriter` block until compaction finishes. Compaction is rare
/// (every few minutes/days at most) so the brief stall is fine.
///
/// Idempotent: running compaction twice on an unchanged log is a
/// no-op the second time (log is already empty after the first run).
pub fn compact_own_log(shared_dir: &Path, log_handle: &EventLog) -> AppResult<CompactReport> {
    let device = log_handle.device().to_string();
    let snap_path = shared_dir
        .join("logs")
        .join(format!("{device}.snapshot.json"));

    let pre_log_size = fs::metadata(log_handle.path())
        .map(|m| m.len() as i64)
        .unwrap_or(0);
    let pre_snap_size = fs::metadata(&snap_path)
        .map(|m| m.len() as i64)
        .unwrap_or(0);

    let report = log_handle.with_locked_log(|log_path, events| {
        if events.is_empty() {
            return Ok(CompactReport::default());
        }

        // Fold the prior snapshot (if any) + every log event into a
        // fresh in-memory DB, then dump as the new snapshot. Same
        // engine merge::apply_event uses for peer events — guarantees
        // the snapshot reflects the same state a peer would compute.
        let prior = if snap_path.exists() {
            Some(Snapshot::read_from(&snap_path)?)
        } else {
            None
        };

        let mut conn = Connection::open_in_memory()?;
        Db::run_migrations_on(&conn)?;
        {
            let tx = conn.transaction()?;
            if let Some(s) = &prior {
                s.apply_peer(&tx, &device)?;
            }
            for ev in events {
                merge::apply_event(&tx, ev)?;
            }
            tx.commit()?;
        }
        let state = dump_state(&conn)?;

        // The new snapshot's watermark is the highest event id we
        // just folded. After truncation the log is empty, so any
        // peer reading the snapshot has no log tail to consume —
        // exactly the post-compaction invariant we want.
        let new_truncated = events
            .iter()
            .map(|e| e.id.clone())
            .max()
            .or_else(|| prior.as_ref().and_then(|s| s.truncated_before.clone()));

        let new_snap = Snapshot {
            v: SNAPSHOT_SCHEMA_VERSION,
            device: device.clone(),
            id: new_truncated
                .clone()
                .unwrap_or_else(|| Ulid::new().to_string()),
            generated_at: chrono::Utc::now().timestamp_millis(),
            truncated_before: new_truncated,
            state,
        };
        // Step 1: durably commit the new snapshot. `write_atomic`
        // includes a parent-dir fsync — when it returns, the snapshot's
        // directory entry survives a power cut. THIS MUST HAPPEN
        // BEFORE the log is truncated; otherwise a crash window where
        // the empty-log rename is durable but the snapshot rename is
        // not loses every event the log held.
        new_snap.write_atomic(&snap_path)?;

        // Step 2: truncate the log. Atomic temp + rename + fsync of
        // the empty file AND the parent dir so the truncation itself
        // is durable. Without these fsyncs, a crash here can come
        // back with the old log contents — fine for correctness
        // (next compaction folds them again, idempotent) but breaks
        // the storage-reclamation contract this function is here to
        // provide.
        let tmp = log_path.with_extension("jsonl.tmp");
        let f = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, log_path)?;
        fsync_parent_dir(log_path)?;

        Ok(CompactReport {
            events_folded: events.len(),
            snapshot_written: true,
            bytes_freed: 0, // computed below
        })
    })?;

    if !report.snapshot_written {
        return Ok(report);
    }

    let post_log_size = fs::metadata(log_handle.path())
        .map(|m| m.len() as i64)
        .unwrap_or(0);
    let post_snap_size = fs::metadata(&snap_path)
        .map(|m| m.len() as i64)
        .unwrap_or(0);
    Ok(CompactReport {
        bytes_freed: (pre_log_size - post_log_size) - (post_snap_size - pre_snap_size),
        ..report
    })
}
