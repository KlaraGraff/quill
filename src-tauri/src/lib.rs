mod ai;
mod commands;
// `pub` so `tests/mcp_binary.rs` can call `Db::init` to seed a DB at
// the temp HOME path the binary will read from. Otherwise integration
// tests would need to hand-roll the full migration suite.
pub mod db;
mod epub;
mod error;
mod icloud;
mod mcp;
mod panic_hook;
mod pdfium;
mod secrets;
mod sync;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use commands::sync::SyncState;
use db::Db;
use secrets::Secrets;
use sync::device::DeviceIdentity;
use sync::log::EventLog;
use sync::replay::ReplayEngine;
use sync::writer::SyncWriter;
use tauri::Emitter;
use tauri::Manager;

/// The resolved local app data directory, accounting for dev-mode isolation.
pub struct LocalDir(pub PathBuf);

/// Load and bind the bundled PDFium library without starting Tauri.
///
/// The release workflow invokes this through the signed app executable so
/// macOS applies the exact hardened-runtime library-validation policy that
/// users get. A static `codesign --verify --deep` cannot catch Team ID
/// mismatches discovered only when `dlopen()` maps a nested dylib.
pub fn pdfium_smoke_test() -> Result<(), String> {
    pdfium::pdfium().map(|_| ()).map_err(str::to_owned)
}

/// Resolve the plugin's level filter, honoring `RUST_LOG` over the cfg
/// default. Plain `LevelFilter` only — no `env_logger`-style
/// `target=level` syntax. The spec asks devs be able to "crank it up
/// without rebuilding," which a single global level satisfies; the
/// plugin's native `.level()` is the only filter point we wire.
///
/// Unparseable values (typos like `RUST_LOG=verbose`) fall back to the
/// default rather than killing plugin init.
fn resolve_log_level(default: log::LevelFilter) -> log::LevelFilter {
    std::env::var("RUST_LOG")
        .ok()
        .and_then(|s| s.trim().parse::<log::LevelFilter>().ok())
        .unwrap_or(default)
}

/// The bundle identifier this build should use for OS-scoped paths
/// (logs, app_data). Adds the `-dev` suffix in debug builds so a
/// `pnpm tauri dev` session doesn't pollute the production log dir or
/// app-data dir the released app uses. Mirrors the dev-suffix logic
/// already in the setup() callback for `app_data_dir`.
fn bundle_identifier_for_build() -> &'static str {
    if cfg!(debug_assertions) {
        "com.klaragraff.lantern-dev"
    } else {
        "com.klaragraff.lantern"
    }
}

/// Copy data from an earlier build's data directory: the pre-rename
/// `com.klaragraff.quill` id, the original app, or the first Personal build
/// whose bundle identifier contained a typo. This runs only before a new data
/// directory gains `quill.db`, so it never overwrites data already owned by
/// this build.
fn migrate_legacy_app_data(target: &Path) -> error::AppResult<()> {
    if target.join("quill.db").exists() {
        return Ok(());
    }
    let legacy_ids: &[&str] = if cfg!(debug_assertions) {
        &[
            "com.klaragraff.quill-dev",
            "com.klagragraff.quill-dev",
            "com.wycstudios.quill-dev",
        ]
    } else {
        &[
            "com.klaragraff.quill",
            "com.klagragraff.quill",
            "com.wycstudios.quill",
        ]
    };
    for id in legacy_ids {
        let source = target.with_file_name(id);
        if !source.join("quill.db").is_file() {
            continue;
        }
        log::info!(
            "migration: adopting legacy application data from {}",
            source.display()
        );
        copy_dir_missing(&source, target)?;
        break;
    }
    Ok(())
}

fn copy_dir_missing(source: &Path, target: &Path) -> error::AppResult<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            copy_dir_missing(&source_path, &target_path)?;
        } else if metadata.is_file() && !target_path.exists() {
            std::fs::copy(&source_path, &target_path)?;
        }
    }
    Ok(())
}

/// Resolve the OS-conventional log directory for *this* build.
///
/// Single source of truth used by both plugin registration (the file
/// target) and the `reveal_logs` Tauri command, so they can never drift
/// apart. We construct the path from `HOME` / `LOCALAPPDATA` /
/// `XDG_DATA_HOME` directly (not `app.path().app_log_dir()`) because
/// `app_log_dir()` derives from `tauri.conf.json::identifier` with no
/// dev/prod suffix, and the plugin builder runs before an `AppHandle`
/// exists anyway.
///
/// Platform layout matches `tauri-plugin-log::TargetKind::LogDir`'s
/// documented defaults, with the identifier dev-suffixed in debug.
pub(crate) fn resolve_log_dir() -> PathBuf {
    let identifier = bundle_identifier_for_build();

    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .expect("HOME env var");
        home.join("Library/Logs").join(identifier)
    }
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .expect("LOCALAPPDATA env var");
        base.join(identifier).join("logs")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .expect("HOME or XDG_DATA_HOME env var");
        base.join(identifier).join("logs")
    }
}

/// Resolve the OS-conventional **app-data** directory for this build.
/// Mirrors what `tauri::path::app_data_dir()` would return for the
/// active bundle identifier (dev-suffixed in debug). Used by the
/// `quill mcp` stdio subcommand which runs outside the Tauri runtime
/// and has no `AppHandle` to ask.
///
/// Platform layout:
/// - macOS:   `$HOME/Library/Application Support/<identifier>`
/// - Windows: `%APPDATA%\<identifier>` (Roaming)
/// - Linux:   `$XDG_DATA_HOME/<identifier>` or `$HOME/.local/share/<identifier>`
pub(crate) fn resolve_app_data_dir() -> PathBuf {
    let identifier = bundle_identifier_for_build();

    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .expect("HOME env var");
        home.join("Library/Application Support").join(identifier)
    }
    #[cfg(target_os = "windows")]
    {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .expect("APPDATA env var");
        base.join(identifier)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .expect("HOME or XDG_DATA_HOME env var");
        base.join(identifier)
    }
}

/// Entry point for the `quill mcp` subcommand. Opens the SQLite
/// materialized view and serves the MCP protocol over stdin/stdout
/// until the client disconnects.
///
/// When `mcp_write_enabled` is `"true"` in the settings table, the DB
/// is opened read-write and a `SyncWriter` is constructed so write
/// tools can mutate the library. Otherwise the DB stays read-only.
///
/// Runs entirely outside the Tauri runtime — no plugins, no windows,
/// no event loop. Stderr is reserved for diagnostic messages so it
/// doesn't pollute the MCP wire on stdout.
pub fn mcp_stdio_main() {
    let local_dir = resolve_app_data_dir();
    let db_path = local_dir.join("quill.db");

    if !db_path.exists() {
        eprintln!(
            "quill mcp: no library found at {} — launch the Lantern app at least once to initialize it.",
            db_path.display()
        );
        std::process::exit(1);
    }

    let write_enabled = is_mcp_write_enabled(&db_path);

    // The marker stores user intent, not unrestricted path authority. Only
    // use the shared directory after confirming it is still an accessible,
    // writable iCloud Drive folder; otherwise MCP stays on the local copy.
    let sync_enabled = sync::migration::is_sync_enabled(&local_dir);
    let shared_dir = sync_enabled
        .then(|| sync::migration::recorded_usable_icloud_dir(&local_dir))
        .flatten();
    let data_dir = shared_dir.clone().unwrap_or_else(|| local_dir.clone());

    let (db, sync) = if write_enabled {
        let db = match Db::open_readwrite(&db_path) {
            Ok(mut db) => {
                db.set_data_dir(&data_dir);
                db
            }
            Err(e) => {
                eprintln!("quill mcp: failed to open (rw) {}: {e}", db_path.display());
                std::process::exit(1);
            }
        };
        let device = sync::device::DeviceIdentity::load_or_create(&local_dir)
            .expect("failed to load device identity");
        let sw = SyncWriter::new(device.device_uuid);
        sw.set_process_lock_path(local_dir.join(".sync-transition.lock"));
        if sync_enabled {
            sw.set_should_queue(true);
        }
        if shared_dir.is_some() {
            sw.spawn_cover_writer();
        }
        (db, Some(sw))
    } else {
        let db = match Db::open_readonly(&db_path) {
            Ok(mut db) => {
                db.set_data_dir(&data_dir);
                db
            }
            Err(e) => {
                eprintln!("quill mcp: failed to open {}: {e}", db_path.display());
                std::process::exit(1);
            }
        };
        (db, None)
    };

    let state = mcp::McpState::new(db, sync, Some(&local_dir));

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    if let Err(e) = runtime.block_on(mcp::server::serve_stdio(state)) {
        eprintln!("quill mcp: serve error: {e}");
        std::process::exit(1);
    }
}

/// Check whether `mcp_write_enabled` is `"true"` in the settings table.
/// Opens a temporary read-only connection just for this check.
fn is_mcp_write_enabled(db_path: &Path) -> bool {
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return false;
    };
    conn.query_row(
        "SELECT value FROM settings WHERE key = 'mcp_write_enabled'",
        [],
        |row| row.get::<_, String>(0),
    )
    .map(|v| v == "true")
    .unwrap_or(false)
}

/// Boot the sync engine: open EventLog, create ReplayEngine, spawn
/// watcher, wire the SyncWriter, and kick off the initial replay tick.
///
/// All iCloud I/O lives here (EventLog::open, watcher::spawn, initial
/// tick). Called from a background thread so setup() is never blocked.
fn boot_sync_engine(
    shared_dir: PathBuf,
    device_uuid: &str,
    db: &Db,
    sync_writer: &SyncWriter,
    sync_state: &SyncState,
    app_handle: &tauri::AppHandle,
    needs_cover_backfill: bool,
) -> error::AppResult<()> {
    let _transition = sync_writer.begin_transition()?;
    let local_dir: tauri::State<LocalDir> = app_handle.state();
    commands::sync::reconcile_local_blobs_to_ubiquity(&local_dir.0, &shared_dir)?;

    let log_path = shared_dir.join("logs").join(format!("{device_uuid}.jsonl"));
    let log = Arc::new(EventLog::open(&log_path, device_uuid, true)?);

    let engine = Arc::new(
        ReplayEngine::new(
            shared_dir.clone(),
            device_uuid.to_string(),
            Arc::clone(&log),
        )
        .with_app_handle(app_handle.clone()),
    );

    let watcher = sync::watcher::spawn(shared_dir, db.clone(), Arc::clone(&engine))?;

    // Atomic check-and-install: hold the engine mutex across both the
    // marker recheck and the state writes. sync_disable also locks
    // engine before clearing, so this prevents the race where disable
    // slips in between the check and the install.
    {
        let mut engine_guard = sync_state
            .engine
            .lock()
            .map_err(|e| error::AppError::Other(format!("engine mutex: {e}")))?;
        if !sync::migration::is_sync_enabled(&local_dir.0) {
            log::warn!("sync: boot finished but sync was disabled during boot — discarding engine");
            drop(watcher);
            return Ok(());
        }
        let mut watcher_guard = sync_state
            .watcher
            .lock()
            .map_err(|e| error::AppError::Other(format!("watcher mutex: {e}")))?;
        *engine_guard = Some(Arc::clone(&engine));
        *watcher_guard = Some(watcher);
        sync_writer.set_log(Some(Arc::clone(&log)));
        sync_writer.spawn_cover_writer();
        sync_writer.spawn_flush_worker(db.clone(), log);
    }

    let bg_engine = Arc::clone(&engine);
    let bg_db = db.clone();
    let bg_handle = app_handle.clone();
    std::thread::Builder::new()
        .name("sync-initial-tick".into())
        .spawn(move || {
            let result = bg_engine.tick_with_progress(&bg_db, Some(&bg_handle));
            if let Err(e) = result {
                log::warn!("sync: initial replay tick failed: {e}");
            }
            // Chained: DB backfill first (cover files → BLOBs), then
            // file backfill (BLOBs → .img). Sequential so .img writes
            // always see the full set of cover_data.
            if needs_cover_backfill {
                bg_db.backfill_cover_data();
            }
            let bg_writer: tauri::State<SyncWriter> = bg_handle.state();
            bg_writer.backfill_cover_files(&bg_db);
            // Peer imports can add text books after the startup scan ran.
            // Their source blobs are now reconciled, so build local reader
            // caches without holding the sync worker or UI thread.
            commands::books::schedule_pending_text_book_preparations(bg_handle.clone());
            let _ = bg_handle.emit("sync-initial-tick-done", ());
        })
        .ok();

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // First line on purpose: the file target initializes lazily on the first
    // `log::` call, so a panic during plugin init still lands in the log.
    panic_hook::install();

    let default_level = if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    let level = resolve_log_level(default_level);

    // Debug-only smoke trigger for the panic-hook pipeline. Reproduces
    // spec smoke #5: `QUILL_PANIC_TEST=1 cargo run` arms a panicking
    // thread 5s after launch so we can verify the hook chained, the
    // backtrace landed in the log file, and the OS CrashReporter still
    // fired. Gated on debug builds so it can't ship to users.
    #[cfg(debug_assertions)]
    if std::env::var("QUILL_PANIC_TEST").is_ok() {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(5));
            panic!("QUILL_PANIC_TEST: intentional panic for smoke testing the panic hook");
        });
    }

    let app = tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Folder {
                        path: resolve_log_dir(),
                        file_name: Some("quill".into()),
                    }),
                    #[cfg(debug_assertions)]
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                ])
                .level(level)
                .max_file_size(10 * 1024 * 1024)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepSome(3))
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .menu(|handle| {
            // Start from the per-platform default menu so the standard
            // App / Edit / View / Window entries stay intact. Only the
            // Help submenu is augmented — the default Help on macOS is
            // effectively empty (the About entry lives in the app menu),
            // so we append a single "Reveal Logs" item there.
            //
            // Label is English at boot. Tauri's `.menu()` callback runs
            // before `.setup()`, so the user-language setting (in the
            // SQLite `settings` table) isn't readable yet. The i18n keys
            // (`menu.help.revealLogs`) exist in en.json/zh.json for the
            // Settings UI; menu localization would mean deferring menu
            // construction to setup() via `app.set_menu()`. Tracked as a
            // follow-up.  [[follow-up-220-menu-i18n]]
            let menu = tauri::menu::Menu::default(handle)?;
            let label = if cfg!(target_os = "macos") {
                "Reveal Logs in Finder"
            } else {
                "Show Logs in Explorer"
            };
            if let Some(help_kind) = menu.get(tauri::menu::HELP_SUBMENU_ID) {
                if let Some(help) = help_kind.as_submenu() {
                    let item = tauri::menu::MenuItem::with_id(
                        handle,
                        "reveal_logs",
                        label,
                        true,
                        None::<&str>,
                    )?;
                    help.append(&item)?;
                }
            }

            Ok(menu)
        })
        .on_menu_event(|app, event| {
            if event.id() == "reveal_logs" {
                if let Err(e) = commands::app::reveal_logs(app.clone()) {
                    log::warn!("menu: reveal_logs failed: {e}");
                }
            }
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                commands::books::import_external_paths(
                    paths.clone(),
                    window.state::<Db>().inner(),
                    window.state::<SyncWriter>().inner(),
                    window.app_handle(),
                );
            }

            // On macOS, closing the main window via the red button should hide
            // it, not quit the app — matches the standard Mac convention. The
            // user reopens it from the dock icon (RunEvent::Reopen below) or
            // by relaunching from Spotlight. cmd-Q still quits because that
            // path goes through applicationShouldTerminate, not CloseRequested.
            //
            // Reader windows are unaffected and close normally.
            #[cfg(target_os = "macos")]
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (window, event);
            }
        })
        .setup(|app| {
            let local_dir = {
                let base = app
                    .path()
                    .app_data_dir()
                    .expect("failed to resolve app data dir");
                if cfg!(debug_assertions) {
                    base.with_file_name("com.klaragraff.lantern-dev")
                } else {
                    base
                }
            };
            migrate_legacy_app_data(&local_dir).expect("failed to migrate legacy app data");
            std::fs::create_dir_all(&local_dir).expect("failed to create app data dir");
            std::fs::create_dir_all(local_dir.join("prepared"))
                .expect("failed to create text preparation cache");
            let imported_font_dir = local_dir.join("imported-fonts");
            std::fs::create_dir_all(&imported_font_dir)
                .expect("failed to create imported font directory");
            app.asset_protocol_scope()
                .allow_directory(&imported_font_dir, true)
                .expect("failed to allow imported fonts in asset scope");

            // Self-heal: if .sync_setting survived but quill.db
            // was deleted (e.g. user cleared app data via Finder, which
            // skips hidden dot-files), remove the stale marker so the
            // app starts fresh.
            if !local_dir.join("quill.db").exists() && sync::migration::is_sync_enabled(&local_dir)
            {
                log::warn!(
                    "sync: quill.db missing but .sync_setting survived — \
                     clearing stale marker to start fresh"
                );
                let _ = std::fs::remove_file(local_dir.join(".sync_setting"));
            }

            // The marker stores intent only. Never create or serve blobs from
            // a recorded path unless it remains inside iCloud Drive and is
            // writable this launch.
            let sync_enabled = sync::migration::is_sync_enabled(&local_dir);
            let ubiquity_dir = sync_enabled
                .then(|| sync::migration::recorded_usable_icloud_dir(&local_dir))
                .flatten();

            let device =
                DeviceIdentity::load_or_create(&local_dir).expect("failed to load device id");

            // When sync is on, blobs (books/, covers/) live in iCloud;
            // otherwise they're local.
            let data_dir = ubiquity_dir.clone().unwrap_or_else(|| local_dir.clone());
            if let Some(shared_dir) = ubiquity_dir.as_ref() {
                app.asset_protocol_scope()
                    .allow_directory(shared_dir, true)
                    .expect("failed to allow selected sync folder in asset scope");
            }
            let (db, needs_cover_backfill) =
                Db::init_split(&local_dir, &data_dir).expect("failed to initialize database");

            // DB cover backfill for non-sync users. When sync boots, the
            // initial-tick thread handles both DB and .img backfill sequentially.
            let sync_will_boot = ubiquity_dir.is_some();
            if needs_cover_backfill && !sync_will_boot {
                let backfill_db = db.clone();
                std::thread::Builder::new()
                    .name("cover-backfill".into())
                    .spawn(move || {
                        backfill_db.backfill_cover_data();
                    })
                    .ok();
            }

            log::info!(
                "quill start v{version} os={os} arch={arch} data_dir={data_dir} schema_v={schema}",
                version = env!("CARGO_PKG_VERSION"),
                os = std::env::consts::OS,
                arch = std::env::consts::ARCH,
                data_dir = data_dir.display(),
                schema = db.schema_version(),
            );

            let secrets = Secrets::init(&local_dir).expect("failed to initialize secrets store");
            secrets
                .migrate_from_settings(&db)
                .expect("failed to migrate secrets");
            ai::router::migrate_legacy_config(&db, &secrets)
                .expect("failed to migrate AI profile configuration");
            ai::router::migrate_embedding_source(&db, &secrets)
                .expect("failed to migrate embedding configuration");
            secrets
                .register_legacy_candidates(&db)
                .expect("failed to register legacy secret metadata");

            let sync_writer = SyncWriter::new(device.device_uuid.clone());
            sync_writer.set_process_lock_path(local_dir.join(".sync-transition.lock"));
            if sync_enabled {
                sync_writer.set_should_queue(true);
            }

            // Register managed state immediately so setup() returns fast
            // and the webview can render. The sync engine boots on a
            // background thread — EventLog::open, watcher::spawn, and the
            // initial tick all touch iCloud paths that can stall for
            // seconds when files are evicted. Running them here would
            // white-screen the app.

            // Watch the MCP sentinel file so the UI can refresh when an
            // MCP subprocess writes to the library. The sentinel is a
            // tiny JSON file (~80 bytes) that the MCP subprocess
            // overwrites after each write tool invocation.
            let mcp_notify_path = local_dir.join(".mcp-notify");
            let app_handle = app.handle().clone();
            mcp::notify::spawn_watcher(mcp_notify_path, app_handle);

            app.manage(LocalDir(local_dir.clone()));
            app.manage(db);
            app.manage(secrets);
            app.manage(device);
            app.manage(sync_writer);
            app.manage(SyncState::new(None, None));

            // TXT, Markdown, and HTML books keep their source files in the
            // library, while their reader documents are local derived caches.
            // Resume any interrupted work after managed state is available;
            // this deliberately runs away from setup's UI-critical path.
            commands::books::resume_interrupted_text_book_preparations(app.handle().clone());

            // Boot the sync engine on a background thread. Everything
            // that touches iCloud paths (EventLog::open, watcher::spawn,
            // initial tick) runs here so setup() is never blocked by
            // bird/NSFileCoordinator stalls or evicted-file downloads.
            //
            // The SyncWriter is already in queue-only mode (if sync is
            // enabled), so any writes the user makes before the engine
            // finishes booting are safely queued in _pending_publish
            // and drained on the first successful tick.
            let should_boot = ubiquity_dir.clone();
            if let Some(ub) = should_boot {
                let bg_handle = app.handle().clone();
                let bg_needs_backfill = needs_cover_backfill;
                std::thread::Builder::new()
                    .name("sync-boot".into())
                    .spawn(move || {
                        let bg_db: tauri::State<Db> = bg_handle.state();
                        let bg_writer: tauri::State<SyncWriter> = bg_handle.state();
                        let bg_device: tauri::State<DeviceIdentity> = bg_handle.state();
                        let bg_sync: tauri::State<SyncState> = bg_handle.state();
                        match boot_sync_engine(
                            ub,
                            &bg_device.device_uuid,
                            &bg_db,
                            &bg_writer,
                            &bg_sync,
                            &bg_handle,
                            bg_needs_backfill,
                        ) {
                            Ok(()) => {
                                log::info!("sync: engine booted (replay + watcher active)");
                                let _ = bg_handle.emit("sync-status-changed", ());
                            }
                            Err(e) => {
                                log::error!("sync: failed to boot sync engine: {e}");
                            }
                        }
                    })
                    .ok();
            } else if sync_enabled {
                log::warn!(
                    "sync: skipping engine boot — selected sync folder not reachable \
                     this launch; outbox preserved for the next launch"
                );
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // App lifecycle
            commands::app::app_ready,
            commands::app::reveal_logs,
            commands::app::app_build_info,
            commands::app::log_webview_warning,
            // Books
            commands::books::import_book_from_dialog,
            commands::books::list_books,
            commands::books::get_book,
            commands::books::get_book_counts,
            commands::books::delete_book,
            commands::books::update_reading_progress,
            commands::books::mark_finished,
            commands::books::update_book_status,
            commands::books::update_book_pages,
            commands::books::check_book_available,
            commands::books::update_book_cover,
            commands::books::update_book_metadata,
            commands::books::get_text_book_document,
            commands::books::retry_text_book_preparation,
            commands::ai::ai_reindex_book,
            commands::ai::ai_update_book_index,
            commands::ai::ai_index_details,
            commands::ai::get_book_overview,
            commands::ai::update_book_overview,
            commands::ai::update_book_section_summary,
            commands::ai::ai_regenerate_book_summaries,
            // Settings
            commands::settings::get_all_settings,
            commands::settings::ai_api_key_configured,
            commands::settings::ai_vector_retrieval_status,
            commands::settings::ai_embedding_probe,
            commands::settings::set_ai_vector_retrieval,
            commands::settings::vault_status,
            commands::settings::vault_authorize,
            commands::settings::vault_deny,
            commands::settings::vault_migrate_to_local,
            commands::settings::set_ai_api_key,
            commands::settings::ai_active_profile,
            commands::settings::ai_list_profiles,
            commands::settings::ai_create_profile,
            commands::settings::ai_duplicate_profile,
            commands::settings::ai_save_profile,
            commands::settings::ai_update_profile,
            commands::settings::ai_set_profile_enabled,
            commands::settings::ai_reorder_profiles,
            commands::settings::ai_delete_profile,
            commands::settings::ai_list_models,
            commands::settings::ai_test_profile,
            commands::settings::ai_list_credentials,
            commands::settings::ai_add_credential,
            commands::settings::ai_replace_credential,
            commands::settings::ai_set_credential_enabled,
            commands::settings::ai_reorder_credentials,
            commands::settings::ai_delete_credential,
            commands::settings::ai_test_credential,
            commands::settings::get_setting,
            commands::settings::set_setting,
            commands::settings::set_settings_bulk,
            commands::settings::get_book_settings,
            commands::settings::set_book_settings_bulk,
            commands::settings::open_settings_on_main,
            commands::settings::open_library_on_main,
            // Local reading fonts (font binaries are never synced)
            commands::fonts::import_custom_fonts,
            commands::fonts::list_custom_fonts,
            commands::fonts::delete_custom_font,
            // Learner profile and exam-to-CEFR estimates
            commands::language_assessments::estimate_cefr,
            commands::language_assessments::summarize_language_assessments,
            commands::language_assessments::save_language_assessment,
            commands::language_assessments::list_language_assessments,
            commands::language_assessments::delete_language_assessment,
            // Bookmarks & Highlights
            commands::bookmarks::add_bookmark,
            commands::bookmarks::remove_bookmark,
            commands::bookmarks::list_bookmarks,
            commands::bookmarks::add_highlight,
            commands::bookmarks::remove_highlight,
            commands::bookmarks::replace_highlights,
            commands::bookmarks::list_highlights,
            commands::bookmarks::update_highlight_note,
            commands::bookmarks::update_highlight_color,
            // Learning notes and automatic whole-book word markers
            commands::notes::save_note,
            commands::notes::delete_note,
            commands::notes::list_notes,
            commands::notes::list_context_notes,
            commands::word_marks::upsert_word_mark,
            commands::word_marks::ensure_word_mark_rule,
            commands::word_marks::set_word_mark_rule_enabled,
            commands::word_marks::remove_word_mark,
            commands::word_marks::list_word_marks,
            commands::word_marks::set_word_mark_exception,
            commands::word_marks::list_word_mark_exceptions,
            commands::word_marks::ensure_lookup_occurrence_mark,
            commands::word_marks::set_lookup_occurrence_mark_enabled,
            commands::word_marks::list_lookup_occurrence_marks,
            commands::word_marks::clear_lookup_marks_for_book,
            commands::word_marks::list_word_forms,
            commands::word_marks::set_word_forms,
            commands::word_marks::get_word_forms,
            // Collections
            commands::collections::list_collections,
            commands::collections::create_collection,
            commands::collections::rename_collection,
            commands::collections::delete_collection,
            commands::collections::reorder_collections,
            commands::collections::add_book_to_collection,
            commands::collections::remove_book_from_collection,
            commands::collections::list_books_in_collection,
            // AI
            commands::ai::ai_chat,
            commands::ai::ai_prepare_book,
            commands::ai::get_book_ai_state,
            commands::ai::ai_learning_card,
            commands::ai::ai_optimize_prompt,
            commands::ai::ai_custom_action,
            commands::ai::ai_word_forms,
            commands::ai::ai_lookup,
            commands::ai::ai_explain,
            commands::ai::ai_generate_title,
            commands::ai::ai_cancel,
            // OAuth
            commands::oauth::openai_oauth_login,
            commands::oauth::openai_oauth_status,
            commands::oauth::openai_oauth_logout,
            // Vocabulary
            commands::vocab::add_vocab_word,
            commands::vocab::remove_vocab_word,
            commands::vocab::list_vocab_words,
            commands::vocab::check_vocab_exists,
            commands::vocab::list_all_vocab_words,
            commands::vocab::update_vocab_mastery,
            commands::vocab::record_vocab_review,
            commands::vocab::list_vocab_due_for_review,
            commands::vocab::get_vocab_stats,
            commands::vocab::export_vocab_backup,
            commands::vocab::preview_vocab_import,
            commands::vocab::import_vocab_backup,
            commands::vocab::bulk_delete_vocab_words,
            commands::vocab::bulk_update_vocab_mastery,
            // Local lookup history
            commands::lookup_history::save_lookup_record,
            commands::lookup_history::list_lookup_records,
            commands::lookup_history::list_all_lookup_records,
            commands::lookup_history::delete_lookup_record,
            commands::lookup_history::clear_lookup_records,
            commands::lookup_history::prune_lookup_records,
            // Chats
            commands::chats::create_chat,
            commands::chats::list_chats,
            commands::chats::list_all_chats,
            commands::chats::get_chat,
            commands::chats::delete_chat,
            commands::chats::rename_chat,
            commands::chats::list_chat_messages,
            commands::chats::save_chat_message,
            commands::chats::replace_chat_message,
            // Translation
            commands::translation::ai_translate_passage,
            // MCP client integrations
            commands::mcp::mcp_integration_status,
            commands::mcp::mcp_set_integration,
            commands::mcp::mcp_config_snippet,
            commands::mcp::mcp_set_write_access,
            // Sync
            commands::sync::sync_status,
            commands::sync::sync_set_shared_dir,
            commands::sync::sync_enable,
            commands::sync::sync_disable,
            commands::sync::sync_now,
            commands::sync::sync_cancel,
            commands::sync::sync_compact,
            commands::sync::sync_remove_peer,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| match &event {
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Opened { urls } => {
            // Files dropped on the dock icon or opened through a file
            // association are OS-originated selections, so import them without
            // round-tripping their paths through the webview.
            let paths: Vec<PathBuf> = urls
                .iter()
                .filter_map(|url| url.to_file_path().ok())
                .collect();
            if !paths.is_empty() {
                commands::books::import_external_paths(
                    paths,
                    app_handle.state::<Db>().inner(),
                    app_handle.state::<SyncWriter>().inner(),
                    app_handle,
                );
            }
        }
        // Dock icon click while the app is already running. If the main
        // (library) window is hidden — including when only reader windows are
        // visible — bring it back. The user explicitly asked for this so the
        // library is always one dock-click away.
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => {
            if let Some(window) = app_handle.get_webview_window("main") {
                if !window.is_visible().unwrap_or(true) {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        }
        // On non-macOS, closing the main window quits the app (close-all-windows
        // convention). On macOS the main window is hidden instead (handled above
        // in on_window_event), so this branch is a no-op there.
        #[cfg(not(target_os = "macos"))]
        tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::Destroyed,
            ..
        } if label == "main" => {
            for (_, window) in app_handle.webview_windows() {
                let _ = window.close();
            }
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::{resolve_log_dir, resolve_log_level};
    use log::LevelFilter;

    /// Regression guard: `tauri-plugin-log` resolves the file target's
    /// directory via the bundle identifier in `tauri.conf.json`. Renaming
    /// the identifier silently moves the log path on every platform and
    /// any user's existing log directory orphans. Pinning the expected
    /// value here forces a deliberate update of both this test and the
    /// migration story if the identifier ever needs to change.
    #[test]
    fn bundle_identifier_matches_log_path_assumption() {
        let conf = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json"))
            .expect("read tauri.conf.json");
        let v: serde_json::Value = serde_json::from_str(&conf).expect("parse tauri.conf.json");
        let id = v["identifier"].as_str().expect("identifier field");
        assert_eq!(
            id, "com.klaragraff.lantern",
            "bundle identifier changed — update log path docs and migration",
        );
    }

    /// `RUST_LOG` overrides win over the cfg-based default; unset or
    /// garbage falls back to the default. Single test that flips
    /// `RUST_LOG` between cases — env vars are process-global so we
    /// serialize the mutations within one test rather than relying on
    /// test isolation.
    #[test]
    fn resolve_log_level_honors_rust_log_env() {
        // Save whatever the caller's env had so the test harness isn't
        // poisoned for later tests in the same process.
        let saved = std::env::var("RUST_LOG").ok();

        // Unset → default.
        std::env::remove_var("RUST_LOG");
        assert_eq!(resolve_log_level(LevelFilter::Info), LevelFilter::Info);
        assert_eq!(resolve_log_level(LevelFilter::Warn), LevelFilter::Warn);

        // Valid value overrides the default.
        std::env::set_var("RUST_LOG", "warn");
        assert_eq!(resolve_log_level(LevelFilter::Info), LevelFilter::Warn);
        std::env::set_var("RUST_LOG", "trace");
        assert_eq!(resolve_log_level(LevelFilter::Info), LevelFilter::Trace);
        std::env::set_var("RUST_LOG", "off");
        assert_eq!(resolve_log_level(LevelFilter::Info), LevelFilter::Off);

        // Garbage falls back to default rather than killing init.
        std::env::set_var("RUST_LOG", "verbose");
        assert_eq!(resolve_log_level(LevelFilter::Info), LevelFilter::Info);

        // Whitespace tolerated.
        std::env::set_var("RUST_LOG", "  debug  ");
        assert_eq!(resolve_log_level(LevelFilter::Info), LevelFilter::Debug);

        // Restore.
        match saved {
            Some(v) => std::env::set_var("RUST_LOG", v),
            None => std::env::remove_var("RUST_LOG"),
        }
    }

    /// `pnpm tauri dev` and a packaged release build must not write to
    /// the same log file. The helper appends `-dev` to the bundle
    /// identifier under `cfg(debug_assertions)` so the two layouts are
    /// physically isolated. `cfg!(debug_assertions)` is fixed per-build,
    /// so only one branch is meaningfully exercised per `cargo test`
    /// invocation — but both branches compile, and the assertion picks
    /// the right expected value for the build being tested.
    #[test]
    fn resolve_log_dir_uses_dev_suffix_in_debug() {
        let dir = resolve_log_dir();
        let expected_id = if cfg!(debug_assertions) {
            "com.klaragraff.lantern-dev"
        } else {
            "com.klaragraff.lantern"
        };
        let dir_str = dir.to_string_lossy().to_string();
        assert!(
            dir_str.contains(expected_id),
            "log dir {dir_str} should contain {expected_id} for this build",
        );
        // Guard against the dev path accidentally landing under the
        // prod identifier (e.g. if someone changes the suffix logic).
        // In debug builds, the prod-only id must not appear in the
        // path UNLESS it's the substring of the dev id itself.
        if cfg!(debug_assertions) {
            let stripped = dir_str.replace("com.klaragraff.lantern-dev", "");
            assert!(
                !stripped.contains("com.klaragraff.lantern"),
                "debug build log dir {dir_str} leaks the prod identifier outside the -dev suffix",
            );
        }
    }
}
